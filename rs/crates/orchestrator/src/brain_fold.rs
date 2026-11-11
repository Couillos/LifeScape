//! Brain report folding (split from brain.rs, same behavior): pushed
//! task reports replan the graph; launching stays in brain.rs. A
//! travel failure bans the proven-unreachable method instead of
//! re-locking it (dispatch aveugle : jamais de marche impossible).

#![forbid(unsafe_code)]

use std::sync::Arc;

use lifescape_json::Value;
use lifescape_planner::{needs_replan, replan_node};

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

fn report_missing(report: &Value) -> Vec<String> {
    report
        .get("missing")
        .as_arr()
        .map(|rows| rows.iter().filter_map(|row| row.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn fail_count(life: &lifescape_planner::Life, node: &str) -> usize {
    life.history.iter().filter(|entry| entry.node == node && entry.outcome == "failed").count()
}

/// Fold one pushed report into the graph; launches nothing (the launch
/// pass below runs right after, same tick).
pub(crate) fn handle_report(orch: &Arc<Orchestrator>, job: &Arc<Job>, report: &Value) {
    let account = job.account.clone();
    let Some(mut life) = load_life(&account) else { return };
    let node_id = report.get("node_id").as_str().unwrap_or("").to_string();
    if node_id.is_empty() {
        return;
    }
    let Some(index) = life.nodes.iter().position(|node| node.id == node_id) else { return };
    let stop = report.get("stop").as_str().unwrap_or("").to_string();
    let missing = report_missing(report);
    let ticks = report.get("ticks").as_i64().unwrap_or(0).max(0) as u64;
    let died = missing.iter().any(|need| need.contains("died") || need.contains("death"));
    let stock_lost = stop == "failed" && missing.iter().any(|need| need.contains("lost"));
    // Price drift: the market moved >25% on a locked buy since resolve.
    let price_drift = false;
    // P1.6: a typed `RescueBudget` in last_error replans like a failure
    // even with no task report detail; consumed here (dossier on disk).
    // A travel failure also reads here (typed NoRoute vocabulary), and
    // bans the method below instead of re-locking it.
    let (rescue, travel): (Option<lifescape_planner::ReplanCause>, bool) = job
        .inner
        .try_lock()
        .map(|inner| {
            let cause = inner.last_error.as_ref().and_then(rescue_replan_cause);
            let travel = inner.last_error.as_ref().is_some_and(is_travel_failure);
            (cause, travel)
        })
        .unwrap_or((None, false));
    life.nodes[index].spent_ticks = life.nodes[index].spent_ticks.saturating_add(ticks);
    let kind = life.nodes[index].kind.clone();
    match stop.as_str() {
        "goal_reached" => {
            let done = match &kind {
                lifescape_planner::NodeKind::ReachLevel { skill, level } => {
                    if let Some(reached) = reach_level(report, skill) {
                        life.nodes[index].progress_current = i64::from(reached);
                        reached >= *level
                    } else {
                        life.nodes[index].progress_current = life.nodes[index].progress_target;
                        true
                    }
                }
                lifescape_planner::NodeKind::ObtainItems { item, count } => {
                    // Market buys settle later: a placed order is not a
                    // filled one. Shop buys settle now: a shortfall means
                    // the leg failed (broke or unstocked), never a wait.
                    let held = holdings_count(job, item);
                    life.nodes[index].progress_current = held;
                    held >= *count
                }
                _ => {
                    life.nodes[index].progress_current = life.nodes[index].progress_target;
                    true
                }
            };
            if done {
                life.nodes[index].status = lifescape_planner::NodeStatus::Done;
                if life.active.as_deref() == Some(life.nodes[index].id.as_str()) {
                    life.active = None;
                }
                if life.engagement.as_ref().is_some_and(|engaged| engaged.node == life.nodes[index].id) {
                    life.engagement = None;
                }
                let id = life.nodes[index].id.clone();
                push_history(&mut life, &id, "goal_reached");
            } else if matches!(&kind, lifescape_planner::NodeKind::ObtainItems { .. })
                && life.nodes[index].method.as_deref() != Some("market:buy")
            {
                // Instant leg, still short: the method failed, not the
                // market. Replan (bounded by the 3-strike rule) instead
                // of waiting for a fill that never comes on its own.
                let id = life.nodes[index].id.clone();
                life.nodes[index].status = lifescape_planner::NodeStatus::Ready;
                life.nodes[index].method = None;
                if life.active.as_deref() == Some(id.as_str()) {
                    life.active = None;
                }
                if fail_count(&life, &id) >= 2 {
                    life.nodes[index].status = lifescape_planner::NodeStatus::Abandoned;
                    push_history(&mut life, &id, "abandoned");
                } else {
                    push_history(&mut life, &id, "failed");
                }
            } else {
                // Placed but unfilled: wait for stock, keep the method.
                life.nodes[index].status = lifescape_planner::NodeStatus::Active;
            }
        }
        other => {
            // `blocked` without a typed `missing` is a script bug, not a
            // shopping list: count it as a failure so the 3-strike rule
            // bounds the replan-relaunch loop instead of spinning it.
            let effective = if other == "blocked" && missing.is_empty() { "failed" } else { other };
            // Voyage prouvé impossible : la méthode bannie ne repart
            // jamais telle quelle (replan sur l'alternative suivante,
            // abandon typé quand il n'en reste aucune).
            if travel && effective == "failed" {
                if let Some(locked) = life.nodes[index].method.clone() {
                    job.inner.try_lock().map(|mut inner| inner.last_error = None).ok();
                    match fold_travel_failure(&mut life.nodes[index], &locked) {
                        TravelFold::Retry => {
                            let id = life.nodes[index].id.clone();
                            life.nodes[index].status = lifescape_planner::NodeStatus::Ready;
                            if life.active.as_deref() == Some(id.as_str()) {
                                life.active = None;
                            }
                            push_history(&mut life, &id, "failed");
                            println!("brain {}: {id} no route for {locked}, retrying next alternative", job.account);
                        }
                        TravelFold::NoRoute(banned) => {
                            let id = life.nodes[index].id.clone();
                            life.nodes[index].status = lifescape_planner::NodeStatus::Abandoned;
                            if life.active.as_deref() == Some(id.as_str()) {
                                life.active = None;
                            }
                            push_history(&mut life, &id, "abandoned");
                            println!("brain {}: {id} no route for {banned}, abandoning (typed, no alternative)", job.account);
                        }
                    }
                    if save_life(&life).is_err() {
                        println!("brain {}: life save failed, report dropped", job.account);
                        return;
                    }
                    println!("brain {}: folded report node={} stop={}", job.account, node_id, report.get("stop").as_str().unwrap_or("?"));
                    let _ = orch;
                    return;
                }
            }
            if let Some(cause) = rescue.or_else(|| needs_replan(effective, &missing, died, stock_lost, price_drift)) {
                if fail_count(&life, &life.nodes[index].id) >= 3 && effective == "failed" {
                    let id = life.nodes[index].id.clone();
                    life.nodes[index].status = lifescape_planner::NodeStatus::Abandoned;
                    life.active = None;
                    push_history(&mut life, &id, "abandoned");
                } else {
                    let id = life.nodes[index].id.clone();
                    let mut counter = life.nodes.len();
                    // Unique ids across growth: bump past every n<k>.
                    for node in &life.nodes {
                        if let Some(number) = node.id.strip_prefix('n').and_then(|raw| raw.parse::<usize>().ok()) {
                            counter = counter.max(number);
                        }
                    }
                    let fresh = {
                        let node = &mut life.nodes[index];
                        replan_node(node, &cause, &missing, &mut counter)
                    };
                    if effective == "failed" {
                        push_history(&mut life, &id, "failed");
                    }
                    if matches!(cause, lifescape_planner::ReplanCause::EngagementEnd)
                        && life.nodes[index].status == lifescape_planner::NodeStatus::Done
                    {
                        push_history(&mut life, &id, "engagement_end");
                    }
                    life.nodes.extend(fresh);
                    // A replanned node leaves Active: the launch pass may
                    // pick it up again, never strand on the old lock.
                    if life.nodes[index].status != lifescape_planner::NodeStatus::Done
                        && life.active.as_deref() == Some(id.as_str())
                    {
                        life.active = None;
                    }
                }
            }
        }
    }
    if save_life(&life).is_err() {
        println!("brain {}: life save failed, report dropped", job.account);
        return;
    }
    println!(
        "brain {}: folded report node={} stop={}",
        job.account,
        node_id,
        report.get("stop").as_str().unwrap_or("?")
    );
    let _ = orch;
}
