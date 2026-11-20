//! Site-wide depletion: every node of the brain's site (rocks of every
//! ore, trees) mined out or felled. User decision 2026-11-20 (« pour
//! l'adamantite, ça dépend du temps de marche pour accéder à un autre
//! site, si c'est plus long que l'attente, alors on attend »), for every
//! resource whose nodes deplete and respawn:
//!
//! - the wait here is the remaining respawn of the soonest node: the
//!   server rate (`rock_respawnrate`, `woodcutting_trees:respawnrate`)
//!   minus the ticks since the node was first seen depleted; a node whose
//!   depletion was not seen counts the full rate;
//! - the alternative is the brain's next site for the same resource
//!   (`--alt-site`, picked by the brain's own rules: safe, gates, not
//!   full, not cooled): the route walk there plus that site's own wait
//!   when the bot sees it depleted too;
//! - the bot waits on the spot when the wait is shorter or equal, else it
//!   reports `Blocked { depleted:<x>,<z>,<level>,<ticks> }`: the brain
//!   cools this site for the wait (no failure, no ban) and its next
//!   resolve sends the bot to the other site.
//!
//! Live 2026-11-20: ~82 failures a day on the adamantite sites (3242,3165
//! and 3300,3318): a launch on mined-out rocks read the 800-tick respawn
//! as « no rock spot » for the ~250-tick repick window, failed, and the
//! brain cooled the method.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use lifescape_sdk::Bot;

use crate::ScriptError;

/// Slack past the server rate before a node still seen depleted reads as
/// overdue (tick jitter; the player-count scale only shortens a respawn).
pub const RESPAWN_MARGIN_TICKS: u64 = 10;

/// Tag of a depleted site in `Blocked { missing }`:
/// `depleted:<x>,<z>,<level>,<wait ticks>` (the brain's site key, the
/// wait it cools the site for).
pub const DEPLETED_TAG: &str = "depleted:";

/// Per-node depletion stamps of one run: the tick each node was first
/// seen depleted. Run memory only (owned by the activity, never shared,
/// never persisted).
#[derive(Debug, Default)]
pub struct DepletionClock {
    first_seen: HashMap<(i32, i32), u64>,
}

impl DepletionClock {
    pub fn new() -> DepletionClock {
        DepletionClock { first_seen: HashMap::new() }
    }

    /// Record what the bot sees of `node` now: a depleted node keeps its
    /// first sighting (stamped now when new), a standing one forgets it.
    pub fn observe(&mut self, node: (i32, i32), depleted: bool, now: u64) {
        if depleted {
            self.first_seen.entry(node).or_insert(now);
        } else {
            self.first_seen.remove(&node);
        }
    }

    /// Ticks before `node` (seen depleted now) stands again, for a server
    /// rate of `rate` ticks: the rate minus the ticks since its first
    /// sighting; never seen before, the full rate. A node still depleted
    /// past the rate (respawned and mined again unseen) restarts its
    /// clock: the full rate again.
    pub fn remaining(&mut self, node: (i32, i32), rate: u64, now: u64) -> u64 {
        let seen = *self.first_seen.entry(node).or_insert(now);
        let elapsed = now.saturating_sub(seen);
        if elapsed > rate + RESPAWN_MARGIN_TICKS {
            self.first_seen.insert(node, now);
            return rate;
        }
        rate.saturating_sub(elapsed)
    }

    /// The soonest respawn among `nodes` (all seen depleted now), `None`
    /// for no node.
    pub fn soonest(&mut self, nodes: &[(i32, i32)], rate: u64, now: u64) -> Option<u64> {
        nodes.iter().map(|node| self.remaining(*node, rate, now)).min()
    }
}

/// The brain's other site, measured from the bot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OtherSite {
    pub site: (i32, i32, i32),
    /// Route walk from the bot, in ticks.
    pub walk_ticks: u64,
    /// That site's own wait when the bot sees every node of it depleted
    /// too (0 otherwise: nothing known against it).
    pub wait_there: u64,
}

/// Wait here or move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepletedChoice {
    /// Wait this many ticks on the spot.
    Wait(u64),
    /// The other site is sooner: the brain moves the bot there.
    Move(OtherSite),
}

/// Pure: wait `wait_here` ticks on the spot when it is shorter than or
/// equal to the walk to the other site plus that site's own wait; move
/// otherwise. No other site (none open, or no route to it): wait.
pub fn depleted_choice(wait_here: u64, other: Option<OtherSite>) -> DepletedChoice {
    match other {
        Some(other) if wait_here > other.walk_ticks.saturating_add(other.wait_there) => DepletedChoice::Move(other),
        _ => DepletedChoice::Wait(wait_here),
    }
}

/// `Blocked { depleted:<x>,<z>,<level>,<wait> }` for the brain's site.
pub fn depleted_error(site: (i32, i32, i32), wait_here: u64) -> ScriptError {
    let (x, z, level) = site;
    ScriptError::Blocked { missing: vec![format!("{DEPLETED_TAG}{x},{z},{level},{wait_here}")] }
}

/// The brain's other site (`Bot::alternative_site`) measured from the
/// bot: the route walk there (`walk_costs`, the nav service) and its own
/// wait (`wait_there`, the caller's reading of its nodes). `None` without
/// one, or when no route reaches it (logged): nowhere sooner to go.
pub fn other_site(bot: &mut Bot, wait_there: impl FnOnce(&mut Bot, (i32, i32, i32)) -> u64) -> Option<OtherSite> {
    let site = bot.alternative_site?;
    let (x, z, level) = site;
    let cost = bot.walk_costs(&[lifescape_nav::plan::Tile { x, z, level }]).into_iter().next().flatten();
    let Some(cost) = cost else {
        bot.log(&format!("other site ({x},{z},{level}): no route measured from {:?}, not an alternative", bot.position()));
        return None;
    };
    let walk_ticks = crate::kit::woodcut::travel_ticks(u64::from(cost));
    let wait_there = wait_there(bot, site);
    Some(OtherSite { site, walk_ticks, wait_there })
}

/// Decide for a site whose every node is depleted: `wait_here` against
/// the brain's other site, logged. The site the error names is the
/// brain's (`assigned_site`); without one there is nothing to move from.
pub fn decide(bot: &mut Bot, resource: &str, nodes: usize, wait_here: u64, wait_there: impl FnOnce(&mut Bot, (i32, i32, i32)) -> u64) -> DepletedChoice {
    let other = if bot.assigned_site.is_some() { other_site(bot, wait_there) } else { None };
    let choice = depleted_choice(wait_here, other);
    let against = match other {
        Some(other) => format!("other site ({},{},{}) walk {}t + wait {}t", other.site.0, other.site.1, other.site.2, other.walk_ticks, other.wait_there),
        None => "no other site".to_string(),
    };
    match choice {
        DepletedChoice::Wait(ticks) => bot.log(&format!("{nodes} {resource} node(s) depleted: respawn in {ticks}t vs {against}, waiting here")),
        DepletedChoice::Move(_) => bot.log(&format!("{nodes} {resource} node(s) depleted: respawn in {wait_here}t vs {against}, moving")),
    }
    choice
}

#[cfg(test)]
mod tests {
    use super::{depleted_choice, depleted_error, DepletedChoice, DepletionClock, OtherSite};
    use crate::ScriptError;

    /// User 2026-11-20 (adamantite): two rocks mined out on arrival (the
    /// depletion unseen: the full 800-tick server rate). The other site
    /// 1200 walk ticks away: wait here; 100 ticks away: move there.
    #[test]
    fn adamantite_waits_unless_the_other_site_is_nearer_than_the_respawn() {
        let rate = lifescape_gamedata::action_odds::rock_respawn("adamantite_ore").expect("adamantite respawns") as u64;
        assert_eq!(rate, 800, "mine.dbrow rock_respawnrate");
        let mut clock = DepletionClock::new();
        let wait_here = clock.soonest(&[(3300, 3318), (3301, 3318)], rate, 5_000).expect("two depleted rocks");
        assert_eq!(wait_here, 800, "unseen depletion: the full rate");
        let far = OtherSite { site: (3242, 3165, 0), walk_ticks: 1200, wait_there: 0 };
        assert_eq!(depleted_choice(wait_here, Some(far)), DepletedChoice::Wait(800));
        let near = OtherSite { site: (3242, 3165, 0), walk_ticks: 100, wait_there: 0 };
        assert_eq!(depleted_choice(wait_here, Some(near)), DepletedChoice::Move(near));
        assert_eq!(depleted_choice(wait_here, None), DepletedChoice::Wait(800), "no other site: wait");
        let tie = OtherSite { site: (3242, 3165, 0), walk_ticks: 800, wait_there: 0 };
        assert_eq!(depleted_choice(wait_here, Some(tie)), DepletedChoice::Wait(800), "a tie waits");
        let depleted_too = OtherSite { site: (3242, 3165, 0), walk_ticks: 100, wait_there: 750 };
        assert_eq!(depleted_choice(wait_here, Some(depleted_too)), DepletedChoice::Wait(800), "the other site's own wait counts");
    }

    /// The soonest node counts, minus the ticks since it was seen
    /// depleted; a standing sighting forgets the stamp; a node still
    /// depleted past its rate restarts the clock.
    #[test]
    fn the_clock_counts_from_the_first_sighting() {
        let mut clock = DepletionClock::new();
        clock.observe((1, 1), true, 100);
        clock.observe((1, 1), true, 300);
        clock.observe((2, 2), true, 500);
        assert_eq!(clock.soonest(&[(1, 1), (2, 2)], 800, 600), Some(300), "the older sighting is sooner");
        clock.observe((1, 1), false, 650);
        assert_eq!(clock.soonest(&[(1, 1), (2, 2)], 800, 700), Some(600), "a standing rock forgets its stamp");
        assert_eq!(clock.remaining((2, 2), 800, 1_300), 0, "due now");
        assert_eq!(clock.remaining((2, 2), 800, 2_000), 800, "overdue: mined again unseen, the full rate");
        assert_eq!(clock.soonest(&[], 800, 0), None);
    }

    #[test]
    fn the_move_names_the_site_and_the_wait() {
        match depleted_error((3300, 3318, 0), 800) {
            ScriptError::Blocked { missing } => assert_eq!(missing, vec!["depleted:3300,3318,0,800".to_string()]),
            other => panic!("typed block expected, got {other:?}"),
        }
    }
}
