#![forbid(unsafe_code)]
//! The market wait of an obtain node (split from `brain.rs`, rule 4):
//! a `market:buy` leg placed daemon-side keeps the node active, without
//! a worker, until its holdings say how it ended.
//!
//! Persistent needs (plan marché M2, C3): the deadline ends the node's
//! MARKET LEG, never its bid. The bid is the node's need on the book: it
//! keeps its lineage and walk while the node replans (production, coins
//! block, another buy that takes the same bid over); it closes when the
//! need is met (`need_met`) or gone (`brain_market::release_orphan_needs`).

use lifescape_planner::Life;

use crate::brain::push_history;
use crate::brain_plan::holdings_count;
use crate::jobs::Job;

/// Market fills settle through other players: the market leg of an
/// obtain waits at most this many ticks (wall time = ticks × tick
/// length, D7) before its node replans.
pub(crate) const AWAIT_FILL_TICKS: u64 = 6000;

/// The market side of a fill wait (the live service, or a test fake).
pub(crate) trait FillMarket {
    /// No resting buy and no handover in flight on `item` any more.
    fn buy_closed(&self, item: &str) -> bool;
    /// Closes the need's resting buys on `item` (it was met).
    fn close_need(&self, item: &str) -> usize;
}

struct LiveMarket<'account> {
    account: &'account str,
    deps: &'account crate::brain_deps::BrainDeps,
}

impl FillMarket for LiveMarket<'_> {
    fn buy_closed(&self, item: &str) -> bool {
        crate::brain_market::buy_closed(self.account, item, self.deps)
    }

    fn close_need(&self, item: &str) -> usize {
        crate::brain_market::cancel_open_buys(self.account, item, self.deps)
    }
}

/// One pass over the obtain node at `position`, waiting `waited` seconds
/// for its buy of `count` `item`. Returns true while it keeps waiting
/// (nothing changed), false once the node moved on (`settle_with`).
pub(crate) fn settle_fill(job: &Job, life: &mut Life, position: usize, item: &str, count: i64, waited: i64, deps: &crate::brain_deps::BrainDeps) -> bool {
    let held = holdings_count(job, item, deps);
    let deadline = (AWAIT_FILL_TICKS * deps.clock.tick_ms() / 1000) as i64;
    let baseline = life.nodes[position].progress_current;
    let outcome = settle_with(life, position, item, count, held, waited, deadline, &LiveMarket { account: &job.account, deps });
    // A partial fill landed: the reservation shrinks by it (a full one
    // ends with the need, `cancel_open_buys`).
    let landed = life.nodes.get(position).map(|node| node.progress_current - baseline).unwrap_or(0);
    if landed > 0 {
        crate::market_claims::purchase_landed(&job.account, item, landed);
    }
    if let Some(line) = outcome.log {
        crate::logln!("brain {}: {line}", job.account);
    }
    outcome.waiting
}

/// What one settle pass did.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Settled {
    pub waiting: bool,
    pub log: Option<String>,
}

/// Pure core of `settle_fill` (`held`: usable holdings now; `deadline`:
/// seconds the market leg may wait):
/// - holdings reach the count: done, the need's bid closes;
/// - a partial buy landed (holdings above the launch baseline, the order
///   and its handover over): ready again, uncounted;
/// - the deadline passed: the market leg failed (counted, the node
///   replans: production, a coins block, or a buy that takes the same
///   bid over); the bid STAYS with its lineage and walk;
/// - otherwise it keeps waiting.
#[allow(clippy::too_many_arguments)]
pub(crate) fn settle_with(life: &mut Life, position: usize, item: &str, count: i64, held: i64, waited: i64, deadline: i64, market: &dyn FillMarket) -> Settled {
    let active_id = life.nodes[position].id.clone();
    // Holdings at the placement (set at launch).
    let baseline = life.nodes[position].progress_current;
    if held >= count {
        market.close_need(item);
        lifescape_planner::complete_node(life, position, held);
        push_history(life, &active_id, "goal_reached");
    } else if held > baseline && market.buy_closed(item) {
        life.nodes[position].progress_current = held;
        lifescape_planner::retry_uncounted(life, position);
        push_history(life, &active_id, "partial_fill");
        return Settled { waiting: false, log: Some(format!("{active_id} partial buy landed ({baseline} -> {held}/{count} {item}), re-resolving")) };
    } else if waited > deadline && held > 0 && feeds_a_climb(life, &active_id) {
        // A climb burns its input unit by unit: what it holds is worked now,
        // the bid stays for the rest (live fiona_park 2026-11-20: 1 141
        // of 4 570 bars held, the climb never smithed one, the obtain was
        // abandoned after three deadlines).
        lifescape_planner::complete_node(life, position, held);
        push_history(life, &active_id, "partial_done");
        return Settled { waiting: false, log: Some(format!("{active_id} market leg ended after {AWAIT_FILL_TICKS} ticks ({held}/{count} {item}): the climb works what it holds, its bid stays on the book")) };
    } else if waited > deadline {
        let text = lifescape_planner::market_unfilled_error(count, item, AWAIT_FILL_TICKS, held, 0);
        let fold = lifescape_planner::record_failure(life, position, &text);
        push_history(life, &active_id, if fold == lifescape_planner::FailureFold::Abandoned { "abandoned" } else { "failed" });
        return Settled { waiting: false, log: Some(format!("{active_id} market leg ended after {AWAIT_FILL_TICKS} ticks ({held}/{count} {item}): the node replans, its bid stays on the book")) };
    } else {
        return Settled { waiting: true, log: None };
    }
    Settled { waiting: false, log: None }
}

/// True when an open level climb depends on the node `node_id` (its
/// input obtain, sized on the tranche: any unit of it is work).
fn feeds_a_climb(life: &Life, node_id: &str) -> bool {
    life.nodes.iter().any(|node| {
        matches!(node.kind, lifescape_planner::NodeKind::ReachLevel { .. })
            && !matches!(node.status, lifescape_planner::NodeStatus::Done | lifescape_planner::NodeStatus::Abandoned)
            && node.deps.iter().any(|dep| dep == node_id)
    })
}

#[cfg(test)]
mod tests {
    use super::{settle_with, FillMarket};
    use lifescape_planner::{Life, Node, NodeKind, NodeStatus};
    use std::cell::Cell;

    struct FakeMarket {
        closed: Cell<usize>,
        buy_closed: bool,
    }

    impl FillMarket for FakeMarket {
        fn buy_closed(&self, _item: &str) -> bool {
            self.buy_closed
        }

        fn close_need(&self, _item: &str) -> usize {
            self.closed.set(self.closed.get() + 1);
            1
        }
    }

    fn waiting_life() -> Life {
        let mut life = Life {
            schema: 1,
            account: "fiona_park".to_string(),
            created_at: 0,
            seed: 0,
            profile: lifescape_planner::Profile::new(false, 0.5, 0.5, 0, lifescape_planner::Archetype::Producer),
            goal: lifescape_planner::GoalKind::MakeMoney,
            nodes: Vec::new(),
            engagement: None,
            active: None,
            history: Vec::new(),
            ambition: None,
            goal_origin: None,
            memory: Default::default(),
        };
        let mut node = Node::new("n5", NodeKind::ObtainItems { item: "unstrung_sapphire_amulet".to_string(), count: 5 }, Vec::new(), 0);
        node.method = Some("market:buy".to_string());
        node.status = NodeStatus::Ready;
        life.nodes.push(node);
        life
    }

    /// Plan marché M2 (C3): 6000 ticks without a fill end the node's
    /// market leg (failure counted, method dropped: it replans), never
    /// its bid (391 amulet bids a day were cancelled then re-placed lower).
    #[test]
    fn a_deadline_keeps_the_bid_and_replans_the_node() {
        let mut life = waiting_life();
        let market = FakeMarket { closed: Cell::new(0), buy_closed: false };
        let outcome = settle_with(&mut life, 0, "unstrung_sapphire_amulet", 5, 0, 400, 360, &market);
        assert!(!outcome.waiting);
        assert_eq!(market.closed.get(), 0, "the bid stays on the book");
        let node = &life.nodes[0];
        assert_eq!((node.status.clone(), node.method.clone(), node.failures), (NodeStatus::Ready, None, 1), "replanned");
        assert!(lifescape_planner::market_failed(node), "the rescue reads a market failure: {}", node.last_error);
        assert_eq!(life.history.last().map(|entry| entry.outcome.as_str()), Some("failed"));
    }

    /// Live 2026-11-20 (fiona_park n2432: `market order for 4570 iron_bar
    /// unfilled after 6000 ticks (held 1141)`, 246 graphs stuck a day on
    /// such roots): a climb's input obtain is sized on the whole tranche
    /// (smithing 74->75 by iron platebodies: 4 570 bars); three deadlines
    /// abandoned it while 1 141 bars sat in the bank and the climb never
    /// smithed one. At the deadline, what landed goes to the climb: the
    /// obtain is done with it, its bid stays on the book for the rest
    /// (the climb's next block files the remainder).
    #[test]
    fn a_deadline_hands_what_landed_to_the_climb() {
        let mut life = waiting_life();
        life.nodes[0].kind = NodeKind::ObtainItems { item: "iron_bar".to_string(), count: 4570 };
        life.nodes[0].progress_current = 0;
        let mut climb = Node::new("n4", NodeKind::ReachLevel { skill: "smithing".to_string(), level: 75 }, vec!["n5".to_string()], 75);
        climb.status = NodeStatus::Pending;
        life.nodes.push(climb);
        let market = FakeMarket { closed: Cell::new(0), buy_closed: false };
        let outcome = settle_with(&mut life, 0, "iron_bar", 4570, 1141, 400, 360, &market);
        assert!(!outcome.waiting);
        assert_eq!(market.closed.get(), 0, "the bid stays on the book for the rest");
        assert_eq!((life.nodes[0].status.clone(), life.nodes[0].failures, life.nodes[0].progress_current), (NodeStatus::Done, 0, 1141), "done with what landed");
        // Nothing landed: the market leg failed as before.
        let mut empty = waiting_life();
        let mut climb = Node::new("n4", NodeKind::ReachLevel { skill: "smithing".to_string(), level: 75 }, vec!["n5".to_string()], 75);
        climb.status = NodeStatus::Pending;
        empty.nodes.push(climb);
        settle_with(&mut empty, 0, "unstrung_sapphire_amulet", 5, 0, 400, 360, &market);
        assert_eq!(empty.nodes[0].failures, 1, "nothing landed: a counted market failure");
        // An obtain no climb waits on keeps its full count (a recipe, a
        // quest needs every unit).
        let mut plain = waiting_life();
        settle_with(&mut plain, 0, "unstrung_sapphire_amulet", 5, 3, 400, 360, &market);
        assert_eq!((plain.nodes[0].status.clone(), plain.nodes[0].failures), (NodeStatus::Ready, 1));
    }

    #[test]
    fn a_met_need_closes_its_bid_and_a_young_wait_waits() {
        let mut life = waiting_life();
        let market = FakeMarket { closed: Cell::new(0), buy_closed: false };
        assert!(settle_with(&mut life, 0, "unstrung_sapphire_amulet", 5, 0, 100, 360, &market).waiting);
        assert!(!settle_with(&mut life, 0, "unstrung_sapphire_amulet", 5, 5, 100, 360, &market).waiting);
        assert_eq!(market.closed.get(), 1, "need met: its bid closes");
        assert_eq!(life.nodes[0].status, NodeStatus::Done);
    }
}
