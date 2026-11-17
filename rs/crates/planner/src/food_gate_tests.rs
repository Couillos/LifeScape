//! Tests of `food_gate.rs` (live 2026-11-17, jim_ellis: 10 HP, fishing
//! and cooking 1, 29 coins, no food, herring at 12 a unit).

use super::{food_short, obtainable_food, production_steps, ready_now, Larder, PRODUCE_DEPTH};
use crate::expand::ReadyCtx;
use crate::model::{Node, NodeKind};
use crate::resolve_obtain::resolve_money;

fn jim_ctx() -> ReadyCtx {
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.bank_known = true;
    ctx.combat = 3;
    ctx.coins = 29;
    ctx.levels = vec![("hitpoints".to_string(), 10), ("attack".to_string(), 2), ("fishing".to_string(), 1), ("cooking".to_string(), 1), ("woodcutting".to_string(), 87)];
    ctx.tools = ["iron_axe", "bronze_pickaxe", "hammer", "tinderbox", "yew_logs"].iter().map(|name| name.to_string()).collect();
    ctx
}

fn jim_level(skill: &str) -> u8 {
    match skill {
        "hitpoints" => 10,
        "attack" => 2,
        "woodcutting" => 87,
        "firemaking" => 75,
        _ => 1,
    }
}

/// Men (max hit 1) bite a 10-HP bot: no food in hand, the fight is not
/// workable now; cows and chickens are; a fed bot or a 30-HP one hunts
/// men at once.
#[test]
fn a_biting_spot_needs_food_in_hand_before_it_is_workable() {
    let mut ctx = jim_ctx();
    assert_eq!(food_short("fight:melee/men", &ctx).as_deref(), Some("herring"));
    assert!(!ready_now("fight:melee/men", &ctx));
    assert!(ready_now("fight:melee/chickens", &ctx) && ready_now("fight:melee/cows", &ctx));
    assert!(ready_now("gather:woodcut/yew_logs", &ctx), "axe held: workable now");
    ctx.tools.push("shrimp".to_string());
    assert!(ready_now("fight:melee/men", &ctx), "fed");
    let mut sturdy = jim_ctx();
    sturdy.levels[0].1 = 30;
    assert!(food_short("fight:melee/men", &sturdy).is_none(), "max hit 1 under 5 % of 30 HP");
    let mut blind = jim_ctx();
    blind.levels.clear();
    assert!(food_short("fight:melee/men", &blind).is_none(), "levels unknown: no judgment");
}

/// A broke bot makes its food: herring is neither affordable (240 for a
/// load) nor makeable at fishing/cooking 1; a heal-3 food it fishes or
/// hunts foodless and cooks is (a 5-coin net from the shop at most).
#[test]
fn a_broke_bot_makes_the_food_it_cannot_buy() {
    let held = |name: &str| i64::from(["iron_axe", "bronze_pickaxe", "hammer", "tinderbox"].contains(&name));
    let cost_of = |item: &str, units: i64| match item {
        // The small fishing net (server debugname `net`), 5 at the shops.
        "net" => Some(5 * units),
        _ => None,
    };
    let larder = Larder { level_of: &jim_level, hitpoints: 10, held: &held, cost_of: &cost_of, coins: 29 };
    let prices = |food: &str| match food {
        "herring" => Some(12),
        "shrimp" => Some(6),
        _ => None,
    };
    assert!(production_steps("herring", 20, &larder, PRODUCE_DEPTH).is_none(), "fishing/cooking 1");
    assert_eq!(production_steps("shrimp", 20, &larder, PRODUCE_DEPTH), Some(2), "net bought, catch, cook");
    let chosen = obtainable_food("herring", 20, &prices, &larder);
    assert_eq!(lifescape_gamedata::food::heal(&chosen), 3, "{chosen}");
    assert!(production_steps(&chosen, 20, &larder, PRODUCE_DEPTH).is_some(), "{chosen} is made, not bought");
    // A wallet that covers a load still buys the best fitted fish.
    let rich = Larder { coins: 300, ..larder };
    assert_eq!(obtainable_food("herring", 20, &prices, &rich), "herring");
    // Nothing affordable, nothing makeable: the asked food stays.
    let nothing = |_: &str, _: i64| None;
    let bare = Larder { level_of: &|_| 1, hitpoints: 10, held: &|_| 0, cost_of: &nothing, coins: 0 };
    let unpriced = |_: &str| None;
    let kept = obtainable_food("herring", 20, &unpriced, &bare);
    assert!(kept == "herring" || production_steps(&kept, 20, &bare, PRODUCE_DEPTH).is_some(), "{kept}");
}

/// A hunt whose spot bites the bot never feeds it (that hunt needs food
/// itself); a foodless spot does (chickens: raw chicken, cooked).
#[test]
fn a_biting_hunt_is_no_way_to_food() {
    let cost_of = |_: &str, _: i64| None;
    let weak = Larder { level_of: &|_| 1, hitpoints: 10, held: &|_| 0, cost_of: &cost_of, coins: 0 };
    assert_eq!(production_steps("cooked_chicken", 20, &weak, PRODUCE_DEPTH), Some(2), "chickens are fought foodless");
}


/// jim_ellis's coin step (10 HP, no food, 29 coins): the hunt that needs
/// food he cannot get never leads; the best published profit among what
/// he works now wins (net and shears not held: shrimp and wool wait).
/// Fed, men rank by their profit like any method; a method a cycle
/// excluded never comes back.
#[test]
fn a_foodless_coin_step_works_what_it_can_now_by_profit() {
    let mut ctx = jim_ctx();
    ctx.method_profit = [
        ("fight:melee/men", 598.0),
        ("fight:melee/chickens", 900.0),
        ("gather:fishing/raw_shrimp", 1660.0),
        ("gather:sheep/wool", 1500.0),
        ("gather:woodcut/yew_logs", 667.0),
    ]
    .iter()
    .map(|(method, profit)| (method.to_string(), *profit))
    .collect();
    let mut coins = Node::new("n476", NodeKind::HaveCoins { amount: 339 }, Vec::new(), 339);
    resolve_money(&mut coins, &ctx);
    assert_eq!(coins.method.as_deref(), Some("fight:melee/chickens"), "{:?}", coins.alternatives);
    let order: Vec<&str> = coins.alternatives.iter().map(|alt| alt.method.as_str()).collect();
    let men = order.iter().position(|method| *method == "fight:melee/men").expect("men listed");
    let yews = order.iter().position(|method| *method == "gather:woodcut/yew_logs").expect("yews listed");
    assert!(yews < men, "workable now before a hunt that needs food first: {order:?}");
    // Fed: men workable now, ranked by profit.
    let mut fed = ctx.clone();
    fed.tools.push("herring".to_string());
    for row in &mut fed.method_profit {
        if row.0 == "fight:melee/men" {
            row.1 = 2_000.0;
        }
    }
    let mut fed_coins = Node::new("n500", NodeKind::HaveCoins { amount: 339 }, Vec::new(), 339);
    resolve_money(&mut fed_coins, &fed);
    assert_eq!(fed_coins.method.as_deref(), Some("fight:melee/men"));
    // Excluded by a broken cycle: the next workable method.
    let mut cut = Node::new("n476", NodeKind::HaveCoins { amount: 339 }, Vec::new(), 339);
    cut.excluded = vec!["fight:melee/chickens".to_string()];
    resolve_money(&mut cut, &ctx);
    assert_eq!(cut.method.as_deref(), Some("gather:woodcut/yew_logs"), "{:?}", cut.alternatives);
}
