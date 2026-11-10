//! Engagement draws and replanning triggers (§5.2). A draw is uniform
//! in [6000, 18000] at the start of the activity script itself;
//! dependencies never count. Replanning fires on blocked/failed/death,
//! lost stock, price drift over threshold, or engagement end.

use crate::model::{Node, NodeKind, NodeStatus};

/// Engagement bounds (D1 ticks; 6 min–18 min in dev, 1 h–3 h in prod).
pub const ENGAGE_MIN: u64 = 6000;
pub const ENGAGE_MAX: u64 = 18000;
/// Price drift that forces a re-resolve of a ready node.
pub const PRICE_DRIFT_PCT: f64 = 0.25;

/// Deterministic xorshift (no RNG dependency): same seed and counter
/// always draw the same engagement, so profiles replay.
pub fn draw_engagement_ticks(seed: u64, counter: u64) -> u64 {
    let mut word = seed.wrapping_add(counter.wrapping_mul(0x9E37_79B9_7F4A_7C15)).max(1);
    word ^= word << 13;
    word ^= word >> 7;
    word ^= word << 17;
    ENGAGE_MIN + word % (ENGAGE_MAX - ENGAGE_MIN + 1)
}

/// Why the brain replans a node.
#[derive(Debug, Clone, PartialEq)]
pub enum ReplanCause {
    Blocked,
    Failed,
    Death,
    StockLost,
    PriceDrift,
    EngagementEnd,
}

/// Decide from a pushed report: the stop string plus typed details the
/// session already reports (`missing`, death markers). `None` means the
/// node continues (goal not reached, budget not exhausted).
pub fn needs_replan(stop: &str, missing: &[String], died: bool, stock_lost: bool, price_drift: bool) -> Option<ReplanCause> {
    if died {
        return Some(ReplanCause::Death);
    }
    match stop {
        "blocked" => Some(ReplanCause::Blocked),
        "failed" => Some(ReplanCause::Failed),
        // P1.6: an expired rescue (typed `RescueBudget` in last_error,
        // dossier on disk) replans like a failure: the node retries with
        // a fresh method instead of stranding.
        "rescue_budget" => Some(ReplanCause::Failed),
        _ => {
            if stock_lost {
                Some(ReplanCause::StockLost)
            } else if price_drift {
                Some(ReplanCause::PriceDrift)
            } else if stop == "budget_exhausted" {
                Some(ReplanCause::EngagementEnd)
            } else {
                let _ = missing;
                None
            }
        }
    }
}

/// Apply one replan cause: returns fresh sub-task nodes to insert (e.g. an
/// `obtain_items` for each missing tool/input). The node itself moves to
/// `ready` (retry with a new method) or stays `blocked` until its deps
/// land; `failed` with no alternative abandons it.
pub fn replan_node(node: &mut Node, cause: &ReplanCause, missing: &[String], counter: &mut usize) -> Vec<Node> {
    match cause {
        ReplanCause::Blocked => {
            let mut fresh = Vec::new();
            for need in missing {
                *counter += 1;
                let mut sub = Node::new(
                    &format!("n{}", *counter),
                    NodeKind::ObtainItems { item: need.clone(), count: 1 },
                    Vec::new(),
                    1,
                );
                // Same source-aware legs as creation (tools never farm);
                // `resolve_ready` re-prices them with fresh public data.
                let (farmable, shopable) = crate::expand::source_flags(need);
                if farmable {
                    sub.alternatives.push(crate::model::Alternative {
                        method: format!("farm:{need}"),
                        est_ticks: 0,
                        est_coins: 0,
                    });
                }
                if shopable {
                    sub.alternatives.push(crate::model::Alternative {
                        method: "shop:buy".to_string(),
                        est_ticks: 0,
                        est_coins: 0,
                    });
                }
                // Market leg re-added at `resolve_ready` for non-Iron
                // only; the farm/shop legs always apply.
                sub.alternatives.push(crate::model::Alternative {
                    method: "market:buy".to_string(),
                    est_ticks: 0,
                    est_coins: 0,
                });
                fresh.push(sub);
            }
            // The blocked node waits on its new deps (or retries when
            // nothing concrete is missing).
            if fresh.is_empty() {
                node.status = NodeStatus::Ready;
                node.method = None;
            } else {
                let deps: Vec<String> = fresh.iter().map(|sub| sub.id.clone()).collect();
                node.deps.extend(deps);
                node.status = NodeStatus::Pending;
            }
            fresh
        }
        ReplanCause::Failed | ReplanCause::Death | ReplanCause::StockLost => {
            node.status = NodeStatus::Ready;
            node.method = None;
            node.spent_ticks = 0;
            Vec::new()
        }
        ReplanCause::PriceDrift => {
            // Method re-resolves at the next `ready` pass with fresh
            // numbers; the old lock drops so a cheaper way can win.
            node.status = NodeStatus::Ready;
            node.method = None;
            Vec::new()
        }
        ReplanCause::EngagementEnd => {
            match &node.kind {
                NodeKind::MakeMoney => {
                    node.status = NodeStatus::Done;
                    node.progress_current = node.progress_target;
                    Vec::new()
                }
                _ => {
                    node.status = NodeStatus::Ready;
                    node.method = None;
                    Vec::new()
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ENGAGE_MAX, ENGAGE_MIN, ReplanCause, draw_engagement_ticks, needs_replan, replan_node};
    use crate::model::{Node, NodeKind, NodeStatus};

    #[test]
    fn engagement_draw_stays_in_bounds_and_replays() {
        for counter in 0..50 {
            let drawn = draw_engagement_ticks(918273645, counter);
            assert!((ENGAGE_MIN..=ENGAGE_MAX).contains(&drawn), "draw {drawn} in [6000, 18000]");
        }
        assert_eq!(draw_engagement_ticks(42, 3), draw_engagement_ticks(42, 3), "seeded draws replay");
        assert_ne!(draw_engagement_ticks(42, 3), draw_engagement_ticks(42, 4), "counter advances the draw");
    }

    #[test]
    fn triggers_cover_the_prd_list() {
        assert_eq!(needs_replan("blocked", &[], false, false, false), Some(ReplanCause::Blocked));
        assert_eq!(needs_replan("failed", &[], false, false, false), Some(ReplanCause::Failed));
        assert_eq!(needs_replan("rescue_budget", &[], false, false, false), Some(ReplanCause::Failed), "expired rescue replans");
        assert_eq!(needs_replan("goal_reached", &[], true, false, false), Some(ReplanCause::Death), "death wins over the stop");
        assert_eq!(needs_replan("goal_reached", &[], false, true, false), Some(ReplanCause::StockLost));
        assert_eq!(needs_replan("goal_reached", &[], false, false, true), Some(ReplanCause::PriceDrift));
        assert_eq!(needs_replan("budget_exhausted", &[], false, false, false), Some(ReplanCause::EngagementEnd));
        assert_eq!(needs_replan("goal_reached", &[], false, false, false), None, "progress continues");
    }

    #[test]
    fn blocked_spawns_obtain_and_pends() {
        let mut node = Node::new("n9", NodeKind::ObtainItems { item: "logs".to_string(), count: 10 }, Vec::new(), 10);
        let fresh = replan_node(&mut node, &ReplanCause::Blocked, &["bronze_axe".to_string()], &mut 9);
        assert_eq!(fresh.len(), 1);
        assert!(matches!(&fresh[0].kind, NodeKind::ObtainItems { item, .. } if item == "bronze_axe"));
        assert_eq!(node.status, NodeStatus::Pending, "waits on its new dep");
        assert_eq!(node.deps, vec!["n10".to_string()]);
    }

    #[test]
    fn engagement_end_closes_money_only() {
        let mut money = Node::new("n3", NodeKind::MakeMoney, Vec::new(), 0);
        replan_node(&mut money, &ReplanCause::EngagementEnd, &[], &mut 3);
        assert_eq!(money.status, NodeStatus::Done, "one engagement = one money node");
        let mut level = Node::new("n4", NodeKind::ReachLevel { skill: "fishing".to_string(), level: 20 }, Vec::new(), 20);
        replan_node(&mut level, &ReplanCause::EngagementEnd, &[], &mut 4);
        assert_eq!(level.status, NodeStatus::Ready, "levels continue toward the goal");
    }
}
