use super::*;
fn parsed_fishing(quantity: i64, ticks: i64, level: i64) -> ParsedReport {
    ParsedReport {
        account: "t01".to_string(),
        node_id: "n9".to_string(),
        method: "fishing".to_string(),
        script: "fishing".to_string(),
        wall_ticks: ticks,
        pause_ticks: 0,
        stop: "budget_exhausted".to_string(),
        level,
        band: level_band(level),
        items: vec![ParsedItem {
            name: "Raw shrimps".to_string(),
            provenance: "produced".to_string(),
            quantity,
        }],
        xp: vec![ParsedXp { skill: "fishing".to_string(), amount: 4000 }],
    }
}

/// Normalisation: a half window doubles the rate; zero ticks
/// carries no rate.
#[test]
fn normalization_scales_to_the_window() {
    assert_eq!(normalize_per_6000(300, 6000), Some(300.0));
    assert_eq!(normalize_per_6000(300, 3000), Some(600.0));
    assert_eq!(normalize_per_6000(0, 6000), Some(0.0));
    assert_eq!(normalize_per_6000(10, 0), None);
    assert_eq!(normalize_per_6000(10, -5), None);
}

/// Bands of five: 1-5, 6-10, 11-15; level 0 clamps to band 1.
#[test]
fn bands_cover_five_levels() {
    assert_eq!((level_band(1), level_band(5)), (1, 1));
    assert_eq!((level_band(6), level_band(10)), (6, 6));
    assert_eq!((level_band(11), level_band(15)), (11, 11));
    assert_eq!(level_band(0), 1);
    assert_eq!(level_band(99), 96);
}

/// Moving average: first sample seeds, then 0.3 smoothing.
#[test]
fn moving_average_smooths_in_order() {
    assert_eq!(moving_average(&[], EMA_ALPHA), 0.0);
    assert_eq!(moving_average(&[200.0], EMA_ALPHA), 200.0);
    let ema = moving_average(&[100.0, 200.0], EMA_ALPHA);
    assert!((ema - 130.0).abs() < 0.001, "got {ema}");
    let ema = moving_average(&[100.0, 200.0, 300.0], EMA_ALPHA);
    assert!((ema - 181.0).abs() < 0.001, "got {ema}");
}

/// Mean and population deviation on a known series.
#[test]
fn mean_and_deviation_match() {
    assert_eq!(mean(&[]), 0.0);
    assert_eq!(mean(&[100.0, 300.0]), 200.0);
    assert_eq!(stddev(&[200.0]), 0.0);
    assert!((stddev(&[100.0, 300.0]) - 100.0).abs() < 0.001);
}

/// Priors serve while n < 3, then the measurement replaces them.
#[test]
fn priors_yield_to_three_measures() {
    let prior = prior_for("fishing", "Raw shrimps").map(|(yield_per, _)| yield_per);
    assert!(prior.is_some(), "shrimp prior exists");
    let first = aggregate(&[400.0], prior);
    assert!(first.prior, "one report keeps the prior");
    let second = aggregate(&[400.0, 420.0], prior);
    assert!(second.prior, "two reports keep the prior");
    let third = aggregate(&[400.0, 420.0, 440.0], prior);
    assert!(!third.prior, "three reports measure");
    assert!((third.mean - 420.0).abs() < 0.001);
    let unknown = aggregate(&[500.0], prior_for("fishing", "Nope").map(|(yield_per, _)| yield_per));
    assert!(!unknown.prior, "no prior means measured, even thin");
}

/// Provenance filter: farm flows count, shop and trade never do;
/// consumed feeds inputs instead. `picked` is retired (D45, P0.2): the
/// SDK no longer emits or parses it, so it never rates.
#[test]
fn provenance_filter_keeps_farm_only() {
    assert!(is_farm_provenance("produced"));
    assert!(is_farm_provenance("loot"));
    assert!(is_farm_provenance("ground_spawn"));
    assert!(!is_farm_provenance("picked"), "picked is retired, never a yield");
    for excluded in ["shop_bought", "shop_sold", "traded", "consumed", "lost", "death", "unattributed"] {
        assert!(!is_farm_provenance(excluded), "{excluded} excluded");
    }
    assert!(is_input_provenance("consumed"));
    assert!(!is_input_provenance("produced"));
    let mut snapshot = Snapshot::new();
    let mut mixed = parsed_fishing(300, 6000, 8);
    mixed.items.push(ParsedItem {
        name: "Fishing bait".to_string(),
        provenance: "consumed".to_string(),
        quantity: -30,
    });
    mixed.items.push(ParsedItem {
        name: "Fishing bait".to_string(),
        provenance: "shop_bought".to_string(),
        quantity: 100,
    });
    mixed.items.push(ParsedItem {
        name: "Coins".to_string(),
        provenance: "traded".to_string(),
        quantity: 500,
    });
    snapshot.record(&mixed);
    let rows = snapshot.yield_rows();
    assert_eq!(rows.len(), 1, "only the farm line rates, got {}", rows.len());
    assert_eq!(rows[0].item, "Raw shrimps");
    assert!(!snapshot.inputs.is_empty(), "consumed feeds inputs");
}

/// Parsing: method falls back to activity, band from the XP skill
/// level, XP lines skipped when empty.
#[test]
fn parse_report_maps_the_push_shape() {
    let report = object(vec![
        ("account", str_value("t01")),
        ("activity", str_value("fishing")),
        ("node_id", str_value("n9")),
        ("stop", str_value("budget_exhausted")),
        ("ticks", Value::Int(6000)),
        ("market_pause_ticks", Value::Int(0)),
        (
            "xp",
            Value::Arr(vec![object(vec![
                ("stat", Value::Int(10)),
                ("skill", str_value("fishing")),
                ("xp", Value::Int(4000)),
            ])]),
        ),
        (
            "items",
            Value::Arr(vec![object(vec![
                ("name", str_value("Raw shrimps")),
                ("provenance", str_value("produced")),
                ("qty", Value::Int(300)),
            ])]),
        ),
        (
            "levels_before",
            Value::Arr(vec![object(vec![("stat", Value::Int(10)), ("level", Value::Int(8))])]),
        ),
        ("levels_after", Value::Arr(vec![])),
    ]);
    let parsed = parse_report(&report).expect("parses");
    assert_eq!(parsed.method, "fishing");
    assert_eq!(parsed.script, "fishing");
    assert_eq!(parsed.band, 6);
    assert_eq!(parsed.items.len(), 1);
    assert_eq!(parsed.xp.len(), 1);
    assert!(parse_report(&object(vec![])).is_err(), "empty account fails loud");
}

/// Snapshot round-trips through JSON: restart loses nothing.
#[test]
fn snapshot_survives_a_restart() {
    let mut snapshot = Snapshot::new();
    for quantity in [300, 360, 420] {
        snapshot.record(&parsed_fishing(quantity, 6000, 8));
    }
    let text = lifescape_json::stringify(&snapshot_to_value(&snapshot));
    let reloaded = snapshot_from_json(&text).expect("reloads");
    let rows = reloaded.yield_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].count, 3);
    assert!(!rows[0].prior, "three measures drop the prior");
    assert!((rows[0].mean - 360.0).abs() < 0.001, "mean {}, want 360", rows[0].mean);
    assert_eq!(reloaded.xp_rows().len(), 1);
}

/// Profitability: revenue minus inputs minus pause friction plus
/// the trend bonus.
#[test]
fn profit_subtracts_costs_and_friction() {
    let plain = profit_per_6000(&[(300.0, 5.0)], 0.0, 0.0, 0.0, TREND_WEIGHT);
    assert!((plain - 1500.0).abs() < 0.001, "got {plain}");
    let loaded = profit_per_6000(&[(300.0, 5.0)], 200.0, 600.0, 0.5, TREND_WEIGHT);
    // 1500 revenue - 200 inputs - 150 friction (600 ticks at
    // 0.25/window-tick) + 75 bonus (0.5 x 1500 x 0.1).
    assert!((loaded - 1225.0).abs() < 0.001, "got {loaded}");
}

/// End to end on an in-memory store: price from the shop row,
/// profit aggregates the bucket.
#[test]
fn profit_prices_through_the_store() -> Result<(), lifescape_store::Error> {
    let store = Store::open_in_memory()?;
    store.upsert_item(&lifescape_store::Item {
        item_id: 251,
        name: "Raw shrimps".to_string(),
        shop_price: 5,
        alch_value: 3,
    })?;
    let mut snapshot = Snapshot::new();
    for _ in 0..3 {
        snapshot.record(&parsed_fishing(300, 6000, 3));
    }
    let rows = profit_rows(&store, &snapshot);
    assert_eq!(rows.len(), 1);
    assert!((rows[0].revenue - 1500.0).abs() < 0.001, "got {}", rows[0].revenue);
    assert!((rows[0].profit - 1500.0).abs() < 0.001, "no inputs, no pauses, no trend");
    assert!(rows[0].price_prior, "shop price is the cold-start prior, not a market print");
    Ok(())
}

/// Dashboard payload shape: ok flag, yield/xp/profit rows and the
/// operator-only engaged counts the page renders.
#[test]
fn payload_carries_rows_and_engaged() -> Result<(), lifescape_store::Error> {
    let store = Store::open_in_memory()?;
    store.upsert_item(&lifescape_store::Item {
        item_id: 251,
        name: "Raw shrimps".to_string(),
        shop_price: 5,
        alch_value: 3,
    })?;
    let mut snapshot = Snapshot::new();
    snapshot.record(&parsed_fishing(300, 6000, 8));
    let mut engaged = std::collections::BTreeMap::new();
    engaged.insert("fishing".to_string(), 1);
    let payload = rates_payload(&store, &snapshot, engaged);
    assert_eq!(payload.get("ok").as_bool(), Some(true));
    assert_eq!(payload.get("rates").as_arr().map(|rows| rows.len()), Some(1));
    assert_eq!(payload.get("xp").as_arr().map(|rows| rows.len()), Some(1));
    assert_eq!(payload.get("profit").as_arr().map(|rows| rows.len()), Some(1));
    assert!(payload.get("trading").as_arr().is_some(), "trading table rides along");
    assert_eq!(payload.get("engaged").get("fishing").as_i64(), Some(1));
    let row = &payload.get("rates").as_arr().expect("rows")[0];
    assert_eq!(row.get("method").as_str(), Some("fishing"));
    assert_eq!(row.get("prior").as_bool(), Some(true), "one report keeps the prior");
    Ok(())
}

/// D44: trading methods never enter yields or XP, whatever the
/// provenance; their P&L feeds the trading book instead.
#[test]
fn trading_methods_leave_yields_and_xp() {
    assert!(is_trading_method("market"), "today's flip script reports bare market");
    assert!(is_trading_method("shop"), "reserved bare shop script");
    assert!(is_trading_method("market:flip"));
    assert!(is_trading_method("shop:buy"));
    for farm in ["fishing", "woodcut", "mining", "melee", "cook", "gather:fishing/shrimp_anchovy"] {
        assert!(!is_trading_method(farm), "{farm} is not trading");
    }
    let mut snapshot = Snapshot::new();
    for method in ["market", "market:flip", "shop:buy"] {
        let mut market = parsed_fishing(300, 6000, 8);
        market.method = method.to_string();
        snapshot.record(&market);
        assert!(snapshot.yield_rows().is_empty(), "{method} yields nothing");
        assert!(snapshot.xp_rows().is_empty(), "{method} earns no XP");
    }
    assert!(!super::trading::trading_rows_without_store(&snapshot.trades).is_empty(), "trading book caught the P&L");
}

/// D45: unknown provenances fail loud at parse time (the contract is the
/// frozen ten); `unattributed`, `death` and `lost` parse but never rate.
#[test]
fn parse_rejects_unknown_provenance() {
    let mut report = fishing_push("produced");
    assert!(parse_report(&report).is_ok(), "produced parses");
    for known in ["unattributed", "death", "lost", "traded"] {
        set_provenance(&mut report, known);
        assert!(parse_report(&report).is_ok(), "{known} parses");
    }
    for unknown in ["picked", "dropped", "bogus", "PRODUCED"] {
        set_provenance(&mut report, unknown);
        assert!(parse_report(&report).is_err(), "{unknown} fails loud");
    }
}

/// D45: `unattributed` (and `death`/`lost`) lines never become yields,
/// whatever the method.
#[test]
fn unattributed_never_rates() {
    let mut snapshot = Snapshot::new();
    for provenance in ["unattributed", "death", "lost"] {
        let mut parsed = parsed_fishing(300, 6000, 8);
        parsed.items[0].provenance = provenance.to_string();
        snapshot.record(&parsed);
    }
    assert!(snapshot.yield_rows().is_empty(), "gaps and losses never rate");
}

/// D44 trading book: one buy run plus one sell run books the realized
/// coin P&L per 6000 ticks on the flipped item, with settled counts and
/// the residual stock valued at the estimated price.
#[test]
fn flip_books_profit_per_item() -> Result<(), lifescape_store::Error> {
    let store = Store::open_in_memory()?;
    store.upsert_item(&lifescape_store::Item {
        item_id: 314,
        name: "Feather".to_string(),
        shop_price: 3,
        alch_value: 1,
    })?;
    let mut snapshot = Snapshot::new();
    snapshot.record(&market_report(
        "market:flip",
        "budget_exhausted",
        vec![("Feather", "traded", 100), ("Coins", "traded", -200)],
    ));
    snapshot.record(&market_report(
        "market:flip",
        "budget_exhausted",
        vec![("Feather", "traded", -100), ("Coins", "traded", 260)],
    ));
    let rows = super::trading::trading_rows(&store, &snapshot.trades);
    assert_eq!(rows.len(), 1, "one flipped item, got {}", rows.len());
    assert_eq!(rows[0].item, "Feather");
    assert!((rows[0].profit_per_6000 - 30.0).abs() < 0.001, "got {}", rows[0].profit_per_6000);
    assert_eq!(rows[0].deals_settled, 2);
    assert!((rows[0].fill_rate - 1.0).abs() < 0.001);
    assert_eq!(rows[0].stock_value, 0, "round trip leaves no stock");
    Ok(())
}

/// Pure-coin windows (a same-run buy+sell nets the item to zero) inherit
/// the method's last stocked item; failed exits lower the fill rate and
/// leave the bought stock valued.
#[test]
fn pure_coin_reports_inherit_last_item() -> Result<(), lifescape_store::Error> {
    let store = Store::open_in_memory()?;
    store.upsert_item(&lifescape_store::Item {
        item_id: 314,
        name: "Feather".to_string(),
        shop_price: 3,
        alch_value: 1,
    })?;
    let mut snapshot = Snapshot::new();
    snapshot.record(&market_report(
        "market",
        "budget_exhausted",
        vec![("Feather", "traded", 100), ("Coins", "traded", -200)],
    ));
    snapshot.record(&market_report("market", "budget_exhausted", vec![("Coins", "traded", 60)]));
    snapshot.record(&market_report("market", "failed", vec![("Feather", "traded", 50), ("Coins", "traded", -120)]));
    let rows = super::trading::trading_rows(&store, &snapshot.trades);
    assert_eq!(rows.len(), 1);
    assert!((rows[0].profit_per_6000 - (-260.0 / 3.0)).abs() < 0.01, "got {}", rows[0].profit_per_6000);
    assert_eq!(rows[0].deals_settled, 2, "failed exit is not settled");
    assert!((rows[0].fill_rate - 2.0 / 3.0).abs() < 0.001);
    assert_eq!(rows[0].stock_value, 450, "150 feathers at 3 coins");
    Ok(())
}

/// Orphan coin P&L (no stocked item ever seen on the method) reads
/// `unattributed`: honest placeholder, never dropped silently.
#[test]
fn orphan_coin_profit_reads_unattributed() {
    let mut snapshot = Snapshot::new();
    snapshot.record(&market_report("market", "budget_exhausted", vec![("Coins", "traded", 60)]));
    let rows = super::trading::trading_rows_without_store(&snapshot.trades);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].item, "unattributed");
    assert!((rows[0].profit_per_6000 - 60.0).abs() < 0.001);
}

/// The store log re-cuts exact trading positions (ticks-free nets), even
/// when the snapshot file is gone: bought/sold raw quantities rebuild,
/// per-window samples do not (no ticks in the read API).
#[test]
fn trading_positions_rebuild_from_store() -> Result<(), lifescape_store::Error> {
    let store = Store::open_in_memory()?;
    let report_id = store.record_task_report(&lifescape_store::NewTaskReport {
        account: "t01".to_string(),
        node_id: "n3".to_string(),
        method: "market:flip".to_string(),
        script: "market".to_string(),
        ticks: 6000,
        stop: "budget_exhausted".to_string(),
        context_level: 1,
        clock: 1_700_000_000,
    })?;
    for (item, provenance, quantity) in [("Feather", "traded", 100), ("Coins", "traded", -200)] {
        store.record_task_yield(&lifescape_store::NewTaskYield {
            report_id,
            item_name: item.to_string(),
            provenance: provenance.to_string(),
            quantity,
            skill: String::new(),
            xp: 0,
        })?;
    }
    let rebuilt = super::trading::rebuild_from_store(&store).map_err(lifescape_store::Error::Invalid)?;
    let rows = super::trading::trading_rows_without_store(&rebuilt.trades);
    assert_eq!(rows.len(), 1, "position survives without the file");
    assert_eq!(rows[0].item, "Feather");
    assert_eq!(rows[0].stock_value, 100, "raw net position, exact without ticks");
    assert!(rows[0].profit_per_6000 == 0.0, "samples need ticks, not rebuilt");
    Ok(())
}

/// Pre-P1.4 snapshot files carry `market` rows in yields/inputs/pauses
/// (live file: 15 yield rows, 1 input, 3 pauses, xp clean): `record`
/// routes trading methods to the book only, so reload drops them —
/// the trading book rides along untouched.
#[test]
fn reload_drops_stale_trading_rows_but_keeps_the_book() {
    let bucket = || {
        object(vec![
            ("count", Value::Int(2)),
            ("sum", Value::Float(600.0)),
            ("sum_squares", Value::Float(180_000.0)),
            ("ema", Value::Float(300.0)),
            ("ordered", Value::Arr(vec![Value::Float(300.0), Value::Float(300.0)])),
        ])
    };
    let stat = || {
        object(vec![
            ("reports", Value::Int(2)),
            ("settled", Value::Int(1)),
            ("profits", Value::Arr(vec![Value::Float(60.0)])),
            ("bought", Value::Int(100)),
            ("sold", Value::Int(0)),
        ])
    };
    let text = lifescape_json::stringify(&object(vec![
        ("schema", Value::Int(2)),
        ("yields", Value::Obj([(String::from("fishing\u{1f}Raw shrimps\u{1f}6"), bucket()), (String::from("market\u{1f}Coal\u{1f}6"), bucket())].into_iter().collect())),
        ("xp", Value::Obj([(String::from("fishing\u{1f}fishing\u{1f}6"), bucket())].into_iter().collect())),
        ("inputs", Value::Obj([(String::from("market\u{1f}Fishing bait\u{1f}6"), bucket())].into_iter().collect())),
        ("pauses", Value::Obj([(String::from("market\u{1f}6"), bucket()), (String::from("fishing\u{1f}6"), bucket())].into_iter().collect())),
        ("trades", Value::Obj([(String::from("Feather"), stat())].into_iter().collect())),
        ("trade_last", Value::Obj([(String::from("market"), str_value("Feather"))].into_iter().collect())),
    ]));
    let reloaded = super::snapshot_from_json(&text).expect("stale file loads");
    assert_eq!(reloaded.yield_rows().len(), 1, "market yield row dropped");
    assert_eq!(reloaded.yield_rows()[0].method, "fishing");
    assert_eq!(reloaded.xp_rows().len(), 1);
    assert_eq!(reloaded.pause_per_6000("market", 6), 0.0, "market pause dropped");
    assert!(reloaded.pause_per_6000("fishing", 6) > 0.0, "fishing pause kept");
    let rows = super::trading::trading_rows_without_store(&reloaded.trades);
    assert_eq!(rows.len(), 1, "trading book survives the purge, got {rows:?}");
    assert_eq!(rows[0].item, "Feather");
}

/// Schema 1 files (pre-trading) still load with an empty book; schema 2
/// round-trips the trading positions.
#[test]
fn snapshot_file_keeps_schema1() {
    let legacy = lifescape_json::parse("{\"schema\":1,\"yields\":{},\"xp\":{},\"inputs\":{},\"pauses\":{}}").expect("parses");
    let legacy_text = lifescape_json::stringify(&legacy);
    let loaded = super::snapshot_from_json(&legacy_text).expect("schema 1 loads");
    assert!(super::trading::trading_rows_without_store(&loaded.trades).is_empty(), "no trades pre-P1.4");
    let mut snapshot = Snapshot::new();
    snapshot.record(&market_report(
        "market",
        "budget_exhausted",
        vec![("Feather", "traded", 100), ("Coins", "traded", -200)],
    ));
    let text = lifescape_json::stringify(&super::snapshot_to_value(&snapshot));
    let reloaded = super::snapshot_from_json(&text).expect("schema 2 reloads");
    assert!(!super::trading::trading_rows_without_store(&reloaded.trades).is_empty(), "trades survive a restart");
}

fn fishing_push(provenance: &str) -> Value {
    object(vec![
        ("account", str_value("t01")),
        ("activity", str_value("fishing")),
        ("node_id", str_value("n9")),
        ("stop", str_value("budget_exhausted")),
        ("ticks", Value::Int(6000)),
        ("market_pause_ticks", Value::Int(0)),
        (
            "xp",
            Value::Arr(vec![object(vec![
                ("stat", Value::Int(10)),
                ("skill", str_value("fishing")),
                ("xp", Value::Int(4000)),
            ])]),
        ),
        (
            "items",
            Value::Arr(vec![object(vec![
                ("name", str_value("Raw shrimps")),
                ("provenance", str_value(provenance)),
                ("qty", Value::Int(300)),
            ])]),
        ),
        (
            "levels_before",
            Value::Arr(vec![object(vec![("stat", Value::Int(10)), ("level", Value::Int(8))])]),
        ),
        ("levels_after", Value::Arr(vec![])),
    ])
}

fn set_provenance(report: &mut Value, provenance: &str) {
    if let Value::Obj(map) = report {
        if let Some(Value::Arr(items)) = map.get_mut("items") {
            if let Some(Value::Obj(first)) = items.first_mut() {
                first.insert("provenance".to_string(), str_value(provenance));
            }
        }
    }
}

fn market_report(method: &str, stop: &str, flows: Vec<(&str, &str, i64)>) -> ParsedReport {
    ParsedReport {
        account: "t01".to_string(),
        node_id: "n3".to_string(),
        method: method.to_string(),
        script: "market".to_string(),
        wall_ticks: 6000,
        pause_ticks: 0,
        stop: stop.to_string(),
        level: 1,
        band: 1,
        items: flows
            .into_iter()
            .map(|(name, provenance, quantity)| ParsedItem {
                name: name.to_string(),
                provenance: provenance.to_string(),
                quantity,
            })
            .collect(),
        xp: Vec::new(),
    }
}
