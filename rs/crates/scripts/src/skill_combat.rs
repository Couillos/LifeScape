//! Melee farmer: best F2P spot for the level, best banked gear, banked food.
//! Rotates attack/strength/defence, eats when hurt, loots every drop except
//! bones, banks when the pack fills (gather loop, bank trip, sell, upgrade).

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::waits::wait_ticks;
use lifescape_sdk::Bot;

use crate::kit::gear::{available_gear, equip_plan, gear_plan, melee_level, prepare_at_bank};
use crate::kit::style::{fight_weapon_style, style_damage_key};
use crate::kit::survival::{bank_stocks_food, danger_error, food_count, DeathTally};
use crate::skill_common::{
    bury_bones, check_events, free_slots, walk_checked,
};
use crate::skill_gather::same_side;
use crate::{ScriptError, ScriptResult};

const LOOT_RADIUS: i32 = 6;
/// Worst-case take: the drop lands on the death tick (goblin.rs2 obj_add
/// right after npc_death), walk <= 6 tiles + click. 600 burned 12 s on every
/// vanished/unreachable item; failures now log so the bound stays honest.
const TAKE_TICKS: u64 = 100;











pub(crate) fn loot_ground(bot: &mut Bot) -> Result<u32, ScriptError> {
    let mut taken = 0u32;
    let mut items: Vec<((i32, i32, i32), u16)> = bot
        .world
        .ground_items(bot.world.x, bot.world.z, bot.world.level, LOOT_RADIUS, None)
        .into_iter()
        .map(|((tile, oid), _)| (tile, oid))
        .collect();
    items.sort_by_key(|(tile, _)| (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs());
    // Coins first: the goblin bootstrap exists to fund the axe, junk never
    // starves the coin pickup when the pack is nearly full.
    let coins = (bot.tables.find_obj_by_name)("coins");
    items.sort_by_key(|(_, oid)| if Some(*oid) == coins { 0 } else { 1 });
    for (_, oid) in items {
        if free_slots(bot) == 0 {
            break;
        }
        let name = (bot.tables.obj_name)(oid).unwrap_or_default();
        if name.is_empty() || name.to_lowercase().contains("bones") {
            continue;
        }
        // Dead socket, not a missed pickup: typed instead of skipping.
        match bot.take(Some(oid), None, TAKE_TICKS) {
            Ok(true) => taken += 1,
            Ok(false) => bot.log(&format!("loot {name} gave nothing at {:?}", bot.position())),
            Err(error)
                if matches!(
                    error,
                    lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)
                ) =>
            {
                return Err(error.into())
            }
            Err(error) => bot.log(&format!("loot {name} failed at {:?}: {error}", bot.position())),
        }
    }
    bury_bones(bot)?;
    Ok(taken)
}



/// Clusters on the bot's toll-fence flank (see skill_gather::same_side): the
/// crossing is never paid in melee, so an eastern cluster from a western
/// hunt is a ~220-tile detour per visit, never a shortcut. When every
/// cluster is across and the run did not opt into `toll`, the list stays
/// empty and the caller fails named instead of burning NoRoute dossiers.
/// Falls back to every cluster only when paid (an all-across spot still
/// hunts instead of idling once the gate is crossable).
fn same_flank_clusters(line: i32, home: i32, targets: &[(i32, i32, usize)], allow_paid: bool) -> Vec<(i32, i32, usize)> {
    let kept: Vec<(i32, i32, usize)> =
        targets.iter().filter(|(center_x, _, _)| same_side(line, *center_x, home)).copied().collect();
    if kept.is_empty() && allow_paid {
        targets.to_vec()
    } else {
        kept
    }
}

/// Kills still wanted: 0 means unlimited, so a plain subtraction panics
/// on the second cluster once anything died (t02 live).
fn remaining_kills(kills_limit: u32, total: u32) -> u32 {
    if kills_limit == 0 {
        return 0;
    }
    kills_limit.saturating_sub(total)
}

fn run_spot(
    bot: &mut Bot,
    spot: &knowledge::combat_spots::CombatSpot,
    kills_limit: u32,
    ticks_limit: u64,
    no_bank: bool,
) -> Result<(u32, bool, bool), ScriptError> {
    if spot.entrance.is_some() {
        // shortcut: trapdoors/ladders to underground areas are not traversed
        // (single-level router); surface spots only.
        return Err(ScriptError::Usage(format!(
            "spot {} is underground, not traversable from {:?}",
            spot.key,
            bot.position()
        )));
    }
    let targets = crate::kit::fight::ranked_clusters(bot, spot);
    if targets.is_empty() {
        bot.log(&format!("no spawn for {}", spot.key));
        return Ok((0, false, false));
    }
    // One flank per hunt: cluster centers across the toll fence would
    // each cost the ~220-tile foot detour (paid doors stay off all
    // hunt). The per-pick filter in attack_nearest stops the same
    // oscillation inside the fight; this stops it between clusters.
    let mut targets = targets;
    if let Some(line) = crate::skill_common::toll_gate_x(bot) {
        let kept = same_flank_clusters(line, bot.world.x, &targets, bot.allow_paid);
        if kept.len() != targets.len() {
            bot.log(&format!("toll fence line {line}: hunting {} of {} clusters on this flank", kept.len(), targets.len()));
        }
        // Unpaid and nothing on this flank: the router refuses the paid
        // gate, so every cluster would burn a NoRoute dossier and end
        // "ok killed 0" (t02 live). Fail named instead.
        if kept.is_empty() {
            return Err(ScriptError::Usage(format!(
                "spot {} is across the toll fence (line {line}) from {:?}: re-run with `toll` once the bot carries coins",
                spot.key,
                bot.position()
            )));
        }
        targets = kept;
    }
    let mut total = 0u32;
    let mut banked_trip = false;
    for (center_x, center_z, count) in targets {
        // Pace retries: instant walk failures consume no ticks, so an
        // unpaced loop machine-guns the server into kicking us.
        bot.pump(2)?;
        bot.log(&format!("spot {} target ({center_x},{center_z}) spawns={count}", spot.key));
        let mut walk_fails = 0u32;
        while kills_limit == 0 || total < kills_limit {
            // Hunt entry: the one full evaluation (pack + bank when near
            // one, pack only with --no-bank). Inside the hunt only a loot
            // that was picked up re-evaluates; a death ends the fight and
            // returns to the run for the re-pick (see fight()).
            if !no_bank {
                if prepare_at_bank(bot)? {
                    banked_trip = true;
                }
            } else {
                let available = available_gear(bot);
                equip_plan(bot, &gear_plan(bot, &available, fight_weapon_style, style_damage_key))?;
            }
            // A dead socket must reconnect, not limp to the next cluster:
            // propagate connection failures, swallow the rest as before.
            match walk_checked(bot, center_x, center_z, 15000) {
                Err(error) if crate::skill_common::is_conn_abort(&error) => return Err(error),
                Err(_) | Ok(false) => {
                    // A sticking door (or a step refused for a tick) must
                    // not exile the hunt across the map: re-bank and retry
                    // the same cluster, move on only on the third failure
                    // in a row (t02 live: Lumbridge pen doors).
                    walk_fails += 1;
                    if walk_fails >= 3 {
                        break;
                    }
                    wait_ticks(bot, 25)?;
                    continue;
                }
                Ok(true) => {
                    walk_fails = 0;
                }
            }
            {
                let (kills, bank_wanted, died) =
                    crate::kit::fight::fight(bot, spot, (center_x, center_z), remaining_kills(kills_limit, total), ticks_limit, no_bank)?;
                total += kills;
                if died {
                    return Ok((total, true, banked_trip));
                }
                if !bank_wanted || (kills_limit > 0 && total >= kills_limit) {
                    return Ok((total, false, banked_trip));
                }
            }
        }
    }
    Ok((total, false, banked_trip))
}

/// Auto spot: broke beginners (no food carried, none banked) cannot
/// trade hits, so chickens until the account can afford to bleed.
/// Stocked accounts keep best_spot() exactly (t02 live: foodless at
/// melee 2 died to cow chip damage every few kills).
fn auto_spot(bot: &Bot) -> knowledge::combat_spots::CombatSpot {
    let level = melee_level(bot);
    if level <= 2 && food_count(bot) == 0 && !bank_stocks_food(bot) {
        if let Some(chickens) = knowledge::combat_spots::by_key("chickens") {
            return chickens;
        }
    }
    knowledge::combat_spots::best_spot(level)
}


/// Fallback after a death at `killer`: nearest surface spot that fits the
/// current melee level and is no harder than the killer (existing
/// `min_level` data only, no new threshold), killers excluded. Gear is
/// unchanged since the death (re-wear from survivors only), so an
/// easier-or-equal level range stays within gear reach. Nearest cluster to
/// the respawn position wins so the walk back stays short. None when
/// nothing reasonable is on foot: the caller banks instead.
fn fallback_spot(
    bot: &Bot,
    killer: &knowledge::combat_spots::CombatSpot,
    excluded: &[&str],
) -> Option<knowledge::combat_spots::CombatSpot> {
    let level = melee_level(bot);
    let here = (bot.world.x, bot.world.z);
    knowledge::combat_spots::all_spots()
        .into_iter()
        .filter(|spot| spot.entrance.is_none())
        .filter(|spot| !excluded.iter().any(|key| *key == spot.key))
        .filter(|spot| spot.min_level <= level && spot.min_level <= killer.min_level)
        .filter_map(|spot| {
            crate::kit::fight::ranked_clusters(bot, &spot)
                .into_iter()
                .map(|(spot_x, spot_z, _)| (spot_x - here.0).abs() + (spot_z - here.1).abs())
                .min()
                .map(|dist| (dist, spot))
        })
        .min_by_key(|(dist, _)| *dist)
        .map(|(_, spot)| spot)
}

/// Melee farmer: `melee [spot ...] [--kills N] [--ticks N] [--no-bank]`.
pub fn run_melee(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let mut spots: Vec<String> = Vec::new();
    let mut kills = 0u32;
    let mut ticks = 0u64;
    let mut no_bank = false;
    if args.iter().any(|arg| arg == "toll") {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--kills" => {
                kills = args.get(index + 1).and_then(|arg| arg.parse().ok()).unwrap_or(0);
                index += 2;
            }
            "--ticks" => {
                ticks = args.get(index + 1).and_then(|arg| arg.parse().ok()).unwrap_or(0);
                index += 2;
            }
            "--no-bank" => {
                no_bank = true;
                index += 1;
            }
            other => {
                spots.push(other.to_string());
                index += 1;
            }
        }
    }
    crate::skill_common::wait_pack_streamed(bot)?;
    let target = bot.world.ticks + 60;
    while bot.world.ticks < target && (!bot.world.stats.contains_key(&0) || !bot.world.stats.contains_key(&3)) {
        bot.pump(5)?;
    }
    check_events(bot)?;
    let picked: Vec<knowledge::combat_spots::CombatSpot> = {
        let named: Vec<_> = spots.iter().filter_map(|key| knowledge::combat_spots::by_key(key)).collect();
        if named.is_empty() {
            vec![auto_spot(bot)]
        } else {
            named
        }
    };
    bot.log(&format!("melee={} spots={}", melee_level(bot), picked.iter().map(|spot| spot.key).collect::<Vec<_>>().join(",")));
    // Deaths re-pick, never identically: the killer spot is excluded for
    // the run (one death hunts the fallback or the next picked spot, two
    // consecutive deaths fail typed). Kills and bank trips reset the
    // streak: only back-to-back deaths with no progress between are fatal.
    let mut queue = picked;
    let mut tally = DeathTally::fresh();
    // Auto mode (no explicit spot): the pick is a suggestion, never an
    // order. Unpaid and across the toll fence, it re-picks walkable
    // instead of ending the run idle; explicit picks keep the Usage.
    let auto = spots.is_empty();
    while !queue.is_empty() {
        let next = queue.iter().position(|queued| !tally.excludes(queued.key));
        let Some(next) = next else {
            bot.log("all remaining spots excluded after deaths, stopping");
            break;
        };
        let spot = queue.remove(next);
        bot.log(&format!("=== {} ===", spot.key));
        bot.intend(&format!("melee at {}", spot.key));
        let (total, died, banked) = match run_spot(bot, &spot, kills, ticks, no_bank) {
            Err(crate::ScriptError::Usage(detail)) if auto && detail.contains("toll fence") => {
                if !tally.excludes(spot.key) {
                    tally.exclude(spot.key);
                }
                match fallback_spot(bot, &spot, tally.excluded()) {
                    Some(fallback) => {
                        bot.log(&format!("{} across the toll fence, falling back to {}", spot.key, fallback.key));
                        queue.insert(0, fallback);
                        continue;
                    }
                    None => return Err(crate::ScriptError::Usage(detail)),
                }
            }
            other => other?,
        };
        if total > 0 || banked {
            tally.note_progress();
        }
        bot.log(&format!("ok {} killed {total}", spot.key));
        if !died {
            continue;
        }
        tally.note_death(spot.key);
        if tally.fatal() {
            return Err(danger_error(spot.key, tally.deaths()));
        }
        if queue.iter().any(|queued| !tally.excludes(queued.key)) {
            bot.log(&format!("{} killed us, re-picking without it", spot.key));
            continue;
        }
        match fallback_spot(bot, &spot, tally.excluded()) {
            Some(fallback) => {
                bot.log(&format!("{} killed us, falling back to {}", spot.key, fallback.key));
                queue.insert(0, fallback);
            }
            None => {
                if !no_bank {
                    prepare_at_bank(bot)?;
                }
                bot.log(&format!("{} killed us, nothing safer on foot: banked and stopping", spot.key));
                break;
            }
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::{auto_spot, fallback_spot, remaining_kills};
    use crate::kit::survival::{DeathTally, danger_error};
    use crate::tables_live::attach_live_tables;
    use lifescape_gamedata as knowledge;
    use lifescape_sdk::Bot;

    fn stat_bot(attack: u8, defence: u8, strength: u8) -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.stats.insert(0, (attack, 0));
        bot.world.stats.insert(1, (defence, 0));
        bot.world.stats.insert(2, (strength, 0));
        bot
    }



    /// Unlimited (0) stays unlimited past the first cluster; a bound
    /// never goes negative.
    #[test]
    fn remaining_kills_never_underflows() {
        assert_eq!(remaining_kills(0, 0), 0);
        assert_eq!(remaining_kills(0, 5), 0, "unlimited hunt survives kills");
        assert_eq!(remaining_kills(10, 3), 7);
        assert_eq!(remaining_kills(10, 10), 0);
    }


    /// Broke beginners hunt chickens, stocked ones keep best_spot.
    #[test]
    fn auto_spot_shelters_the_foodless() {
        let broke = stat_bot(2, 2, 2);
        assert_eq!(auto_spot(&broke).key, "chickens");
        let mut stocked = stat_bot(2, 2, 2);
        let bank_com = (stocked.tables.com_id)("bank_main:inv").expect("bank com in dumps");
        stocked.world.inv.insert(bank_com, [(0u8, (315u16, 3u32))].into_iter().collect());
        assert_eq!(auto_spot(&stocked).key, "cows");
    }







    /// Toll-fence side test: the gate line splits flanks (Al-Kharid gate
    /// halves 2882/2883); same flank hunts together, never across.
    #[test]
    fn fence_side_splits_at_the_gate_line() {
        use crate::skill_gather::same_side;
        assert!(same_side(3268, 3262, 3255), "west flank together");
        assert!(same_side(3268, 3270, 3300), "east flank together");
        assert!(!same_side(3268, 3262, 3270), "across never targets");
        assert!(!same_side(3268, 3270, 3262), "across either way");
    }


    /// Cluster flank filter: mixed flanks keep the bot's side only; an
    /// all-across list falls back to everything only when paid, and
    /// stays empty when unpaid so the caller fails named (t02 live:
    /// unpaid al_kharid_warriors from the west burned NoRoute dossiers
    /// and ended "ok killed 0").
    #[test]
    fn clusters_stay_on_the_hunting_flank() {
        let mixed = vec![(3252, 3243, 16), (3274, 3213, 4)];
        assert_eq!(super::same_flank_clusters(3268, 3260, &mixed, false), vec![(3252, 3243, 16)], "east cluster dropped");
        assert_eq!(super::same_flank_clusters(3268, 3270, &mixed, true), vec![(3274, 3213, 4)], "west cluster dropped");
        let east_only = vec![(3274, 3213, 4)];
        assert_eq!(super::same_flank_clusters(3268, 3270, &east_only, true), east_only, "paid all-across still hunts");
        assert!(super::same_flank_clusters(3268, 3260, &east_only, false).is_empty(), "unpaid all-across stays empty");
    }

    /// Auto re-pick from the west never idles on an across-toll pick: the
    /// fallback is a surface spot at or below the killer tier with a
    /// same-flank cluster to hunt (t02 live: auto warriors from Lumbridge
    /// ended the run with Usage instead of hunting).
    #[test]
    fn auto_toll_fallback_hunts_the_home_flank() {
        let mut bot = stat_bot(17, 17, 17);
        bot.world.x = 3219;
        bot.world.z = 3219;
        bot.world.level = 0;
        let killer = knowledge::combat_spots::by_key("al_kharid_warriors").expect("warriors");
        let excluded = vec![killer.key];
        let fallback = fallback_spot(&bot, &killer, &excluded).expect("a walkable repli exists");
        assert_ne!(fallback.key, killer.key, "never re-pick the across spot");
        assert!(fallback.entrance.is_none(), "surface only, the router never rides down");
        assert!(fallback.min_level <= killer.min_level, "no harder than the across pick, got: {}", fallback.key);
        let targets = crate::kit::fight::ranked_clusters(&bot, &fallback);
        assert!(!targets.is_empty(), "fallback has same-level spawns");
        let kept = super::same_flank_clusters(3268, bot.world.x, &targets, false);
        assert!(!kept.is_empty(), "{} has no west-flank cluster to hunt", fallback.key);
    }





    /// One death excludes the killer from every later re-pick: the
    /// fallback is never the spot that just killed (t02 live: warriors
    /// -> Lumbridge -> warriors on foot, forever).
    #[test]
    fn death_excludes_the_killer_spot() {
        let bot = stat_bot(9, 9, 9);
        let killer = knowledge::combat_spots::by_key("cows").expect("cows");
        let mut tally = DeathTally::fresh();
        tally.note_death(killer.key);
        assert!(tally.excludes("cows"));
        let fallback =
            fallback_spot(&bot, &killer, tally.excluded()).expect("a same-tier repli exists");
        assert_ne!(fallback.key, "cows", "never re-pick the killer identically");
        assert!(fallback.entrance.is_none(), "surface only, the router never rides down");
        assert!(
            fallback.min_level <= killer.min_level,
            "no harder than what just killed, got: {}",
            fallback.key
        );
    }

    /// Nothing safer on foot (every surface spot excluded or above the
    /// melee level): no fallback, the caller banks instead of re-picking.
    #[test]
    fn no_fallback_when_nothing_reasonable() {
        let bot = stat_bot(2, 2, 2);
        let killer = knowledge::combat_spots::by_key("chickens").expect("chickens");
        let all: Vec<&str> =
            knowledge::combat_spots::all_spots().iter().map(|spot| spot.key).collect();
        assert!(
            fallback_spot(&bot, &killer, &all).is_none(),
            "everything excluded: bank, never re-pick"
        );
    }

    /// Two deaths in a row (no kill, no bank trip between) fail typed
    /// Unreachable instead of walking back a third time.
    #[test]
    fn two_deaths_fail_typed_unreachable() {
        let mut tally = DeathTally::fresh();
        tally.note_death("al_kharid_warriors");
        assert!(!tally.fatal(), "one death re-picks, never fails");
        tally.note_death("cows");
        assert!(tally.fatal(), "two consecutive deaths are fatal");
        let error = danger_error("cows", tally.deaths());
        assert!(
            matches!(&error, crate::ScriptError::Sdk(lifescape_sdk::Error::Unreachable(_))),
            "typed Unreachable, got: {error}"
        );
        assert!(error.to_string().contains("too dangerous after 2 deaths"), "explicit, got: {error}");
        assert!(error.to_string().contains("re-gear"), "points at the fix, got: {error}");
    }

    /// A kill or a bank trip between deaths starts a new streak (deaths
    /// with progress between are not consecutive); the killer stays out
    /// for the run.
    #[test]
    fn progress_resets_the_death_streak() {
        let mut tally = DeathTally::fresh();
        tally.note_death("cows");
        tally.note_progress();
        assert_eq!(tally.deaths(), 0, "kill/bank resets the streak");
        assert!(!tally.fatal());
        assert!(tally.excludes("cows"), "the killer stays out for the run");
        tally.note_death("goblins");
        assert!(!tally.fatal(), "progress between deaths: new streak, not fatal");
    }
}
