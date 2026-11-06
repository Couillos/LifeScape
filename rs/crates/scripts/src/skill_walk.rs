//! Checked walking for skill scripts: 40-tile legs with event checks,
//! cooperative rescue on blockage. Split from `skill_common` (file-size
//! rule); `skill_common` re-exports both so callers do not change.
//!
//! Every wait is tick-bounded (`bot.world.ticks`); nothing sleeps. Errors
//! are typed, never swallowed.

#![forbid(unsafe_code)]

use lifescape_sdk::{Bot, Error};

use crate::skill_common::{check_events, hitpoints};
use crate::ScriptError;

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

/// Chunked `walk_to` in 40-tile legs with event checks: a hurt walk aborts
/// (Ok(false)) so the caller can flee, a first `Unreachable` runs the
/// cooperative rescue and retries the leg, a second one errors loudly.
/// A final-leg Ok on a blocked décor tile (the router settles on the
/// adjacent tile) ends the walk with Ok(true): the caller clicks from there.
pub fn walk_checked(bot: &mut Bot, dest_x: i32, dest_z: i32, timeout: u64) -> Result<bool, ScriptError> {
    let deadline = bot.world.ticks + timeout;
    let mut hitpoints_before = hitpoints(bot);
    let mut last_greet = bot.world.ticks;
    let mut rescued = false;
    while (bot.world.x, bot.world.z) != (dest_x, dest_z) {
        if bot.world.ticks > deadline {
            return Err(Error::Timeout(format!(
                "walk to {dest_x},{dest_z} timed out at {:?} ({timeout} ticks)",
                bot.position()
            ))
            .into());
        }
        if bot.world.ticks - last_greet > 50 {
            last_greet = bot.world.ticks;
            check_events(bot)?;
        }
        // Already dead (respawn pending): no point walking a corpse.
        if hitpoints(bot) == 0 {
            return Err(ScriptError::Usage(format!(
                "died walking to {dest_x},{dest_z} at {:?}",
                bot.position()
            )));
        }
        let step_x = bot.world.x + (dest_x - bot.world.x).clamp(-40, 40);
        let step_z = bot.world.z + (dest_z - bot.world.z).clamp(-40, 40);
        let rest = deadline.saturating_sub(bot.world.ticks).max(500).min(6000);
        let final_leg = (step_x, step_z) == (dest_x, dest_z);
        match bot.walk_to(step_x, step_z, bot.world.level, rest) {
            // Blocked décor (tree, rock, booth) never converges on its exact
            // tile: the router stops on the BFS-nearest reachable tile and
            // reports Ok. That adjacent arrival is the click spot, not a
            // reason to re-walk until the budget burns out.
            Ok(()) if final_leg => return Ok(true),
            Ok(()) => {}
            Err(err) => {
                bot.log(&format!("walk blocked: {err}"));
                if rescued {
                    return Err(err.into());
                }
                rescued = true;
                rescue_then_retry(bot, err)?;
            }
        }
        let now_hitpoints = hitpoints(bot);
        // Death is not a flesh wound: surface it typed so callers re-gear
        // and continue the job instead of mistaking a corpse for a limp.
        if now_hitpoints == 0 {
            return Err(ScriptError::Usage(format!(
                "died walking to {dest_x},{dest_z} at {:?}",
                bot.position()
            )));
        }
        if now_hitpoints < hitpoints_before {
            bot.log("hurt walking, fleeing");
            return Ok(false);
        }
        hitpoints_before = now_hitpoints;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{rescue_then_retry, walk_checked};
    use crate::skill_common::obj_id;
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
        assert!(crate::skill_common::is_death_abort(&err), "typed death abort, got: {err}");
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
        let outcome = walk_checked(&mut bot, 3100, 3100, 500);
        let error = format!("{}", outcome.expect_err("blocked walk fails loudly"));
        assert!(error.contains("not logged in"), "rescue ran and surfaced its own failure, got: {error}");
        assert!(!error.contains("nothing reachable"), "walk error replaced by the rescue outcome, got: {error}");
    }

    /// First walk failure with no event is a dead socket offline (never
    /// logged in): the typed connection error propagates untouched so the
    /// session reconnects — no rescue, no retry, no silent fallback.
    #[test]
    fn walk_checked_fails_loud_without_event() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3220;
        bot.world.z = 3220;
        let outcome = walk_checked(&mut bot, 3100, 3100, 500);
        let error = outcome.expect_err("blocked walk fails loudly");
        assert!(format!("{error}").contains("not logged in"), "conn error propagates untouched, got: {error}");
        assert!(crate::skill_common::is_conn_abort(&error), "typed for the reconnect arm");
    }

    /// Logging does not change the return: every `walk_to` failure is now
    /// logged as "walk blocked: ..." (first and retry, router text kept)
    /// before the rescue/return. Offline the failure is the dead socket,
    /// still typed for the reconnect arm.
    #[test]
    fn walk_checked_logs_blockage_without_changing_return() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3220;
        bot.world.z = 3220;
        let outcome = walk_checked(&mut bot, 3100, 3100, 500);
        let error = outcome.expect_err("blocked walk fails loudly");
        assert!(crate::skill_common::is_conn_abort(&error), "typed conn error, got: {error}");
    }

    /// Blocked décor (tree on its tile) with the bot already adjacent:
    /// `walk_checked` ends on the first leg with Ok(true) — no re-walk to
    /// the budget, no sleep, barely a tick consumed out of the 600 allowed.
    #[test]
    fn walk_checked_blocked_decor_returns_true_fast() {
        use lifescape_sdk::collision::{self, MapPack};
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.maps = Some(MapPack::new(std::path::PathBuf::from("no-pack.zip"), std::collections::HashMap::new(), Vec::new()));
        bot.world.level = 0;
        bot.world.x = 3216;
        bot.world.z = 3241;
        for tile_x in 3214..=3218 {
            for tile_z in 3239..=3243 {
                bot.collision.set(tile_x, tile_z, 0, collision::OPEN);
            }
        }
        bot.collision.set(3217, 3241, 0, collision::WALK_BLOCKED);
        let before = bot.world.ticks;
        assert!(walk_checked(&mut bot, 3217, 3241, 600).expect("adjacent to blocked decor clicks from here"));
        assert_eq!((bot.world.x, bot.world.z), (3216, 3241));
        assert!(bot.world.ticks - before < 50, "settles at once, far inside the 600-tick budget");
    }
}
