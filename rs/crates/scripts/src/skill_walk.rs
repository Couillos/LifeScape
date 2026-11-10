//! Checked walking for skill scripts: one plan per trip, controls
//! between legs. Split from `skill_common` (file-size rule);
//! `skill_common` re-exports both so callers do not change.
//!
//! Every wait is tick-bounded (`bot.world.ticks`); nothing sleeps. Errors
//! are typed, never swallowed.

#![forbid(unsafe_code)]

use std::sync::Arc;

use lifescape_sdk::client::travel::{install_between_legs, remove_between_legs};
use lifescape_sdk::client::travel_hook::{BetweenLegsHook, LegInfo, LegVerdict};
use lifescape_sdk::{Bot, Error};

use crate::kit::survival::{check_events, hitpoints};
use crate::ScriptError;

/// Remote walking-cost refinement, shared by the foe/spot pickers (network
/// in `skill_walk`, never in gather/nav): one `walk_costs` batch over the
/// Manhattan-ordered head (never more than `shortlist` plans), pure
/// `lifescape_nav::walk_rank` reorder, single upset log with `label`.
/// Unwired or all-disconnected keeps the Manhattan order. Callers pass
/// Manhattan order in and their own K (`FOE_SHORTLIST`, `WALK_SHORTLIST`).
pub fn refine_walking<T>(
    bot: &mut Bot,
    cands: Vec<T>,
    shortlist: usize,
    label: &str,
    to_tile: impl Fn(&T) -> lifescape_nav::plan::Tile,
) -> Vec<(Option<u32>, T)>
where
    T: Eq + std::hash::Hash + Copy,
{
    if cands.len() < 2 {
        return cands.into_iter().map(|cand| (None, cand)).collect();
    }
    let first = cands[0];
    let from = to_tile(&first);
    if cands.len() > shortlist {
        bot.log(&format!("walk-refine {label}: {} cands, planning top-{shortlist} Manhattan (bound)", cands.len()));
    }
    let head = cands.len().min(shortlist);
    let dests: Vec<lifescape_nav::plan::Tile> = cands[..head].iter().map(&to_tile).collect();
    let costs = bot.walk_costs(&dests);
    let mut by_cand: std::collections::HashMap<T, Option<u32>> = std::collections::HashMap::new();
    for (cand, cost) in cands[..head].iter().copied().zip(costs) {
        by_cand.insert(cand, cost);
    }
    let ranked = lifescape_nav::walk_rank::rank_walk_shortlist(cands, shortlist, |cand| {
        by_cand.get(cand).copied().unwrap_or(None)
    });
    let (cost, winner) = (ranked[0].0, ranked[0].1);
    if winner != first {
        if let Some(walk) = cost {
            let into = to_tile(&winner);
            bot.log(&format!(
                "walk-refine {label}: Manhattan ({},{}) loses to ({},{}) walk {walk}",
                from.x, from.z, into.x, into.z
            ));
        }
    }
    ranked
}

/// One step toward the target through the 3x3 ring (nearest-first,
/// `walk_to` 100 each): shared by the gather `work_spot` loops. Conn
/// aborts propagate typed; other failures try the next tile. True when a
/// step landed, false when no ring tile was reachable (caller bans). Far
/// drift (>4) aborts the ring early.
pub fn step_adjacent(bot: &mut Bot, target_x: i32, target_z: i32) -> Result<bool, ScriptError> {
    let mut neighbors: Vec<(i32, i32)> = (-1..=1)
        .flat_map(|dx| (-1..=1).map(move |dz| (target_x + dx, target_z + dz)))
        .filter(|tile| *tile != (target_x, target_z))
        .collect();
    neighbors.sort_by_key(|tile| (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs());
    for (next_x, next_z) in neighbors {
        match bot.walk_to(next_x, next_z, bot.world.level, 100) {
            Ok(()) => return Ok(true),
            Err(error)
                if matches!(
                    error,
                    Error::Interrupted(_) | Error::Io(_) | Error::Login(_)
                ) =>
            {
                return Err(error.into())
            }
            Err(_) => {}
        }
        if (bot.world.x - target_x).abs() + (bot.world.z - target_z).abs() > 4 {
            return Ok(false);
        }
    }
    Ok(false)
}

/// Cooperative blockage recovery: the transparent pause gate cannot fire
/// from inside a failed `walk_to` (no way to pause under the error), so
/// the script runs the rescue itself, on this thread, then retries its
/// current cycle. Only `Unreachable` is rescued (mapped event rescue, no
/// blind fallback when no event shows); anything else propagates as-is.
/// Returns true when a rescue ran and the caller should retry.
pub fn rescue_then_retry(bot: &mut Bot, error: Error) -> Result<bool, ScriptError> {
    let Error::Unreachable(_) = error else {
        return Err(error.into());
    };
    let Some(kind) = crate::randomevent::event_at(bot) else {
        return Err(error.into());
    };
    let key = crate::randomevent::rescue_key(kind);
    bot.intend(&format!("blocked at {:?}, cooperative rescue {key}", bot.position()));
    bot.log(&format!("walk blocked at {:?}, running {key} rescue then retrying", bot.position()));
    crate::rescues::run_rescue(key, bot, &[])?;
    Ok(true)
}

/// One trip to the target (D65): a single `walk_to` (one plan, executed
/// leg by leg) with the between-legs control installed. Random events,
/// injury and death ride the hook between two sent legs — the executor
/// aborts the trip on the verdict, this function maps it: death is a typed
/// abort (the job re-gears), a limp with no event pending ends the walk
/// with Ok(false) so the caller flees, a first `Unreachable` runs the
/// cooperative rescue and retries the trip, a second one errors loudly.
/// A final-leg Ok on a blocked décor tile (the router settles on the
/// adjacent tile) ends the walk with Ok(true): the caller clicks from there.
pub fn walk_checked(bot: &mut Bot, dest_x: i32, dest_z: i32, timeout: u64) -> Result<bool, ScriptError> {
    let deadline = bot.world.ticks + timeout;
    if (bot.world.x, bot.world.z) == (dest_x, dest_z) {
        return Ok(true);
    }
    // Already dead (respawn pending): no point walking a corpse.
    if hitpoints(bot) == 0 {
        return Err(ScriptError::Usage(format!(
            "died walking to {dest_x},{dest_z} at {:?}",
            bot.position()
        )));
    }
    check_events(bot)?;
    let mut rescued = false;
    loop {
        if bot.world.ticks > deadline {
            return Err(Error::Timeout(format!(
                "walk to {dest_x},{dest_z} timed out at {:?} ({timeout} ticks)",
                bot.position()
            ))
            .into());
        }
        if (bot.world.x, bot.world.z) == (dest_x, dest_z) {
            return Ok(true);
        }
        if hitpoints(bot) == 0 {
            return Err(ScriptError::Usage(format!(
                "died walking to {dest_x},{dest_z} at {:?}",
                bot.position()
            )));
        }
        let rest = deadline.saturating_sub(bot.world.ticks).max(500).min(6000);
        // The verdict enum is frozen (P0.2): hurt has no verdict of its
        // own, so it aborts as `RandomEvent` and this function tells the
        // limp from the event through live state (hp vs event present).
        let hitpoints_before = hitpoints(bot);
        let hook: BetweenLegsHook = Arc::new(move |hook_bot: &mut Bot, _: &LegInfo| {
            if hitpoints(hook_bot) == 0 {
                return LegVerdict::Dead;
            }
            if crate::randomevent::event_at(hook_bot).is_some() {
                return LegVerdict::RandomEvent;
            }
            if hitpoints(hook_bot) < hitpoints_before {
                return LegVerdict::RandomEvent;
            }
            LegVerdict::Continue
        });
        install_between_legs(bot, hook);
        let outcome = bot.walk_to(dest_x, dest_z, bot.world.level, rest);
        remove_between_legs(bot);
        match outcome {
            // Blocked décor (tree, rock, booth) never converges on its exact
            // tile: the router settles on the BFS-nearest reachable tile and
            // reports Ok. That adjacent arrival is the click spot, not a
            // reason to walk again until the budget burns out.
            Ok(()) => return Ok(true),
            Err(err) => {
                bot.log(&format!("walk blocked: {err}"));
                // Death is not a flesh wound: surface it typed so callers
                // re-gear and continue the job instead of mistaking a
                // corpse for a limp.
                if hitpoints(bot) == 0 {
                    return Err(ScriptError::Usage(format!(
                        "died walking to {dest_x},{dest_z} at {:?}",
                        bot.position()
                    )));
                }
                // A limp with no event pending: the caller flees, never a
                // rescue (the hook aborted the trip on the limp itself).
                if hitpoints(bot) < hitpoints_before && crate::randomevent::event_at(bot).is_none() {
                    bot.log("hurt walking, fleeing");
                    return Ok(false);
                }
                if rescued {
                    return Err(err.into());
                }
                rescued = true;
                // The rescue ran (or the error replaces the walk failure):
                // the loop retries the trip from here.
                rescue_then_retry(bot, err)?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{rescue_then_retry, walk_checked};
    use crate::kit::pack::obj_id;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::{Bot, Error};

    /// Bot with a faked carried cube: canonical pack com plus one cube
    /// slot, so `event_at` sees the Cube event without any server.
    fn bot_with_cube() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let cube = obj_id(&bot, "macro_cube").expect("macro_cube in dumps");
        let com = (bot.tables.com_id)("inventory:inv").expect("inventory:inv in dumps");
        bot.world.level = 0;
        bot.world.x = 3220;
        bot.world.z = 3220;
        bot.world.inv.insert(com, [(0u8, (cube, 1u32))].into_iter().collect());
        assert!(crate::randomevent::cube_in_pack(&bot), "faked cube reads as carried");
        bot
    }

    /// Corpse check: hp flatlined means death, typed abort for the job
    /// to re-gear on — never mistaken for a limp, never needing a server.
    #[test]
    fn walk_checked_dead_is_typed_abort() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3220;
        bot.world.z = 3220;
        bot.world.stats.insert(3, (0, 0));
        let err = walk_checked(&mut bot, 3100, 3100, 500).expect_err("corpse does not walk");
        assert!(crate::kit::survival::is_death_abort(&err), "typed death abort, got: {err}");
    }

    #[test]
    fn blockage_without_event_propagates() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3220;
        bot.world.z = 3220;
        let error = Error::Unreachable("wall".to_string());
        let outcome = rescue_then_retry(&mut bot, error);
        assert!(matches!(outcome, Err(crate::ScriptError::Sdk(Error::Unreachable(_)))));
    }

    #[test]
    fn non_blockage_errors_pass_through() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3220;
        bot.world.z = 3220;
        let outcome = rescue_then_retry(&mut bot, Error::Timeout("slow".to_string()));
        assert!(matches!(outcome, Err(crate::ScriptError::Sdk(Error::Timeout(_)))));
    }

    /// First Unreachable with an event present runs the mapped rescue: the
    /// walk error is replaced by the rescue's outcome, never swallowed.
    /// Offline the cube rescue fails at its first send, which still proves
    /// the rescue ran (a skipped rescue would surface the walk error).
    #[test]
    fn walk_checked_runs_rescue_on_first_blockage() {
        let mut bot = bot_with_cube();
        let outcome = rescue_then_retry(&mut bot, Error::Unreachable("wall".to_string()));
        let error = format!("{}", outcome.expect_err("rescue runs and surfaces its own failure"));
        assert!(error.contains("not logged in"), "rescue ran and surfaced its own failure, got: {error}");
        assert!(!error.contains("nothing reachable"), "walk error replaced by the rescue outcome, got: {error}");
    }

    /// First walk failure with no service: no `plan_client` means the
    /// plan step fails `nav-down` before any socket exists — never a
    /// local plan. The typed travel error propagates untouched so the
    /// caller retries through its own loop — no rescue, no silent guess.
    #[test]
    fn walk_checked_fails_loud_without_event() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        crate::wire_nav(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3220;
        bot.world.z = 3220;
        let outcome = walk_checked(&mut bot, 3100, 3100, 500);
        let error = outcome.expect_err("no service fails loudly");
        assert!(
            matches!(&error, crate::ScriptError::Sdk(Error::Travel(detail)) if detail.contains("nav-down")),
            "nav-down without sync plan, got: {error}"
        );
    }

    /// Logging does not change the return: every `walk_to` failure is
    /// logged as "walk blocked: ..." before the rescue/return. Without
    /// a service the failure is the typed `nav-down` travel error.
    #[test]
    fn walk_checked_logs_blockage_without_changing_return() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        crate::wire_nav(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3220;
        bot.world.z = 3220;
        let outcome = walk_checked(&mut bot, 3100, 3100, 500);
        let error = outcome.expect_err("blocked walk fails loudly");
        assert!(
            matches!(&error, crate::ScriptError::Sdk(Error::Travel(detail)) if detail.contains("nav-down")),
            "typed nav-down, got: {error}"
        );
    }

    /// One trip, one plan: a far destination plans once from the current
    /// position (no 40-tile clamp, no 20-tile fallback). Without a service
    /// the single plan step is already `nav-down`, so the walk fails loud
    /// before any send — the typed error propagates untouched.
    #[test]
    fn walk_checked_single_trip_fails_nav_down_without_service() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        crate::wire_nav(&mut bot);
        bot.world.level = 0;
        bot.world.x = 2948;
        bot.world.z = 3368;
        let outcome = walk_checked(&mut bot, 2998, 3328, 500);
        let error = outcome.expect_err("no service fails the walk");
        assert!(
            matches!(&error, crate::ScriptError::Sdk(Error::Travel(detail)) if detail.contains("nav-down")),
            "nav-down before any fallback, got: {error}"
        );
    }

    /// Blocked décor (Draynor bank booth): with a service `walk_checked`
    /// ends on the zero-segment plan with Ok(true). Without one the
    /// plan step itself is `nav-down`, so the walk fails loud instead
    /// of settling — no send, no tick, no silent click spot.
    #[test]
    fn walk_checked_blocked_decor_fails_nav_down_without_service() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        crate::wire_nav(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3092;
        bot.world.z = 3243;
        let error = walk_checked(&mut bot, 3091, 3242, 600).expect_err("no service, no adjacent settle");
        assert!(
            matches!(&error, crate::ScriptError::Sdk(Error::Travel(detail)) if detail.contains("nav-down")),
            "nav-down, not a silent settle, got: {error}"
        );
    }

    /// Already there: no plan, no send, no service needed.
    #[test]
    fn walk_checked_same_tile_is_noop() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3220;
        bot.world.z = 3220;
        assert!(walk_checked(&mut bot, 3220, 3220, 500).expect("no trip for no distance"));
    }

    /// Unwired Manhattan: no service means no costs, order untouched.
    #[test]
    fn refine_unwired_keeps_manhattan() {
        use super::refine_walking;
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3230;
        bot.world.z = 3241;
        let cands = vec![(7u16, 3252, 3237), (9u16, 3203, 3246)];
        let ranked = refine_walking(&mut bot, cands, 3, "foe", |(_, x, z)| {
            lifescape_nav::plan::Tile { x: *x, z: *z, level: 0 }
        });
        let order: Vec<_> = ranked.into_iter().map(|(_, cand)| cand).collect();
        assert_eq!(order, vec![(7u16, 3252, 3237), (9u16, 3203, 3246)]);
    }

    /// Lure battu: Manhattan-nearest loses to the walk-cheap foe, one
    /// remote batch, L1-cached on repeat.
    #[test]
    fn refine_lure_loses_with_service() {
        use super::refine_walking;
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        crate::wire_nav(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3230;
        bot.world.z = 3241;
        let world = bot.nav_world.clone().expect("wired");
        let tables = bot.nav_tables.clone().expect("wired");
        let service = lifescape_nav::service::Service::start(
            lifescape_nav::service::Snapshot::new(world, tables, "test-walk-refine".to_string()),
            &lifescape_nav::service::test_socket("walk-refine"),
            1,
        )
        .expect("service starts");
        bot.plan_client = Some(lifescape_nav::service::PlanClient::new(service.socket_path(), "refine"));
        bot.plan_data_version = Some("test-walk-refine".to_string());
        bot.last_keepalive = Some(std::time::Instant::now());
        let ranked = refine_walking(&mut bot, vec![(7u16, 3252, 3237), (9u16, 3203, 3246)], 3, "foe", |(_, x, z)| {
            lifescape_nav::plan::Tile { x: *x, z: *z, level: 0 }
        });
        let order: Vec<_> = ranked.into_iter().map(|(_, cand)| cand).collect();
        assert_eq!(order, vec![(9u16, 3203, 3246), (7u16, 3252, 3237)], "walk-cheap wins, got: {order:?}");
    }

    /// Borne K: 5 cands with shortlist 2 cost at most 2 plans (tail rides
    /// along in order, never costed).
    #[test]
    fn refine_bounds_batch_to_shortlist() {
        use super::refine_walking;
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        crate::wire_nav(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3230;
        bot.world.z = 3241;
        let world = bot.nav_world.clone().expect("wired");
        let tables = bot.nav_tables.clone().expect("wired");
        let service = lifescape_nav::service::Service::start(
            lifescape_nav::service::Snapshot::new(world, tables, "test-walk-bound".to_string()),
            &lifescape_nav::service::test_socket("walk-bound"),
            1,
        )
        .expect("service starts");
        bot.plan_client = Some(lifescape_nav::service::PlanClient::new(service.socket_path(), "bound"));
        bot.plan_data_version = Some("test-walk-bound".to_string());
        bot.last_keepalive = Some(std::time::Instant::now());
        let before = service.stats();
        let cands = vec![(1u16, 3252, 3237), (2u16, 3203, 3246), (3u16, 3220, 3220), (4u16, 3210, 3210), (5u16, 3200, 3200)];
        let ranked = refine_walking(&mut bot, cands, 2, "foe", |(_, x, z)| {
            lifescape_nav::plan::Tile { x: *x, z: *z, level: 0 }
        });
        assert_eq!(ranked.len(), 5, "tail rides along, never dropped");
        let after = service.stats();
        assert!(after.misses - before.misses <= 2, "never more plans than the shortlist, got: {:?}", after);
    }

    /// No service: every ring `walk_to` fails `nav-down`, which is not a
    /// conn abort — `step_adjacent` swallows it per tile by contract and
    /// reports no step landed. The direct `walk_to` below proves the root
    /// cause is `nav-down` and nothing else (never a local plan).
    #[test]
    fn step_adjacent_offline_yields_no_step() {
        use super::step_adjacent;
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        crate::wire_nav(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3220;
        bot.world.z = 3220;
        assert!(!step_adjacent(&mut bot, 3225, 3225).expect("nav-down per tile degrades to no-step"), "no step without a service");
        let error = bot.walk_to(3224, 3224, bot.world.level, 100).expect_err("direct walk is nav-down");
        assert!(
            matches!(&error, Error::Travel(detail) if detail.contains("nav-down")),
            "nav-down and nothing else, got: {error}"
        );
    }
}
