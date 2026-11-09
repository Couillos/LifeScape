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
/// consumed feeds inputs instead.
#[test]
fn provenance_filter_keeps_farm_only() {
    assert!(is_farm_provenance("produced"));
    assert!(is_farm_provenance("loot"));
    assert!(is_farm_provenance("ground_spawn"));
    assert!(is_farm_provenance("picked"));
    for excluded in ["shop_bought", "shop_sold", "traded", "consumed", "lost"] {
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
    assert_eq!(payload.get("engaged").get("fishing").as_i64(), Some(1));
    let row = &payload.get("rates").as_arr().expect("rows")[0];
    assert_eq!(row.get("method").as_str(), Some("fishing"));
    assert_eq!(row.get("prior").as_bool(), Some(true), "one report keeps the prior");
    Ok(())
}
