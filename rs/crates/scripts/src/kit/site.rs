//! Site: the brain's `--site x,z,level` as the executor's anchor. The
//! bank serving a site is the one nearest the SITE (not the bot), and a
//! gatherer with nothing to pick walks to the site instead of waiting
//! where it stands (live 2026-11-13: sara_miller, `mining --site
//! 3294,3287,0` from Lumbridge, banked at Draynor and waited 16 rounds
//! "no rock spot" at (3232,3203) without a step toward Al Kharid mine).

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::spots::{nearest_bank_at, SITE_RADIUS};
use crate::ScriptError;

/// Walk budget to reach a far site (a cross-map trek).
pub const SITE_WALK_TICKS: u64 = 6000;

/// Area name in `access:<area>` when the only way to the site is the Al
/// Kharid toll gate and the brain did not pass `toll`.
pub const TOLL_AREA: &str = "al_kharid_toll";

/// Bank serving the run: nearest F2P bank to the assigned site when the
/// brain fixed one, else `None` (the caller keeps its own pick).
pub fn site_bank(bot: &Bot) -> Option<knowledge::banks::Bank> {
    let (x, z, level) = bot.assigned_site?;
    nearest_bank_at(x, z, level)
}

/// True when the bot stands inside the assigned site's working area (or
/// no site was assigned).
pub fn at_site(bot: &Bot) -> bool {
    bot.assigned_site.is_none_or(|(x, z, level)| level == bot.world.level && (bot.world.x - x).abs().max((bot.world.z - z).abs()) <= SITE_RADIUS)
}

/// Walk to the assigned site when the bot is outside it. `Ok(false)`
/// when there is nothing to do (no site, already there); `Ok(true)` once
/// arrived. A walk the planner refuses because only the toll gate leads
/// there (no `toll` flag) is `Blocked{access:al_kharid_toll}`; any other
/// failed trek is a typed failure, never an idle wait.
pub fn reach_site(bot: &mut Bot) -> Result<bool, ScriptError> {
    if at_site(bot) {
        return Ok(false);
    }
    let Some((x, z, _)) = bot.assigned_site else { return Ok(false) };
    bot.log(&format!("nothing to work here, walking to the assigned site ({x},{z})"));
    bot.intend(&format!("walking to the work site at ({x},{z})"));
    let fence = if bot.allow_paid { None } else { crate::kit::bank::toll_gate_x(bot) };
    match crate::skill_walk::walk_checked(bot, x, z, SITE_WALK_TICKS) {
        Ok(true) => Ok(true),
        Err(error) if crate::kit::survival::is_walk_abort(&error) => Err(error),
        outcome => {
            if crate::kit::bans::across_toll(bot.world.x, x, fence) {
                return Err(crate::kit::cycle::blocked_access(TOLL_AREA));
            }
            let reason = match outcome {
                Err(error) => error.to_string(),
                _ => "walk fell short".to_string(),
            };
            Err(ScriptError::Failed(format!("assigned site ({x},{z}) not reached from {:?}: {reason}", bot.position())))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{at_site, site_bank};
    use lifescape_sdk::Bot;

    /// sara_miller's order: Al Kharid mine from Lumbridge. The bank is
    /// the one nearest the mine (Al Kharid), never Draynor; the bot at
    /// Lumbridge is outside the site, at the rocks inside it.
    #[test]
    fn site_anchors_bank_and_presence() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.level = 0;
        bot.world.x = 3232;
        bot.world.z = 3203;
        assert!(site_bank(&bot).is_none(), "no site, caller's own pick");
        assert!(at_site(&bot), "no site reads as there");
        bot.assigned_site = Some((3294, 3287, 0));
        let bank = site_bank(&bot).expect("a bank near the mine");
        assert!((bank.x - 3269).abs() <= 3 && (bank.z - 3167).abs() <= 6, "Al Kharid bank, got ({},{})", bank.x, bank.z);
        assert!(!at_site(&bot), "Lumbridge is not the mine");
        bot.world.x = 3296;
        bot.world.z = 3290;
        assert!(at_site(&bot), "at the rocks");
    }
}
