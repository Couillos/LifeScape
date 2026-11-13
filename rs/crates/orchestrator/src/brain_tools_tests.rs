use super::{kept_tools, listing_locks_needed_tool, spare_tool_units};
use lifescape_planner::{Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};

fn life_with(methods: &[&str]) -> Life {
    let nodes = methods
        .iter()
        .enumerate()
        .map(|(index, method)| {
            let mut node = Node::new(&format!("n{}", index + 1), NodeKind::MakeMoney, Vec::new(), 20);
            node.status = NodeStatus::Ready;
            node.method = Some(method.to_string());
            node
        })
        .collect();
    Life {
        schema: 1,
        account: "t_tools".to_string(),
        created_at: 0,
        seed: 1,
        profile: Profile::new(false, 0.5, 0.5, 1, Archetype::Producer),
        goal: GoalKind::MaxSkills { skills: vec!["woodcutting".to_string()], level: 20 },
        nodes,
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
    }
}

fn stacks(rows: &[(&str, i64)]) -> Vec<(String, i64)> {
    rows.iter().map(|(name, qty)| (name.to_string(), *qty)).collect()
}

/// User rule 2026-11-13: tools sell. A woodcutter keeps its best axe,
/// nothing else of its kit-less trades: the bronze axe, the hammer and the
/// net it never uses are surplus.
#[test]
fn only_the_best_tool_of_the_work_stays() {
    let life = life_with(&["gather:woodcut/logs"]);
    let held = stacks(&[("bronze_axe", 1), ("steel_axe", 1), ("hammer", 1), ("net", 1)]);
    assert_eq!(kept_tools(&life, &held), vec!["steel_axe".to_string()]);
}

/// A tool the bot does not hold is never "kept" (nothing to keep), and a
/// bot with no work keeps no tool at all.
#[test]
fn no_work_no_kept_tool() {
    let idle = life_with(&[]);
    assert!(kept_tools(&idle, &stacks(&[("hammer", 1), ("bronze_pickaxe", 1)])).is_empty());
}

/// Spare copies sell: one bucket kept for milking, 23 listed.
#[test]
fn spare_copies_of_a_kept_tool_sell() {
    let life = life_with(&["milk:bucket_milk"]);
    let held = stacks(&[("bucket_empty", 24)]);
    let kept = kept_tools(&life, &held);
    assert_eq!(kept, vec!["bucket_empty".to_string()]);
    assert_eq!(spare_tool_units(&life, &held, &kept), vec![("bucket_empty".to_string(), 23)]);
}

/// Bait is consumed: the whole stack of the method's bait stays.
#[test]
fn bait_stacks_stay_whole() {
    let life = life_with(&["gather:fishing/raw_sardine"]);
    let held = stacks(&[("fishing_rod", 1), ("fishing_bait", 300)]);
    let kept = kept_tools(&life, &held);
    assert!(kept.contains(&"fishing_bait".to_string()) && kept.contains(&"fishing_rod".to_string()), "{kept:?}");
    assert!(spare_tool_units(&life, &held, &kept).is_empty(), "bait never sold by the unit rule");
}

/// Productions read their tools from gamedata (a smith keeps its hammer).
#[test]
fn production_tools_come_from_the_recipe() {
    let life = life_with(&["smith:bronze_dagger"]);
    let kept = kept_tools(&life, &stacks(&[("hammer", 1), ("chisel", 1)]));
    assert_eq!(kept, vec!["hammer".to_string()]);
}

/// b3gybcev (live 2026-11-13): its only hammer was listed. The listing is
/// withdrawn only when the bot needs it and it locks the last free copy.
#[test]
fn only_a_listing_of_the_last_needed_copy_is_withdrawn() {
    let kept = vec!["hammer".to_string()];
    assert!(listing_locks_needed_tool("hammer", 1, &stacks(&[("hammer", 1)]), &kept));
    assert!(!listing_locks_needed_tool("hammer", 1, &stacks(&[("hammer", 2)]), &kept), "a spare copy may sell");
    assert!(!listing_locks_needed_tool("net", 1, &stacks(&[("net", 1)]), &kept), "a tool it does not need sells");
}
