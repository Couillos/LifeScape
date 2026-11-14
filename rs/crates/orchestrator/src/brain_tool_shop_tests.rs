//! Tests of `brain_tool_shop` (split out, rule 4): which tools the kit
//! buys through a step, never through a wish bid (the purchase rule is
//! `planner::tool_kit`'s, tested there).

use super::{merchant_tool, obtain_in_flight};
use lifescape_planner::{Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};

/// Every pickaxe (Nurmof) and Bob's axes (bronze to steel, user
/// 2026-11-14: a 1-2 coin bid nobody fills is no way to an axe) go
/// through a kit step; mithril axes and up are smithed only: the market.
#[test]
fn shop_sold_tools_go_through_a_step() {
    assert!(merchant_tool("rune_pickaxe") && merchant_tool("iron_pickaxe") && merchant_tool("bronze_pickaxe"));
    assert!(merchant_tool("bronze_axe") && merchant_tool("steel_axe"), "Bob sells them");
    assert!(!merchant_tool("mithril_axe") && !merchant_tool("rune_axe"), "players only");
    assert!(!merchant_tool("bronze_battleaxe"), "a weapon, not a tool");
    assert!(!merchant_tool("net"), "not a gathering tool of the wishlist");
}

fn life_with_obtain(status: NodeStatus) -> Life {
    let mut node = Node::new("n1", NodeKind::ObtainItems { item: "hammer".to_string(), count: 1 }, Vec::new(), 1);
    node.status = status;
    Life {
        schema: 1,
        account: "t_kit".to_string(),
        created_at: 0,
        seed: 1,
        profile: Profile::new(false, 0.5, 0.5, 1, Archetype::Producer),
        goal: GoalKind::MaxSkills { skills: vec!["smithing".to_string()], level: 20 },
        nodes: vec![node],
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
        memory: Default::default(),
    }
}

/// An obtain of the same tool already running: the kit waits for it,
/// a merely open one does not hold the kit back (a resting bid nobody
/// fills must not block the shop).
#[test]
fn only_a_running_obtain_holds_the_kit_back() {
    assert!(obtain_in_flight(&life_with_obtain(NodeStatus::Active), "hammer"));
    assert!(!obtain_in_flight(&life_with_obtain(NodeStatus::Ready), "hammer"));
    assert!(!obtain_in_flight(&life_with_obtain(NodeStatus::Active), "tinderbox"));
}
