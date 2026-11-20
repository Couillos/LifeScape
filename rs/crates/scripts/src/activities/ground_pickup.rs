//! Ground pickup (S): coins, tools, eggs and bones off the floor.
//! Declaration + sweep particularités on the kit: nearest-first takes
//! (coins first, like `kit::loot`), banked sweeps through `kit::cycle`.
//! No level, no tool, no station (catalogue §6; eggs P1, the rest P2).

#![forbid(unsafe_code)]

use lifescape_sdk::{Bot, Error};

use crate::kit::bank::{bank_trip_resilient, nearest_bank};
use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::loot::TAKE_TICKS;
use crate::kit::pack::obj_id;
use crate::kit::survival::{check_events, is_death_abort, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};
use lifescape_gamedata as knowledge;

/// Sweep radius around the bot (same reach as the kill loot).
pub const PICKUP_RADIUS: i32 = 6;
/// Slack past the table respawn (server tick jitter).
pub const RESPAWN_MARGIN_TICKS: u64 = 10;
/// Longest watch of a bare site: past it the progress guard
/// (`kit::progress::IDLE_TICKS`) would end the run anyway.
pub const WATCH_CAP_TICKS: u64 = crate::kit::progress::IDLE_TICKS;
/// Ticks between two looks at the floor while watching.
const WATCH_POLL_TICKS: u64 = 5;
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

/// Ticks a bare spawn site is watched before the run ends: the slowest
/// wanted object's respawn (`respawnrate`, `ObjType` default 100) plus a
/// margin, capped. Live 2026-11-17: the old bound (5 sweeps of
/// `pump(2)`, ~40 ticks) gave up on a steel sword (respawn 1000), a
/// cooked meat (200) or big bones (50) emptied by the previous take: 26
/// `nothing to pick` failures in 20 minutes.
pub fn respawn_watch_ticks(names: &[&str]) -> Result<u64, ScriptError> {
    let mut slowest = 0u64;
    for name in names {
        let row = knowledge::objs_by_name(name)
            .into_iter()
            .next()
            .ok_or_else(|| ScriptError::Failed(format!("no obj row for {name}: respawn unknown")))?;
        slowest = slowest.max(row.respawn_ticks.max(0) as u64);
    }
    Ok((slowest + RESPAWN_MARGIN_TICKS).min(WATCH_CAP_TICKS))
}

/// True once a site bare since `empty_since` has been watched for
/// `watch` ticks (the respawn bound): only then does the run end.
pub fn watch_over(empty_since: u64, now: u64, watch: u64) -> bool {
    now.saturating_sub(empty_since) >= watch
}

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

/// What a sweep leads to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SweepNext {
    /// The load is done (pack full or `--want` met): bank it.
    Bank,
    /// Room left and the want short: stay on the spawn, watch the respawn.
    Watch,
}

/// Pure: bank only a done load (`kit::want::load_done`), whatever this
/// sweep took. Live 2026-11-17 (oscar_cole): banking every non-empty
/// sweep walked Edgeville ↔ Graveyard of Shadows for one bone a trip.
pub fn after_sweep(load_done: bool) -> SweepNext {
    if load_done {
        SweepNext::Bank
    } else {
        SweepNext::Watch
    }
}

/// The brain's spawn site to walk to from `here`, `None` when the bot
/// already sweeps it (same level, within `PICKUP_RADIUS`). Pure. The walk
/// keeps the site's own level: from the Karamja boat deck (level 1) the
/// bot's level asked for (2907,3146,1), `Disconnected` (live 2026-11-19).
fn site_walk(here: (i32, i32, i32), site: (i32, i32, i32)) -> Option<(i32, i32, i32)> {
    let on_site = here.2 == site.2 && (here.0 - site.0).abs().max((here.1 - site.1).abs()) <= PICKUP_RADIUS;
    (!on_site).then_some(site)
}

/// Pick banked sweeps in the open session: `ground_pickup [item] [count]
/// [--site x,z,level] [--want item N]`. With an item (the brain's
/// `ground:` order) only that object is taken, at the brain's site.
pub fn run_ground_pickup(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let (resource, rest) = crate::kit::cycle::split_resource(args);
    let spec = crate::kit::cycle::parse_cli("ground_pickup", &rest)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    bot.assigned_site = spec.site;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let coins = obj_id(bot, "coins")?;
    let wanted_names: Vec<&str> = match resource.as_deref() {
        Some(item) => vec![item],
        None => std::iter::once("coins").chain(WANTED).collect(),
    };
    let mut wanted: Vec<u16> = Vec::new();
    for name in &wanted_names {
        wanted.push(obj_id(bot, name)?);
    }
    let watch = respawn_watch_ticks(&wanted_names)?;
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    // First tick the site was seen bare (reset by every take).
    let mut empty_since: Option<u64> = None;
    run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        // The brain's spawn site first: never sweep wherever the bot stands.
        let here = (step_bot.world.x, step_bot.world.z, step_bot.world.level);
        if let Some((site_x, site_z, site_level)) = step_bot.assigned_site.and_then(|site| site_walk(here, site)) {
            if !crate::skill_walk::walk_checked_at(step_bot, site_x, site_z, site_level, crate::kit::site::SITE_WALK_TICKS)? {
                return Err(ScriptError::Failed(format!("spawn site ({site_x},{site_z},{site_level}) not reached from {:?}", step_bot.position())));
            }
        }
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
        let until = state.want_goal(step_bot, &spec);
        for (_, oid) in rank_pickups(floor, here, coins) {
            // A full pack or the met `--want` ends the sweep: never a
            // pickup past the order.
            if crate::kit::want::load_done(step_bot, until) {
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
        if after_sweep(crate::kit::want::load_done(step_bot, until)) == SweepNext::Watch {
            if taken > 0 {
                // Room left: the next sweep watches the respawn here.
                empty_since = None;
                return Ok(StepControl::Continue);
            }
            let since = *empty_since.get_or_insert(step_bot.world.ticks);
            if watch_over(since, step_bot.world.ticks, watch) {
                // Other players took it: contention, never a failure (D4).
                if let Some(contended) = crate::kit::site::contended_error(step_bot) {
                    return Err(contended);
                }
                return Err(ScriptError::Failed(format!(
                    "nothing to pick near {here:?} at {:?} after watching {} ticks (respawn bound {watch})",
                    step_bot.position(),
                    step_bot.world.ticks.saturating_sub(since)
                )));
            }
            step_bot.log(&format!("sweep empty, waiting for respawns (<= {watch}t)"));
            step_bot.intend("waiting for a ground spawn to respawn");
            watch_floor(step_bot, here, &wanted, since + watch)?;
            return Ok(StepControl::Continue);
        }
        empty_since = None;
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

/// Watches the floor around `here` until a wanted object lies there, the
/// bot gets hurt (the next step decides) or the tick `until`; waits in
/// server ticks, events checked on the way.
fn watch_floor(bot: &mut Bot, here: (i32, i32), wanted: &[u16], until: u64) -> Result<(), ScriptError> {
    let hitpoints_before = crate::kit::survival::hitpoints(bot);
    let mut polled = bot.world.ticks;
    while bot.world.ticks < until {
        lifescape_sdk::waits::wait_ticks(bot, WATCH_POLL_TICKS)?;
        if bot.world.ticks >= polled + 20 {
            polled = bot.world.ticks;
            check_events(bot)?;
        }
        if crate::kit::survival::hitpoints(bot) < hitpoints_before {
            return Ok(());
        }
        let level = bot.world.level;
        if bot.world.ground_items(here.0, here.1, level, PICKUP_RADIUS, None).keys().any(|(_, oid)| wanted.contains(oid)) {
            return Ok(());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{after_sweep, rank_pickups, respawn_watch_ticks, site_walk, watch_over, SweepNext, PICKUP_RADIUS, RESPAWN_MARGIN_TICKS, WANTED};
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

    /// Live 2026-11-19 (shaw_sara, olivia_ellis, bananas at Karamja
    /// (2907,3146,0)): the boat leaves the bot on the deck, level 1; the
    /// site walk took the bot's level and asked for (2907,3146,1),
    /// `Disconnected`, three failures and a skill ban. The walk goes to
    /// the site's own level; a bot on another level always walks, even
    /// right above the site.
    #[test]
    fn the_site_walk_keeps_the_site_level() {
        assert_eq!(site_walk((2956, 3143, 1), (2907, 3146, 0)), Some((2907, 3146, 0)));
        assert_eq!(site_walk((2907, 3146, 1), (2907, 3146, 0)), Some((2907, 3146, 0)), "upstairs is not on site");
        assert_eq!(site_walk((2907 + PICKUP_RADIUS, 3146, 0), (2907, 3146, 0)), None, "inside the sweep radius");
        assert_eq!(site_walk((2907 + PICKUP_RADIUS + 1, 3146, 0), (2907, 3146, 0)), Some((2907, 3146, 0)));
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

    /// Bounds: the sweep reaches as far as the kill loot.
    #[test]
    fn sweep_reach_is_the_loot_reach() {
        assert_eq!(PICKUP_RADIUS, crate::kit::loot::LOOT_RADIUS, "same reach as the kill loot");
    }

    /// Live 2026-11-17 (tim_ford, luke_hill): a bare spawn site ended the
    /// run after ~40 ticks while the steel sword respawns in 1000 ticks,
    /// the cooked meat in 200, big bones in 50. The watch covers the
    /// slowest wanted respawn (`respawnrate`) plus a margin.
    #[test]
    fn bare_site_is_watched_for_the_respawn() {
        let sword = respawn_watch_ticks(&["steel_sword"]).expect("sword row");
        assert_eq!(sword, 1000 + RESPAWN_MARGIN_TICKS, "steel sword respawns in 1000 ticks");
        let meat = respawn_watch_ticks(&["cooked_meat"]).expect("meat row");
        assert_eq!(meat, 200 + RESPAWN_MARGIN_TICKS);
        let sweep: Vec<&str> = std::iter::once("coins").chain(WANTED).collect();
        let slowest = respawn_watch_ticks(&sweep).expect("sweep rows");
        assert!(slowest >= 500 + RESPAWN_MARGIN_TICKS, "gold ore (500) bounds the full sweep, got {slowest}");
        assert!(!watch_over(100, 100 + 40, sword), "40 ticks bare is not the end");
        assert!(watch_over(100, 100 + sword, sword), "a full respawn watched bare ends the run");
    }

    /// Live 2026-11-17 (oscar_cole, Graveyard of Shadows): one bone per
    /// Edgeville round trip (~450 ticks each) while the spawn respawns in
    /// 100: every non-empty sweep banked, 27 slots free. The load banks
    /// only once done (pack full or `--want` met); short of it the bot
    /// stays and watches the respawn.
    #[test]
    fn a_partial_load_stays_on_the_spawn() {
        assert_eq!(after_sweep(false), SweepNext::Watch, "one bone, pack not full: stay and watch the respawn");
        assert_eq!(after_sweep(true), SweepNext::Bank, "pack full or want met: bank");
    }
}
