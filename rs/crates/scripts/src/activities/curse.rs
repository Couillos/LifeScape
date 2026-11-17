//! Curses (D18, 2026-11-16): confuse (3), weaken (11), curse (19) and
//! bind (20) cast on the NPCs of a combat camp, one cast per action, the
//! XP paid at the cast whether it lands or splashes (`player_magic.rs2`
//! `pvm_spell_cast`). The planner names the camp (the magic climb's own
//! chickens or cows: their retaliation never threatens the caster) and
//! the spell; runes come from the pack then the known bank
//! (`kit::spell_load`, a worn staff paying its element).
//!
//! Server rules the loop follows (rev 274):
//! - a foe whose stat the curse lowers refuses the next one until it
//!   regenerates ("Your foe's attack has already been lowered.",
//!   `pvm_debuff_allowed`; NPC stats regenerate one point per 100 ticks),
//!   a held foe refuses bind ("already held", then "currently immune");
//! - a cursed foe retaliates (`npc_retaliate`): in a single-way camp no
//!   other foe takes a cast while it attacks ("I'm already under
//!   attack!"), so the caster finishes a lowered aggressor in melee and
//!   casts again on one that splashed;
//! - "Someone else is fighting that." skips a foe held by another player.
//!
//! Process: poste camp de combat · action lancer · interface sort ·
//! intrants runes · produit aucun (XP par lancer).

#![forbid(unsafe_code)]

use std::collections::HashMap;

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::magic::{spell_com, wield_staff_for};
use crate::kit::spell_load::{run_casts, SpellLoad};
use crate::kit::survival::{check_events, learn_level, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};

/// Magic stat id (server order: attack 0 .. magic 6).
pub const MAGIC_STAT: u8 = 6;
/// Ticks a cast gets to show its XP (walk into range, 5-tick delay).
pub const CURSE_LAND_TICKS: u64 = 20;
/// Ticks a lowered foe stays out of the picks (one NPC regen period).
pub const LOWERED_BAN_TICKS: u64 = 100;
/// Ticks a foe taken by someone else, or unreachable, stays out.
pub const TAKEN_BAN_TICKS: u64 = 30;
/// Ticks a retaliating foe gets to die before the loop moves on.
pub const FINISH_TICKS: u64 = 150;
/// Re-click period while finishing an aggressor (melee keeps swinging).
const FINISH_RECLICK_TICKS: u64 = 25;
/// Picks one cast may try before it fails loud.
const PICKS_PER_CAST: u32 = 12;
/// Tiles from the bot within which a camp NPC is a candidate.
const FOE_RADIUS: i32 = 15;

/// Server answer to a curse click, by what the loop does about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurseRefusal {
    /// The foe's stat is already lowered, or it is held or immune: out
    /// until it regenerates.
    Lowered,
    /// Another foe attacks us (single-way camp): deal with it first.
    UnderAttack,
    /// Another player fights it, or it is out of reach: skip it.
    Taken,
}

/// Classify one server line after a curse click (`None`: not a refusal).
pub fn curse_refusal(line: &str) -> Option<CurseRefusal> {
    let lowered = line.to_lowercase();
    if lowered.contains("has already been lowered") || lowered.contains("has already been weakened") || lowered.contains("already held by a magical force") || lowered.contains("currently immune") {
        Some(CurseRefusal::Lowered)
    } else if lowered.contains("already under attack") {
        Some(CurseRefusal::UnderAttack)
    } else if lowered.contains("someone else is fighting") || lowered.contains("can't reach that") || lowered.contains("not after you") {
        Some(CurseRefusal::Taken)
    } else {
        None
    }
}

/// The F2P NPC debuff `spell` and its level (`None`: no such debuff in
/// the dump: a strike, a members curse, a typo).
pub fn debuff_level(spell: &str) -> Option<i64> {
    knowledge::spells::by_name(spell).into_iter().find(|row| !row.members && row.npc_debuff()).map(|row| row.level)
}

/// Per-run memory of the camp's foes (never persisted): slots out of the
/// picks until a tick, and the slots our casts touched.
#[derive(Debug, Default, Clone)]
pub struct CurseFoes {
    banned: HashMap<u16, u64>,
    lowered: HashMap<u16, u64>,
}

impl CurseFoes {
    /// Keep `slot` out of the picks until `until`.
    pub fn ban(&mut self, slot: u16, until: u64) {
        let known = self.banned.entry(slot).or_insert(until);
        *known = (*known).max(until);
    }

    /// The foe's stat is down (a refusal said so): finish it if it
    /// attacks, never cast on it until it regenerates.
    pub fn mark_lowered(&mut self, slot: u16, now: u64) {
        self.lowered.insert(slot, now + LOWERED_BAN_TICKS);
        self.ban(slot, now + LOWERED_BAN_TICKS);
    }

    pub fn is_lowered(&self, slot: u16, now: u64) -> bool {
        self.lowered.get(&slot).is_some_and(|until| *until > now)
    }

    pub fn is_banned(&self, slot: u16, now: u64) -> bool {
        self.banned.get(&slot).is_some_and(|until| *until > now)
    }
}

/// What the loop does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurseStep {
    /// A lowered foe attacks us: melee it down first.
    Finish(u16),
    /// Cast on this foe.
    Cast(u16),
    /// No candidate in sight: walk to the camp or wait.
    Search,
}

/// Pure choice: an aggressor first (the only legal target while it hits
/// us), finished when lowered; else the nearest camp foe not banned.
pub fn choose_step(aggressor: Option<u16>, candidates: &[(u16, i32, i32)], here: (i32, i32), foes: &CurseFoes, now: u64) -> CurseStep {
    if let Some(slot) = aggressor {
        return if foes.is_lowered(slot, now) { CurseStep::Finish(slot) } else { CurseStep::Cast(slot) };
    }
    candidates
        .iter()
        .filter(|(slot, x, z)| !foes.is_banned(*slot, now) && (x - here.0).abs().max((z - here.1).abs()) <= FOE_RADIUS)
        .min_by_key(|(slot, x, z)| ((x - here.0).abs() + (z - here.1).abs(), *slot))
        .map_or(CurseStep::Search, |(slot, _, _)| CurseStep::Cast(*slot))
}

fn magic_xp(bot: &Bot) -> i32 {
    bot.world.stats.get(&MAGIC_STAT).map(|(_, xp)| *xp).unwrap_or(0)
}

/// The camp NPC attacking us, if any (its FACE_ENTITY names our slot).
fn aggressor(bot: &Bot, names: &[&str]) -> Option<u16> {
    let me = bot.world.self_slot?;
    bot.npcs_named(names).into_iter().map(|(slot, _, _)| slot).find(|slot| bot.world.npcs.get(slot).and_then(lifescape_sdk::engagement::targeted_player) == Some(me))
}

/// Melee a lowered aggressor until it dies or lets go.
fn finish(bot: &mut Bot, slot: u16) -> Result<(), ScriptError> {
    let deadline = bot.world.ticks + FINISH_TICKS;
    let mut clicked_at = 0u64;
    let me = bot.world.self_slot;
    while bot.world.ticks < deadline {
        let Some(npc) = bot.world.npcs.get(&slot) else { return Ok(()) };
        if me.is_some() && lifescape_sdk::engagement::targeted_player(npc) != me {
            return Ok(());
        }
        if clicked_at == 0 || bot.world.ticks >= clicked_at + FINISH_RECLICK_TICKS {
            bot.op_npc(2, slot)?;
            clicked_at = bot.world.ticks.max(1);
        }
        if crate::kit::survival::should_eat(crate::kit::survival::hitpoints(bot), crate::kit::survival::max_hitpoints(bot), 0.5) {
            crate::kit::survival::eat(bot)?;
        }
        bot.pump(1)?;
    }
    bot.log(&format!("aggressor {slot} still on us after {FINISH_TICKS} ticks at {:?}", bot.position()));
    Ok(())
}

/// Walk to the camp: the brain's site, else its nearest spawn cluster.
fn reach_camp(bot: &mut Bot, spot: &knowledge::combat_spots::CombatSpot) -> Result<(), ScriptError> {
    if crate::kit::site::reach_site(bot)? {
        return Ok(());
    }
    let Some((x, z, _)) = crate::kit::foes::ranked_clusters(bot, spot).into_iter().next() else {
        return Err(crate::kit::hunt::no_spawn_error(bot, spot.key));
    };
    if (x - bot.world.x).abs().max((z - bot.world.z).abs()) > FOE_RADIUS {
        bot.intend(&format!("walking to the {} camp", spot.key));
        crate::skill_walk::walk_checked_at(bot, x, z, bot.world.level, crate::kit::site::SITE_WALK_TICKS)?;
    } else {
        bot.pump(5)?;
    }
    Ok(())
}

/// One cast that paid its XP (the unit `run_casts` counts).
fn curse_once(bot: &mut Bot, spot: &knowledge::combat_spots::CombatSpot, spell: &str, foes: &mut CurseFoes) -> Result<(), ScriptError> {
    let com = spell_com(bot, spell)?;
    let names: Vec<&str> = spot.npc_names.to_vec();
    for _ in 0..PICKS_PER_CAST {
        check_events(bot)?;
        let now = bot.world.ticks;
        let mut candidates = bot.npcs_named(&names);
        candidates.retain(|(slot, _, _)| !bot.npc_engaged_by_other(*slot));
        let step = choose_step(aggressor(bot, &names), &candidates, (bot.world.x, bot.world.z), foes, now);
        let slot = match step {
            CurseStep::Search => {
                reach_camp(bot, spot)?;
                continue;
            }
            CurseStep::Finish(slot) => {
                finish(bot, slot)?;
                continue;
            }
            CurseStep::Cast(slot) => slot,
        };
        let xp_before = magic_xp(bot);
        let seen = bot.world.messages.len();
        bot.op_npc_t(slot, com)?;
        let deadline = bot.world.ticks + CURSE_LAND_TICKS;
        let mut refusal = None;
        while bot.world.ticks < deadline && refusal.is_none() {
            bot.pump(1)?;
            if magic_xp(bot) > xp_before {
                // Landed or splashed: XP paid. A landed curse lowers the
                // foe; a second cast on it would be refused.
                foes.ban(slot, bot.world.ticks + LOWERED_BAN_TICKS);
                return Ok(());
            }
            refusal = bot.world.messages.get(seen..).unwrap_or(&[]).iter().find_map(|line| curse_refusal(line));
        }
        let now = bot.world.ticks;
        match refusal {
            Some(CurseRefusal::Lowered) => foes.mark_lowered(slot, now),
            Some(CurseRefusal::Taken) | None => foes.ban(slot, now + TAKEN_BAN_TICKS),
            Some(CurseRefusal::UnderAttack) => {}
        }
    }
    // Every foe already weakened or under attack while other players work
    // the camp: contention (D4, tour 2 P2-3c), never a broken method.
    if let Some(contended) = crate::kit::site::contended_error(bot) {
        return Err(contended);
    }
    let last = bot.world.messages.last().cloned().unwrap_or_default();
    Err(ScriptError::Failed(format!("no {} foe took {spell} in {PICKS_PER_CAST} picks at {:?} (last line {last:?})", spot.key, bot.position())))
}

/// `curse <spot> --spell <confuse|weaken|curse|bind> [count] [--level
/// magic N] [--site X,Z,L] [toll] [boat]`: casts until the order's stop
/// (level, count, budget); runes refilled from the bank between loads.
pub fn run_curse(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = crate::kit::cycle::parse_cli("curse", args)?;
    let key = spec.resource.first().ok_or_else(|| ScriptError::Usage(format!("curse <spot> --spell <spell>, no camp named at {:?}", bot.position())))?;
    let spot = knowledge::combat_spots::by_key(key).ok_or_else(|| ScriptError::Usage(format!("unknown combat spot {key:?}")))?;
    let spell = spec.spell.clone().ok_or_else(|| ScriptError::Usage(format!("curse {key}: no --spell named")))?;
    let need = debuff_level(&spell).ok_or_else(|| ScriptError::Usage(format!("{spell:?} is no F2P curse (confuse, weaken, curse, bind)")))?;
    crate::kit::toll::arm_fares(bot, spec.allow_paid, spec.allow_boat, spec.site)?;
    bot.allow_shop = spec.allow_shop;
    bot.assigned_site = spec.site;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    if learn_level(bot, MAGIC_STAT, 1) < need {
        return Err(crate::kit::cycle::blocked_level("magic", need));
    }
    let keep = crate::kit::cycle::resolve_keep(bot, &spec)?;
    let runes = crate::activities::enchant_teleport::spell_runes(&spell);
    let load = SpellLoad::new(&spell, runes, Vec::new());
    wield_staff_for(bot, &load.runes)?;
    bot.log(&format!("curse {spell} on {} (magic {})", spot.key, learn_level(bot, MAGIC_STAT, 1)));
    let mut foes = CurseFoes::default();
    let (done, stop) = run_casts(bot, &spec, &load, &keep, |cast_bot| curse_once(cast_bot, &spot, &spell, &mut foes))?;
    bot.log(&format!("ok {spell} x{done}"));
    stop.map_or(Ok(0), crate::kit::cycle::exit_code)
}

#[cfg(test)]
#[path = "curse_tests.rs"]
mod tests;
