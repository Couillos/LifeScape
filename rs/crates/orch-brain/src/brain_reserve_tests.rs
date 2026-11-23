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

/// bct5rxpg, report 8220, re-read 2026-11-14 (frank_lane): a fight's
/// drops are FOUND, they take no share of its ticks (the fight paid in
/// XP); a gathered line in the same report keeps its share, the drops
/// still weigh in the total.
#[test]
fn combat_drops_take_no_ticks() {
    let weight = |item: &str| if item == "Bronze med helm" { 24.0 } else { 1.0 };
    let reports = vec![
        report("melee", 1500, vec![line("Coins", "loot", 50), line("Bronze med helm", "loot", 1), line("Logs", "produced", 26)]),
        report("market", 9000, vec![line("Logs", "traded", 500)]),
    ];
    assert_eq!(measured_ticks_per_unit(&reports, "Bronze med helm", &weight), None, "loot: no time cost");
    // Total value 50 + 24 + 26 = 100: the logs take 26 % of 1500 ticks.
    let logs = measured_ticks_per_unit(&reports, "Logs", &weight).unwrap_or(0.0);
    assert!((logs - 15.0).abs() < 1e-9, "26 % of 1500 over 26 logs: {logs}");
    assert_eq!(own_origin(&reports, "Bronze med helm"), (0, 1));
    assert_eq!(own_origin(&reports, "Logs"), (26, 0), "a market report never counts");
}

/// Live frank_lane 2026-11-14 (Al Kharid warriors): "holding 1
/// bronze_dagger (no own cost known)", steel arrow listed at 7082
/// (reserve 3541 measured), bronze arrows held (reserve 12 over the 5x
/// band of 2). Found stock gets a 1-coin `Found` reserve; a purchase
/// still floors it; an object it never recorded and cannot make is
/// found too; one it can make keeps its make cost.
#[test]
fn found_stock_has_a_one_coin_reserve() {
    let found = Reserve { coins: 1, source: CostSource::Found };
    assert_eq!(found_reserve(42, None, None), found);
    assert_eq!(found_reserve(42, None, Some(30.0)), found, "looted: the make cost is no own cost");
    assert_eq!(found_reserve(0, None, None), found, "nothing known, cannot make it: found");
    assert_eq!(found_reserve(0, None, Some(30.5)), Reserve { coins: 30, source: CostSource::MakeCost });
    assert_eq!(found_reserve(42, Some(9), None), Reserve { coins: 9, source: CostSource::Bought }, "never under what it paid");
    // The live report 20092 (14551 ticks of melee): every drop is found.
    let reports = vec![report(
        "melee",
        14551,
        vec![
            line("Bronze arrow", "loot", 42),
            line("Bronze med helm", "loot", 3),
            line("Iron dagger", "loot", 2),
            line("Steel arrow", "loot", 1),
            line("Coins", "shop_sold", 949),
            line("Cabbage", "consumed", -10),
        ],
    )];
    for display in ["Bronze arrow", "Bronze med helm", "Iron dagger", "Steel arrow"] {
        assert_eq!(measured_ticks_per_unit(&reports, display, &item_weight), None, "{display}");
        assert!(measured_inputs_per_unit(&reports, display, &item_weight).is_empty(), "eaten food is no input of a drop: {display}");
    }
    // Found inputs cost no time either (looted copper ore in a bar).
    let costs = OwnCosts { account: "frank_lane".to_string(), reports: vec![report("melee", 6000, vec![line("Copper ore", "loot", 2)])], coins_per_tick: 4.8 };
    assert_eq!(costs.unit_cost(&|_: &str| None, "Copper ore", INPUT_DEPTH), 0.0);
    assert!(prior_ticks_per_unit("Feather").is_none(), "a combat prior is loot, never a time cost");
}

/// One tick is never free: the rate handed in (the D1 time value) never
/// falls under the shared floor `bid_response` prices with (0.1); never
/// a 1-coin default. One rate for the whole brain (lot 5, 2026-11-19):
/// `OwnCosts` keeps the time value it is given, it measures no second one.
#[test]
fn coins_per_tick_is_the_time_value_never_under_the_shared_floor() {
    let floor = lifescape_planner::market_fit::MIN_COIN_RATE;
    let store = lifescape_store::Store::open_in_memory().expect("store");
    assert!((OwnCosts::load(&store, "lara_lane", 0.03).coins_per_tick - floor).abs() < 1e-9, "0.03 measured, 0.1 floor");
    assert!((OwnCosts::load(&store, "lara_lane", 0.28).coins_per_tick - 0.28).abs() < 1e-9, "the D1 time value as given");
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
    // The game prior of the gathering leg at its gate (woodcutting 30).
    let query = lifescape_planner::method_rate::PriorQuery { method: "farm:willow_logs".to_string(), level: 30, ..Default::default() };
    let gained: f64 = lifescape_planner::method_rate::method_prior(&query).expect("willow prior").flows.iter().filter(|(item, _, _)| item == "willow_logs").map(|(_, _, qty)| qty).sum();
    assert!((prior - 6000.0 / gained).abs() < 1e-9, "{prior}");
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
