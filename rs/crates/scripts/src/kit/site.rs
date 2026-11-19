//! Site: the brain's `--site x,z,level` as the executor's anchor. The
//! bank serving a site is the one nearest the SITE (not the bot), and a
//! gatherer with nothing to pick walks to the site instead of waiting
//! where it stands (live 2026-11-13: sara_miller, `mining --site
//! 3294,3287,0` from Lumbridge, banked at Draynor and waited 16 rounds
//! "no rock spot" at (3232,3203) without a step toward Al Kharid mine).

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::spots::SITE_RADIUS;
use crate::ScriptError;

/// Walk budget to reach a far site (a cross-map trek).
pub const SITE_WALK_TICKS: u64 = 6000;

/// Area name in `access:<area>` when the only way to the site is the Al
/// Kharid toll gate and the brain did not pass `toll`.
pub const TOLL_AREA: &str = "al_kharid_toll";

/// Bank serving the run when the brain fixed a site: the bank of the
/// offline route table (`gamedata::site_banks`, tour 3 point 2; tour 2
/// N3: the swamp mine is 27 tiles from Al Kharid as the crow flies, 467
/// walked; Draynor 170), the same bank the planner's site requirement
/// read (its danger included), else `None` (the caller keeps its own
/// pick). A Karamja site reads from the Port Sarim dock its boat lands
/// on.
pub fn site_bank(bot: &mut Bot) -> Option<knowledge::banks::Bank> {
    let (x, z, level) = bot.assigned_site?;
    knowledge::site_banks::serving(i64::from(x), i64::from(z), i64::from(level)).map(|served| served.bank)
}

/// True when the bot stands inside the assigned site's working area (or
/// no site was assigned).
pub fn at_site(bot: &Bot) -> bool {
    bot.assigned_site.is_none_or(|(x, z, level)| level == bot.world.level && (bot.world.x - x).abs().max((bot.world.z - z).abs()) <= SITE_RADIUS)
}

/// Tag of a contended site in `Blocked { missing }` (user decision D4,
/// 2026-11-17): the brain cools the site for this bot, counts no failure
/// and no ban, and the next resolve sends it to another site at once.
pub const CONTENDED_TAG: &str = "contended:";

/// Pure: a load that ended without its resource while other players
/// work the site is contention, not a broken method (plan P7b, live
/// 2026-11-17: "5 mithril_ore rocks mined out" with 23 miners on them
/// banned mining). `players` = other players' tiles, `here` the bot's.
pub fn contended_at(here: (i32, i32, i32), players: &[(i32, i32, i32)]) -> bool {
    players.iter().any(|(x, z, level)| *level == here.2 && (x - here.0).abs().max((z - here.1).abs()) <= SITE_RADIUS)
}

/// Other players within `SITE_RADIUS` of the bot (same level).
pub fn players_near(bot: &Bot) -> usize {
    let here = (bot.world.x, bot.world.z, bot.world.level);
    bot.world.players.values().filter(|player| player.level == here.2 && (player.x - here.0).abs().max((player.z - here.1).abs()) <= SITE_RADIUS).count()
}

/// `Blocked { contended:<x>,<z>,<level> }` for the assigned site (else
/// the bot's tile) when other players work within `SITE_RADIUS`; `None`
/// when the bot was alone (a real failure, counted).
pub fn contended_error(bot: &Bot) -> Option<ScriptError> {
    let here = (bot.world.x, bot.world.z, bot.world.level);
    let players: Vec<(i32, i32, i32)> = bot.world.players.values().map(|player| (player.x, player.z, player.level)).collect();
    if !contended_at(here, &players) {
        return None;
    }
    let (x, z, level) = bot.assigned_site.unwrap_or(here);
    Some(ScriptError::Blocked { missing: vec![format!("{CONTENDED_TAG}{x},{z},{level}")] })
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
    let Some((x, z, level)) = bot.assigned_site else { return Ok(false) };
    bot.log(&format!("nothing to work here, walking to the assigned site ({x},{z},{level})"));
    bot.intend(&format!("walking to the work site at ({x},{z})"));
    let fence = if bot.allow_paid { None } else { crate::kit::bank::toll_gate_x(bot) };
    // The site's own level, never the bot's: from the mill top (level 2)
    // a level-0 fishing site was asked on level 2 and failed Disconnected.
    match crate::skill_walk::walk_checked_at(bot, x, z, level, SITE_WALK_TICKS) {
        Ok(true) => Ok(true),
        Err(error) if crate::kit::survival::is_walk_abort(&error) => Err(error),
        outcome => {
            if crate::kit::bans::across_toll((bot.world.x, bot.world.z), (x, z), fence) {
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
    use crate::ScriptError;

    /// sara_miller's order: Al Kharid mine from Lumbridge. The bank is
    /// the one nearest the mine (Al Kharid), never Draynor; the bot at
    /// Lumbridge is outside the site, at the rocks inside it.
    #[test]
    fn site_anchors_bank_and_presence() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.level = 0;
        bot.world.x = 3232;
        bot.world.z = 3203;
        assert!(site_bank(&mut bot).is_none(), "no site, caller's own pick");
        assert!(at_site(&bot), "no site reads as there");
        bot.assigned_site = Some((3294, 3287, 0));
        let bank = site_bank(&mut bot).expect("a bank near the mine");
        assert!((bank.x - 3269).abs() <= 3 && (bank.z - 3167).abs() <= 6, "Al Kharid bank, got ({},{})", bank.x, bank.z);
        assert!(!at_site(&bot), "Lumbridge is not the mine");
        bot.world.x = 3296;
        bot.world.z = 3290;
        assert!(at_site(&bot), "at the rocks");
    }

    /// Live 2026-11-14 (lara_lane, george_97): stranded on the mill top
    /// (3165,3306,2), the fishing site (3085,3230,0) was walked "on the
    /// bot's level" and failed `Disconnected`. The walk targets the site's
    /// level: the plan rides the ladders down and the trip only fails at
    /// the offline socket, never on the plan.
    #[test]
    fn site_walk_targets_the_site_level() {
        let (mut bot, _service) = crate::served_nav_bot("site-level");
        bot.world.level = 2;
        bot.world.x = 3165;
        bot.world.z = 3306;
        bot.world.stats.insert(3, (30, 0));
        bot.assigned_site = Some((3085, 3230, 0));
        let error = super::reach_site(&mut bot).expect_err("offline: the trip cannot be sent");
        let text = error.to_string();
        assert!(!text.contains("Disconnected") && !text.contains("NoRoute"), "planned across levels, got: {text}");
    }

    /// Plan P7b / D4: no resource with other players on the site is a
    /// contention block, alone it stays a failure.
    #[test]
    fn a_stall_among_other_players_is_contention() {
        assert!(super::contended_at((3046, 9734, 0), &[(3049, 9737, 0)]));
        assert!(!super::contended_at((3046, 9734, 0), &[]), "alone: a real failure");
        assert!(!super::contended_at((3046, 9734, 0), &[(3046, 9734, 1)]), "another floor");
        assert!(!super::contended_at((3046, 9734, 0), &[(3146, 9734, 0)]), "far away");
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.x = 3046;
        bot.world.z = 9734;
        bot.assigned_site = Some((3047, 9735, 0));
        assert!(super::contended_error(&bot).is_none());
        bot.world.players.insert(7, lifescape_sdk::world::PlayerInfo { x: 3048, z: 9736, level: 0, jump: false, name: None });
        match super::contended_error(&bot) {
            Some(ScriptError::Blocked { missing }) => assert_eq!(missing, vec!["contended:3047,9735,0".to_string()]),
            other => panic!("contention, got {other:?}"),
        }
    }

    /// The bank serving an upper-floor site (the mill hopper, level 2) is
    /// a free surface bank, never "none" (mill_grain fell back to the
    /// bot's own pick and failed "no reachable bank").
    #[test]
    fn upper_floor_site_banks_on_the_surface() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.assigned_site = Some((3166, 3307, 2));
        let bank = site_bank(&mut bot).expect("a surface bank for the mill");
        assert!(!bank.members && bank.level == 0, "got {bank:?}");
    }
}
