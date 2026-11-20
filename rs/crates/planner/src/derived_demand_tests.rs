//! Derived demand: inputs priced from the product a producer makes.

use super::{derived_bounds, input_reserve, obtain_ceiling, product_lot, product_value, sale_reserve, DerivedPrices};
use crate::market_fit::BookView;
use crate::model::{GoalKind, Node, NodeKind};

/// A helm quota whose smith blocked on bars, whose smelting blocked on
/// ores (`record_block_from` puts the sub-task in the consumer's deps).
fn helm_graph() -> Vec<Node> {
    let mut helm = Node::new("n1", NodeKind::ObtainItems { item: "iron_full_helm".to_string(), count: 3 }, vec!["n2".to_string()], 3);
    helm.method = Some("smith:iron_full_helm".to_string());
    let mut bars = Node::new("n2", NodeKind::ObtainItems { item: "iron_bar".to_string(), count: 6 }, vec!["n3".to_string()], 6);
    bars.method = Some("smelt:iron_bar".to_string());
    let ores = Node::new("n3", NodeKind::ObtainItems { item: "iron_ore".to_string(), count: 12 }, Vec::new(), 12);
    vec![helm, bars, ores]
}

fn rates(_: &str, _: &str) -> f64 {
    600.0
}

fn reserve_at_helm_price(price: f64, index: usize) -> Option<f64> {
    let value = move |item: &str| (item == "iron_full_helm").then_some(price);
    let input = |_: &str| 0.0;
    let prices = DerivedPrices { value_of: &value, input_price: &input, units_per_6000: &rates, coin_rate: 0.5 };
    input_reserve(&helm_graph(), index, &prices)
}

/// The bar's reservation price is the helm's value less the smith's
/// time, over the bars a helm takes; it rises with the helm.
#[test]
fn bar_bids_rise_with_the_product_price() {
    let bars_per_helm = crate::bid_response::per_unit_inputs("smith:iron_full_helm", &crate::obtain_legs::production_legs("iron_full_helm")[0].inputs)[0].1;
    let cheap = reserve_at_helm_price(123.0, 1).expect("derived");
    let expected = (123.0 - 10.0 * 0.5) / bars_per_helm;
    assert!((cheap - expected).abs() < 1e-6, "{cheap} vs {expected}");
    let dear = reserve_at_helm_price(1230.0, 1).expect("derived");
    assert!(dear > 9.0 * cheap, "{cheap} → {dear}");
    // Ores follow the bars (a miner answers the smelter's bid).
    let ore_cheap = reserve_at_helm_price(123.0, 2).expect("derived");
    let ore_dear = reserve_at_helm_price(1230.0, 2).expect("derived");
    assert!(ore_cheap < cheap && ore_dear > ore_cheap * 5.0, "{ore_cheap} → {ore_dear}");
    // The product itself feeds nothing: no derived price.
    assert_eq!(reserve_at_helm_price(123.0, 0), None);
}

/// The standing bid opens at half the reservation, capped by the wallet
/// and the reservation; a product worth less than its inputs never buys
/// them high.
#[test]
fn bounds_cap_by_wallet_and_reservation() {
    assert_eq!(derived_bounds(46.7, 500), (24, 46));
    assert_eq!(derived_bounds(46.7, 20), (20, 20), "the wallet caps");
    assert_eq!(derived_bounds(-12.0, 500), (1, 1), "a loss never bids up");
}

/// A bought input never bids over what its product pays (the
/// reservation), whatever the wallet; a purchase for itself keeps the
/// wallet as its only limit.
#[test]
fn a_bought_input_bids_under_its_reservation() {
    assert_eq!(obtain_ceiling(1092, Some(150.4)), 150);
    assert_eq!(obtain_ceiling(80, Some(150.0)), 80, "the wallet still caps");
    assert_eq!(obtain_ceiling(1092, Some(-3.0)), 1, "a product worth less than its inputs");
    assert_eq!(obtain_ceiling(1092, None), 1092);
}

/// The helm quota of `helm_graph` sized to a lot of `helms`.
fn helm_lot_graph(helms: i64) -> Vec<Node> {
    let mut nodes = helm_graph();
    nodes[0].kind = NodeKind::ObtainItems { item: "iron_full_helm".to_string(), count: helms };
    nodes
}

/// Bars per helm of the smith's recipe.
fn bars_per_helm() -> f64 {
    crate::bid_response::per_unit_inputs("smith:iron_full_helm", &crate::obtain_legs::production_legs("iron_full_helm")[0].inputs)[0].1
}

/// Real time value of a bot (plan équilibre D1: ~0.25 coin a tick).
const REAL_COIN_RATE: f64 = 0.25;

/// The bar's reservation for a helm lot of 100 read from the helm's
/// public book, bars on sale at 1 coin (the brain's `derived_reserve`).
fn bar_reserve_from(helm_book: &BookView) -> Option<f64> {
    let nodes = helm_lot_graph(100);
    let value = |item: &str| if item == "iron_full_helm" { product_value(Some(helm_book), product_lot(&nodes, item)) } else { None };
    let input = |_: &str| 1.0;
    let prices = DerivedPrices { value_of: &value, input_price: &input, units_per_6000: &rates, coin_rate: REAL_COIN_RATE };
    input_reserve(&nodes, 1, &prices)
}

fn smith_levels(skill: &str) -> u8 {
    match skill {
        "smithing" => 22,
        "mining" => 55,
        "attack" | "strength" | "defence" => 20,
        _ => 1,
    }
}

fn hammer_only(item: &str) -> i64 {
    i64::from(item == "hammer")
}

/// The lot a smith at the real time value takes, bars on sale at 1 coin
/// (`bid_response::opportunity`, preempting its ambition).
fn helm_lot(helm_book: &BookView) -> Option<crate::bid_response::Opportunity> {
    let bars = BookView { asks: vec![(1, 400)], volume_24h: 5000, current: Some(1), ..BookView::default() };
    let helm = helm_book.clone();
    let market = move |item: &str| match item {
        "iron_bar" => Some(bars.clone()),
        "iron_full_helm" => Some(helm.clone()),
        _ => None,
    };
    let smith = crate::bid_response::Producer {
        level_of: &smith_levels,
        held: &hammer_only,
        avoid: &[],
        quests_done: &[],
        coin_rate: REAL_COIN_RATE,
        units_per_6000: &rates,
        ironman: false,
        seed: 3,
        tick: 0,
        combat: 30,
        position: Some((3185, 3436, 0)),
        free_pass: false,
        planned_level: &crate::bid_response::no_plan,
        alch_net: &crate::bid_response::no_alch,
        rate: &crate::work_rate::prior_rate,
        seats: &[],
        cooled: &[],
    };
    crate::bid_response::opportunity("iron_full_helm", helm_book, &market, &smith).filter(|found| crate::bid_response::preempts(found, smith.coin_rate))
}

/// Plan équilibre step 4 (live 2026-11-19: coal bid 17 under asks at
/// 27, gold bars bid 1 under 110, inputs resting at 1 coin by the
/// million): a bar at 1 coin under its transformation value (the helm
/// sells 8, the smith's time at its real rate 2.5) is bid ABOVE the ask
/// (ceiling = what the helm sells for − the smith's time, over the bars
/// a helm takes; no private margin on top) and the smith takes the lot.
#[test]
fn an_input_under_its_transformation_value_is_bid_above_the_ask_and_transformed() {
    let helm_book = BookView { bids: vec![(8, 200)], ..BookView::default() };
    let reserve = bar_reserve_from(&helm_book).expect("the helm sells");
    let expected = (8.0 - 6000.0 / 600.0 * REAL_COIN_RATE) / bars_per_helm();
    assert!((reserve - expected).abs() < 1e-6, "{reserve} vs {expected}");
    let ceiling = obtain_ceiling(10_000, Some(reserve));
    assert!(ceiling > 1, "the bar bid climbs over the ask at 1: ceiling {ceiling}, reserve {reserve}");
    let found = helm_lot(&helm_book).expect("the smith buys bars at 1 and sells helms at 8");
    assert!(found.unit.plan.iter().any(|step| step == "buy iron_bar"), "{found:?}");
    assert!(found.count > 0, "{found:?}");
}

/// The same bar when the helm has no demand left: the bids are claimed
/// by other producers, or the only price is a stale one (no bid, no
/// volume in a day): no bid above the ask, no lot.
#[test]
fn the_same_input_without_demand_for_its_product_is_neither_bid_up_nor_transformed() {
    let claimed = BookView { bids: vec![(50, 200)], claimed: 200, ..BookView::default() };
    let stale = BookView { current: Some(200), ..BookView::default() };
    for (case, helm_book) in [("claimed", claimed), ("stale price", stale)] {
        let reserve = bar_reserve_from(&helm_book).expect("a valued helm");
        assert_eq!(obtain_ceiling(10_000, Some(reserve)), 1, "{case}: no demand, no bid over the ask (reserve {reserve})");
        assert!(helm_lot(&helm_book).is_none(), "{case}: no lot");
    }
}

/// What the product sells for is read along the demand the others leave:
/// bids first, then a day's sales at the current price behind the asks
/// resting at or under it; the units the market no longer takes are
/// worth nothing to the lot.
#[test]
fn the_product_value_is_the_shared_demand_over_the_lot() {
    let book = BookView { bids: vec![(30, 10), (20, 10)], ..BookView::default() };
    assert_eq!(product_value(Some(&book), 10), Some(30.0));
    assert_eq!(product_value(Some(&book), 20), Some(25.0));
    assert_eq!(product_value(Some(&book), 40), Some(12.5), "half the lot sells");
    let claimed = BookView { claimed: 10, ..book.clone() };
    assert_eq!(product_value(Some(&claimed), 10), Some(20.0), "the units others claim come first: they take the best bids");
    assert_eq!(product_value(Some(&claimed), 20), Some(10.0));
    let traded = BookView { asks: vec![(14, 20)], volume_24h: 100, current: Some(15), ..BookView::default() };
    assert_eq!(product_value(Some(&traded), 30), Some(15.0), "50 a day at 15, behind 20 resting at 14");
    assert_eq!(product_value(Some(&traded), 60), Some(7.5));
    assert_eq!(product_value(None, 10), None, "never priced: no value known");
}

/// The ceiling of the steel bars of a steel platebody read from the
/// platebody's public book (a known price, no volume: no demand left), in
/// a life whose goal is `goal`; `graph` the platebody's graph.
fn platebody_bar_ceiling(graph: &[Node], bars: usize, goal: &GoalKind) -> i64 {
    let book = BookView { current: Some(200), ..BookView::default() };
    let value = |item: &str| if item == "steel_platebody" { product_value(Some(&book), product_lot(graph, item)) } else { None };
    let input = |_: &str| 1.0;
    let prices = DerivedPrices { value_of: &value, input_price: &input, units_per_6000: &rates, coin_rate: REAL_COIN_RATE };
    obtain_ceiling(5_000, sale_reserve(graph, bars, goal, &prices))
}

/// Review lot 4, point 3: a product the bot USES (gear it wears, a tool
/// of its kit, a quest item) is never valued at its sale on the shared
/// demand: a known price with no volume would bid its steel bars at 1
/// while asks rest at 34-37, never filled. Its inputs keep the wallet as
/// their ceiling; the same platebody made for sale (a `Produce` quota) is
/// bounded by its demand. A product a sold chain uses as a tool (not an
/// input of its recipe) is used, not sold. (The steel pickaxe of the
/// review is no smithing recipe in revision 274: the kit buys it.)
#[test]
fn inputs_of_a_product_the_bot_uses_keep_the_wallet_ceiling() {
    let mut platebody = Node::new("n1", NodeKind::ObtainItems { item: "steel_platebody".to_string(), count: 1 }, vec!["n2".to_string()], 1);
    platebody.method = Some("smith:steel_platebody".to_string());
    let bars = Node::new("n2", NodeKind::ObtainItems { item: "steel_bar".to_string(), count: 5 }, Vec::new(), 5);
    let gear = vec![platebody.clone(), bars.clone()];
    let worn = GoalKind::EquipBest { style: "melee".to_string() };
    assert_eq!(platebody_bar_ceiling(&gear, 1, &worn), 5_000, "gear it wears: the wallet");
    let own = GoalKind::Obtain { item: "steel_platebody".to_string(), count: 1 };
    assert_eq!(platebody_bar_ceiling(&gear, 1, &own), 5_000, "an object it obtains for itself: the wallet");
    let quota = GoalKind::Produce { item: "steel_platebody".to_string(), count: 1 };
    assert_eq!(platebody_bar_ceiling(&gear, 1, &quota), 1, "a platebody to sell with no demand: no bid over the ask");
    // A mining quota the platebody serves without being an input of its
    // recipe (worn for the trip): used, not sold through the ore.
    let mut ore = Node::new("n0", NodeKind::ObtainItems { item: "iron_ore".to_string(), count: 100 }, vec!["n1".to_string()], 100);
    ore.method = Some("farm:iron_ore".to_string());
    let chain = vec![ore, platebody, bars];
    let mining = GoalKind::Produce { item: "iron_ore".to_string(), count: 100 };
    assert_eq!(platebody_bar_ceiling(&chain, 2, &mining), 5_000, "not an input of the quota's recipe: the wallet");
}
