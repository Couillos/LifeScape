//! Ground pickup (S): coins, tools, eggs and bones off the floor.
//! Declaration + sweep particularités on the kit: nearest-first takes
//! (coins first, like `kit::loot`), banked sweeps through `kit::cycle`.
//! No level, no tool, no station (catalogue §6; eggs P1, the rest P2).

#![forbid(unsafe_code)]

use lifescape_sdk::{Bot, Error};

use crate::kit::bank::{bank_trip_resilient, nearest_bank};
use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::loot::TAKE_TICKS;
use crate::kit::pack::{free_slots, obj_id};
use crate::kit::survival::{check_events, is_death_abort, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};

/// Sweep radius around the bot (same reach as the kill loot).
pub const PICKUP_RADIUS: i32 = 6;
/// Empty sweeps before the run ends loud instead of idling on bare floor.
pub const EMPTY_SWEEPS_LIMIT: u32 = 5;
/// Floor resources besides coins (catalogue §4 + §6, all F2P in the dump):
/// starter tools, spider eggs, big bones, gold ore spawns.
pub const WANTED: [&str; 12] = [
    "bronze_pickaxe",
    "bronze_axe",
    "net",
    "tinderbox",
    "hammer",
    "chisel",
    "shears",
    "bucket_empty",
    "cake_tin",
    "red_spiders_eggs",
    "big_bones",
    "gold_ore",
];

/// Coins-first, nearest-first order (the loot policy, minus its bones
/// skip: big bones ARE wanted here). Pure so the order stays testable.
fn rank_pickups(
    items: Vec<((i32, i32, i32), u16)>,
    at: (i32, i32),
    coins: u16,
) -> Vec<((i32, i32, i32), u16)> {
    let mut ranked = items;
    ranked.sort_by_key(|(tile, oid)| (if *oid == coins { 0 } else { 1 }, (tile.0 - at.0).abs() + (tile.1 - at.1).abs()));
    ranked
}

/// Pick banked sweeps in the open session: `ground_pickup [count]`.
pub fn run_ground_pickup(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = crate::kit::cycle::parse_cli("ground_pickup", args)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let coins = obj_id(bot, "coins")?;
    let mut wanted: Vec<u16> = vec![coins];
    for name in WANTED {
        wanted.push(obj_id(bot, name)?);
    }
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let mut empty = 0u32;
    run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        bank = nearest_bank(step_bot)?;
        let here = (step_bot.world.x, step_bot.world.z);
        let floor: Vec<((i32, i32, i32), u16)> = step_bot
            .world
            .ground_items(here.0, here.1, step_bot.world.level, PICKUP_RADIUS, None)
            .into_iter()
            .map(|((tile, oid), _)| (tile, oid))
            .filter(|(_, oid)| wanted.contains(oid))
            .collect();
        let mut taken = 0u32;
        for (_, oid) in rank_pickups(floor, here, coins) {
            if free_slots(step_bot) == 0 {
                break;
            }
            match step_bot.take(Some(oid), None, TAKE_TICKS) {
                Ok(true) => taken += 1,
                Ok(false) => step_bot.log("pickup gave nothing, moving on"),
                Err(error)
                    if matches!(error, Error::Interrupted(_) | Error::Io(_) | Error::Login(_)) =>
                {
                    return Err(error.into())
                }
                Err(error) => step_bot.log(&format!("pickup failed: {error}")),
            }
        }
        if taken == 0 {
            empty += 1;
            if empty >= EMPTY_SWEEPS_LIMIT {
                return Err(ScriptError::Failed(format!("nothing to pick near {here:?} at {:?}", step_bot.position())));
            }
            step_bot.log("sweep empty, waiting for respawns");
            for _ in 0..10 {
                step_bot.pump(2)?;
            }
            return Ok(StepControl::Continue);
        }
        empty = 0;
        let keep: Vec<u16> = task_keep.clone();
        match bank_trip_resilient(step_bot, &mut bank, &keep, 0, &wanted) {
            Ok(_) => {}
            Err(error) if is_death_abort(&error) => {
                step_bot.log("died on the bank trip, sweeping again");
                return Ok(StepControl::Continue);
            }
            Err(error) => return Err(error),
        }
        step_bot.log(&format!("ok sweep {} banked", state.units() + 1));
        Ok(StepControl::UnitDone)
    })
}

#[cfg(test)]
mod tests {
    use super::{rank_pickups, EMPTY_SWEEPS_LIMIT, PICKUP_RADIUS, WANTED};
    use crate::kit::pack::obj_id;
    use crate::tables_live::attach_live_tables;
    use lifescape_gamedata as knowledge;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Declaration resolves against the live tables: every wanted name
    /// (plus coins) is a real object id.
    #[test]
    fn wanted_names_resolve() {
        let bot = offline_bot();
        obj_id(&bot, "coins").expect("coins in tables");
        for name in WANTED {
            obj_id(&bot, name).expect("wanted in tables");
        }
    }

    /// Declaration is F2P: every wanted row (and coins) is non-member in
    /// the dump (D63: the dump flag, not the spawn flags).
    #[test]
    fn wanted_are_f2p() {
        for name in ["coins"].into_iter().chain(WANTED) {
            let row = knowledge::objs_by_name(name).into_iter().next().expect("dump row");
            assert_eq!(row.members, Some(false), "{name} is F2P, got {:?}", row.members);
        }
    }

    /// Order: coins first, then nearest (the loot policy).
    #[test]
    fn coins_first_then_nearest() {
        let bot = offline_bot();
        let coins = obj_id(&bot, "coins").expect("coins");
        let bone = obj_id(&bot, "big_bones").expect("bones");
        let items = vec![((10, 0, 0), bone), ((5, 0, 0), coins), ((1, 0, 0), bone)];
        let ranked = rank_pickups(items, (0, 0), coins);
        assert_eq!(ranked[0].1, coins, "coins first, got: {ranked:?}");
        assert_eq!((ranked[1].0.0, ranked[2].0.0), (1, 10), "then nearest, got: {ranked:?}");
    }

    /// CLI: a bare number is a sweep count (the gather convention), no
    /// resource positional (the sweep takes every wanted kind).
    #[test]
    fn bare_count_parses() {
        let spec = crate::kit::cycle::parse_cli("ground_pickup", &["3".to_string()]).expect("count parses");
        assert_eq!(spec.targets.count, Some(3));
        assert!(spec.resource.is_empty(), "no resource positional");
    }

    /// Bounds: the sweep reaches as far as the kill loot, and an empty
    /// floor ends the run after a few waits, never idles.
    #[test]
    fn sweep_bounds_are_sane() {
        assert_eq!(PICKUP_RADIUS, crate::kit::loot::LOOT_RADIUS, "same reach as the kill loot");
        assert!(EMPTY_SWEEPS_LIMIT <= 10, "empty floor ends fast: {EMPTY_SWEEPS_LIMIT}");
    }
}
