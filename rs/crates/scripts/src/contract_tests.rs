//! The brain's args contract (planner `mapping.rs`), end to end: every
//! method the planner maps, as a quantity order (`--want`, `--site`,
//! `toll`), as a level climb (`--level`) and on a guild site (`--wear`),
//! names a registered script whose own parser accepts the args. Main was
//! undeployable twice: gatherers refused `fishing raw_trout --want ...`,
//! then magic refused `--spell` and stations `--wear`.

use super::check_order;
use lifescape_planner::mapping::map_method;
use lifescape_planner::model::{Node, NodeKind};

/// Quest keys the planner maps (`mapping::quest_script`, every F2P quest).
const QUESTS: [&str; 18] = [
    "cook", "sheep", "hetty", "haunted", "romeojuliet", "doric", "gobdip", "imp", "runemysteries", "blackarmgang", "blackknight", "demon",
    "dragon", "squire", "hunt", "priest", "vampire", "prince",
];

/// Safe areas `access:<area>` walks to.
const AREAS: [&str; 13] = [
    "lumbridge", "cooks_guild", "crafting_guild", "mining_guild", "champions_guild", "karamja", "al_kharid", "varrock", "falador", "draynor",
    "port_sarim", "barbarian_village", "edgeville",
];

/// Every method family `map_method` emits (one or more members each).
fn methods() -> Vec<String> {
    let mut methods: Vec<String> = [
        "gather:fishing/raw_trout", "gather:mining/gold_ore", "gather:mining/clay", "gather:woodcut/willow_logs", "gather:sheep/wool",
        "gather:wheat/grain", "farm:raw_trout", "farm:gold_ore", "farm:coal", "farm:clay", "farm:willow_logs", "farm:feather", "farm:bones",
        "farm:raw_beef", "farm:cow_hide", "farm:blankrune", "cook:trout", "cook/raw_shrimp", "bake:bread", "bake:redberry_pie", "bake:meat_pie",
        "bake:stew", "bake:apple_pie", "bake:plain_pizza", "bake:cake", "smelt:iron_bar", "smelt:bronze_bar", "smith:steel_platebody",
        "tan:cow_hide", "craft:leather_gloves", "fire:willow_logs", "spin:ball_of_wool", "pottery:pot_empty", "pottery:piedish",
        "pottery:bowl_empty", "jewelry:gold_ring", "jewelry:unstrung_gold_amulet", "runecraft:airrune", "pick:grain", "mill:pot_flour",
        "milk:bucket_milk", "fill:jug_water", "essence:blankrune", "string:unstrung_gold_amulet", "alch:gold_ring", "superheat:iron_bar",
        "superheat:steel_bar", "enchant:strung_sapphire_amulet", "fight:melee/cows", "fight:melee/ladder", "fight:ranged/cows",
        "fight:magic/cows", "fight:magic/fire_strike/cows", "fight:magic/earth_bolt/goblins", "cut:sapphire", "cut:ruby", "dye:yellowdye",
        "dye:reddye", "dye:bluedye", "ground:hammer", "ground:chisel", "telegrab:wine_of_zamorak", "bury:bones", "bury:big_bones",
        "market:buy", "market:sell", "shop:buy", "dialogue:buy", "market:flip", "space:cleanup", "equip:melee", "equip:ranged", "equip:magic",
    ]
    .iter()
    .map(|method| method.to_string())
    .collect();
    methods.extend(QUESTS.iter().map(|key| format!("quest:{key}")));
    methods.extend(AREAS.iter().map(|area| format!("access:{area}")));
    methods
}

/// The three node shapes a launch carries: a quantity order across the
/// toll, a level climb, a quantity order inside the Cooks' Guild.
fn nodes() -> Vec<Node> {
    let mut order = Node::new("n9", NodeKind::ObtainItems { item: "feather".to_string(), count: 2000 }, Vec::new(), 2000);
    order.site = Some((3294, 3287, 0));
    order.toll = true;
    let climb = Node::new("n4", NodeKind::ReachLevel { skill: "fishing".to_string(), level: 20 }, Vec::new(), 20);
    let mut guild = Node::new("n7", NodeKind::ObtainItems { item: "bread".to_string(), count: 50 }, Vec::new(), 50);
    guild.site = Some((3144, 3450, 0));
    vec![order, climb, guild]
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| arg.to_string()).collect()
}

#[test]
fn every_mapped_order_parses_in_its_script() {
    for method in methods() {
        for node in nodes() {
            let Some((script, args)) = map_method(&method, &node, Some((6000, 9000))) else {
                // Purchases move an obtain's missing units: no leg on a climb.
                let purchase = matches!(method.as_str(), "market:buy" | "market:sell" | "shop:buy" | "dialogue:buy");
                assert!(purchase && !matches!(node.kind, NodeKind::ObtainItems { .. }), "{method} maps on {:?}", node.kind);
                continue;
            };
            assert!(crate::names().contains(&script.as_str()), "{method} names unregistered {script}");
            if let Err(error) = check_order(&script, &args) {
                panic!("{script} refuses {args:?} for {method}: {error}");
            }
        }
    }
    let gather = map_method("farm:raw_trout", &nodes()[0], None).expect("farm maps").1;
    let spec = crate::kit::cycle::parse_order("fishing", &gather).expect("fishing parses");
    assert_eq!(spec.resource, strings(&["raw_trout"]), "the fish rides first: {gather:?}");
    assert_eq!((spec.targets.want_item.as_deref(), spec.allow_paid, spec.site), (Some("feather"), true, Some((3294, 3287, 0))));
}

/// The new flags land in the spec the scripts honor: the spell magic
/// casts, the uniform worn before a guild door, the level a station
/// climb stops at.
#[test]
fn contract_flags_fill_the_spec() {
    let climb = Node::new("n4", NodeKind::ReachLevel { skill: "magic".to_string(), level: 30 }, Vec::new(), 30);
    let (_, args) = map_method("fight:magic/fire_strike/cows", &climb, None).expect("magic maps");
    let spec = crate::kit::cycle::parse_order("magic", &args).expect("magic parses");
    assert_eq!((spec.resource.clone(), spec.spell.as_deref()), (strings(&["cows"]), Some("fire_strike")));
    assert_eq!((spec.targets.level_skill.as_deref(), spec.targets.level), (Some("magic"), Some(30)));
    let guild = &nodes()[2];
    for (method, script, uniform) in [("bake:bread", "bake_bread", "chefs_hat"), ("cook/raw_shrimp", "cook", "chefs_hat"), ("mill:pot_flour", "mill_grain", "chefs_hat")] {
        let (mapped, args) = map_method(method, guild, None).expect("guild maps");
        assert_eq!(mapped, script);
        let spec = crate::kit::cycle::parse_order(crate::names().into_iter().find(|name| *name == script).expect("registered"), &args).expect("parses");
        assert_eq!(spec.wear, strings(&[uniform]), "{method}: {args:?}");
    }
    let mut crafting = Node::new("n8", NodeKind::ReachLevel { skill: "crafting".to_string(), level: 45 }, Vec::new(), 45);
    crafting.site = Some((2938, 3282, 0));
    let (_, args) = map_method("pottery:piedish", &crafting, None).expect("pottery maps");
    let spec = crate::kit::cycle::parse_order("pottery", &args).expect("pottery parses");
    assert_eq!((spec.resource.clone(), spec.wear.clone()), (strings(&["piedish"]), strings(&["brown_apron"])));
    assert!(crate::kit::cycle::parse_cli("magic", &strings(&["--spell"])).is_err(), "a dangling spell fails loud");
    assert!(crate::kit::cycle::parse_cli("cook", &strings(&["--wear", "--node"])).is_err(), "a flag is no uniform");
}

/// `market shopbuy` buys at the planner's shop (`--shop <key>`) and its
/// keeper's site; buy/sell/flip/cleanup carry no site (the market square
/// is no planner site).
#[test]
fn shopbuy_carries_the_planners_shop() {
    let order = crate::activities::market_shop::parse_shopbuy(&strings(&["shopbuy", "shears", "2", "--shop", "generalshop1", "--site", "3212,3247,0", "toll", "--node", "n5"]))
        .expect("shopbuy parses");
    assert_eq!((order.item.as_str(), order.qty, order.shop.as_deref(), order.site, order.toll), ("shears", 2, Some("generalshop1"), Some((3212, 3247, 0)), true));
    assert!(order.wear.is_empty());
    assert!(crate::activities::market_shop::parse_shopbuy(&strings(&["shopbuy", "shears", "2", "--shop"])).is_err(), "a dangling shop fails loud");
    let order = &nodes()[0];
    for method in ["market:buy", "market:sell", "market:flip", "space:cleanup"] {
        let (_, args) = map_method(method, order, None).expect("market maps");
        assert!(!args.iter().any(|arg| arg == "--site" || arg == "toll"), "{method}: {args:?}");
    }
}

/// Every positional the planner sends resolves in the script's own
/// table: a valid order never fails `Usage` on its resource.
#[test]
fn positionals_resolve_in_their_scripts() {
    use crate::activities::{bury_loot, cook_meals, cut_gems, dye, magic_style, pottery, superheat};
    let quota = &nodes()[0];
    let first = |method: &str| map_method(method, quota, None).and_then(|(_, args)| args.first().cloned()).unwrap_or_else(|| panic!("{method} maps"));
    for meal in ["redberry_pie", "meat_pie", "stew", "apple_pie", "plain_pizza", "cake"] {
        assert!(cook_meals::meal_for(&first(&format!("bake:{meal}"))).is_some(), "{meal}");
    }
    for product in ["pot_empty", "piedish", "bowl_empty"] {
        assert!(pottery::shape_row(&first(&format!("pottery:{product}"))).is_some(), "{product}");
    }
    for bones in ["bones", "big_bones"] {
        assert!(bury_loot::is_bones(&first(&format!("bury:{bones}"))), "{bones}");
    }
    for colour in ["yellowdye", "reddye", "bluedye"] {
        assert!(dye::dye_row(&first(&format!("dye:{colour}"))).is_some(), "{colour}");
    }
    for gem in ["sapphire", "emerald", "ruby", "diamond"] {
        assert!(cut_gems::gem_recipe(&first(&format!("cut:{gem}"))).is_some(), "{gem}");
    }
    for bar in ["iron_bar", "steel_bar", "gold_bar", "silver_bar", "bronze_bar", "mithril_bar"] {
        assert!(superheat::ore_for(&first(&format!("superheat:{bar}"))).is_some(), "{bar}");
    }
    for item in ["hammer", "chisel", "wine_of_zamorak"] {
        assert!(lifescape_gamedata::objs_by_name(&first(&format!("ground:{item}"))).first().is_some(), "{item}");
    }
    for level in 1..=99u8 {
        let spell = lifescape_planner::method_way::best_combat_spell(level);
        assert!(magic_style::damage_spell(&spell).is_some(), "the planner's {spell} is a damage spell magic casts");
    }
    for raw in ["raw_shrimp", "raw_trout"] {
        assert!(crate::kit::produce::cook_recipe(raw).is_some(), "{raw}");
    }
    assert_eq!(first("cook:trout"), "raw_trout");
}
