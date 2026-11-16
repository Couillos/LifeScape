//! Tests of `skeleton_deps`: tools, consumption and quests filed at
//! expansion, recipe levels open at each tranche's start.

use super::reach_chain_planned;
use crate::model::{Node, NodeKind};

fn chain(skill: &str, from: u8, to: u8) -> Vec<Node> {
    let mut counter = 0;
    let mut after = String::new();
    reach_chain_planned(skill, from, to, &mut counter, &mut after, crate::expand::NESTED_DEPTH)
}

fn obtains(nodes: &[Node]) -> Vec<(String, i64, Vec<String>)> {
    nodes
        .iter()
        .filter_map(|node| match &node.kind {
            NodeKind::ObtainItems { item, count } => Some((item.clone(), *count, node.deps.clone())),
            _ => None,
        })
        .collect()
}

fn reach_id(nodes: &[Node], target: u8) -> String {
    nodes
        .iter()
        .find(|node| matches!(&node.kind, NodeKind::ReachLevel { level, .. } if *level == target))
        .map(|node| node.id.clone())
        .unwrap_or_else(|| panic!("tranche to {target}"))
}

/// Fishing 1→25: the net first, then the rod and the bait the sardine
/// tranches burn, then the fly rod and the feathers of 20→25, each
/// filed after the tranche before it (just in time).
#[test]
fn fishing_files_its_tackle_and_bait() {
    let nodes = chain("fishing", 1, 25);
    let filed = obtains(&nodes);
    let names: Vec<&str> = filed.iter().map(|(item, _, _)| item.as_str()).collect();
    assert_eq!(names.iter().filter(|name| **name == "net").count(), 1, "{names:?}");
    assert_eq!(names.iter().filter(|name| **name == "fishing_rod").count(), 1, "one rod for both sardine tranches: {names:?}");
    assert!(names.contains(&"fly_fishing_rod") && names.contains(&"feather"), "{names:?}");
    let bait: i64 = filed.iter().filter(|(item, _, _)| item == "fishing_bait").map(|(_, count, _)| *count).sum();
    let expected = crate::sizing::tranche_need("gather:fishing/raw_sardine", 5, 10, "fishing_bait").unwrap_or(0)
        + crate::sizing::tranche_need("gather:fishing/raw_sardine", 10, 20, "fishing_bait").unwrap_or(0);
    assert_eq!(bait, expected, "bait sized by the XP of both sardine tranches");
    let feathers = filed.iter().find(|(item, _, _)| item == "feather").expect("feathers");
    assert_eq!(feathers.1, crate::sizing::tranche_need("gather:fishing/raw_trout", 20, 25, "feather").unwrap_or(0));
    assert_eq!(feathers.2, vec![reach_id(&nodes, 20)], "feathers wait for the 10→20 tranche");
    let trout = nodes.iter().find(|node| matches!(&node.kind, NodeKind::ReachLevel { level: 25, .. })).expect("20→25");
    assert!(trout.deps.contains(&reach_id(&nodes, 20)), "the chain order holds");
    assert_eq!(trout.deps.len(), 3, "previous tranche, fly rod, feathers: {:?}", trout.deps);
}

/// Smithing 1→5 hammers bronze daggers (2026-11-14: smelt then smith):
/// the hammer once and the bars sized by the anvil XP (12.5 a bar); the
/// bars' own obtain smelts or buys them. Runecraft files the talisman and
/// the essence.
#[test]
fn producers_file_their_inputs_and_tools() {
    let smith = obtains(&chain("smithing", 1, 5));
    let bars = crate::sizing::tranche_actions("smith:bronze_dagger", 1, 5).expect("bronze xp");
    assert_eq!(bars, (388.0_f64 / 12.5).ceil() as i64, "XP 1→5 at 12.5 a bar");
    assert!(smith.iter().any(|(item, count, _)| item == "bronze_bar" && *count == bars), "{smith:?}");
    assert!(smith.iter().any(|(item, count, _)| item == "hammer" && *count == 1), "{smith:?}");
    // A magic climb from confuse (3, D18) is priced at resolve: no runes
    // filed ahead (combat spell, curse, teleport, bananas or alchemy).
    assert!(obtains(&chain("magic", 25, 30)).is_empty(), "decided with the market");
    assert!(obtains(&chain("magic", 10, 20)).is_empty(), "curses compete from 3");
    assert!(!obtains(&chain("magic", 1, 3)).is_empty(), "wind strike alone below confuse keeps its runes ahead");
    let runes = obtains(&chain("runecraft", 1, 2));
    assert!(runes.iter().any(|(item, count, _)| item == "air_talisman" && *count == 1), "{runes:?}");
    assert!(runes.iter().any(|(item, count, _)| item == "blankrune" && *count == 17), "{runes:?}");
    // Melee burns nothing and carries nothing: no dependency at all.
    assert!(obtains(&chain("attack", 1, 20)).is_empty());
}

/// Audit 2026-11-13 (A10): every tranche's recipe opens at its START
/// level (a tranche never works a tier it unlocks only at its target).
#[test]
fn every_tranche_recipe_opens_at_its_start() {
    for skill in ["fishing", "woodcutting", "mining", "cooking", "firemaking", "smithing", "crafting", "runecraft", "prayer"] {
        for start in 1..=98u8 {
            let method = crate::tranches::reach_method(skill, start);
            let Some((_, way)) = crate::method_way::method_way(&method) else {
                panic!("{skill} {start}: {method} has no way");
            };
            let (gate_skill, gate) = way.skill.clone().unwrap_or_default();
            assert_eq!(gate_skill, skill, "{method} trains {skill}");
            assert!(gate <= i64::from(start), "{skill} at {start}: {method} needs {gate}");
            assert!(crate::sizing::xp_per_action(&method, start).is_some(), "{method} has XP data");
        }
    }
}
