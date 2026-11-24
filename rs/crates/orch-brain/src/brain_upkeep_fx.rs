#![forbid(unsafe_code)]
//! What the life upkeep and the graph state ask of the world (lot 4 of the
//! launch refactor). The decisions are the planner's (`upkeep*`); the
//! drive (`brain_upkeep_drive`) runs them in the order the pass always had
//! and, between them, asks this trait for what only the daemon knows (the
//! filed operator switch, the market maker's orders and coins, the fleet
//! draws, the prices and time value, the claims and seats) and for what it
//! does (cancel a quote, free a board, draw a goal, print a line). The
//! live implementation (`LiveUpkeepFx`) calls the functions the passes
//! always called; the tests run the same drive over a recording fake.

use std::sync::Arc;

use lifescape_json::Value;
use lifescape_planner::ambition::{BotNow, Stamp};
use lifescape_planner::serve_value::ServeVerdict;
use lifescape_planner::upkeep::{Clock, ServeFacts, ServeQuery};
use lifescape_planner::upkeep_graph::{RescueReader, RouteFacts};
use lifescape_planner::upkeep_role::OrderFact;
use lifescape_planner::{Life, Node};

use crate::fleet::Fleet;
use crate::jobs::Job;

pub(crate) trait UpkeepFx: RescueReader {
    /// One log line, as `logln!` prints it.
    fn log(&self, line: &str);
    /// A brain phase of the CPU profile.
    fn timed<R>(&self, label: &'static str, body: impl FnOnce() -> R) -> R;
    /// Unix seconds and the daemon's tick length, read now.
    fn clock(&self) -> Clock;
    /// Live level of one skill (1 when unknown).
    fn level_of(&self, skill: &str) -> u8;
    /// The bot's live numbers (levels, coins, holdings).
    fn with_now(&self, body: &mut dyn FnMut(&BotNow));
    /// The draw clock (daemon tick, unix seconds).
    fn stamp(&self) -> Stamp;

    // Operator.
    /// The ambition switch filed for the account, taken out of the table.
    fn take_pending(&self, account: &str) -> Option<String>;
    /// `brain_ambition::assign_natural` on a draft.
    fn assign_natural(&self, draft: &mut Life) -> bool;
    /// `brain_ambition::assign_kind` on a draft.
    fn assign_kind(&self, draft: &mut Life, kind: &str, why: &str) -> bool;

    // Draws (the fleet, the market and the claims are read there).
    /// Close the current goal and draw the next (`brain_succession::succeed_goal`).
    fn succeed_goal(&self, life: &mut Life, stuck: bool) -> Option<String>;
    /// Close the current goal (`brain_succession::close_goal`).
    fn close_goal(&self, life: &mut Life, stuck: bool);
    /// Draw the next goal (`brain_succession::draw_next`).
    fn draw_next(&self, life: &mut Life) -> Option<String>;
    /// Retry the draw of a closed goal (`brain_succession::redraw`).
    fn redraw(&self, life: &mut Life) -> (Option<String>, bool);
    /// A retired maker's trade (`brain_ambition::redraw_neediest`).
    fn redraw_neediest(&self, life: &mut Life, why: &str);

    // Market maker.
    /// The bot's coins (pack, bank, worn).
    fn coins(&self) -> i64;
    /// The account's resting orders; `None` without a market service.
    fn orders(&self) -> Option<Vec<OrderFact>>;
    /// The items of a maker's slice.
    fn slice_items(&self, slice: u32) -> Vec<i64>;
    /// Cancels one order with its reason; true when the service did.
    fn cancel(&self, order: i64, reason: &str) -> bool;
    /// Ends what a bid answer posted (claims, purchases, seats).
    fn free_board(&self, account: &str);

    // Ambition.
    /// The first pass without ambition assigns the whole fleet.
    fn ensure(&self, life: &mut Life) -> bool;
    /// The coverage log (throttled per account, never changes the life).
    fn coverage(&self, life: &Life);
    /// The fleet's piles (`market_demand::fleet_surplus`).
    fn fleet_surplus(&self) -> Vec<(String, f64)>;

    // Serving value.
    /// The margin, rate and time value of a side demand goal.
    fn serve_facts(&self, life: &Life, query: &ServeQuery) -> Option<ServeFacts>;
    /// The verdict of each open demand milestone `(id, item)`.
    fn judge(&self, life: &Life, items: &[(String, String)]) -> Vec<(String, ServeVerdict)>;
    /// After a restart an open bid goal holds its seats again
    /// (`brain_bid_release::reengage_after_restart`).
    fn reengage(&self, life: &Life) -> bool;
}

/// The daemon's world for one launch pass. Built bare (no orchestrator, no
/// job) by the unit tests of the rules that read neither.
pub(crate) struct LiveUpkeepFx<'a> {
    orch: Option<&'a Arc<dyn Fleet>>,
    job: Option<&'a Arc<Job>>,
    live: &'a Value,
    /// The pass's handles (market, rates, guides).
    deps: crate::brain_deps::BrainDeps,
}

impl<'a> LiveUpkeepFx<'a> {
    pub(crate) fn new(orch: &'a Arc<dyn Fleet>, job: &'a Arc<Job>, live: &'a Value, deps: crate::brain_deps::BrainDeps) -> Self {
        LiveUpkeepFx { orch: Some(orch), job: Some(job), live, deps }
    }

    /// Without a worker: the rules that read the live snapshot only.
    #[cfg(test)]
    pub(crate) fn bare(live: &'a Value) -> Self {
        LiveUpkeepFx { orch: None, job: None, live, deps: crate::brain_deps::BrainDeps::live() }
    }
}

impl RescueReader for LiveUpkeepFx<'_> {
    fn wallet(&self) -> i64 {
        self.coins()
    }

    fn facts(&self, life: &Life, node: &Node) -> RouteFacts {
        let lifescape_planner::NodeKind::ObtainItems { item, .. } = &node.kind else {
            return RouteFacts { best_ask: None, price: 0, held: 0, committed: 0 };
        };
        let mut ctx = crate::brain_plan::ready_ctx_in(life, node, self.live, &self.deps);
        crate::brain_market::fill_ctx_in(&mut ctx, node, self.live, &self.deps);
        let held = self.job.map(|job| crate::brain_plan::holdings_count(job, item)).unwrap_or(0);
        let committed = crate::brain_gear::committed_coins(&life.account, crate::brain_gear::obj_id(item));
        RouteFacts { best_ask: ctx.best_ask, price: ctx.price, held, committed }
    }
}

impl UpkeepFx for LiveUpkeepFx<'_> {
    fn log(&self, line: &str) {
        crate::logln!("{line}");
    }

    fn timed<R>(&self, label: &'static str, body: impl FnOnce() -> R) -> R {
        crate::cpu_profile::timed(label, body)
    }

    fn clock(&self) -> Clock {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0);
        Clock { now, tick_ms: crate::market_eta::live_tick_ms(), down: crate::downtime::overlap }
    }

    fn level_of(&self, skill: &str) -> u8 {
        crate::brain_succession::live_level(self.live, skill)
    }

    fn with_now(&self, body: &mut dyn FnMut(&BotNow)) {
        crate::brain_ambition::with_now(self.live, |now| body(now));
    }

    fn stamp(&self) -> Stamp {
        crate::brain_ambition::stamp_now()
    }

    fn take_pending(&self, account: &str) -> Option<String> {
        crate::brain_operator::take_pending(account)
    }

    fn assign_natural(&self, draft: &mut Life) -> bool {
        crate::brain_ambition::assign_natural(draft, self.live)
    }

    fn assign_kind(&self, draft: &mut Life, kind: &str, why: &str) -> bool {
        crate::brain_ambition::assign_kind(draft, self.live, kind, why)
    }

    fn succeed_goal(&self, life: &mut Life, stuck: bool) -> Option<String> {
        crate::brain_succession::succeed_goal(life, self.live, stuck)
    }

    fn close_goal(&self, life: &mut Life, stuck: bool) {
        crate::brain_succession::close_goal(life, self.live, stuck);
    }

    fn draw_next(&self, life: &mut Life) -> Option<String> {
        crate::brain_succession::draw_next(life, self.live)
    }

    fn redraw(&self, life: &mut Life) -> (Option<String>, bool) {
        // Behind the gate (lot 3.3, `LIFESCAPE_REDRAW_GATE`, shadow by default).
        crate::brain_redraw_gate::redraw(life, self.live, crate::brain_succession::redraw)
    }

    fn redraw_neediest(&self, life: &mut Life, why: &str) {
        crate::brain_ambition::redraw_neediest(life, self.live, why);
    }

    fn coins(&self) -> i64 {
        self.job.map(|job| crate::brain_plan::holdings_count(job, "coins")).unwrap_or(0)
    }

    fn orders(&self) -> Option<Vec<OrderFact>> {
        let (job, service) = (self.job?, self.deps.market.as_deref()?);
        let rows = service.lock_store().account_orders(&job.account).unwrap_or_default();
        Some(
            rows.iter()
                .map(|row| OrderFact {
                    order_id: row.order_id,
                    buy: row.side == lifescape_store::OrderSide::Buy,
                    book_item: crate::market_audit::book_item(row.item_id),
                    market_make: row.motive == crate::market_lineage::MARKET_MAKE,
                    live: matches!(row.status, lifescape_store::OrderStatus::Open | lifescape_store::OrderStatus::Matched) && row.remaining > 0,
                })
                .collect(),
        )
    }

    fn slice_items(&self, slice: u32) -> Vec<i64> {
        crate::market_maker::slice_items(slice)
    }

    fn cancel(&self, order: i64, reason: &str) -> bool {
        match (self.job, self.deps.market.as_deref()) {
            (Some(job), Some(service)) => crate::market_maker::cancel(service, &job.account, order, reason),
            _ => false,
        }
    }

    fn free_board(&self, account: &str) {
        crate::brain_bid_release::free_board(account);
    }

    fn ensure(&self, life: &mut Life) -> bool {
        self.orch.is_some_and(|orch| crate::brain_ambition::ensure(&**orch, life, self.live))
    }

    fn coverage(&self, life: &Life) {
        crate::brain_ambition::coverage_pass(life);
    }

    fn fleet_surplus(&self) -> Vec<(String, f64)> {
        crate::market_demand::fleet_surplus()
    }

    fn serve_facts(&self, life: &Life, query: &ServeQuery) -> Option<ServeFacts> {
        crate::brain_serve_value::serve_facts(life, self.live, query)
    }

    fn judge(&self, life: &Life, items: &[(String, String)]) -> Vec<(String, ServeVerdict)> {
        crate::brain_milestone_value::judge_items(life, self.live, items)
    }

    fn reengage(&self, life: &Life) -> bool {
        crate::brain_bid_release::reengage_after_restart(life, self.live)
    }
}
