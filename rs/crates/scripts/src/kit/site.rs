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
    bot.assigned_site.is_none_or(|site| within_site((bot.world.x, bot.world.z, bot.world.level), site))
}

/// Pure: `here` stands inside the working area of `site` (same level,
/// `SITE_RADIUS` tiles).
pub fn within_site(here: (i32, i32, i32), site: (i32, i32, i32)) -> bool {
    here.2 == site.2 && (here.0 - site.0).abs().max((here.1 - site.1).abs()) <= SITE_RADIUS
}

/// Pure: a market pause closed since `pauses_mark` (the session's
/// closed-pause count, `Bot::pauses_done`). Since 2026-11-19 the pause
/// leaves the bot at the Varrock square with the pack banked but the
/// tools (`session_rescue::gate_market`): a single-pass script that
/// walked to its site and carried its goods must take them and walk
/// again before it acts.
pub fn paused_since(pauses_mark: u64, pauses_now: u64) -> bool {
    pauses_now != pauses_mark
}

/// Leg of a single-pass script (take the goods, walk to the site, act
/// once there): shop sale, shop buy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassLeg {
    /// Take the goods (lots, coins) from the bank: first pass, or a
    /// market pause banked the pack meanwhile.
    Load,
    /// Walk to the site.
    Walk,
    /// Act at the site (keeper trade).
    Act,
}

/// Restarts from the bank one single pass may take after market pauses
/// (the SDK trip's own bound, `travel_pause::PAUSE_REPLANS`).
pub const PASS_PAUSE_RESTARTS: u32 = 2;

/// Pure: a load-and-act loop (tanning, dialogue buys) whose act came
/// back empty takes its load again instead of failing: a pause closed
/// since the load (`pauses_mark`) and fewer than `PASS_PAUSE_RESTARTS`
/// restarts were spent.
pub fn reload_after_pause(pauses_mark: u64, pauses_now: u64, restarts: u32) -> bool {
    paused_since(pauses_mark, pauses_now) && restarts < PASS_PAUSE_RESTARTS
}

/// Pure: the leg a single pass runs next. `pauses_at_load` is the
/// closed-pause count when the goods were taken (`None`: not yet),
/// `pauses_now` the count now. A pause closed since then left the bot at
/// the Varrock square with the pack banked (live 2026-11-19 alice_swift
/// "trade with noterazzo failed at (0, 3185, 3436)", lara_lane "trade
/// gave no emerald": "You don't have enough coins"): load again. Off the
/// site: walk, once per load (`walked`): a walk that ended short of the
/// site's area (the router's nearest reachable tile, a keeper upstairs
/// such as Scavvo at level 1) acts from there as before, never an
/// endless walk. Else act. No site: act where the bot stands.
pub fn pass_leg(pauses_at_load: Option<u64>, pauses_now: u64, walked: bool, here: (i32, i32, i32), site: Option<(i32, i32, i32)>) -> PassLeg {
    match pauses_at_load {
        None => PassLeg::Load,
        Some(mark) if paused_since(mark, pauses_now) => PassLeg::Load,
        Some(_) if !walked && site.is_some_and(|site| !within_site(here, site)) => PassLeg::Walk,
        Some(_) => PassLeg::Act,
    }
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

    /// Live 2026-11-19 (lara_lane, `market shopbuy emerald`): a delivery
    /// paused the walk to the Al Kharid gem trader, banked the coins at
    /// Varrock; the walk replanned, the trade answered "You don't have
    /// enough coins" ("trade gave no emerald"). After a pause the buy
    /// takes its coins again before walking back; a buy with no site
    /// acts where it stands; the site's area is `SITE_RADIUS` wide.
    #[test]
    fn a_paused_single_pass_loads_again() {
        use super::{pass_leg, within_site, PassLeg};
        let gem_trader = Some((3287, 3211, 0));
        let varrock = (3185, 3436, 0);
        assert_eq!(pass_leg(None, 7, false, (3289, 3212, 0), gem_trader), PassLeg::Load, "coins first");
        assert_eq!(pass_leg(Some(7), 8, false, (3289, 3212, 0), gem_trader), PassLeg::Load, "back at the trader, coins banked");
        assert_eq!(pass_leg(Some(8), 8, false, varrock, gem_trader), PassLeg::Walk);
        assert_eq!(pass_leg(Some(8), 8, false, (3289, 3212, 0), gem_trader), PassLeg::Act);
        assert_eq!(pass_leg(Some(8), 8, false, varrock, None), PassLeg::Act, "no site: the keeper pick reads where we stand");
        assert_eq!(pass_leg(Some(8), 9, false, varrock, None), PassLeg::Load, "still: the coins are banked");
        // Lot 8 review: Scavvo trades upstairs (3191,3351,1); a walk that
        // ends on another floor or short of the area acts from there.
        let scavvo = Some((3191, 3351, 1));
        assert_eq!(pass_leg(Some(8), 8, false, (3191, 3351, 0), scavvo), PassLeg::Walk, "first walk");
        assert_eq!(pass_leg(Some(8), 8, true, (3191, 3351, 0), scavvo), PassLeg::Act, "walked once: act, never walk again");
        assert_eq!(pass_leg(Some(8), 9, true, (3191, 3351, 0), scavvo), PassLeg::Load, "a pause still reloads");
        assert!(within_site((3307, 3211, 0), (3287, 3211, 0)) && !within_site((3308, 3211, 0), (3287, 3211, 0)));
        assert!(!within_site((3287, 3211, 1), (3287, 3211, 0)), "another floor");
    }

    /// A tanning or dialogue-buy load emptied by a pause (hides and coins
    /// banked at Varrock) is taken again, twice at most; an empty act
    /// with no pause stays a failure.
    #[test]
    fn an_emptied_load_is_taken_again_after_a_pause() {
        use super::{reload_after_pause, PASS_PAUSE_RESTARTS};
        assert!(reload_after_pause(4, 5, 0), "a pause banked the load");
        assert!(!reload_after_pause(4, 4, 0), "no pause: the act really failed");
        assert!(!reload_after_pause(4, 6, PASS_PAUSE_RESTARTS), "bounded");
    }
}
