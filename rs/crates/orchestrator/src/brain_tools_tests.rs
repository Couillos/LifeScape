use super::{kept_tools, kept_usable_tools, listing_locks_needed_tool, spare_tool_units};
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
        memory: Default::default(),
    }
}

fn stacks(rows: &[(&str, i64)]) -> Vec<(String, i64)> {
    rows.iter().map(|(name, qty)| (name.to_string(), *qty)).collect()
}

/// User rule 2026-11-13: tools sell, amended 2026-11-14 (permanent
/// kit). A woodcutter keeps its best axe and the kit's hammer; the lower
/// bronze axe and the net it never uses are surplus.
#[test]
fn only_the_best_tool_of_the_work_and_the_kit_stay() {
    let life = life_with(&["gather:woodcut/logs"]);
    let held = stacks(&[("bronze_axe", 1), ("steel_axe", 1), ("hammer", 1), ("net", 1)]);
    assert_eq!(kept_tools(&life, &held, &[]), vec!["hammer".to_string(), "steel_axe".to_string()]);
}

/// User 2026-11-14: a bot keeps its tools when it changes goals. With no
/// work at all it still keeps its kit (the best pickaxe and the one its
/// mining level works with, a hammer, a tinderbox); a tool it does not
/// hold is never "kept", and a foreign tool (net) still sells.
#[test]
fn the_kit_stays_without_work() {
    let idle = life_with(&[]);
    let held = stacks(&[("hammer", 1), ("bronze_pickaxe", 1), ("rune_pickaxe", 1), ("iron_pickaxe", 1), ("tinderbox", 1), ("net", 1)]);
    let kept = kept_tools(&idle, &held, &[("mining", 5)]);
    assert_eq!(kept, vec!["hammer".to_string(), "iron_pickaxe".to_string(), "rune_pickaxe".to_string(), "tinderbox".to_string()]);
    assert!(kept_tools(&idle, &stacks(&[("net", 1)]), &[]).is_empty());
}

/// Spare kit copies sell even without work: one hammer stays.
#[test]
fn spare_kit_copies_sell() {
    let idle = life_with(&[]);
    let held = stacks(&[("hammer", 3), ("tinderbox", 1)]);
    let kept = kept_tools(&idle, &held, &[]);
    assert_eq!(spare_tool_units(&idle, &held, &kept), vec![("hammer".to_string(), 2)]);
}

/// Spare copies sell: one hammer kept for smithing, 3 listed. A milker's
/// buckets are consumed (each becomes a bucket of milk, the way's input
/// from the server data): the whole stack stays.
#[test]
fn spare_copies_of_a_kept_tool_sell() {
    let life = life_with(&["smith:bronze_sword"]);
    let held = stacks(&[("hammer", 4)]);
    let kept = kept_tools(&life, &held, &[]);
    assert_eq!(kept, vec!["hammer".to_string()]);
    assert_eq!(spare_tool_units(&life, &held, &kept), vec![("hammer".to_string(), 3)]);
    let milker = life_with(&["milk:bucket_milk"]);
    let buckets = stacks(&[("bucket_empty", 24)]);
    let kept = kept_tools(&milker, &buckets, &[]);
    assert_eq!(kept, vec!["bucket_empty".to_string()]);
    assert!(spare_tool_units(&milker, &buckets, &kept).is_empty(), "consumed: never sold by the unit rule");
}

/// Bait is consumed: the whole stack of the method's bait stays.
#[test]
fn bait_stacks_stay_whole() {
    let life = life_with(&["gather:fishing/raw_sardine"]);
    let held = stacks(&[("fishing_rod", 1), ("fishing_bait", 300)]);
    let kept = kept_tools(&life, &held, &[]);
    assert!(kept.contains(&"fishing_bait".to_string()) && kept.contains(&"fishing_rod".to_string()), "{kept:?}");
    assert!(spare_tool_units(&life, &held, &kept).is_empty(), "bait never sold by the unit rule");
}

/// Productions read their tools from gamedata (a smith keeps its hammer).
#[test]
fn production_tools_come_from_the_recipe() {
    let life = life_with(&["smith:bronze_dagger"]);
    let kept = kept_tools(&life, &stacks(&[("hammer", 1), ("chisel", 1)]), &[]);
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

/// Plan P5 red test (live 2026-11-17, luke_hill): the steel axe is locked
/// by the bot's own listing, the iron axe is its only usable one: the
/// iron axe is kept, never offered as surplus.
#[test]
fn a_locked_best_tool_keeps_the_best_free_one() {
    let life = life_with(&["gather:woodcut/logs"]);
    let held = stacks(&[("steel_axe", 1), ("iron_axe", 1)]);
    let free = stacks(&[("iron_axe", 1)]);
    let levels = [("woodcutting", 15)];
    assert!(!kept_tools(&life, &held, &levels).contains(&"iron_axe".to_string()), "the raw keep picks the locked steel axe");
    let kept = kept_usable_tools(&life, &held, &free, &levels);
    assert!(kept.contains(&"iron_axe".to_string()), "the usable axe stays: {kept:?}");
    assert!(kept.contains(&"steel_axe".to_string()), "the locked best one too (its listing is withdrawn): {kept:?}");
}

/// Tour 2 P2-7 red test (luke_hill): one iron axe held, listed as
/// surplus: the bronze_axe obtain is not "done" on a locked copy; the
/// listing goes first. A free copy is done; nothing held launches.
#[test]
fn a_listed_copy_is_no_held_copy() {
    use super::{held_verdict, HeldVerdict};
    assert_eq!(held_verdict(1, 1, 0), HeldVerdict::WithdrawListings);
    assert_eq!(held_verdict(1, 1, 1), HeldVerdict::Done);
    assert_eq!(held_verdict(1, 0, 0), HeldVerdict::Launch);
    assert_eq!(held_verdict(28, 30, 20), HeldVerdict::WithdrawListings, "a quota counts free units only");
}
