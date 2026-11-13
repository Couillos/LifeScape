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

/// One inventory load of a consumed input (the last slot holds the tool:
/// tinderbox, hammer, knife).
pub const INPUT_BATCH: i64 = 27;

/// Food a fight blocked on food asks for (the ladder's `MAX_FOOD` load).
pub const FOOD_BATCH: i64 = lifescape_gamedata::melee_ladder::MAX_FOOD as i64;

/// Object a skill method works on: the tail of `cook/raw_shrimp`,
/// `fire:logs`, `smelt:bronze_bar`, `spin:ball_of_wool`.
fn method_subject(method: &str) -> Option<&str> {
    let (_, tail) = method.split_once(['/', ':'])?;
    Some(tail.rsplit('/').next().unwrap_or(tail))
}

/// How many of `need` an obtain sub-task asks for. A consumed input of
/// the node's own work comes by the inventory load: a cook missing raw
/// shrimps asks 27, a smelter 13 copper and 13 tin (live 2026-11-12: one
/// unit per block bought one fish, and three blocks abandoned the
/// node). An obtain node's input scales with its count (5 bronze bars =
/// 5 copper + 5 tin). Tools and anything else come one at a time.
pub fn need_count(node: &Node, need: &str) -> i64 {
    let (product, units) = match &node.kind {
        NodeKind::ObtainItems { item, count } => (item.clone(), Some((*count).max(1))),
        _ => {
            let method = node.method.clone().or_else(|| node.alternatives.first().map(|alt| alt.method.clone())).unwrap_or_default();
            // A melee hunt blocked on food (`Blocked: shrimp`, the ladder
            // never fights a biting spot foodless) asks a full load: one
            // shrimp per block would block again after the first bite.
            if method.starts_with("fight:") && lifescape_gamedata::food::heal(need) > 0 {
                return FOOD_BATCH;
            }
            match method_subject(&method) {
                Some(subject) => (subject.to_string(), None),
                None => return 1,
            }
        }
    };
    // `cook/raw_shrimp`, `fire:logs`, `tan:cow_hide`: the subject is the
    // consumed input itself.
    if units.is_none() && product == need {
        return INPUT_BATCH;
    }
    for way in lifescape_gamedata::obtain::ways(&product) {
        if !way.is_production() {
            continue;
        }
        if way.tools.iter().any(|tool| tool == need) {
            return 1;
        }
        let Some((_, per_action)) = way.inputs.iter().find(|(input, _)| input == need) else { continue };
        let per_action = (*per_action).max(1);
        return match units {
            Some(count) => count.saturating_mul(per_action),
            None => {
                let load: i64 = way.inputs.iter().map(|(_, qty)| (*qty).max(1)).sum();
                (INPUT_BATCH / load.max(1)).max(1) * per_action
            }
        };
    }
    1
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
                let count = need_count(node, need);
                let mut sub = Node::new(
                    &format!("n{}", *counter),
                    NodeKind::ObtainItems { item: need.clone(), count },
                    Vec::new(),
                    count,
                );
                // Same legs as creation (PRD3: market first, every
                // production way, the shop under D-A); Iron is filtered
                // at resolve time, which re-prices them.
                sub.alternatives = crate::obtain_legs::obtain_alternatives(need, false);
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

    /// Live 2026-11-12: a cook missing raw shrimps filed `obtain 1`: one
    /// fish per block, abandoned after three. Inputs now come by the load.
    #[test]
    fn missing_inputs_come_by_the_inventory_load() {
        let reach = |method: &str| {
            let mut node = Node::new("n7", NodeKind::ReachLevel { skill: "cooking".to_string(), level: 20 }, Vec::new(), 20);
            node.method = Some(method.to_string());
            node
        };
        assert_eq!(super::need_count(&reach("cook/raw_shrimp"), "raw_shrimp"), super::INPUT_BATCH, "the cook's own input");
        assert_eq!(super::need_count(&reach("fire:logs"), "logs"), super::INPUT_BATCH);
        assert_eq!(super::need_count(&reach("fire:logs"), "tinderbox"), 1, "a tool comes alone");
        assert_eq!(super::need_count(&reach("smelt:bronze_bar"), "copper_ore"), 13, "copper + tin share the pack");
        let bars = Node::new("n8", NodeKind::ObtainItems { item: "bronze_bar".to_string(), count: 5 }, Vec::new(), 5);
        assert_eq!(super::need_count(&bars, "tin_ore"), 5, "an obtain scales its inputs");
        // The melee ladder blocks on food: a full load, never one shrimp.
        assert_eq!(super::need_count(&reach("fight:melee/ladder"), "shrimp"), super::FOOD_BATCH);
        assert_eq!(super::FOOD_BATCH, 20);
        assert_eq!(super::need_count(&reach("fight:melee/ladder"), "bronze_sword"), 1, "gear still comes alone");
        let mut cook = reach("cook/raw_shrimp");
        let fresh = replan_node(&mut cook, &ReplanCause::Blocked, &["raw_shrimp".to_string()], &mut 7);
        assert!(matches!(&fresh[0].kind, NodeKind::ObtainItems { item, count } if item == "raw_shrimp" && *count == 27), "{:?}", fresh[0].kind);
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
