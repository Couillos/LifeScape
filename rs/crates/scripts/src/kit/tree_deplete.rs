//! A felled woodcut site (`kit::deplete`, user 2026-11-20): every tree of
//! the brain's site a stump, the bot waits on the spot for the soonest
//! regrow (`woodcutting_trees:respawnrate`, a normal tree at most 199
//! ticks) when that is no longer than the walk to the brain's other site
//! plus its own wait, else it reports `depleted:` and the brain moves it.
//! Split from `kit::woodcut` (rule 4); the rock twin lives in `kit::mine`.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::bans::GONE_BAN_TICKS;
use crate::kit::deplete::{DepletedChoice, DepletionClock, RESPAWN_MARGIN_TICKS};
use crate::kit::load::{LoopState, NoSpot};
use crate::kit::observe::Observe;
use crate::kit::trees::{pick_tier, tier_spawns_wide, tree_live};
use crate::ScriptError;

/// The tier's trees inside `site` (same level, `SITE_RADIUS`).
fn site_trees(tier: &knowledge::trees::TreeTier, site: (i32, i32, i32)) -> Vec<(i32, i32)> {
    let mut trees: Vec<(i32, i32)> = tier_spawns_wide(tier, i64::from(site.2)).into_iter().filter(|(x, z)| crate::kit::site::within_site((*x, *z, site.2), site)).collect();
    trees.sort();
    trees.dedup();
    trees
}

/// Longest stump of the tier's tree (server rate).
fn tree_rate(tier: &knowledge::trees::TreeTier) -> u64 {
    knowledge::action_odds::tree_respawn_max(&tier.product).unwrap_or(knowledge::action_odds::NORMAL_TREE_RESPAWN_MAX).max(0) as u64
}

/// The felled trees of `trees` when none stands (seen felled: a stump or
/// a cleared tile; an unstreamed tile reads as standing), `None` when one
/// still stands.
fn all_felled(bot: &Bot, tier: &knowledge::trees::TreeTier, trees: &[(i32, i32)]) -> Option<Vec<(i32, i32)>> {
    if trees.is_empty() || trees.iter().any(|(x, z)| tree_live(bot, *x, *z, &tier.trees)) {
        return None;
    }
    Some(trees.to_vec())
}

/// Stamp `felled` in the clock (a stump sighting the woodcut loop already
/// confirmed counts from then) and read the soonest regrow.
fn soonest_regrow(clock: &mut DepletionClock, stumped: &std::collections::HashMap<(i32, i32), u64>, felled: &[(i32, i32)], rate: u64, now: u64) -> Option<u64> {
    for tree in felled {
        clock.observe(*tree, true, stumped.get(tree).copied().unwrap_or(now).min(now));
    }
    clock.soonest(felled, rate, now)
}

/// `LoopOps::on_no_spot` of the woodcut loop: the brain's site with every
/// tree of the best tier felled waits its soonest regrow or moves
/// (`depleted:`); anything else (a standing tree banned or hunted, no
/// site, off the site) keeps the plain repick.
pub fn on_felled_site(bot: &mut Bot, state: &mut LoopState, tiers: &[knowledge::trees::TreeTier], level: i64, observe: &mut Observe) -> Result<NoSpot, ScriptError> {
    let Some(site) = bot.assigned_site else { return Ok(NoSpot::Idle) };
    if !crate::kit::site::at_site(bot) || site.2 != bot.world.level {
        return Ok(NoSpot::Idle);
    }
    let Some(tier) = pick_tier(tiers, level) else { return Ok(NoSpot::Idle) };
    let Some(felled) = all_felled(bot, &tier, &site_trees(&tier, site)) else { return Ok(NoSpot::Idle) };
    let now = bot.world.ticks;
    let rate = tree_rate(&tier);
    let Some(wait_here) = soonest_regrow(&mut observe.clock, &observe.stumped, &felled, rate, now) else { return Ok(NoSpot::Idle) };
    let choice = crate::kit::deplete::decide(bot, &tier.product, felled.len(), wait_here, |bot, other| other_site_wait(bot, &tier, other, observe));
    if let DepletedChoice::Move(_) = choice {
        return Err(crate::kit::deplete::depleted_error(site, wait_here));
    }
    let spent = bot.world.ticks;
    let outcome = wait_regrow(bot, state, &tier, &felled, wait_here, &mut observe.clock);
    state.idle_since += bot.world.ticks - spent;
    outcome
}

/// The other site's own wait: its soonest regrow when the bot sees every
/// tree of the tier there felled, else 0 (nothing known against it).
fn other_site_wait(bot: &mut Bot, tier: &knowledge::trees::TreeTier, other: (i32, i32, i32), observe: &mut Observe) -> u64 {
    if other.2 != bot.world.level {
        return 0;
    }
    let Some(felled) = all_felled(bot, tier, &site_trees(tier, other)) else { return 0 };
    soonest_regrow(&mut observe.clock, &observe.stumped, &felled, tree_rate(tier), bot.world.ticks).unwrap_or(0)
}

/// Watch the felled trees until one stands again (its short stump ban
/// lifted so the pick takes it), at most `wait` plus a margin; a hit ends
/// the load (the caller's flee rules), random events are polled.
fn wait_regrow(bot: &mut Bot, state: &mut LoopState, tier: &knowledge::trees::TreeTier, felled: &[(i32, i32)], wait: u64, clock: &mut DepletionClock) -> Result<NoSpot, ScriptError> {
    bot.log(&format!("{} {} trees felled, waiting a regrow (<= {wait}t)", felled.len(), tier.product));
    bot.intend(&format!("waiting for a {} tree to regrow", tier.product));
    let hitpoints_before = crate::kit::survival::hitpoints(bot);
    let mut polled = bot.world.ticks;
    let until = polled + wait + RESPAWN_MARGIN_TICKS;
    while bot.world.ticks < until {
        bot.pump(1)?;
        if bot.world.ticks >= polled + 20 {
            polled = bot.world.ticks;
            crate::kit::survival::check_events(bot)?;
        }
        if crate::kit::survival::hitpoints(bot) < hitpoints_before {
            return Ok(NoSpot::EndLoad);
        }
        let now = bot.world.ticks;
        let standing: Vec<(i32, i32)> = felled.iter().copied().filter(|(x, z)| tree_live(bot, *x, *z, &tier.trees)).collect();
        if !standing.is_empty() {
            for tree in standing {
                clock.observe(tree, false, now);
                // The stump's short ban (`GONE_BAN_TICKS`) only: a
                // refusal or threat ban stays.
                if state.banned.until(tree).is_some_and(|until| until.saturating_sub(now) <= GONE_BAN_TICKS) {
                    state.banned.remove(tree);
                }
            }
            return Ok(NoSpot::Repick);
        }
    }
    Ok(NoSpot::Repick)
}

#[cfg(test)]
mod tests {
    use super::{all_felled, soonest_regrow};
    use crate::kit::deplete::DepletionClock;

    /// A felled site counts the soonest regrow from the stump the loop
    /// confirmed; a tree still standing (or unstreamed) is no felled site.
    #[test]
    fn a_felled_site_counts_from_the_confirmed_stumps() {
        let mut clock = DepletionClock::new();
        let stumped: std::collections::HashMap<(i32, i32), u64> = [((10, 10), 1_000)].into_iter().collect();
        assert_eq!(soonest_regrow(&mut clock, &stumped, &[(10, 10), (12, 10)], 200, 1_150), Some(50), "stump confirmed 150 ticks ago");
        let mut bot = lifescape_sdk::Bot::new("127.0.0.1", 43594, ".", ".");
        crate::tables_live::attach_live_tables(&mut bot);
        let yew = lifescape_gamedata::trees::all_tiers().into_iter().find(|tier| tier.product == "yew_logs").expect("yew tier");
        assert!(all_felled(&bot, &yew, &[(3087, 3481)]).is_none(), "unstreamed: standing");
        assert!(all_felled(&bot, &yew, &[]).is_none(), "no tree: nothing to wait for");
    }
}
