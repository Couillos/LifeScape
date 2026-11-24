//! Reports received but not folded yet (plan P10, 2026-11-17).
//!
//! A pushed report waits in its brain shard's inbox (or, inbox full, in
//! `JobInner::report_pending`) and is folded after its rates ingest (up
//! to 35 s under load). The idle detector of the launch pass ran in that
//! window and counted "no report: worker went idle without reporting":
//! 237 false failures in 10 h, three in a row banned a skill. The
//! detector now asks here first: a report in flight is the run's verdict,
//! never a missing one.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

fn table() -> &'static Mutex<HashMap<String, u32>> {
    static TABLE: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A report of `account` entered a brain inbox.
pub(crate) fn queued(account: &str) {
    *table().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).entry(account.to_string()).or_insert(0) += 1;
}

/// A report of `account` was folded (or dropped).
pub(crate) fn folded(account: &str) {
    let mut guard = table().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(count) = guard.get_mut(account) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            guard.remove(account);
        }
    }
}

/// True when a report of `account` is queued and not folded yet.
pub(crate) fn in_flight(account: &str) -> bool {
    table().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(account).is_some_and(|count| *count > 0)
}

/// History outcome of a launch the worker refused.
pub const LAUNCH_REFUSED: &str = "launch_refused";

/// A launch the worker refused (tour 2, P2-5: "busy: market is running")
/// never ran: the node goes back to ready, uncounted, the lock released,
/// so the idle detector has no active node to call "no report" (live
/// 12:12 lucy_rhodes n868: refused, then 45 s later a counted failure).
/// The next pass relaunches when the worker reads idle again. Returns
/// true when `node_id` was the active node and was released.
pub fn fold_refused_launch(life: &mut lifescape_planner::Life, node_id: &str) -> bool {
    if life.active.as_deref() != Some(node_id) {
        return false;
    }
    let Some(index) = life.nodes.iter().position(|node| node.id == node_id) else {
        return false;
    };
    if life.nodes[index].status != lifescape_planner::NodeStatus::Active {
        return false;
    }
    lifescape_planner::retry_uncounted(life, index);
    // Never ran: nothing of its claim is on its way (review lot 2, point 2).
    crate::market_claims::release_run(&life.account, node_id);
    true
}

/// The active node at `position` went idle without a report (crash,
/// restart): the failure counts (`record_failure`, history `failed` or
/// `abandoned`) and its run's claim on the order board ends (review lot
/// 2, point 2: never a leak until expiry).
pub fn fold_lost_report(life: &mut lifescape_planner::Life, position: usize, text: &str) -> lifescape_planner::FailureFold {
    let id = life.nodes[position].id.clone();
    crate::market_claims::release_run(&life.account, &id);
    let fold = lifescape_planner::record_failure(life, position, text);
    crate::brain::push_history(life, &id, if fold == lifescape_planner::FailureFold::Abandoned { "abandoned" } else { "failed" });
    fold
}

/// The active node at `position` went idle after an operator stop or a
/// daemon restart: retried uncounted (history `stopped`), its run's
/// claim on the order board ended (review lot 2, point 2).
pub fn fold_stopped(life: &mut lifescape_planner::Life, position: usize) {
    let id = life.nodes[position].id.clone();
    crate::market_claims::release_run(&life.account, &id);
    lifescape_planner::retry_uncounted(life, position);
    crate::brain::push_history(life, &id, "stopped");
}

/// True when a launch error proves the script never started: the worker
/// answered busy, or no session took the run. A transport error is no
/// proof (the run may have started): crash recovery keeps judging it.
pub fn refusal_never_ran(error: &str) -> bool {
    crate::worker_call::Refusal::parse(error).never_ran()
}

/// Pure verdict of the idle detector: the active node failed for want of
/// a report only when none is in flight (inbox) nor kept pending (inbox
/// full, `report_pending`).
pub fn missing_report(in_flight: bool, pending: bool) -> bool {
    !in_flight && !pending
}

#[cfg(test)]
mod tests {
    use super::{fold_refused_launch, folded, in_flight, missing_report, queued};
    use lifescape_planner::{Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};

    /// Red test P2-5: a refused launch leaves no active node, counts no
    /// failure; a node that already moved is left alone.
    #[test]
    fn a_refused_launch_is_a_ready_node_not_a_failure() {
        let mut node = Node::new("n868", NodeKind::ObtainItems { item: "swordfish".to_string(), count: 28 }, Vec::new(), 28);
        node.method = Some("cook:raw_swordfish".to_string());
        let mut life = Life {
            schema: 1,
            account: "refused".to_string(),
            created_at: 0,
            seed: 1,
            profile: Profile::new(false, 0.5, 0.5, 1, Archetype::Producer),
            goal: GoalKind::MaxSkills { skills: vec!["cooking".to_string()], level: 40 },
            nodes: vec![node],
            engagement: None,
            active: None,
            history: Vec::new(),
            ambition: None,
            goal_origin: None,
            memory: Default::default(),
        };
        lifescape_planner::mark_launched(&mut life, 0);
        assert!(fold_refused_launch(&mut life, "n868"));
        assert_eq!(life.active, None, "no active node for the idle detector");
        assert_eq!(life.nodes[0].status, NodeStatus::Ready);
        assert_eq!(life.nodes[0].failures, 0, "uncounted");
        assert!(!fold_refused_launch(&mut life, "n868"), "already ready: nothing to fold");
        assert!(super::refusal_never_ran("busy: market is running"));
        assert!(super::refusal_never_ran("no session, open lucy_rhodes first"));
        assert!(!super::refusal_never_ran("io: timed out"), "transport: the run may have started");
    }

    /// A money node of account `account`, launched (active), its run's
    /// coal on the order board.
    fn claimed_run(account: &str) -> Life {
        let mut node = Node::new("n12", NodeKind::MakeMoney, Vec::new(), 0);
        node.method = Some("gather:mining/coal".to_string());
        let mut life = Life {
            schema: 1,
            account: account.to_string(),
            created_at: 0,
            seed: 1,
            profile: Profile::new(false, 0.5, 0.5, 1, Archetype::Producer),
            goal: GoalKind::MakeMoney,
            nodes: vec![node],
            engagement: None,
            active: None,
            history: Vec::new(),
            ambition: None,
            goal_origin: None,
            memory: Default::default(),
        };
        lifescape_planner::mark_launched(&mut life, 0);
        crate::market_claims::claim_run(account, "n12", &format!("{account}_coal"), 300, 3_600);
        life
    }

    fn claimed(account: &str) -> Option<i64> {
        crate::market_claims::claimed_total().get(&format!("{account}_coal")).copied()
    }

    /// Review lot 2, point 2: a run that ends without a report (idle
    /// worker: crash, or an operator stop), or whose launch the worker
    /// refused, releases its claim at once (never a leak until expiry).
    #[test]
    fn a_run_without_report_releases_its_claim() {
        let mut lost = claimed_run("inflight_lost");
        assert_eq!(claimed("inflight_lost"), Some(300));
        super::fold_lost_report(&mut lost, 0, "no report");
        assert_eq!(claimed("inflight_lost"), None, "crash: released");
        assert_eq!(lost.nodes[0].failures, 1, "the failure counts");
        let mut stopped = claimed_run("inflight_stopped");
        super::fold_stopped(&mut stopped, 0);
        assert_eq!(claimed("inflight_stopped"), None, "operator stop: released");
        assert_eq!(stopped.nodes[0].failures, 0, "uncounted");
        let mut refused = claimed_run("inflight_refused");
        assert!(fold_refused_launch(&mut refused, "n12"));
        assert_eq!(claimed("inflight_refused"), None, "never ran: released");
    }

    /// Plan P10 red test: an idle worker whose report waits in the inbox
    /// (or pending) is no "no report" failure; once folded, a later idle
    /// without report is.
    #[test]
    fn a_report_in_flight_is_never_a_missing_one() {
        assert!(!in_flight("p10_bot"));
        queued("p10_bot");
        assert!(in_flight("p10_bot"));
        assert!(!missing_report(in_flight("p10_bot"), false), "queued: the report decides");
        folded("p10_bot");
        assert!(!in_flight("p10_bot"));
        assert!(!missing_report(false, true), "inbox full, kept pending");
        assert!(missing_report(false, false), "nothing anywhere: a real crash");
        folded("p10_bot");
        assert!(!in_flight("p10_bot"), "never negative");
    }
}
