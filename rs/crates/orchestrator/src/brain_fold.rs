//! Brain report folding: one pushed task report moves one node through
//! `lifescape_planner::lifecycle` (the only place that changes statuses),
//! then the graph is normalized and saved. Launching stays in brain.rs.
//!
//! Classification reads the report's own typed `error` (never a
//! session-wide `last_error` that may belong to an older run):
//! - `Usage` (bad CLI, dispatch the script can never serve): abandoned at
//!   once, the same launch would fail the same way;
//! - `Failed`/`Sdk`/`exit`, death, lost stock: counted, retried with a
//!   fresh resolve until `MAX_FAILURES`, then abandoned;
//! - travel failure: the unreachable method is banned (never relaunched
//!   as is) and the failure counted;
//! - `blocked` with a typed `missing`: one obtain sub-task per item,
//!   reused when already open, bounded by `MAX_BLOCKS`;
//! - `stopped` (operator unload, daemon restart): back to ready, uncounted.

#![forbid(unsafe_code)]

use std::sync::Arc;

use lifescape_json::Value;
use lifescape_planner::{complete_node, end_engagement, normalize, record_block_from, record_contract_error, record_failure, retry_uncounted, FailureFold, NodeKind, NodeStatus};

use crate::brain::{load_life, push_history, save_life};
use crate::brain_plan::{fold_travel_failure, holdings_count, is_travel_failure, rescue_replan_cause, TravelFold};
use crate::daemon::Orchestrator;
use crate::jobs::Job;

fn reach_level(report: &Value, skill: &str) -> Option<u8> {
    let stat = lifescape_sdk::provenance::skill_id(&skill.to_lowercase())? as i64;
    report
        .get("levels_after")
        .as_arr()?
        .iter()
        .find(|row| row.get("stat").as_i64() == Some(stat))
        .and_then(|row| row.get("level").as_i64())
        .map(|level| level.clamp(1, 99) as u8)
}

/// Combat level from the report's `levels_after` (XP-max formula).
fn reach_combat(report: &Value) -> Option<u8> {
    let rows = report.get("levels_after").as_arr()?;
    let stat = |id: i64| rows.iter().find(|row| row.get("stat").as_i64() == Some(id)).and_then(|row| row.get("level").as_i64()).unwrap_or(1).clamp(1, 99) as u8;
    Some(lifescape_planner::combat_level(&lifescape_planner::BaseLevels {
        attack: stat(0),
        defence: stat(1),
        strength: stat(2),
        hitpoints: stat(3).max(10),
        ranged: stat(4),
        prayer: stat(5),
        magic: stat(6),
    }))
}

fn report_missing(report: &Value) -> Vec<String> {
    report
        .get("missing")
        .as_arr()
        .map(|rows| rows.iter().filter_map(|row| row.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// Typed error carried by the report: (`type`, `message`).
pub(crate) fn report_error(report: &Value) -> Option<(String, String)> {
    let error = report.get("error");
    let kind = error.get("type").as_str()?.to_string();
    Some((kind, error.get("message").as_str().unwrap_or("").to_string()))
}

/// What one report does to its node (pure decision, applied by
/// `handle_report`; unit-tested without a daemon).
#[derive(Debug, PartialEq)]
pub(crate) enum Verdict {
    /// Goal reached: the node is done.
    Done,
    /// Market order placed but not filled yet: keep waiting (bounded).
    AwaitFill,
    /// Engagement window spent: money closes, levels continue.
    EngagementEnd,
    /// Operator stop or daemon restart: retry later, uncounted.
    Stopped,
    /// Typed missing requirements: sub-tasks.
    Blocked(Vec<String>),
    /// Contract error: abandon now.
    Contract(String),
    /// Unreachable method: ban it, count the failure.
    Travel(String),
    /// Runtime failure: count it.
    Failure(String),
}

/// XP a report credits toward `kind`: the climbed skill of a
/// `reach_level`, every combat skill of a `reach_combat`, 0 otherwise.
pub(crate) fn xp_toward(report: &Value, kind: &NodeKind) -> i64 {
    const COMBAT: [&str; 6] = ["attack", "strength", "defence", "hitpoints", "ranged", "magic"];
    let counts = |skill: &str| match kind {
        NodeKind::ReachLevel { skill: climbed, .. } => climbed.eq_ignore_ascii_case(skill),
        NodeKind::ReachCombat { .. } => COMBAT.contains(&skill.to_lowercase().as_str()),
        _ => false,
    };
    report
        .get("xp")
        .as_arr()
        .map(|rows| rows.iter().filter(|row| row.get("skill").as_str().is_some_and(counts)).filter_map(|row| row.get("xp").as_i64()).filter(|gain| *gain > 0).sum())
        .unwrap_or(0)
}

/// Decide from the stop, the typed error, missing items, and what the
/// goal check says (`goal_met`, `market_pending`, `progressed`: the run
/// earned XP toward the climb or items toward the obtain).
pub(crate) fn verdict(stop: &str, error: Option<(String, String)>, missing: &[String], goal_met: bool, market_pending: bool, progressed: bool) -> Verdict {
    let text = error.as_ref().map(|(kind, message)| format!("{kind}: {message}")).unwrap_or_default();
    match stop {
        "goal_reached" if goal_met => Verdict::Done,
        "goal_reached" if market_pending => Verdict::AwaitFill,
        // A run that ended on its own budget short of the goal but with
        // progress (prayer at cows: +652 XP a window) continues
        // uncounted, like a spent engagement window (live 2026-11-13:
        // three such windows banned prayer on frank_lane and leo_king).
        "goal_reached" if progressed => Verdict::EngagementEnd,
        "goal_reached" => Verdict::Failure("goal_reached but the goal check is short (method ended without the items/level)".to_string()),
        // A window can end exactly when the goal lands (level reached,
        // stock farmed): that is a success, not one more window.
        "budget_exhausted" if goal_met => Verdict::Done,
        "budget_exhausted" => Verdict::EngagementEnd,
        "stopped" => Verdict::Stopped,
        "blocked" if !missing.is_empty() => Verdict::Blocked(missing.to_vec()),
        "blocked" => Verdict::Failure(format!("blocked without a typed missing item ({text})")),
        _ => match error {
            Some((kind, _)) if kind == "Usage" => Verdict::Contract(text),
            Some((_, message)) if is_travel_failure(&lifescape_json::object(vec![("message", lifescape_json::str_value(&message))])) => Verdict::Travel(text),
            Some(_) => Verdict::Failure(text),
            None if missing.iter().any(|need| need.contains("died") || need.contains("death")) => Verdict::Failure("died".to_string()),
            None => Verdict::Failure(format!("stop {stop} without a typed error")),
        },
    }
}

/// Fold one pushed report into the graph; launches nothing (the launch
/// pass runs right after, same brain tick).
pub(crate) fn handle_report(orch: &Arc<Orchestrator>, job: &Arc<Job>, report: &Value) {
    let _ = orch;
    let account = job.account.clone();
    let Some(mut life) = load_life(&account) else { return };
    let node_id = report.get("node_id").as_str().unwrap_or("").to_string();
    if node_id.is_empty() {
        return;
    }
    let Some(index) = life.nodes.iter().position(|node| node.id == node_id) else {
        crate::logln!("brain {account}: report for unknown node {node_id}, ignored");
        return;
    };
    if life.nodes[index].status != NodeStatus::Active {
        // A late report for a node the graph already moved (crash
        // recovery counted it, operator reset): never fold twice.
        crate::logln!("brain {account}: report for {node_id} which is {}, ignored", life.nodes[index].status.as_str());
        return;
    }
    let stop = report.get("stop").as_str().unwrap_or("").to_string();
    let missing = report_missing(report);
    let ticks = report.get("ticks").as_i64().unwrap_or(0).max(0) as u64;
    life.nodes[index].spent_ticks = life.nodes[index].spent_ticks.saturating_add(ticks);
    // A rescue that ran out of budget mid-script is typed in the
    // session's last_error only (P1.6): read it when the report carries
    // no error of its own.
    let mut error = report_error(report);
    if error.is_none() && stop == "failed" {
        let rescue = job.inner.try_lock().ok().and_then(|inner| inner.last_error.as_ref().and_then(rescue_replan_cause));
        if rescue.is_some() {
            error = Some(("RescueBudget".to_string(), "random event rescue ran out of budget".to_string()));
        }
    }
    let kind = life.nodes[index].kind.clone();
    let method = life.nodes[index].method.clone().unwrap_or_default();
    // XP the run earned toward a climb is progress even short of a level
    // (a food refill is not a dead block; a slow prayer window is no
    // failure).
    let xp = xp_toward(report, &kind);
    lifescape_planner::record_xp(&mut life, index, xp);
    let held_before = life.nodes[index].progress_current;
    let (goal_met, market_pending) = match &kind {
        NodeKind::ReachLevel { skill, level } => match reach_level(report, skill) {
            Some(reached) => {
                // Progress clears the block count (one input batch per
                // block is the normal rhythm of a transformation skill).
                lifescape_planner::record_progress(&mut life, index, i64::from(reached));
                (reached >= *level, false)
            }
            None => (true, false),
        },
        NodeKind::ObtainItems { item, count } => {
            let quota = matches!(&life.goal, lifescape_planner::GoalKind::Produce { item: made, .. } if made == item);
            let held = if quota { holdings_count(job, item) } else { lifescape_planner::tools::family_held(item, *count, &|name| holdings_count(job, name)) };
            life.nodes[index].progress_current = held;
            (held >= *count, method == "market:buy")
        }
        // A coin target is met by the wallet, whatever the method sold.
        NodeKind::HaveCoins { amount } => {
            let held = holdings_count(job, "coins");
            life.nodes[index].progress_current = held;
            (held >= *amount, false)
        }
        // A training node is done when the combat level is reached, not
        // when one window ends (the dependent re-picks its site then).
        NodeKind::ReachCombat { level } => match reach_combat(report) {
            Some(reached) => {
                lifescape_planner::record_progress(&mut life, index, i64::from(reached));
                (reached >= *level, false)
            }
            None => (false, false),
        },
        _ => (true, false),
    };
    let progressed = xp > 0 || (matches!(kind, NodeKind::ObtainItems { .. }) && life.nodes[index].progress_current > held_before);
    let decision = verdict(&stop, error, &missing, goal_met, market_pending, progressed);
    let id = life.nodes[index].id.clone();
    match &decision {
        Verdict::Done => {
            // D-B: whichever leg landed first, the other one closes.
            if let NodeKind::ObtainItems { item, .. } = &kind {
                crate::brain_market::cancel_open_buys(&account, item);
            }
            let proof = life.nodes[index].progress_target.max(life.nodes[index].progress_current);
            complete_node(&mut life, index, proof);
            push_history(&mut life, &id, "goal_reached");
        }
        Verdict::AwaitFill => {
            // Stays active (the lock holds) until the holdings check in
            // the launch pass closes it or its deadline fails it.
            push_history(&mut life, &id, "awaiting_fill");
        }
        Verdict::EngagementEnd => {
            end_engagement(&mut life, index);
            push_history(&mut life, &id, "engagement_end");
        }
        Verdict::Stopped => {
            retry_uncounted(&mut life, index);
            push_history(&mut life, &id, "stopped");
        }
        Verdict::Blocked(needs) => {
            // A missing level climbs from the reported live level.
            let level_of = |skill: &str| reach_level(report, skill).unwrap_or(1);
            let fold = record_block_from(&mut life, index, needs, &level_of);
            push_history(&mut life, &id, if fold == FailureFold::Abandoned { "abandoned" } else { "blocked" });
        }
        Verdict::Contract(text) => {
            record_contract_error(&mut life, index, text);
            push_history(&mut life, &id, "abandoned");
        }
        Verdict::Travel(text) => {
            let banned = matches!(fold_travel_failure(&mut life.nodes[index], &method), TravelFold::NoRoute(_));
            if banned {
                record_contract_error(&mut life, index, &format!("no reachable method left ({text})"));
                push_history(&mut life, &id, "abandoned");
            } else {
                let fold = record_failure(&mut life, index, text);
                push_history(&mut life, &id, if fold == FailureFold::Abandoned { "abandoned" } else { "failed" });
            }
        }
        Verdict::Failure(text) => {
            let fold = record_failure(&mut life, index, text);
            push_history(&mut life, &id, if fold == FailureFold::Abandoned { "abandoned" } else { "failed" });
        }
    }
    let node = &life.nodes[index];
    crate::logln!(
        "brain {account}: {id} {:?} -> {} (failures {}/{} blocks {}) {}",
        decision,
        node.status.as_str(),
        node.failures,
        lifescape_planner::MAX_FAILURES,
        node.blocks,
        node.last_error
    );
    for change in normalize(&mut life) {
        crate::logln!("brain {account}: normalize {change}");
    }
    if save_life(&life).is_err() {
        crate::logln!("brain {account}: life save failed, report dropped");
    }
}

#[cfg(test)]
mod tests {
    use super::{verdict, Verdict};

    fn typed(kind: &str, message: &str) -> Option<(String, String)> {
        Some((kind.to_string(), message.to_string()))
    }

    /// t14/t20: the fishing run failed with `failed: fish failed at …`
    /// (runtime) — counted, never abandoned at the first try; a real
    /// `Usage` (CLI refused) abandons.
    #[test]
    fn runtime_failures_count_and_usage_abandons() {
        assert_eq!(
            verdict("failed", typed("Failed", "fish failed at (0, 3086, 3227)"), &[], false, false, false),
            Verdict::Failure("Failed: fish failed at (0, 3086, 3227)".to_string())
        );
        assert!(matches!(verdict("failed", typed("Usage", "fishing [count|spot...]"), &[], false, false, false), Verdict::Contract(_)));
        assert!(matches!(verdict("failed", typed("Sdk", "sdk: travel failure NoRoute seg=None"), &[], false, false, false), Verdict::Travel(_)));
        assert!(matches!(verdict("failed", None, &["died at (0,1,2)".to_string()], false, false, false), Verdict::Failure(text) if text == "died"));
        assert!(matches!(verdict("failed", None, &[], false, false, false), Verdict::Failure(_)), "an untyped failure still counts");
    }

    #[test]
    fn success_waits_and_stops() {
        assert_eq!(verdict("goal_reached", None, &[], true, false, false), Verdict::Done);
        assert_eq!(verdict("goal_reached", None, &[], false, true, false), Verdict::AwaitFill, "placed market order");
        assert!(matches!(verdict("goal_reached", None, &[], false, false, false), Verdict::Failure(_)), "short instant leg");
        assert_eq!(verdict("budget_exhausted", None, &[], false, false, false), Verdict::EngagementEnd);
        assert_eq!(verdict("budget_exhausted", None, &[], true, false, false), Verdict::Done, "goal landed with the window");
        assert_eq!(verdict("stopped", None, &[], false, false, false), Verdict::Stopped);
        assert_eq!(verdict("blocked", typed("Blocked", "x"), &["small_fishing_net".to_string()], false, false, false), Verdict::Blocked(vec!["small_fishing_net".to_string()]));
        assert!(matches!(verdict("blocked", None, &[], false, false, false), Verdict::Failure(_)), "blocked without missing counts");
    }

    /// Live 2026-11-13 (frank_lane, leo_king): `melee cows --level prayer
    /// 40` buried every bone (+652 prayer XP a window) and ended on its
    /// own budget with `goal_reached` short of 40: three such windows
    /// counted three failures and banned prayer. A window that earned
    /// XP toward the node continues (levels keep climbing), uncounted.
    #[test]
    fn a_short_window_with_progress_continues() {
        assert_eq!(verdict("goal_reached", None, &[], false, false, true), Verdict::EngagementEnd);
        assert!(matches!(verdict("goal_reached", None, &[], false, false, false), Verdict::Failure(_)), "no progress still counts");
        assert_eq!(verdict("goal_reached", None, &[], true, false, true), Verdict::Done);
    }

    /// XP the report credits to the node's skill (or any combat skill for
    /// a combat level), read from the report's `xp` rows.
    #[test]
    fn xp_toward_reads_the_report_rows() {
        let report = lifescape_json::parse(
            r#"{"xp":[{"skill":"attack","stat":0,"xp":3492},{"skill":"prayer","stat":5,"xp":652},{"skill":"hitpoints","stat":3,"xp":1545}]}"#,
        )
        .expect("json");
        assert_eq!(super::xp_toward(&report, &lifescape_planner::NodeKind::ReachLevel { skill: "prayer".to_string(), level: 40 }), 652);
        assert_eq!(super::xp_toward(&report, &lifescape_planner::NodeKind::ReachLevel { skill: "magic".to_string(), level: 40 }), 0);
        assert_eq!(super::xp_toward(&report, &lifescape_planner::NodeKind::ReachCombat { level: 15 }), 3492 + 1545, "combat skills only");
        assert_eq!(super::xp_toward(&report, &lifescape_planner::NodeKind::MakeMoney), 0);
    }
}

#[cfg(test)]
#[path = "brain_sim_tests.rs"]
mod sim;
