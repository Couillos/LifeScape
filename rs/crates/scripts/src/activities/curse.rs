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
//!   casts again on one that splashed; an aggressor it does not see
//!   holds every cast for the 8-tick window (`player_in_combat_check`),
//!   a wait that spends no pick; an aggressor whose cast went unanswered
//!   keeps its ban like any foe;
//! - "Someone else is fighting that." skips a foe held by another player.
//!
//! Process: poste camp de combat · action lancer · interface sort ·
//! intrants runes · produit aucun (XP par lancer).

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

pub use crate::kit::curse_camp::{
    all_taken, choose_step, curse_refusal, CastBudget, CastSpent, CurseFoes, CurseRefusal, CurseStep, CAST_WAIT_TICKS, CURSE_LAND_TICKS,
    FINISH_TICKS, FOE_RADIUS, LOWERED_BAN_TICKS, PICKS_PER_CAST, TAKEN_BAN_TICKS, UNDER_ATTACK_HOLD_TICKS,
};
use crate::kit::magic::{spell_com, wield_staff_for};
use crate::kit::spell_load::{run_cast_steps, CastOutcome, SpellLoad};
use crate::kit::survival::{check_events, learn_level, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};

/// Magic stat id (server order: attack 0 .. magic 6).
pub const MAGIC_STAT: u8 = 6;
/// Re-click period while finishing an aggressor (melee keeps swinging).
const FINISH_RECLICK_TICKS: u64 = 25;
/// The F2P NPC debuff `spell` and its level (`None`: no such debuff in
/// the dump: a strike, a members curse, a typo).
pub fn debuff_level(spell: &str) -> Option<i64> {
    knowledge::spells::by_name(spell).into_iter().find(|row| !row.members && row.npc_debuff()).map(|row| row.level)
}


/// Camp foes another caster already holds (tour 3 point 4, plan P7c):
/// one fighting another player (`npc_engaged_by_other`) or showing the
/// impact graphic of `spell` within a regeneration period (a curse
/// landed on it, whoever cast it: "has already been lowered"). Read from
/// the live NPC state, before any click.
pub fn taken_foe(bot: &Bot, slot: u16, spell: &str) -> bool {
    knowledge::spells::impact_graphic(spell).is_some_and(|graphic| bot.world.npc_showed_graphic(slot, &[graphic], LOWERED_BAN_TICKS)) || bot.npc_engaged_by_other(slot)
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

/// One cast that paid its XP (the unit `run_cast_steps` counts), or a
/// refill when the pack funds no cast any more (a market pause banked
/// the runes: never a cast the server answers "You do not have enough").
fn curse_once(bot: &mut Bot, spot: &knowledge::combat_spots::CombatSpot, spell: &str, foes: &mut CurseFoes, load: &SpellLoad) -> Result<CastOutcome, ScriptError> {
    let com = spell_com(bot, spell)?;
    let names: Vec<&str> = spot.npc_names.to_vec();
    let mut budget = CastBudget::new(bot.world.ticks);
    let mut pauses_seen = bot.pauses_done;
    // Set by a pause: the next walk back to the camp restarts the bound
    // (only that one, never a chase inside the camp).
    let mut returning = false;
    let spent = loop {
        check_events(bot)?;
        let now = bot.world.ticks;
        // A market pause (Varrock square and back) is no camp wait.
        if crate::kit::site::paused_since(pauses_seen, bot.pauses_done) {
            pauses_seen = bot.pauses_done;
            budget.restart(now);
            returning = true;
        }
        if load.pack_casts(bot) == 0 {
            return Ok(CastOutcome::Refill);
        }
        let mut candidates = bot.npcs_named(&names);
        // Taken by another caster: the ones our own casts lowered aside.
        let by_others = candidates.iter().filter(|(slot, _, _)| !foes.is_banned(*slot, now) && taken_foe(bot, *slot, spell)).count();
        candidates.retain(|(slot, _, _)| !taken_foe(bot, *slot, spell));
        let workable = candidates.iter().filter(|(slot, _, _)| !foes.is_banned(*slot, now)).count();
        let attacker = aggressor(bot, &names);
        // Every foe in sight cursed or fought by another player: the
        // camp is contended (typed), the next resolve picks another site.
        if let Some(contended) = (attacker.is_none() && all_taken(by_others, workable)).then(|| crate::kit::site::contended_error(bot)).flatten() {
            return Err(contended);
        }
        let step = choose_step(attacker, &candidates, (bot.world.x, bot.world.z), foes, now);
        if let Err(spent) = budget.step(step, now) {
            break spent;
        }
        let slot = match step {
            CurseStep::Search => {
                let before = bot.world.ticks;
                let from = (bot.world.x, bot.world.z);
                reach_camp(bot, spot)?;
                // The walk back after a pause restarts the bound at the
                // camp.
                if returning && (bot.world.x - from.0).abs().max((bot.world.z - from.1).abs()) > FOE_RADIUS {
                    budget.restart(bot.world.ticks);
                    returning = false;
                }
                // The tick bound only ends a loop whose ticks move: an
                // instant walk refusal still waits one tick.
                if bot.world.ticks == before {
                    bot.pump(1)?;
                }
                continue;
            }
            CurseStep::Finish(slot) => {
                finish(bot, slot)?;
                continue;
            }
            CurseStep::Wait => {
                bot.pump(1)?;
                continue;
            }
            CurseStep::Cast(slot) => slot,
        };
        let xp_before = magic_xp(bot);
        let seen = bot.world.messages.len();
        let pauses_at_cast = bot.pauses_done;
        bot.op_npc_t(slot, com)?;
        let deadline = bot.world.ticks + CURSE_LAND_TICKS;
        let mut refusal = None;
        while bot.world.ticks < deadline && refusal.is_none() {
            bot.pump(1)?;
            if magic_xp(bot) > xp_before {
                // Landed or splashed: XP paid. A landed curse lowers the
                // foe; a second cast on it would be refused.
                foes.ban(slot, bot.world.ticks + LOWERED_BAN_TICKS);
                return Ok(CastOutcome::Cast);
            }
            refusal = bot.world.messages.get(seen..).unwrap_or(&[]).iter().find_map(|line| curse_refusal(line));
        }
        // A pause in the wait took us away: no verdict on the foe.
        if crate::kit::site::paused_since(pauses_at_cast, bot.pauses_done) {
            continue;
        }
        budget.refused();
        let now = bot.world.ticks;
        match refusal {
            Some(CurseRefusal::Lowered) => foes.mark_lowered(slot, now),
            Some(CurseRefusal::Taken) | None => foes.ban(slot, now + TAKEN_BAN_TICKS),
            Some(CurseRefusal::UnderAttack) => foes.under_attack(now),
        }
    };
    // Every foe already weakened or under attack while other players work
    // the camp: contention (D4, tour 2 P2-3c), never a broken method.
    if let Some(contended) = crate::kit::site::contended_error(bot) {
        return Err(contended);
    }
    let last = bot.world.messages.last().cloned().unwrap_or_default();
    let bound = match spent {
        CastSpent::Picks => format!("{PICKS_PER_CAST} refused casts"),
        CastSpent::Ticks => format!("{CAST_WAIT_TICKS} ticks"),
    };
    Err(ScriptError::Failed(format!("no {} foe took {spell} in {bound} at {:?} (last line {last:?})", spot.key, bot.position())))
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
    let (done, stop) = run_cast_steps(bot, &spec, &load, &keep, |cast_bot| curse_once(cast_bot, &spot, &spell, &mut foes, &load))?;
    bot.log(&format!("ok {spell} x{done}"));
    stop.map_or(Ok(0), crate::kit::cycle::exit_code)
}

#[cfg(test)]
#[path = "curse_tests.rs"]
mod tests;
