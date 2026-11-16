use super::*;
use lifescape_json::{object, str_value, Value};
use lifescape_store::Store;

#[path = "rates_service_contract_tests.rs"]
mod contract_tests;

/// Records a report without a contract (trading, contract 1).
fn record_parsed(snapshot: &mut Snapshot, parsed: &ParsedReport) {
    snapshot.record(parsed, None, 1);
}

pub(super) fn parsed_fishing(quantity: i64, ticks: i64, level: i64) -> ParsedReport {
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
    record_parsed(&mut snapshot, &market_report(
        "market:flip",
        "budget_exhausted",
        vec![("Feather", "traded", 100), ("Coins", "traded", -200)],
    ));
    record_parsed(&mut snapshot, &market_report(
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
    record_parsed(&mut snapshot, &market_report(
        "market",
        "budget_exhausted",
        vec![("Feather", "traded", 100), ("Coins", "traded", -200)],
    ));
    record_parsed(&mut snapshot, &market_report("market", "budget_exhausted", vec![("Coins", "traded", 60)]));
    record_parsed(&mut snapshot, &market_report("market", "failed", vec![("Feather", "traded", 50), ("Coins", "traded", -120)]));
    let rows = super::trading::trading_rows(&store, &snapshot.trades);
    assert_eq!(rows.len(), 1);
    assert!((rows[0].profit_per_6000 - (-260.0 / 3.0)).abs() < 0.01, "got {}", rows[0].profit_per_6000);
    assert_eq!(rows[0].deals_settled, 2, "failed exit is not settled");
    assert!((rows[0].fill_rate - 2.0 / 3.0).abs() < 0.001);
    assert_eq!(rows[0].stock_value, 150, "150 untraded feathers at 1 coin");
    Ok(())
}

/// Orphan coin P&L (no stocked item ever seen on the method) reads
/// `unattributed`: honest placeholder, never dropped silently.
#[test]
fn orphan_coin_profit_reads_unattributed() {
    let mut snapshot = Snapshot::new();
    record_parsed(&mut snapshot, &market_report("market", "budget_exhausted", vec![("Coins", "traded", 60)]));
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

