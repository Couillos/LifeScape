use super::*;

fn line(item: &str, provenance: &str, quantity: i64) -> OwnLine {
    OwnLine { item: item.to_string(), provenance: provenance.to_string(), quantity }
}

fn report(method: &str, ticks: i64, lines: Vec<OwnLine>) -> OwnReport {
    OwnReport { method: method.to_string(), ticks, lines }
}

fn flat(_: &str) -> f64 {
    1.0
}

/// lara_lane, report 8221: 141 willow logs in 6142 ticks of woodcutting:
/// every tick belongs to the logs (the coins line is a shop purchase).
#[test]
fn a_gathering_report_gives_its_product_every_tick() {
    let reports = vec![report("woodcut", 6142, vec![line("Coins", "shop_bought", -13), line("Willow logs", "produced", 141)])];
    let per_unit = measured_ticks_per_unit(&reports, "Willow logs", &flat).unwrap_or(0.0);
    assert!((per_unit - 6142.0 / 141.0).abs() < 1e-9, "{per_unit}");
    assert_eq!(measured_ticks_per_unit(&reports, "Oak logs", &flat), None);
}

/// bct5rxpg, report 8220: 1501 ticks of melee shared by loot value; the
/// looted coins take their share, a market report never counts.
#[test]
fn a_combat_report_gives_each_drop_its_loot_share() {
    let weight = |item: &str| if item == "Bronze med helm" { 24.0 } else { 1.0 };
    let reports = vec![
        report("melee", 1500, vec![line("Coins", "loot", 50), line("Bronze med helm", "loot", 1), line("Logs", "loot", 26)]),
        report("market", 9000, vec![line("Logs", "traded", 500)]),
    ];
    // Total value 50 + 24 + 26 = 100: the helm takes 24 % of 1500 ticks.
    let helm = measured_ticks_per_unit(&reports, "Bronze med helm", &weight).unwrap_or(0.0);
    assert!((helm - 360.0).abs() < 1e-9, "{helm}");
    let logs = measured_ticks_per_unit(&reports, "Logs", &weight).unwrap_or(0.0);
    assert!((logs - 15.0).abs() < 1e-9, "26 % of 1500 over 26 logs: {logs}");
}

/// Coins per tick: the best own method, coins looted plus its farmed
/// units at its own last sale price (net of what it shop-sold).
#[test]
fn coins_per_tick_is_the_best_own_method() {
    let reports = vec![
        report("melee", 1500, vec![line("Coins", "loot", 45)]),
        report("woodcut", 6000, vec![line("Willow logs", "produced", 150)]),
        report("ranged", 6000, vec![line("Feather", "ground_spawn", 1000), line("Feather", "shop_sold", -1000), line("Coins", "shop_sold", 120)]),
    ];
    let sale = |item: &str| match item {
        "Willow logs" => Some(8),
        "Feather" => Some(3),
        _ => None,
    };
    // melee 0.03, woodcut 150 x 8 / 6000 = 0.2, ranged 120 / 6000 = 0.02
    // (the shop-sold feathers are not valued twice).
    let rate = measured_coins_per_tick(&reports, &sale).unwrap_or(0.0);
    assert!((rate - 0.2).abs() < 1e-9, "{rate}");
    assert!((coins_per_tick(&reports, &sale) - 0.2).abs() < 1e-9, "its own best method");
    let unsold = |_: &str| None;
    assert!((measured_coins_per_tick(&reports, &unsold).unwrap_or(0.0) - 0.03).abs() < 1e-9, "unsold logs earn nothing yet");
}

/// One tick is never free: under the shared floor `bid_response`
/// prices with (0.1), the floor holds; never a 1-coin default.
#[test]
fn coins_per_tick_never_falls_under_the_shared_floor() {
    let floor = lifescape_planner::market_fit::MIN_COIN_RATE;
    let reports = vec![report("melee", 1500, vec![line("Coins", "loot", 45)])];
    assert!((coins_per_tick(&reports, &|_: &str| None) - floor).abs() < 1e-9, "0.03 measured, 0.1 floor");
    assert!((coins_per_tick(&[], &|_: &str| None) - floor).abs() < 1e-9, "nothing measured: the floor");
    // lara_lane's willow: 43.6 ticks a log at the floor = 5 coins.
    assert_eq!(reserve_coins(6142.0 / 141.0, floor), 5);
}

#[test]
fn reserve_prefers_measured_then_planned_then_prior_and_covers_purchases() {
    let rate = 0.1;
    assert_eq!(reserve_of(Some(43.6), Some(10.0), Some(5.0), rate, 0.0, None, None), Some(Reserve { coins: 5, source: CostSource::Measured }));
    assert_eq!(reserve_of(None, Some(30.0), Some(5.0), rate, 0.0, None, None), Some(Reserve { coins: 3, source: CostSource::Planned }));
    assert_eq!(reserve_of(None, None, Some(33.3), rate, 0.0, None, None), Some(Reserve { coins: 4, source: CostSource::Prior }));
    assert_eq!(reserve_of(Some(20.0), None, None, rate, 0.0, Some(9), None), Some(Reserve { coins: 9, source: CostSource::Bought }), "never under what it paid");
    assert_eq!(reserve_of(None, None, None, rate, 0.0, Some(4), None), Some(Reserve { coins: 4, source: CostSource::Bought }));
    assert_eq!(reserve_of(None, None, None, rate, 0.0, None, None), None, "no own cost: hold, never 1 coin");
}

/// A bid quota is priced at the make cost (`bid_response`): the reserve
/// never exceeds it, or the producer would hold the stock the bids it
/// answered would buy.
#[test]
fn the_reserve_never_exceeds_the_bid_response_make_cost() {
    let slow = reserve_of(Some(120.0), None, None, 0.1, 0.0, None, Some(7.9));
    assert_eq!(slow, Some(Reserve { coins: 7, source: CostSource::MakeCost }), "12 capped at floor(7.9)");
    assert_eq!(reserve_of(Some(40.0), None, None, 0.1, 0.0, None, Some(30.0)), Some(Reserve { coins: 4, source: CostSource::Measured }), "under the cap: its own cost");
    assert_eq!(reserve_of(None, None, None, 0.1, 0.0, Some(20), Some(9.0)).map(|reserve| reserve.coins), Some(9));
    assert_eq!(reserve_of(None, None, None, 0.1, 0.0, None, Some(9.0)), None, "the cap alone is no own cost");
    assert_eq!(reserve_of(Some(40.0), None, None, 0.1, 0.0, None, Some(0.4)).map(|reserve| reserve.coins), Some(1));
}

#[test]
fn planner_estimates_come_from_the_bots_own_graph() {
    use lifescape_planner::{Alternative, Node};
    let mut life = crate::brain_sales::tests::life_training(&[]);
    let mut node = Node::new("n4", NodeKind::ObtainItems { item: "willow_logs".to_string(), count: 27 }, Vec::new(), 27);
    node.alternatives = vec![
        Alternative { method: "market:buy".to_string(), est_ticks: 10, est_coins: 27 },
        Alternative { method: "gather:woodcut/willow_logs".to_string(), est_ticks: 1080, est_coins: 0 },
    ];
    life.nodes.push(node);
    assert_eq!(planned_ticks_per_unit(&life, "willow_logs"), Some(40.0), "the farm way, never the market leg");
    assert_eq!(planned_ticks_per_unit(&life, "logs"), None);
    let prior = prior_ticks_per_unit("Willow logs").unwrap_or(0.0);
    assert!((prior - 6000.0 / 180.0).abs() < 1e-9, "{prior}");
}

#[test]
fn reserves_round_up_and_never_reach_zero() {
    assert_eq!(reserve_coins(43.6, 0.082), 4);
    assert_eq!(reserve_coins(0.5, 0.01), 1);
}

/// Live bst29bh9: the forged helms were held "no own cost known". A
/// processing report now names its product (`produced`) and the inputs
/// it ate (`consumed`): one helm owes its smithing ticks plus two iron
/// bars, each bar its smelting ticks plus two iron ores (cold-start
/// mining ticks, never under the ore's own purchase price).
#[test]
fn processed_units_cost_their_ticks_plus_their_inputs() {
    let reports = vec![
        report("smelt", 300, vec![line("Iron bar", "produced", 10), line("Iron ore", "consumed", -20)]),
        report("smith", 200, vec![line("Iron full helm", "produced", 2), line("Iron bar", "consumed", -4)]),
    ];
    assert_eq!(measured_inputs_per_unit(&reports, "Iron full helm", &flat), vec![("Iron bar".to_string(), 2.0)]);
    assert_eq!(measured_inputs_per_unit(&reports, "Iron bar", &flat), vec![("Iron ore".to_string(), 2.0)]);
    assert!(measured_inputs_per_unit(&reports, "Iron ore", &flat).is_empty(), "nothing made it");
    let costs = OwnCosts { account: "bst29bh9".to_string(), reports, coins_per_tick: 0.5 };
    let ore_ticks = prior_ticks_per_unit("Iron ore").expect("mining prior for iron ore");
    let ore = (ore_ticks * 0.5).max(10.0);
    let bought = |display: &str| (display == "Iron ore").then_some(10.0);
    let bar = 30.0 * 0.5 + 2.0 * ore;
    assert!((costs.unit_cost(&bought, "Iron bar", INPUT_DEPTH) - bar).abs() < 1e-6, "bar = ticks + ores");
    let inputs = costs.input_coins(&bought, "Iron full helm", INPUT_DEPTH);
    assert!((inputs - 2.0 * bar).abs() < 1e-6, "helm inputs = two bars, got {inputs}");
    let helm_ticks = measured_ticks_per_unit(&costs.reports, "Iron full helm", &flat);
    let reserve = reserve_of(helm_ticks, None, None, 0.5, inputs, None, None).expect("an own cost");
    assert_eq!(reserve, Reserve { coins: (100.0 * 0.5 + 2.0 * bar).ceil() as i64, source: CostSource::Measured });
}
