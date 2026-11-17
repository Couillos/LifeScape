//! Tests of `cooldown.rs` (plan P1): an abandoned method cools for the
//! bot that failed it, for every goal kind.

use super::{cool_entry, cooled_keys, cooled_sites, keys_for_abandon, method_cooled, same_error, split_key, COOL_NODE, COOL_TICKS};
use crate::expand::{method_feasible, resolve_ready, ReadyCtx};
use crate::model::{Alternative, HistoryEntry, Node, NodeKind, NodeStatus};

fn entry(outcome: &str, ended: i64) -> HistoryEntry {
    HistoryEntry { node: COOL_NODE.to_string(), started: ended, ended, outcome: outcome.to_string() }
}

#[test]
fn keys_round_trip_with_and_without_site() {
    assert_eq!(split_key("fight:melee/men@3097,3508,0"), ("fight:melee/men", Some((3097, 3508, 0))));
    assert_eq!(split_key("gather:woodcut/willow_logs"), ("gather:woodcut/willow_logs", None));
    assert_eq!(split_key("fight:magic/wind_strike/chickens"), ("fight:magic/wind_strike/chickens", None));
}

#[test]
fn a_cooldown_lasts_its_ticks_then_lapses() {
    let history = vec![entry(&cool_entry("fight:melee/men@3097,3508,0", COOL_TICKS, "Failed: pack too full | x"), 100)];
    let early = cooled_keys(&history, &|_| 10);
    assert_eq!(early, vec!["fight:melee/men@3097,3508,0".to_string()]);
    assert_eq!(cooled_sites(&early, "fight:melee/men"), vec![(3097, 3508, 0)]);
    assert!(!method_cooled(&early, "fight:melee/men"), "only the site cools");
    assert!(cooled_keys(&history, &|_| COOL_TICKS).is_empty(), "lapsed");
}

/// A second site of the same method failing within the cooldown cools
/// the method itself: the failure follows the method, not the place.
#[test]
fn a_second_failed_site_cools_the_method() {
    let cooled = vec!["fight:melee/men@3097,3508,0".to_string()];
    let keys = keys_for_abandon(&cooled, "fight:melee/men", Some((3222, 3218, 0)));
    assert_eq!(keys, vec!["fight:melee/men@3222,3218,0".to_string(), "fight:melee/men".to_string()]);
    assert_eq!(keys_for_abandon(&[], "smith:bronze_bar", None), vec!["smith:bronze_bar".to_string()]);
    assert!(keys_for_abandon(&[], "", None).is_empty());
}

#[test]
fn the_same_error_ignores_positions_and_counts() {
    assert!(same_error(
        "Failed: combat bank trip left the pack too full to hunt (1 free): 60x Coins, 22x Cabbage",
        "Failed: combat bank trip left the pack too full to hunt (2 free): 61x Coins, 27x Cabbage"
    ));
    assert!(!same_error("Failed: no coal mined", "Failed: pack too full"));
    assert!(!same_error("", ""));
}

fn money_node() -> Node {
    let mut node = Node::new("n478", NodeKind::HaveCoins { amount: 240 }, Vec::new(), 240);
    node.status = NodeStatus::Ready;
    node
}

/// Plan P1 red test: a bot whose `fight:melee/men` was abandoned (pack too
/// full) never gets it back from the next money resolve, whatever goal
/// draws it; another money method serves.
#[test]
fn a_cooled_method_is_never_resolved_again() {
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.combat = 40;
    ctx.position = Some((3222, 3218, 0));
    let mut free = money_node();
    resolve_ready(&mut free, &ctx);
    let first = free.method.clone().expect("a money method resolves");
    ctx.cooled = vec![first.clone()];
    assert!(!method_feasible(&first, &ctx), "{first} cooled");
    let mut again = money_node();
    resolve_ready(&mut again, &ctx);
    assert_ne!(again.method.as_deref(), Some(first.as_str()), "the cooled method came back: {:?}", again.alternatives);
}

/// A cooled site is skipped by the site pick; with every site cooled the
/// method blocks `cooled:`, never on a combat level.
#[test]
fn a_cooled_site_is_skipped_by_the_pick() {
    use crate::combat::{pick_site_for, SitePick};
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.combat = 40;
    ctx.position = Some((3222, 3218, 0));
    let method = "gather:woodcut/willow_logs";
    let SitePick::At(first) = pick_site_for(method, &ctx) else { panic!("willows open to CB40") };
    ctx.cooled = vec![super::cool_key(method, Some(first))];
    match pick_site_for(method, &ctx) {
        SitePick::At(other) | SitePick::Tolled(other) => assert_ne!(other, first),
        other => panic!("another willow site serves, got {other:?}"),
    }
    let table = crate::combat::method_site_table(method).expect("willow sites");
    ctx.cooled = table.iter().map(|row| super::cool_key(method, Some(row.site))).collect();
    assert_eq!(pick_site_for(method, &ctx), SitePick::Cooled);
    let mut node = Node::new("n5", NodeKind::ReachLevel { skill: "woodcutting".to_string(), level: 35 }, Vec::new(), 35);
    node.status = NodeStatus::Ready;
    node.method = Some(method.to_string());
    node.alternatives = vec![Alternative { method: method.to_string(), est_ticks: 6000, est_coins: 0 }];
    crate::expand::place_on_site(&mut node, &ctx);
    assert_eq!(node.status, NodeStatus::Blocked { reason: format!("cooled:{method}") });
}

/// Tour 2 P2-3a: a second rock of the SAME site is the same place, not a
/// second site (the method itself stays workable elsewhere).
#[test]
fn a_second_rock_of_one_site_is_not_a_second_site() {
    let cooled = vec!["gather:mining/mithril_ore@3242,3160,0".to_string()];
    let keys = keys_for_abandon(&cooled, "gather:mining/mithril_ore", Some((3243, 3166, 0)));
    assert_eq!(keys, vec!["gather:mining/mithril_ore@3243,3166,0".to_string()], "same site: the method is not cooled");
}

/// Tour 2 P2-3b (D6): a node whose locked method is full everywhere moves
/// to its next method (the alternatives' order), never to a full site.
#[test]
fn a_full_method_hands_over_to_the_next_one() {
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.combat = 99;
    ctx.position = Some((3244, 3165, 0));
    ctx.levels = vec![("mining".to_string(), 99)];
    ctx.tools = vec!["rune_pickaxe".to_string()];
    let full = "gather:mining/mithril_ore";
    let next = "gather:mining/coal";
    let table = crate::combat::method_site_table(full).expect("mithril sites");
    ctx.crowd = table.iter().map(|row| (row.site, 50u32)).collect();
    let mut node = Node::new("n6", NodeKind::ReachLevel { skill: "mining".to_string(), level: 70 }, Vec::new(), 70);
    node.status = NodeStatus::Ready;
    node.method = Some(full.to_string());
    node.alternatives = vec![
        Alternative { method: full.to_string(), est_ticks: 6000, est_coins: 0 },
        Alternative { method: next.to_string(), est_ticks: 6000, est_coins: 0 },
    ];
    crate::expand::place_on_site(&mut node, &ctx);
    assert_eq!(node.method.as_deref(), Some(next), "status {:?}", node.status);
}
