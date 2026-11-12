//! Space pressure (L8) and Iron/market guards (L9). Pure: caller-supplied
//! numbers in, decisions out. No network, no files, no cross-bot state.
//!
//! Priority of the protected set (§5.7): coins, food to 100 units total,
//! then the brain's keep-list (quest non-tradeables, graph requirements,
//! goal-style gear; tools ride in it too, resolved worker-side to the
//! best banked tool per kit spec). Everything else banked is surplus:
//! market sells (D12, down to 1 coin), floor invendus fall back to D2
//! (alch when the formula wins, else drop for D10; PRD3: never an NPC
//! shop sale), and a
//! bank still full of unsold D12 stock drops whole stacks cheapest-total
//! first with the orders cancelled (D17).
//!
//! Alchemy legs are decided here; the worker casts them when it holds the
//! runes and the level, and otherwise keeps the stack listed.

#![forbid(unsafe_code)]

/// Bank distinct-type limit (F2P banks hold 60 types): cleanup at 55.
pub const BANK_TYPE_LIMIT: usize = 55;
/// Food units kept across types before the rest becomes surplus.
pub const FOOD_KEEP_MIN: i64 = 100;

/// The bank needs a `space_cleanup` node at or past the type limit.
pub fn needs_cleanup(bank_types: usize) -> bool {
    bank_types >= BANK_TYPE_LIMIT
}

/// Split banked stacks (debug name, quantity) into protected names and
/// surplus stacks. Coins always survive; food types survive whole while
/// the running food total stays under 100; every keep-list name survives.
pub fn triage_bank(stacks: &[(String, i64)], keep: &[String]) -> (Vec<String>, Vec<(String, i64)>) {
    let mut protected: Vec<String> = Vec::new();
    let mut food_total = 0i64;
    for (name, qty) in stacks {
        if *qty <= 0 || protected.contains(name) {
            continue;
        }
        if name == "coins" {
            protected.push(name.clone());
        } else if food_total < FOOD_KEEP_MIN && lifescape_gamedata::food::heal(name) > 0 {
            protected.push(name.clone());
            food_total = food_total.saturating_add(*qty);
        }
    }
    for wanted in keep {
        if stacks.iter().any(|(name, qty)| name == wanted && *qty > 0) && !protected.contains(wanted) {
            protected.push(wanted.clone());
        }
    }
    let surplus: Vec<(String, i64)> = stacks
        .iter()
        .filter(|(name, qty)| *qty > 0 && !protected.contains(name))
        .cloned()
        .collect();
    (protected, surplus)
}

/// D2 fallback legs for a floor invendu (or an Iron surplus, direct).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback {
    Market,
    AlchHigh,
    AlchLow,
    Drop,
}

/// Priced context for one surplus item: its market value (reference),
/// alchemy values, the caster's level and runes, the rune cost at market
/// prices. PRD3: no NPC buyback exists in the economy.
#[derive(Debug, Clone)]
pub struct ItemEcon {
    pub tradeable: bool,
    pub alch_high: i64,
    pub alch_low: i64,
    pub market_value: i64,
    pub magic_level: i64,
    pub has_runes: bool,
    pub rune_cost: i64,
}

/// D12 routing: tradeable surplus goes through the carnet for non-Iron.
pub fn market_leg(tradeable: bool, iron: bool) -> bool {
    tradeable && !iron
}

/// D2 decision: high alchemy (55) or low alchemy (21) when the caster has
/// the runes and `alch − rune_cost > market value`, else the drop (D10).
/// PRD3: never a sale to an NPC shop.
pub fn fallback_for(item: &ItemEcon) -> Fallback {
    if item.magic_level >= 55 && item.has_runes && item.alch_high.saturating_sub(item.rune_cost) > item.market_value {
        return Fallback::AlchHigh;
    }
    if item.magic_level >= 21 && item.has_runes && item.alch_low.saturating_sub(item.rune_cost) > item.market_value {
        return Fallback::AlchLow;
    }
    Fallback::Drop
}

/// Reserve floor (§5.5): the bot never sells under its castable alchemy
/// net, and D12 never prices under 1. PRD3: no shop buyback term.
pub fn reserve_floor(alch_net: Option<i64>) -> i64 {
    alch_net.unwrap_or(0).max(1)
}

/// D17 drop order for stock stuck at the carnet with a full bank: whole
/// stacks, cheapest total value (quantity × reference) first. The caller
/// drops full stacks and cancels their orders; a partial pile frees no
/// bank slot.
pub fn d17_order(stuck: &[(String, i64, i64)]) -> Vec<String> {
    let mut rows: Vec<(String, i64, i64)> = stuck.to_vec();
    rows.sort_by_key(|(_, qty, price)| qty.saturating_mul(*price));
    rows.into_iter().map(|(name, _, _)| name).collect()
}

/// Iron never touches a `market:*` method (planner legs, brain orders and
/// the physical handover alike); the cleanup method itself is not one.
pub fn iron_may_use(method: &str) -> bool {
    !method.starts_with("market:")
}

/// Launch guard for one locked method on an Iron profile.
pub fn launch_allowed(method: &str, ironman: bool) -> bool {
    !ironman || iron_may_use(method)
}

/// Stall guard: the last two finished janitors both abandoned means the
/// pressure never drains (persistent failure), so inserts stop. A `done`
/// anywhere in the tail breaks the streak: healthy cleanups never stall.
pub fn cleanup_stalled(nodes: &[crate::model::Node]) -> bool {
    let mut finished = nodes
        .iter()
        .filter(|node| {
            matches!(node.kind, crate::model::NodeKind::SpaceCleanup)
                && matches!(node.status, crate::model::NodeStatus::Done | crate::model::NodeStatus::Abandoned)
        })
        .rev();
    matches!(
        (finished.next().map(|node| &node.status), finished.next().map(|node| &node.status)),
        (Some(crate::model::NodeStatus::Abandoned), Some(crate::model::NodeStatus::Abandoned))
    )
}

/// Insert a fresh cleanup node only while the bank is past the limit,
/// nothing is already open, and the janitor is not stalled.
pub fn cleanup_needed(open_exists: bool, stalled: bool, bank_types: usize) -> bool {
    needs_cleanup(bank_types) && !open_exists && !stalled
}

#[cfg(test)]
mod tests {
    use super::{
        cleanup_needed, d17_order, fallback_for, iron_may_use, launch_allowed, market_leg, needs_cleanup,
        reserve_floor, triage_bank, Fallback, ItemEcon, BANK_TYPE_LIMIT, FOOD_KEEP_MIN,
    };
    use crate::model::{Node, NodeKind, NodeStatus};

    fn stacks() -> Vec<(String, i64)> {
        vec![
            ("coins".to_string(), 5000),
            ("trout".to_string(), 60),
            ("shrimp".to_string(), 30),
            ("lobster".to_string(), 200),
            ("iron_sword".to_string(), 1),
            ("yew_logs".to_string(), 400),
        ]
    }

    #[test]
    fn trigger_fires_at_the_type_limit() {
        assert!(!needs_cleanup(BANK_TYPE_LIMIT - 1), "54 types rest");
        assert!(needs_cleanup(BANK_TYPE_LIMIT), "55 types clean");
        assert!(needs_cleanup(60), "full bank cleans");
    }

    #[test]
    fn protected_beats_surplus_in_priority_order() {
        let (protected, surplus) = triage_bank(&stacks(), &["iron_sword".to_string(), "trout".to_string()]);
        assert!(protected.contains(&"coins".to_string()), "coins first");
        // Food to 100 total: trout 60 + shrimp 30 = 90, lobster tips over.
        assert!(protected.contains(&"trout".to_string()), "food under 100 stays");
        assert!(protected.contains(&"shrimp".to_string()), "food under 100 stays");
        assert!(protected.contains(&"lobster".to_string()), "food filling past 100 stays whole");
        assert!(protected.contains(&"iron_sword".to_string()), "keep-list survives");
        let surplus_names: Vec<&str> = surplus.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(surplus_names, vec!["yew_logs"], "only the rest sells, got {surplus_names:?}");
    }

    #[test]
    fn food_cutoff_holds_past_one_hundred() {
        let bank = vec![("lobster".to_string(), 500), ("trout".to_string(), 40)];
        let (protected, surplus) = triage_bank(&bank, &[]);
        assert!(protected.contains(&"lobster".to_string()), "first food type covers the 100");
        assert!(surplus.iter().any(|(name, _)| name == "trout"), "past-100 food becomes surplus");
    }

    #[test]
    fn empty_and_zero_stacks_never_list() {
        let (protected, surplus) = triage_bank(&[("coins".to_string(), 0)], &["ghost".to_string()]);
        assert!(protected.is_empty() && surplus.is_empty(), "nothing banked, nothing decided");
        assert_eq!(FOOD_KEEP_MIN, 100, "food bar pinned");
    }

    fn econ() -> ItemEcon {
        ItemEcon {
            tradeable: true,
            alch_high: 100,
            alch_low: 30,
            market_value: 40,
            magic_level: 55,
            has_runes: true,
            rune_cost: 20,
        }
    }

    #[test]
    fn d2_formula_picks_alch_only_when_it_beats_the_shop() {
        assert_eq!(fallback_for(&econ()), Fallback::AlchHigh, "100 - 20 > 40");
        let low = ItemEcon { magic_level: 21, alch_high: 0, alch_low: 70, ..econ() };
        assert_eq!(fallback_for(&low), Fallback::AlchLow, "70 - 20 > 40 casts low");
    }

    #[test]
    fn d2_falls_through_to_shop_then_drop() {
        let poor_alch = ItemEcon { rune_cost: 90, ..econ() };
        assert_eq!(fallback_for(&poor_alch), Fallback::Drop, "100 - 90 < 40: never a shop sale, drop");
        let no_runes = ItemEcon { has_runes: false, ..econ() };
        assert_eq!(fallback_for(&no_runes), Fallback::Drop, "no runes: drop (PRD3, no NPC buyback)");
    }

    #[test]
    fn reserve_floor_takes_the_best_and_never_drops_under_one() {
        assert_eq!(reserve_floor(Some(80)), 80, "castable alch is the reserve");
        assert_eq!(reserve_floor(Some(0)), 1, "worthless alch: one coin");
        assert_eq!(reserve_floor(None), 1, "D12 minimum one coin");
    }

    #[test]
    fn d17_drops_cheapest_total_first() {
        let stuck = vec![
            ("yew_logs".to_string(), 400, 100),
            ("feather".to_string(), 2000, 3),
            ("lobster".to_string(), 5, 200),
        ];
        // Totals: logs 40000, feathers 6000, lobsters 1000.
        assert_eq!(d17_order(&stuck), vec!["lobster", "feather", "yew_logs"]);
    }

    #[test]
    fn iron_never_touches_market_methods() {
        assert!(iron_may_use("farm:feather"), "farm fine");
        assert!(iron_may_use("space:cleanup"), "cleanup dispatches market sells only for non-Iron");
        assert!(!iron_may_use("market:buy"), "no buys");
        assert!(!iron_may_use("market:sell"), "no sells");
        assert!(!iron_may_use("market:flip"), "no merchanting");
        assert!(launch_allowed("market:buy", false), "non-Iron unaffected");
        assert!(!launch_allowed("market:buy", true), "Iron refused");
        assert!(launch_allowed("farm:feather", true), "Iron farms");
        assert!(market_leg(true, false), "tradeable non-Iron lists");
        assert!(!market_leg(true, true), "Iron never lists");
        assert!(!market_leg(false, false), "untradeable never lists");
    }

    #[test]
    fn cleanup_reinserts_until_stall() {
        assert!(cleanup_needed(false, false, 55), "full bank without a node inserts");
        assert!(!cleanup_needed(true, false, 59), "an open node blocks doubles");
        assert!(!cleanup_needed(false, true, 60), "a stalled janitor stops the loop");
        assert!(!cleanup_needed(false, false, 54), "room left, no node");
    }

    fn cleanup_node(status: NodeStatus) -> Node {
        let mut node = Node::new("n9", NodeKind::SpaceCleanup, Vec::new(), 0);
        node.status = status;
        node
    }

    #[test]
    fn stall_needs_two_abandoned_in_a_row() {
        use super::cleanup_stalled;
        assert!(!cleanup_stalled(&[]), "no janitor never stalls");
        assert!(!cleanup_stalled(&[cleanup_node(NodeStatus::Abandoned)]), "one failure retries");
        assert!(!cleanup_stalled(&[cleanup_node(NodeStatus::Done)]), "success never stalls");
        assert!(cleanup_stalled(&[cleanup_node(NodeStatus::Abandoned), cleanup_node(NodeStatus::Abandoned)]), "two in a row stall");
        assert!(
            !cleanup_stalled(&[cleanup_node(NodeStatus::Abandoned), cleanup_node(NodeStatus::Done)]),
            "a done breaks the streak"
        );
        assert!(
            !cleanup_stalled(&[cleanup_node(NodeStatus::Done), cleanup_node(NodeStatus::Abandoned)]),
            "one trailing failure retries"
        );
    }
}
