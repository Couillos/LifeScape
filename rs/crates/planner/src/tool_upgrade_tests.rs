//! Tests of `tool_upgrade` (split out, rule 4) on the rev 274 data.

use super::*;
use crate::model::{Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};

/// The cheapest way to have a tier now; a tie goes to the shop, then
/// the ask; the own make only when nobody sells; nothing is no offer.
#[test]
fn the_cheapest_offer_wins() {
    assert_eq!(cheapest(Some(200), Some(120), None), Some(ToolOffer { price: 120, source: ToolSource::Ask }), "steel: the 120 ask beats Bob's 200");
    assert_eq!(cheapest(Some(200), Some(200), None).map(|offer| offer.source), Some(ToolSource::Shop));
    let make = || Some(("smith:mithril_axe".to_string(), 90));
    assert_eq!(cheapest(None, Some(312), make()).map(|offer| offer.source), Some(ToolSource::Ask), "a seller first");
    assert_eq!(cheapest(None, None, make()), Some(ToolOffer { price: 90, source: ToolSource::Make("smith:mithril_axe".to_string()) }), "nobody sells: the own make");
    assert_eq!(cheapest(None, None, None), None, "a rune axe nobody sells is no target");
}

/// Own make: a recipe leg the bot's level opens (mithril axe: smithing
/// 51, one mithril bar), priced at its inputs; under the level, or an
/// unpriced input, no make.
#[test]
fn own_make_needs_the_level_and_priced_inputs() {
    let bar = |item: &str| (item == "mithril_bar").then_some(150);
    assert_eq!(own_make("mithril_axe", &|_| 1, &bar), None, "smithing 1: no make (jim_ellis)");
    assert_eq!(own_make("mithril_axe", &|skill| if skill == "smithing" { 51 } else { 1 }, &bar), Some(("smith:mithril_axe".to_string(), 150)));
    assert_eq!(own_make("mithril_axe", &|_| 99, &|_| None), None, "an unpriced bar voids the leg");
    assert_eq!(own_make("rune_pickaxe", &|_| 99, &|_| Some(1)), None, "no pickaxe recipe in rev 274");
}

pub(crate) fn life_with(nodes: Vec<Node>) -> Life {
    Life {
        schema: 1,
        account: "t_upgrade".to_string(),
        created_at: 0,
        seed: 3,
        profile: Profile::new(false, 0.5, 0.5, 3, Archetype::Producer),
        goal: GoalKind::MaxSkills { skills: vec!["firemaking".to_string()], level: 90 },
        nodes,
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
        memory: Default::default(),
    }
}

/// jim_ellis 2026-11-17: 16 134 of 20 425 yews chopped (`farm:yew_logs`
/// locked), then firemaking 90. The axe's planned work is the 4 291
/// yews left; the fires burn with a tinderbox; no pickaxe work.
pub(crate) fn jim_life() -> Life {
    let mut yews = Node::new("n471", NodeKind::ObtainItems { item: "yew_logs".to_string(), count: 20425 }, Vec::new(), 20425);
    yews.status = NodeStatus::Active;
    yews.method = Some("farm:yew_logs".to_string());
    yews.progress_current = 16134;
    let fires = Node::new("n469", NodeKind::ReachLevel { skill: "firemaking".to_string(), level: 90 }, vec!["n471".to_string()], 90);
    let mut done = Node::new("n400", NodeKind::ObtainItems { item: "yew_logs".to_string(), count: 500 }, Vec::new(), 500);
    done.status = NodeStatus::Done;
    done.method = Some("farm:yew_logs".to_string());
    life_with(vec![yews, fires, done])
}

pub(crate) fn jim_levels(skill: &str) -> u8 {
    match skill {
        "woodcutting" => 86,
        "firemaking" => 60,
        _ => 1,
    }
}

#[test]
fn planned_work_counts_the_open_nodes_of_the_tool() {
    let life = jim_life();
    let axe = planned_work(&life, "bronze_axe", &jim_levels);
    assert_eq!(axe, vec![PlannedWork { method: "farm:yew_logs".to_string(), level: 86, units: 4291.0 }]);
    assert!(planned_work(&life, "bronze_pickaxe", &jim_levels).is_empty());
    // A woodcutting climb counts its XP left in units of its tranche.
    let climb = Node::new("n1", NodeKind::ReachLevel { skill: "woodcutting".to_string(), level: 90 }, Vec::new(), 90);
    let work = planned_work(&life_with(vec![climb]), "bronze_axe", &jim_levels);
    assert_eq!(work.len(), 1);
    assert_eq!(work[0].method, "gather:woodcut/yew_logs");
    assert!(work[0].units > 1000.0, "86 -> 90 is thousands of yews: {work:?}");
}

/// Live 2026-11-17 09:55 (jim_ellis, no kit step): a window ends
/// (`EngagementEnd`: Ready, method cleared) and the kit pass runs BEFORE
/// the resolve locks `farm:yew_logs` again. The obtain's production
/// alternative using the tool still counts; a purchase leg never does.
#[test]
fn an_obtain_between_windows_still_counts_its_work() {
    let mut life = jim_life();
    life.nodes[0].status = NodeStatus::Ready;
    life.nodes[0].method = None;
    life.nodes[0].alternatives = vec![
        crate::model::Alternative { method: "market:buy".to_string(), est_ticks: 1714, est_coins: 411936 },
        crate::model::Alternative { method: "farm:yew_logs".to_string(), est_ticks: 2434338, est_coins: 0 },
    ];
    let axe = planned_work(&life, "bronze_axe", &jim_levels);
    assert_eq!(axe, vec![PlannedWork { method: "farm:yew_logs".to_string(), level: 86, units: 4291.0 }]);
    life.nodes[0].alternatives.retain(|alt| alt.method == "market:buy");
    assert!(planned_work(&life, "bronze_axe", &jim_levels).is_empty(), "bought, not chopped: no axe work");
}

/// Server odds (`tree_chance`): each tier chops yews faster; the ticks
/// saved grow with the tier and the volume, and a better tool pays only
/// when they beat its price in the bot's ticks.
#[test]
fn ticks_saved_grow_with_the_tier_and_pay_against_the_price() {
    let work = planned_work(&jim_life(), "bronze_axe", &jim_levels);
    let steel = ticks_saved(&work, "iron_axe", "steel_axe");
    let mithril = ticks_saved(&work, "iron_axe", "mithril_axe");
    assert!(steel > 0.0 && mithril > steel, "steel {steel}, mithril {mithril}");
    assert_eq!(ticks_saved(&work, "iron_axe", "iron_axe"), 0.0);
    assert_eq!(ticks_saved(&work, "mithril_axe", "iron_axe"), 0.0, "a worse tool saves nothing");
    assert!(upgrade_pays(mithril, 312, 0.1), "{mithril} ticks saved > 3 120 ticks of coins");
    let few = vec![PlannedWork { units: 50.0, ..work[0].clone() }];
    assert!(!upgrade_pays(ticks_saved(&few, "iron_axe", "mithril_axe"), 312, 0.1), "50 yews never pay 312 coins");
    assert_eq!(cost_ticks(312, 0.0), 3120.0, "a rate under the floor reads the floor");
}
