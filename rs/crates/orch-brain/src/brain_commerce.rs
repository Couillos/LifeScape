#![forbid(unsafe_code)]
//! The side market passes of the launch pass (lot 5 of the launch
//! refactor), an explicit ordered list: `COMMERCE_STEPS`. They ran inside
//! `brain_launch_steps::market_passes`; the list runs at exactly the same
//! point of the pass (after the pack guard, before the waits), in the same
//! order, under the same `cpu_profile` labels and log texts, and a
//! quoting maker still skips what it skipped (`quoting` guards).
//!
//! The rule written as code (user, 2026-11-23: never preempt an active
//! milestone for lucrative market bids, XP first): **commerce never
//! preempts the ambition**. No step of this list changes `life.goal`, the
//! ambition and its milestone statuses, nor the status, kind or method of
//! a node that already exists. Two kinds of step:
//!
//! - the read-only passes take the life as `&Life` (quest reserve, surplus
//!   listing, quotes, wishes, alchemy, arbitrage, future stock): the
//!   compiler forbids them to touch it; they only post, move or withdraw
//!   market orders (the market's bookkeeping);
//! - the two that file steps take `&mut Life` and may only ADD nodes (the
//!   standing kit: axe, pickaxe, hammer, tinderbox of the best tier; the
//!   gear plan: the full combat set) and withdraw the ones they filed
//!   (`withdraw_gear_step`, only a kit/gear step that nobody started).
//!
//! `brain_commerce_tests` holds the invariant over the real lives.

use lifescape_json::Value;
use lifescape_planner::Life;

use crate::brain_launch_pass::{Flow, LaunchPass};
use crate::jobs::Job;

/// Job accounts of the invariant tests (`brain_gear_fleet` ignores them).
#[cfg(test)]
pub(crate) const PROBE_PREFIX: &str = "commerce_probe_";

/// What a commerce step reads and may change: the job (market identity,
/// holdings), the life, the live snapshot, the pass marks.
pub(crate) struct Commerce<'a> {
    pub job: &'a Job,
    pub life: &'a mut Life,
    pub live: &'a Value,
    /// The pass's handles on the world (market, rates, guides, crowd, clock).
    pub deps: &'a crate::brain_deps::BrainDeps,
    /// A quoting market maker: quotes its slice, none of the other bids.
    pub quoting: bool,
    /// A step changed the life (saved by the stopping step as asked).
    pub dirty: bool,
}

/// One side market pass: its CPU profile label (also its name), what it
/// does.
pub(crate) struct CommerceStep {
    /// Read by the tests (the order is pinned there).
    #[cfg_attr(not(test), allow(dead_code))]
    pub name: &'static str,
    pub run: fn(&mut Commerce),
}

/// The side market passes, in the order of the pass. A quoting maker runs
/// `market_maker.quote_pass` and none of the bidding ones; every other bot
/// the reverse (each step carries its guard).
pub(crate) const COMMERCE_STEPS: [CommerceStep; 9] = [
    CommerceStep { name: "quest_gate.publish", run: quest_reserve },
    CommerceStep { name: "sales.list_surplus", run: list_surplus },
    CommerceStep { name: "market_maker.quote_pass", run: maker_quotes },
    CommerceStep { name: "wishlist.bid_wishes", run: wish_bids },
    CommerceStep { name: "alch.market_pass", run: alchemy },
    CommerceStep { name: "arbitrage.pass", run: arbitrage },
    CommerceStep { name: "stock.future_pass", run: future_stock },
    CommerceStep { name: "tool_shop_pass", run: tool_kit },
    CommerceStep { name: "gear_pass", run: gear_plan },
];

/// Runs the list in order; a step that changed the life sets `dirty`.
pub(crate) fn run(commerce: &mut Commerce) {
    for step in &COMMERCE_STEPS {
        (step.run)(commerce);
    }
}

/// The launch pass's side market passes (`steps::market_passes`).
pub(crate) fn market_passes(pass: &mut LaunchPass) -> Flow {
    let mut commerce = Commerce { job: pass.job, life: &mut pass.life, live: &pass.live, deps: &pass.deps, quoting: pass.quoting, dirty: false };
    run(&mut commerce);
    let dirty = commerce.dirty;
    pass.dirty |= dirty;
    Flow::Continue
}

/// The open quests' objects never go on sale (2026-11-14, bh99tw2f):
/// published before any listing pass.
fn quest_reserve(commerce: &mut Commerce) {
    let deps = commerce.deps;
    let (job, life) = (commerce.job, &*commerce.life);
    crate::cpu_profile::timed("quest_gate.publish", || crate::brain_quest_gate::publish_reserved(&job.account, life, deps));
}

/// PRD3 lot 3: idle bots list their tradeable surplus on the market
/// (rate-limited, never an NPC sale).
fn list_surplus(commerce: &mut Commerce) {
    let deps = commerce.deps;
    let (job, life) = (commerce.job, &*commerce.life);
    crate::cpu_profile::timed("sales.list_surplus", || crate::brain_sales::maybe_list_surplus(job, life, deps));
}

/// Q6: the maker quotes its slice (rate-limited, public data).
fn maker_quotes(commerce: &mut Commerce) {
    let deps = commerce.deps;
    if !commerce.quoting {
        return;
    }
    let (job, life) = (commerce.job, &*commerce.life);
    crate::cpu_profile::timed("market_maker.quote_pass", || crate::market_maker::quote_pass(job, life, deps));
}

/// PRD3 lot 5: upgrade wishes bid on the market, never blocking.
fn wish_bids(commerce: &mut Commerce) {
    let deps = commerce.deps;
    if commerce.quoting {
        return;
    }
    let (job, life) = (commerce.job, &*commerce.life);
    crate::cpu_profile::timed("wishlist.bid_wishes", || crate::brain_wishlist::maybe_bid_wishes(job, life, deps));
}

/// Alchemy (2026-11-14): nature runes for the stock a caster alchs, asks
/// under the alch floor bought to alch.
fn alchemy(commerce: &mut Commerce) {
    let deps = commerce.deps;
    if commerce.quoting {
        return;
    }
    let (job, life, live) = (commerce.job, &*commerce.life, commerce.live);
    crate::cpu_profile::timed("alch.market_pass", || crate::brain_alch::market_pass(job, life, live, deps));
}

/// Merchant arbitrage (plan équilibre D2): asks under what a shop pays,
/// net of the trip at the bot's time value, bought for it.
fn arbitrage(commerce: &mut Commerce) {
    let deps = commerce.deps;
    if commerce.quoting {
        return;
    }
    let (job, life, live) = (commerce.job, &*commerce.life, commerce.live);
    crate::cpu_profile::timed("arbitrage.pass", || crate::brain_arbitrage::arbitrage_pass(job, life, live, deps));
}

/// Future stock (plan marché M6, Q5): patient bids for what the current
/// work slice and the next will burn, after the alchemy pass (its own
/// `stock` bids are claimed first).
fn future_stock(commerce: &mut Commerce) {
    let deps = commerce.deps;
    if commerce.quoting {
        return;
    }
    let (job, life, live) = (commerce.job, &*commerce.life, commerce.live);
    crate::cpu_profile::timed("stock.future_pass", || crate::brain_stock::stock_pass(job, life, live, deps));
}

/// Permanent tool kit (2026-11-14): axe, pickaxe, hammer, tinderbox, the
/// best tier the level opens, filed as gear steps (a shop, or a covered ask
/// for an axe), never a market bid nobody fills. Before the gear plan: the
/// cheap base kit first.
fn tool_kit(commerce: &mut Commerce) {
    let deps = commerce.deps;
    if commerce.quoting {
        return;
    }
    let (job, live) = (commerce.job, commerce.live);
    let life = &mut *commerce.life;
    if crate::cpu_profile::timed("tool_shop_pass", || crate::brain_tool_shop::tool_shop_pass(job, life, live, deps)) {
        commerce.dirty = true;
    }
}

/// Gear plan (2026-11-14): the best affordable piece of every slot,
/// standing bids that wait for a producer, steps for covered asks.
fn gear_plan(commerce: &mut Commerce) {
    let deps = commerce.deps;
    if commerce.quoting {
        return;
    }
    let (job, live) = (commerce.job, commerce.live);
    let life = &mut *commerce.life;
    if crate::cpu_profile::timed("gear_pass", || crate::brain_gear::gear_pass(job, life, live, deps)) {
        commerce.dirty = true;
    }
}

#[cfg(test)]
#[path = "brain_commerce_tests.rs"]
mod tests;
