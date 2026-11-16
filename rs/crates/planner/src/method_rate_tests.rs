use super::{method_prior, units_ceiling_per_tick, PriorError, PriorQuery, RATE_WINDOW_TICKS};

fn query(method: &str, level: u8) -> PriorQuery {
    PriorQuery { method: method.to_string(), level, tool: String::new(), site: None }
}

fn flow(prior: &super::RatePrior, item: &str) -> f64 {
    prior.flows.iter().filter(|(name, _, _)| name == item).map(|(_, _, qty)| qty).sum()
}

/// Every family of the planner gets a prior from the game, or a typed
/// error naming why (never a default figure).
#[test]
fn every_planner_family_has_a_prior_or_a_typed_error() {
    let computed = [
        ("gather:fishing/raw_shrimp", 1),
        ("gather:fishing/raw_trout", 20),
        ("gather:fishing/raw_lobster", 40),
        ("gather:mining/copper_ore", 1),
        ("gather:woodcut/logs", 1),
        ("farm:copper_ore", 1),
        ("farm:feather", 5),
        ("cook/raw_shrimp", 1),
        ("bake:bread", 5),
        ("smelt:bronze_bar", 1),
        ("smelt:iron_bar", 15),
        ("smith:bronze_dagger", 1),
        ("jewelry:gold_ring", 5),
        ("spin:ball_of_wool", 1),
        ("pottery:pot_empty", 1),
        ("craft:leather_gloves", 1),
        ("cut:sapphire", 20),
        ("string:unstrung_gold_amulet", 1),
        ("fire:logs", 1),
        ("bury:bones", 1),
        ("runecraft:airrune", 1),
        ("essence:blankrune", 1),
        ("alch:iron_dagger", 21),
        ("superheat:iron_bar", 43),
        ("enchant:strung_sapphire_amulet", 7),
        ("telegrab:wine_of_zamorak", 33),
        ("teleport:varrock_teleport", 25),
        ("curse:confuse/chickens", 3),
        ("bananas:bones", 15),
        ("fight:melee/cows", 10),
        ("fight:ranged/chickens", 10),
        ("fight:magic/wind_strike/chickens", 1),
        ("gather:sheep/wool", 1),
        ("gather:wheat/grain", 1),
        ("pick:wheat", 1),
        ("pick:potato", 1),
        ("fill:jug_empty", 1),
        ("milk:bucket_milk", 1),
        ("dye:yellowdye", 1),
        ("tan:cow_hide", 1),
    ];
    for (method, level) in computed {
        let prior = method_prior(&query(method, level)).unwrap_or_else(|error| panic!("{method}: {error}"));
        assert!(prior.actions_per_6000.is_finite() && prior.actions_per_6000 > 0.0, "{method}: {prior:?}");
        assert!(prior.actions_per_6000 <= RATE_WINDOW_TICKS, "{method}: at most one op a tick");
        assert!(prior.flows.iter().any(|(_, _, qty)| *qty > 0.0) || !prior.units.is_empty(), "{method} yields something");
    }
    assert!(matches!(method_prior(&query("mill:pot_flour", 1)), Err(PriorError::NoTiming(_))), "mill: no timing modelled");
    assert!(matches!(method_prior(&query("market:buy", 1)), Err(PriorError::NoWay(_))));
    assert!(matches!(method_prior(&query("gather:mining/copper_ore", 1)).map(|_| ()), Ok(())));
    let bad_tool = PriorQuery { tool: "golden_spoon".to_string(), ..query("gather:mining/copper_ore", 1) };
    assert!(matches!(method_prior(&bad_tool), Err(PriorError::NoTool(_))));
}

/// Mining copper at 1 with a bronze pickaxe: a roll every 8 ticks at
/// 129/256 (gem roll aside), banked 27 ores a load.
#[test]
fn mining_reads_the_rock_and_the_pickaxe() {
    let at_bank = PriorQuery { site: None, ..query("gather:mining/copper_ore", 1) };
    let prior = method_prior(&at_bank).expect("copper");
    assert_eq!(prior.ticks_per_action, 8.0);
    assert!((prior.success - 129.0 / 256.0 * 255.0 / 256.0).abs() < 1e-9);
    let rune = method_prior(&PriorQuery { tool: "rune_pickaxe".to_string(), ..at_bank.clone() }).expect("rune");
    assert_eq!(rune.ticks_per_action, 3.0, "rune pickaxe: rate 2 + 1");
    assert!(flow(&rune, "copper_ore") > flow(&prior, "copper_ore"), "a faster pickaxe mines more");
    let higher = method_prior(&query("gather:mining/copper_ore", 30)).expect("copper 30");
    assert!(higher.success > prior.success, "odds grow with the level");
}

/// A far bank slows the yield; the bank walk is shared by a load.
#[test]
fn bank_walks_are_shared_by_the_load() {
    let near = method_prior(&PriorQuery { site: Some((3094, 3492, 0)), ..query("gather:woodcut/logs", 10) }).expect("near");
    let far = method_prior(&PriorQuery { site: Some((3094 + 200, 3492, 0)), ..query("gather:woodcut/logs", 10) }).expect("far");
    assert!(far.bank_ticks_per_action > near.bank_ticks_per_action);
    assert!(flow(&far, "logs") < flow(&near, "logs"));
}

/// Fishing rolls the second fish first once its level is met; bait
/// burns per catch; cooking burns at low level.
#[test]
fn fish_and_cooking_follow_their_rolls() {
    let low = method_prior(&query("gather:fishing/raw_shrimp", 1)).expect("shrimp");
    assert_eq!(flow(&low, "raw_anchovies"), 0.0, "anchovies from level 15");
    let high = method_prior(&query("gather:fishing/raw_shrimp", 20)).expect("shrimp 20");
    assert!(flow(&high, "raw_anchovies") > 0.0);
    let trout = method_prior(&query("gather:fishing/raw_trout", 20)).expect("trout");
    assert!((flow(&trout, "feather") + flow(&trout, "raw_trout") + flow(&trout, "raw_salmon")).abs() < 1e-9, "a feather per catch");
    let cook = method_prior(&query("cook/raw_shrimp", 1)).expect("cook");
    assert!(flow(&cook, "raw_shrimp") < 0.0 && flow(&cook, "shrimp") > 0.0);
    assert!(cook.flows.iter().any(|(_, cause, qty)| *cause == "byproduct" && *qty > 0.0), "burnt at level 1");
    let iron = method_prior(&query("smelt:iron_bar", 15)).expect("iron");
    assert!((flow(&iron, "iron_bar") + flow(&iron, "iron_ore") * 0.5).abs() < 1e-9, "one bar per two ores");
}

/// The ceiling: one shrimp per roll at most, never more.
#[test]
fn ceiling_is_one_unit_per_server_action() {
    let ceiling = units_ceiling_per_tick(&query("gather:fishing/raw_shrimp", 1)).expect("ceiling");
    let total: f64 = ceiling.iter().filter(|(kind, _, _)| *kind == "product").map(|(_, _, per)| per).sum();
    assert!((total - 1.0 / 6.0).abs() < 1e-9, "{ceiling:?}");
}

