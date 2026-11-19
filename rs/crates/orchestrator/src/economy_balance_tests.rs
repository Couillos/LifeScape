use super::{lot_ratio, open_lots, skill_label, stock_ranking, work_shares, WorkShares};
use crate::trade_report::TradeRow;
use lifescape_store::{MethodWork, NewExecution, NewOrder, NewTaskReport, OrderSide, ReportMeasure, Store, UnsoldStock};

fn work(method: &str, style: &str, node: &str, ticks: i64) -> MethodWork {
    MethodWork { planner_method: method.to_string(), combat_style: style.to_string(), node_id: node.to_string(), work_ticks: ticks }
}

fn stock(item_id: i64, name: &str, resting_units: i64, traded_units: i64) -> UnsoldStock {
    UnsoldStock { item_id, name: name.to_string(), resting_units, traded_units }
}

fn lot(opened: i64, status: &'static str, predicted: i64, realized: i64) -> TradeRow {
    TradeRow {
        account: "smith".to_string(),
        node: "n7".to_string(),
        method: "trade:craft/smelt:mithril_bar".to_string(),
        product: "mithril_bar".to_string(),
        lot: 10,
        bound: "capital".to_string(),
        opened,
        predicted,
        planned_cost: 0,
        planned_units: 0,
        bought_units: 0,
        bought_coins: 0,
        made: None,
        craft_yield: None,
        guide_per_6000: 0.0,
        sold_units: 0,
        sold_coins: 0,
        realized,
        status,
    }
}

/// Lot 0: a method's skill comes from the planner (combat style first,
/// else the game-data way, 100 % market lots by their recipe); work no
/// skill gates keeps its family.
#[test]
fn skills_come_from_the_planner() {
    assert_eq!(skill_label("gather:woodcut/yew_logs", ""), "woodcutting");
    assert_eq!(skill_label("farm:yew_logs", ""), "woodcutting");
    assert_eq!(skill_label("fight:melee/men", "melee:strength"), "strength");
    assert_eq!(skill_label("fight:magic/wind_strike/chickens", "magic"), "magic");
    assert_eq!(skill_label("smelt:steel_bar", ""), "smithing");
    assert_eq!(skill_label("trade:craft/smelt:steel_bar", ""), "smithing");
    assert_eq!(skill_label("pottery:pot_unfired", ""), "crafting");
    assert_eq!(skill_label("market:make", ""), "(none) market");
    assert_eq!(skill_label("", ""), "(none) unmeasured");
}

/// Lot 0: shares of the fleet's work time by skill, filler nodes and
/// yew harvest, the transformation skills summed (§M).
#[test]
fn work_shares_split_skill_filler_and_yew() {
    let rows = vec![
        work("gather:woodcut/yew_logs", "", "fill-n4", 400),
        work("gather:woodcut/yew_logs", "", "n9", 100),
        work("gather:woodcut/willow_logs", "", "n9", 100),
        work("fight:melee/men", "melee:strength", "fill-n5", 100),
        work("pottery:pot_unfired", "", "n1", 60),
        work("cook:trout", "", "n2", 40),
        work("market:make", "", "n3", 200),
    ];
    let shares = work_shares(&rows);
    assert_eq!((shares.total, shares.filler, shares.yew), (1_000, 500, 500));
    assert_eq!(
        shares.by_skill,
        vec![("woodcutting".to_string(), 600), ("(none) market".to_string(), 200), ("strength".to_string(), 100), ("crafting".to_string(), 60), ("cooking".to_string(), 40)]
    );
    assert_eq!(shares.transform(), 100, "crafting + cooking");
    assert_eq!(shares.share(shares.filler), Some(0.5));
    assert_eq!(WorkShares::default().share(0), None);
}

/// Lot 0: traded objects ranked by days of stock (top 15), untraded
/// objects with asks apart by resting units, fleet total over all asks.
#[test]
fn stock_ranking_reads_days_of_unsold_stock() {
    let half_day = 43_200;
    let mut stocks = vec![stock(1, "Yew logs", 1_200, 150), stock(2, "Raw beef", 40, 0), stock(3, "Coal", 100, 100), stock(4, "Willow logs", 9_000, 0)];
    for item_id in 10..30 {
        stocks.push(stock(item_id, "Filler", 1, 1_000));
    }
    let resting = 1_200 + 40 + 100 + 9_000 + 20;
    let traded = 150 + 100 + 20 * 1_000;
    let sold_out = 500;
    let (ranked, untraded, fleet) = stock_ranking(&stocks, traded + sold_out, half_day);
    assert_eq!(ranked.len(), 15);
    assert_eq!((ranked[0].0.name.as_str(), ranked[0].1), ("Yew logs", 4.0));
    assert_eq!((ranked[1].0.name.as_str(), ranked[1].1), ("Coal", 0.5));
    assert_eq!(untraded.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(), vec!["Willow logs", "Raw beef"]);
    let per_day = (traded + sold_out) * 2;
    assert_eq!(fleet, Some(resting as f64 / per_day as f64), "the fleet pace counts the units of objects sold out (no ask left)");
    assert_eq!(stock_ranking(&[stock(2, "Raw beef", 40, 0)], 0, half_day).2, None);
}

/// Lot 0: realized over predicted of the closed lots opened in the
/// window (the R5 totals as a ratio).
#[test]
fn lot_ratio_reads_the_closed_lots_of_the_window() {
    let rows = vec![lot(5_000, "closed", 1_000, 160), lot(6_000, "closed", 300, 300), lot(7_000, "selling", 900, -50), lot(100, "closed", 5_000, 0)];
    assert_eq!(lot_ratio(&rows, 1_000), (2, 1_300, 460, Some(460.0 / 1_300.0)));
    assert_eq!(lot_ratio(&rows, 0).0, 3);
    assert_eq!(lot_ratio(&rows, 9_000), (0, 0, 0, None));
    assert_eq!(open_lots(&rows, 1_000), 1, "the selling lot is not closed yet");
    assert_eq!(open_lots(&rows, 9_000), 0);
}

/// Lot 0: the board rebuilt from an in-memory store (asks, trades,
/// measured reports), staff left out.
#[test]
fn board_rebuilt_from_a_store() -> Result<(), lifescape_store::Error> {
    let store = Store::open_in_memory()?;
    store.upsert_item(&lifescape_store::Item { item_id: 1515, name: "Yew logs".to_string(), shop_price: 2, alch_value: 1 })?;
    let since = 10_000;
    store.place_order(&NewOrder { account: "cutter".to_string(), side: OrderSide::Sell, item_id: 1515, quantity: 600, price: 3, clock: since, ..NewOrder::default() })?;
    store.upsert_item(&lifescape_store::Item { item_id: 2359, name: "Mithril bar".to_string(), shop_price: 2, alch_value: 1 })?;
    store.record_execution(&NewExecution { item_id: 1515, quantity: 300, price: 3, clock: since + 1, server_tick: 0, taker_side: None })?;
    store.record_execution(&NewExecution { item_id: 2359, quantity: 300, price: 3, clock: since + 1, server_tick: 0, taker_side: None })?;
    for (account, node, method, ticks) in [("cutter", "fill-n1", "gather:woodcut/yew_logs", 300), ("potter", "n2", "pottery:pot_unfired", 100), ("admin", "n3", "smelt:steel_bar", 5_000)] {
        let report_id = store.record_task_report(&NewTaskReport {
            account: account.to_string(),
            node_id: node.to_string(),
            method: "legacy".to_string(),
            script: "script".to_string(),
            ticks,
            stop: "goal_reached".to_string(),
            context_level: 1,
            clock: since + 2,
        })?;
        store.record_report_measure(report_id, &ReportMeasure { planner_method: method.to_string(), work_ticks: ticks, ..ReportMeasure::default() })?;
    }
    let excluded = vec!["admin".to_string()];
    let shares = work_shares(&store.work_by_method(since, 0, &excluded)?);
    assert_eq!((shares.total, shares.filler, shares.yew, shares.transform()), (400, 300, 300, 100));
    let (ranked, _, fleet) = stock_ranking(&store.unsold_stock(since, 0, &excluded)?, store.window_traded_units(since, 0)?, 86_400);
    assert_eq!(ranked.len(), 1);
    assert_eq!(fleet, Some(1.0), "600 yew units resting, 600 units a day (the mithril bars sold out count)");
    Ok(())
}
