//! Strike: one attack click per run style (melee/ranged Attack, magic
//! cast) and the server's refusals of it, by what the kill loop does
//! about them. Split out of `kit::fight` (kill loop + hunt) to keep that
//! brick focused; ammunition lives in `kit::ammo`.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::ammo::{ensure_ranged_ammo, AmmoFix};
use crate::ScriptError;

/// Server refusal of an attack click (player_combat.rs2), by what the
/// bot should do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FoeRefusal {
    /// "I can't reach that": one door-aware step may burn the door and
    /// prove it uncrossable (excluded for the whole fight).
    Unreachable,
    /// Someone else holds the foe (or holds us): skip it, never step
    /// toward it.
    Taken,
}

pub(crate) fn foe_refusal(line: &str) -> Option<FoeRefusal> {
    let lowered = line.to_lowercase();
    if lowered.contains("can't reach that") {
        Some(FoeRefusal::Unreachable)
    } else if lowered.contains("already under attack")
        || lowered.contains("someone else is fighting")
        || lowered.contains("not after you")
    {
        Some(FoeRefusal::Taken)
    } else {
        None
    }
}

/// One attack on `slot` for the run's combat style (PRD3 lot 6). Melee
/// and ranged click Attack: the server shoots when a bow and arrows are
/// worn. Magic casts the ordered spell (`bot.combat_spell`), else the
/// strongest damage spell the pack funds; no runes
/// left blocks typed so the brain files the runes as an obtain. False
/// when an empty quiver wants a bank trip (arrows banked) instead.
pub(crate) fn attack_npc(bot: &mut Bot, slot: u16, no_bank: bool) -> Result<bool, ScriptError> {
    if bot.combat_style == "magic" {
        // The planner's spell (`--spell`) and nothing else: its runes run
        // short in the pack, the known bank refills them (a bank trip,
        // like an empty quiver); short there too (or `--no-bank`), the run
        // blocks on exactly those (worn staff element excluded).
        if bot.combat_spell_best {
            let target = target_npc_name(bot, slot);
            let spell = match best_cast(bot, target.as_deref(), no_bank) {
                BestCast::Cast(spell) => spell,
                BestCast::BankTrip => {
                    bot.log("out of runes in the pack for every spell, the bank funds one: bank trip");
                    return Ok(false);
                }
                BestCast::Blocked(missing) => {
                    bot.log(&format!("out of runes for any damage spell: {}", missing.join(", ")));
                    return Err(ScriptError::Blocked { missing });
                }
            };
            if bot.combat_spell.as_deref() != Some(spell.as_str()) {
                bot.log(&format!("casting {spell} (was {}): the strongest spell the runes allow", bot.combat_spell.as_deref().unwrap_or("none")));
                bot.combat_spell = Some(spell.clone());
            }
            let com = crate::kit::magic::spell_com(bot, &spell)?;
            bot.op_npc_t(slot, com)?;
            return Ok(true);
        }
        let spell = match bot.combat_spell.clone() {
            Some(ordered) => {
                let runes = crate::activities::magic_style::spell_runes(&ordered);
                let missing = crate::kit::magic::spell_shortfall(bot, &runes);
                if !missing.is_empty() {
                    let unbanked = crate::kit::magic::bank_shortfall(bot, &runes);
                    if !no_bank && unbanked.is_empty() {
                        bot.log(&format!("out of runes for {ordered} in the pack ({}), banked: bank trip", missing.join(", ")));
                        return Ok(false);
                    }
                    let missing = if no_bank { missing } else { unbanked };
                    bot.log(&format!("out of runes for {ordered}: {}", missing.join(", ")));
                    return Err(ScriptError::Blocked { missing });
                }
                ordered
            }
            None => match castable_spell(bot) {
                Some(best) => best.to_string(),
                None if !no_bank && crate::kit::magic::bank_shortfall(bot, &crate::activities::magic_style::spell_runes("wind_strike")).is_empty() => {
                    bot.log("out of runes in the pack, wind strike's banked: bank trip");
                    return Ok(false);
                }
                None => {
                    bot.log("out of runes for any damage spell");
                    return Err(ScriptError::Blocked { missing: vec!["mindrune".to_string(), "airrune".to_string()] });
                }
            },
        };
        let com = crate::kit::magic::spell_com(bot, &spell)?;
        bot.op_npc_t(slot, com)?;
        return Ok(true);
    }
    // Empty quiver: pack, then the arrows on the ground, then the bank
    // (trip) or blocked typed for the brain to obtain more.
    if bot.combat_style == "ranged" && ensure_ranged_ammo(bot, no_bank)? == AmmoFix::BankTrip {
        return Ok(false);
    }
    bot.op_npc(2, slot)?;
    Ok(true)
}

/// What a combat-intent magic run (`Bot::combat_spell_best`) does on
/// its next attack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BestCast {
    /// Cast this spell: the strongest the pack funds.
    Cast(String),
    /// The pack funds none, the known bank one: a bank trip (the bank
    /// leg withdraws every combat rune, `combat_bank::magic_ammo`).
    BankTrip,
    /// Nothing anywhere funds one: blocked on the runes of the spell
    /// cast last (the planner's at first), the brain obtains them.
    Blocked(Vec<String>),
}

/// F2P damage spells the magic `level` casts on `target` (an NPC debug
/// name; `None`: any NPC, so no spell with a target param), strongest
/// first: highest max hit, then highest level (dump `spells`, server
/// `undead` param for crumble undead).
pub fn combat_spells(level: i64, target: Option<&str>) -> Vec<String> {
    let mut rows: Vec<lifescape_gamedata::spells::Spell> = lifescape_gamedata::spells::all_spells()
        .into_iter()
        .filter(|row| !row.members && row.deals_damage() && row.level <= level)
        .filter(|row| match target {
            Some(npc) => lifescape_gamedata::spells::affects(&row.spell, npc),
            None => lifescape_gamedata::spells::target_param(&row.spell).is_none(),
        })
        .collect();
    rows.sort_by(|left, right| right.maxhit.cmp(&left.maxhit).then_with(|| right.level.cmp(&left.level)));
    rows.into_iter().map(|row| row.spell).collect()
}

/// True when pack + known bank fund one cast of a damage spell the
/// level casts on any NPC (a combat-intent run starts on it).
pub fn any_spell_funded(bot: &Bot) -> bool {
    use crate::activities::magic_style::{magic_level, spell_runes};
    combat_spells(magic_level(bot), None).iter().any(|spell| crate::kit::magic::bank_shortfall(bot, &spell_runes(spell)).is_empty())
}

/// Debug name of the NPC in `slot`, if streamed and known.
fn target_npc_name(bot: &Bot, slot: u16) -> Option<String> {
    let npc = bot.world.npcs.get(&slot)?;
    lifescape_gamedata::npc_by_id(usize::from(npc.npc_type)).and_then(|entry| entry.name).map(str::to_string)
}

/// The combat-intent cast (user 2026-11-20: « s'il veut combattre alors
/// le meilleur sort dispo avec les runes dispo »): the strongest spell
/// of `combat_spells` the pack funds (a worn staff's element paid); none
/// in the pack: a bank trip when the known bank funds one (never with
/// `no_bank`); none anywhere: blocked on the last spell's runes.
pub(crate) fn best_cast(bot: &Bot, target: Option<&str>, no_bank: bool) -> BestCast {
    use crate::activities::magic_style::{magic_level, spell_runes};
    let spells = combat_spells(magic_level(bot), target);
    if let Some(spell) = spells.iter().find(|spell| crate::kit::magic::spell_shortfall(bot, &spell_runes(spell)).is_empty()) {
        return BestCast::Cast(spell.clone());
    }
    if !no_bank && spells.iter().any(|spell| crate::kit::magic::bank_shortfall(bot, &spell_runes(spell)).is_empty()) {
        return BestCast::BankTrip;
    }
    let last = bot.combat_spell.clone().unwrap_or_else(|| crate::activities::magic_style::DAMAGE_SPELLS[0].0.to_string());
    let runes = spell_runes(&last);
    BestCast::Blocked(if no_bank { crate::kit::magic::spell_shortfall(bot, &runes) } else { crate::kit::magic::bank_shortfall(bot, &runes) })
}

/// Ticks between re-attacks while engaged: a manual cast never repeats
/// (one spell per click, 5-tick cast speed); melee and ranged keep
/// fighting on their own and only need a nudge.
pub(crate) fn reattack_ticks(bot: &Bot) -> u64 {
    if bot.combat_style == "magic" { 5 } else { 25 }
}

/// Re-attack while engaged (the woodcut rule for combat): a manual cast
/// is one spell per click, so magic re-casts on its cadence; melee and
/// ranged fight on their own (`player_combat.rs2` re-queues the swing),
/// so they are nudged only once the fight went quiet — no combat xp and
/// no step for a whole window — never every 25 ticks while xp flows.
/// Pure so the cadence is locked by tests.
pub(crate) fn reattack_due(magic: bool, window: u64, since_attack: u64, since_progress: u64) -> bool {
    if magic {
        return since_attack >= window;
    }
    since_attack >= window && since_progress >= window
}

/// Strongest damage spell the magic level unlocks whose runes the pack
/// (or a worn staff) funds (PRD3 lot 6: the fight loop casts it on the
/// target).
pub fn castable_spell(bot: &Bot) -> Option<&'static str> {
    use crate::activities::magic_style::{magic_level, spell_runes, DAMAGE_SPELLS};
    let level = magic_level(bot);
    DAMAGE_SPELLS.iter().rev().filter(|(_, need, _)| level >= *need).map(|(spell, _, _)| *spell).find(|spell| {
        crate::kit::magic::spell_shortfall(bot, &spell_runes(spell)).is_empty()
    })
}

#[cfg(test)]
mod tests {
    use super::{foe_refusal, FoeRefusal};

    /// Server refusals split in two: an unreachable foe is worth one
    /// door-aware step (it may burn a door and prove it uncrossable), a
    /// foe already taken by someone else is skipped on the spot — live
    /// (bb4v5d9m 17:52:38) the step toward a taken cow paced ~20 walks.
    /// Melee/ranged never re-click while xp flows; a quiet fight gets one
    /// nudge per window; magic casts on its 5-tick cadence.
    #[test]
    fn engaged_melee_waits_while_the_fight_runs() {
        use super::reattack_due;
        assert!(!reattack_due(false, 25, 25, 3), "xp 3 ticks ago: the swing loop runs, no click");
        assert!(!reattack_due(false, 25, 400, 24), "long fight, steady xp: still no click");
        assert!(reattack_due(false, 25, 25, 25), "quiet for a window: one nudge");
        assert!(!reattack_due(false, 25, 10, 60), "nudged 10 ticks ago: wait the window");
        assert!(reattack_due(true, 5, 5, 0), "magic: one cast per click on the cast cadence");
    }

    #[test]
    fn taken_foe_is_skipped_without_a_step() {
        assert_eq!(foe_refusal("Someone else is fighting that."), Some(FoeRefusal::Taken));
        assert_eq!(foe_refusal("I'm already under attack."), Some(FoeRefusal::Taken));
        assert_eq!(foe_refusal("It's not after you."), Some(FoeRefusal::Taken));
        assert_eq!(foe_refusal("I can't reach that!"), Some(FoeRefusal::Unreachable));
        assert_eq!(foe_refusal("You bury the bones."), None);
    }

    /// Magic run out of the ordered spell's runes in the pack while the
    /// known bank holds them: a bank trip (`Ok(false)`, the hunt re-banks
    /// through `prepare_at_bank`), never `blocked` on banked runes. With
    /// `--no-bank`, or nothing banked, blocked by exact names.
    #[test]
    fn banked_runes_send_a_magic_run_to_the_bank() {
        let mut bot = crate::kit::spell_load::tests::stocked_bot(&[("mindrune", 3)], &[("firerune", 30), ("airrune", 30)]);
        bot.combat_style = "magic".to_string();
        bot.combat_spell = Some("fire_strike".to_string());
        assert!(matches!(super::attack_npc(&mut bot, 1, false), Ok(false)), "bank trip for the banked runes");
        match super::attack_npc(&mut bot, 1, true) {
            Err(crate::ScriptError::Blocked { mut missing }) => {
                missing.sort();
                assert_eq!(missing, vec!["airrune".to_string(), "firerune".to_string()]);
            }
            other => panic!("no-bank run blocks, got {other:?}"),
        }
        let mut bot = crate::kit::spell_load::tests::stocked_bot(&[("mindrune", 3)], &[("firerune", 30)]);
        bot.combat_style = "magic".to_string();
        bot.combat_spell = Some("fire_strike".to_string());
        match super::attack_npc(&mut bot, 1, false) {
            Err(crate::ScriptError::Blocked { missing }) => assert_eq!(missing, vec!["airrune".to_string()], "only the air is nowhere"),
            other => panic!("air nowhere blocks, got {other:?}"),
        }
    }

    /// A magic bot at `level` with `pack` and `banked`, fighting for
    /// combat (`--best-spell`), ordered `spell`.
    fn fighter(level: u8, spell: &str, pack: &[(&str, u32)], banked: &[(&str, u32)]) -> lifescape_sdk::Bot {
        let mut bot = crate::kit::spell_load::tests::stocked_bot(pack, banked);
        bot.world.stats.insert(crate::activities::magic_style::MAGIC_STAT, (level, 0));
        bot.combat_style = "magic".to_string();
        bot.combat_spell = Some(spell.to_string());
        bot.combat_spell_best = true;
        bot
    }

    /// User 2026-11-20 (« s'il veut combattre alors le meilleur sort
    /// dispo avec les runes dispo »): a fight for drops, coins or a kill
    /// casts the strongest spell its runes allow and falls back down the
    /// ladder when they run out, never blocked while a weaker spell is
    /// funded (live 2026-11-20: "out of runes for crumble_undead …
    /// chaosrune", 79 blocks a day, the earth runes still in the pack).
    #[test]
    fn a_combat_run_falls_back_to_the_best_spell_its_runes_fund() {
        use super::{best_cast, combat_spells, BestCast};
        assert_eq!(combat_spells(40, None).first().map(String::as_str), Some("fire_bolt"), "the strongest open at 40");
        assert!(!combat_spells(60, None).contains(&"crumble_undead".to_string()), "crumble undead needs an undead target");
        assert!(combat_spells(60, Some("skeleton_unarmed")).contains(&"crumble_undead".to_string()));
        // The planner's fire bolt, no chaos rune left: earth strike, the
        // strongest the pack funds.
        let bot = fighter(40, "fire_bolt", &[("mindrune", 50), ("earthrune", 100), ("airrune", 50), ("firerune", 2)], &[]);
        assert_eq!(best_cast(&bot, Some("cow"), false), BestCast::Cast("earth_strike".to_string()));
        // The chaos runes held: the bolt again.
        let bot = fighter(40, "fire_bolt", &[("chaosrune", 5), ("firerune", 40), ("airrune", 30)], &[]);
        assert_eq!(best_cast(&bot, Some("cow"), false), BestCast::Cast("fire_bolt".to_string()));
        // Crumble undead ordered on the skeletons, its chaos gone: a strike.
        let bot = fighter(45, "crumble_undead", &[("airrune", 50), ("earthrune", 80), ("mindrune", 20)], &[]);
        assert_eq!(best_cast(&bot, Some("skeleton_unarmed"), false), BestCast::Cast("earth_strike".to_string()));
        // Nothing in the pack, a spell's runes banked: a bank trip.
        let bot = fighter(40, "fire_bolt", &[], &[("mindrune", 50), ("waterrune", 50), ("airrune", 50)]);
        assert_eq!(best_cast(&bot, None, false), BestCast::BankTrip);
        let mut bot = bot;
        assert!(matches!(super::attack_npc(&mut bot, 1, false), Ok(false)), "the fight goes banking, never blocked");
        // Nothing anywhere: blocked on the last spell's runes.
        let mut bot = fighter(40, "fire_bolt", &[], &[]);
        match super::attack_npc(&mut bot, 1, false) {
            Err(crate::ScriptError::Blocked { mut missing }) => {
                missing.sort();
                assert_eq!(missing, vec!["airrune".to_string(), "chaosrune".to_string(), "firerune".to_string()]);
            }
            other => panic!("nothing anywhere blocks, got {other:?}"),
        }
    }

    /// A climb (no `--best-spell`) casts its ordered spell only: out of
    /// chaos runes, it blocks on them even with strike runes in the pack
    /// (XP per cost chose the spell; the brain obtains or re-picks).
    #[test]
    fn a_climb_never_swaps_its_spell() {
        let mut bot = fighter(45, "crumble_undead", &[("airrune", 50), ("earthrune", 80), ("mindrune", 20)], &[]);
        bot.combat_spell_best = false;
        match super::attack_npc(&mut bot, 1, true) {
            Err(crate::ScriptError::Blocked { missing }) => assert_eq!(missing, vec!["chaosrune".to_string()]),
            other => panic!("the climb blocks on its own runes, got {other:?}"),
        }
    }
}
