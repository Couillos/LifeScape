//! Derived demand: inputs priced from the product a producer makes.

use super::{derived_bounds, input_reserve, obtain_ceiling, DerivedPrices, DERIVED_MARGIN_PCT};
use crate::model::{Node, NodeKind};

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

/// The bar's reservation price is the helm's value less the margin and
/// the smith's time, over the bars a helm takes; it rises with the helm.
#[test]
fn bar_bids_rise_with_the_product_price() {
    let bars_per_helm = crate::bid_response::per_unit_inputs("smith:iron_full_helm", &crate::obtain_legs::production_legs("iron_full_helm")[0].inputs)[0].1;
    let cheap = reserve_at_helm_price(123.0, 1).expect("derived");
    let expected = (123.0 * (1.0 - DERIVED_MARGIN_PCT / 100.0) - 10.0 * 0.5) / bars_per_helm;
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
