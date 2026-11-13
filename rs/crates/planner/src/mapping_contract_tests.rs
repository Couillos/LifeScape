//! Args contract (docs/planner.md, "Contrat d'arguments"): the exact
//! order each method sends its script.

use super::map_method;
use crate::model::{Node, NodeKind};

fn obtain(item: &str, count: i64) -> Node {
    Node::new("n9", NodeKind::ObtainItems { item: item.to_string(), count }, Vec::new(), count)
}

fn climb(skill: &str, level: u8) -> Node {
    Node::new("n4", NodeKind::ReachLevel { skill: skill.to_string(), level }, Vec::new(), i64::from(level))
}

/// Every station script works the site the brain picked (`--site`),
/// a guild site names the uniform the door checks (`--wear`).
#[test]
fn stations_carry_the_picked_site() {
    for (method, script) in [
        ("smelt:bronze_bar", "smelt"),
        ("smith:bronze_sword", "smith"),
        ("cook/raw_shrimp", "cook"),
        ("spin:ball_of_wool", "spin"),
        ("pottery:pot_empty", "pottery"),
        ("jewelry:gold_ring", "smelt_jewelry"),
        ("runecraft:airrune", "runecraft"),
        ("fight:melee/cows", "melee"),
    ] {
        let mut node = climb("smithing", 20);
        node.site = Some((3109, 3499, 0));
        let (mapped, args) = map_method(method, &node, None).unwrap_or_else(|| panic!("{method} maps"));
        assert_eq!(mapped, script);
        assert!(args.windows(2).any(|pair| pair == ["--site", "3109,3499,0"]), "{method}: {args:?}");
        assert!(!args.contains(&"--wear".to_string()), "{method}: open site, no uniform");
    }
    let mut guild = climb("crafting", 50);
    guild.site = Some((2938, 3282, 0));
    let (_, args) = map_method("gather:mining/gold_ore", &guild, None).expect("mining maps");
    assert!(args.windows(2).any(|pair| pair == ["--wear", "brown_apron"]), "{args:?}");
}

/// A shop purchase names the planner's shop and keeper site.
#[test]
fn shopbuy_names_the_planners_shop() {
    let mut ctx = crate::expand::ReadyCtx::blind(0.5);
    ctx.combat = 3;
    ctx.position = Some((3222, 3218, 0));
    let (shop, site) = crate::shop_site::pick_shop("shears", &ctx).expect("shears are stocked");
    let mut node = obtain("shears", 1);
    node.site = Some(site);
    let (script, args) = map_method("shop:buy", &node, None).expect("shop maps");
    assert_eq!(script, "market");
    assert_eq!(&args[..3], &["shopbuy".to_string(), "shears".to_string(), "1".to_string()]);
    assert!(args.windows(2).any(|pair| pair[0] == "--shop" && pair[1] == shop), "{args:?}");
    assert!(args.contains(&"--site".to_string()), "{args:?}");
}
