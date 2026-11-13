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
        assert!(args.windows(3).any(|window| window == ["--level", "smithing", "20"]), "{method}: the tranche stops at its level: {args:?}");
    }
    let mut guild = climb("crafting", 50);
    guild.site = Some((2938, 3282, 0));
    let (_, args) = map_method("gather:mining/gold_ore", &guild, None).expect("mining maps");
    assert!(args.windows(2).any(|pair| pair == ["--wear", "brown_apron"]), "{args:?}");
}

/// New families (audit 2026-11-13 A12/B8/B9): each names its script and
/// its resource first.
#[test]
fn new_families_name_their_scripts() {
    let quota = obtain("sapphire", 10);
    for (method, script, first) in [
        ("cut:sapphire", "cut_gems", Some("uncut_sapphire")),
        ("dye:yellowdye", "dye", Some("yellowdye")),
        ("ground:hammer", "ground_pickup", Some("hammer")),
        ("telegrab:wine_of_zamorak", "telegrab_wine", None),
        ("bury:bones", "bury_loot", Some("bones")),
        ("bake:bread", "bake_bread", None),
        ("bake:redberry_pie", "cook_meals", Some("redberry_pie")),
        ("superheat:iron_bar", "superheat", Some("iron_bar")),
    ] {
        let (mapped, args) = map_method(method, &quota, None).unwrap_or_else(|| panic!("{method} maps"));
        assert_eq!(mapped, script, "{method}");
        match first {
            Some(resource) => assert_eq!(args.first().map(String::as_str), Some(resource), "{method}: {args:?}"),
            None => assert_eq!(args.first().map(String::as_str), Some("--want"), "{method}: {args:?}"),
        }
    }
    // Magic carries the spell the planner sized the runes for.
    let climb = climb("magic", 30);
    let (script, args) = map_method("fight:magic/fire_strike/cows", &climb, None).expect("magic maps");
    assert_eq!(script, "magic");
    assert_eq!(&args[..3], &["cows".to_string(), "--spell".to_string(), "fire_strike".to_string()]);
    assert!(args.windows(3).any(|window| window == ["--level", "magic", "30"]), "{args:?}");
    let (_, legacy) = map_method("fight:magic/cows", &climb, None).expect("legacy magic maps");
    assert_eq!(&legacy[..3], &["cows".to_string(), "--spell".to_string(), "wind_strike".to_string()]);
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

/// A shop the planner picks across the Al Kharid fence (fare covered)
/// launches with `toll`, or `shopbuy` would refuse the keeper it was sent
/// to; on the same side, never.
#[test]
fn shop_across_the_fence_carries_toll() {
    let mut ctx = crate::expand::ReadyCtx::blind(0.5);
    ctx.combat = 30;
    ctx.coins = 1000;
    ctx.position = Some((3222, 3218, 0));
    let items: Vec<String> = lifescape_gamedata::shops::all_shops().into_iter().flat_map(|shop| shop.stocks.into_iter().map(|stock| stock.obj)).collect();
    let mut seen = (false, false);
    for item in items {
        let Some((_, site)) = crate::shop_site::pick_shop(&item, &ctx) else { continue };
        let mut node = obtain(&item, 1);
        node.method = Some("shop:buy".to_string());
        crate::expand::place_on_site(&mut node, &ctx);
        let across = lifescape_gamedata::toll::crossing((3222, 3218, 0), site);
        assert_eq!((node.site, node.toll), (Some(site), across), "{item}");
        let (_, args) = map_method("shop:buy", &node, None).expect("shop maps");
        assert_eq!(args.contains(&"toll".to_string()), across, "{item}: {args:?}");
        if across {
            seen.0 = true;
        } else {
            seen.1 = true;
        }
    }
    assert_eq!(seen, (true, true), "both sides of the fence exercised");
}
