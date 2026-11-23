#![forbid(unsafe_code)]
//! Tests of migration 13 (`levels.rs`): the lossless delta of
//! `task_levels` and the index plans of the hot queries.

use crate::{schema, Error, NewTaskReport, ReportMeasure, Store};

type Levels = Vec<(&'static str, i64)>;

fn owned(levels: &Levels) -> Vec<(String, i64)> {
    levels.iter().map(|(skill, level)| (skill.to_string(), *level)).collect()
}

/// Report sequence exercising: identical report, one changed level,
/// zero levels, a new skill, a subset of skills, the full set again,
/// and a second account interleaved.
fn scenario() -> Vec<(&'static str, Levels)> {
    vec![
        ("ann", vec![("attack", 1), ("magic", 5)]),
        ("bob", vec![("attack", 9), ("magic", 9)]),
        ("ann", vec![("attack", 1), ("magic", 5)]),
        ("ann", vec![("attack", 2), ("magic", 5)]),
        ("ann", vec![]),
        ("bob", vec![("attack", 9), ("magic", 10), ("prayer", 1)]),
        ("ann", vec![("attack", 2), ("magic", 5), ("prayer", 1)]),
        ("ann", vec![("magic", 6)]),
        ("ann", vec![("attack", 2), ("magic", 6), ("prayer", 1)]),
        ("bob", vec![("attack", 9), ("magic", 10), ("prayer", 1)]),
        ("ann", vec![("attack", 3), ("magic", 6), ("prayer", 1)]),
    ]
}

fn new_report(store: &Store, account: &str) -> Result<i64, Error> {
    store.record_task_report(&NewTaskReport {
        account: account.to_string(),
        node_id: "n".to_string(),
        method: "m".to_string(),
        script: "s".to_string(),
        ticks: 1,
        stop: "goal_reached".to_string(),
        context_level: 1,
        clock: 1,
    })
}

fn level_rows(store: &Store) -> Result<i64, Error> {
    Ok(store.connection.query_row("SELECT COUNT(*) FROM task_levels", [], |row| row.get(0))?)
}

/// Reports recorded through the store read back exactly, and only
/// changed or new levels are stored.
#[test]
fn delta_levels_read_back_exactly() -> Result<(), Error> {
    let store = Store::open_in_memory()?;
    let mut ids = Vec::new();
    for (account, levels) in scenario() {
        let id = new_report(&store, account)?;
        store.record_report_measure(id, &ReportMeasure { levels: owned(&levels), ..ReportMeasure::default() })?;
        ids.push(id);
    }
    for (id, (_, levels)) in ids.iter().zip(scenario()) {
        assert_eq!(store.report_measure(*id)?.expect("measure 2").levels, owned(&levels), "report {id}");
    }
    let full: usize = scenario().iter().map(|(_, levels)| levels.len()).sum();
    assert!((level_rows(&store)? as usize) < full, "a delta stores fewer rows than the full sets");
    Ok(())
}

/// A report recorded after a newer report of its account (out of
/// order) leaves the newer one exact.
#[test]
fn out_of_order_report_keeps_newer_reports_exact() -> Result<(), Error> {
    let store = Store::open_in_memory()?;
    let first = new_report(&store, "ann")?;
    let second = new_report(&store, "ann")?;
    let third = new_report(&store, "ann")?;
    let full: Levels = vec![("attack", 1), ("magic", 5)];
    store.record_report_measure(first, &ReportMeasure { levels: owned(&full), ..ReportMeasure::default() })?;
    store.record_report_measure(third, &ReportMeasure { levels: owned(&full), ..ReportMeasure::default() })?;
    let changed: Levels = vec![("attack", 7), ("magic", 5), ("prayer", 3)];
    store.record_report_measure(second, &ReportMeasure { levels: owned(&changed), ..ReportMeasure::default() })?;
    assert_eq!(store.report_measure(first)?.expect("m").levels, owned(&full));
    assert_eq!(store.report_measure(second)?.expect("m").levels, owned(&changed));
    assert_eq!(store.report_measure(third)?.expect("m").levels, owned(&full), "newer report not altered by the older one's rows");
    Ok(())
}

/// A report listing a skill twice still fails loud, as before.
#[test]
fn duplicate_skill_fails_loud() -> Result<(), Error> {
    let store = Store::open_in_memory()?;
    let id = new_report(&store, "ann")?;
    let twice: Levels = vec![("attack", 1), ("attack", 2)];
    assert!(store.record_report_measure(id, &ReportMeasure { levels: owned(&twice), ..ReportMeasure::default() }).is_err());
    Ok(())
}

/// A v12 file (full per-report level rows) migrates with every
/// report's levels identical, the zero-level and subset reports
/// included.
#[test]
fn levels_migration_is_lossless() -> Result<(), Error> {
    let connection = rusqlite::Connection::open_in_memory()?;
    connection.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
    schema::pending_steps_upto(&connection, 0, 12)?;
    connection.execute_batch("COMMIT")?;
    assert_eq!(schema::current_version(&connection)?, 12);
    let mut expected = Vec::new();
    for (index, (account, levels)) in scenario().iter().enumerate() {
        let id = index as i64 + 1;
        connection.execute(
            "INSERT INTO task_reports(report_id, account, node_id, method, script, ticks, stop, context_level, created_clock, measure_version) \
             VALUES (?1, ?2, 'n', 'm', 's', 1, 'goal_reached', 1, 1, 2)",
            rusqlite::params![id, account],
        )?;
        for (skill, level) in levels {
            connection.execute("INSERT INTO task_levels(report_id, skill, level) VALUES (?1, ?2, ?3)", rusqlite::params![id, skill, level])?;
        }
        connection.execute("INSERT INTO task_units(report_id, unit, subject, quantity) VALUES (?1, 'product', 'Logs', 3)", [id])?;
        expected.push((id, owned(levels)));
    }
    schema::apply_migrations(&connection)?;
    let store = Store { connection };
    assert_eq!(store.schema_version()?, schema::SCHEMA_VERSION);
    for (id, levels) in expected {
        let measure = store.report_measure(id)?.expect("measure 2");
        assert_eq!(measure.levels, levels, "report {id}");
        assert_eq!(measure.units, vec![("product".to_string(), "Logs".to_string(), 3)], "units kept");
    }
    let full: usize = scenario().iter().map(|(_, levels)| levels.len()).sum();
    assert!((level_rows(&store)? as usize) < full);
    // The next report of the account continues the delta chain.
    let next = new_report(&store, "ann")?;
    let wanted: Levels = vec![("attack", 3), ("magic", 7), ("prayer", 1)];
    store.record_report_measure(next, &ReportMeasure { levels: owned(&wanted), ..ReportMeasure::default() })?;
    assert_eq!(store.report_measure(next)?.expect("m").levels, owned(&wanted));
    assert_eq!(store.report_measure(11)?.expect("m").levels, owned(&scenario()[10].1), "older report unchanged");
    Ok(())
}

/// Every hot store query runs on an index: a full `SCAN orders` /
/// `SCAN deals` / `SCAN task_reports` fails (212 ms per resting-order
/// scan under the market lock, 2026-11-23). Keep these strings equal
/// to the ones in orders.rs, deals.rs, prices.rs and work_gains.rs.
#[test]
fn hot_queries_never_scan_a_whole_table() -> Result<(), Error> {
    let store = Store::open_in_memory()?;
    let cols = "order_id, account, side, item_id, quantity, remaining, price, status, dormant, created_clock, updated_clock";
    let deal_cols = "deal_id, buyer, seller, item_id, quantity, price, status, created_clock, updated_clock";
    let queries: Vec<(&str, String, usize)> = vec![
        ("open book buy", format!("SELECT {cols} FROM orders WHERE item_id = ?1 AND side = ?2 AND status = 'open' AND dormant = 0 ORDER BY price DESC, created_clock ASC"), 2),
        ("book levels", "SELECT price, remaining FROM orders WHERE item_id = ?1 AND status = 'open' AND side = 'sell' AND dormant = 0 AND remaining > 0 ORDER BY price ASC, created_clock ASC LIMIT ?2".to_string(), 2),
        ("account orders", format!("SELECT {cols} FROM orders WHERE account = ?1 AND status IN ('open','matched') ORDER BY created_clock"), 1),
        ("resting orders", format!("SELECT {cols} FROM orders WHERE status IN ('open','matched') AND dormant = 0 ORDER BY created_clock"), 0),
        ("criteria book", "SELECT account, side, item_id, price, remaining FROM orders WHERE status IN ('open','matched') AND remaining > 0 AND dormant = 0".to_string(), 0),
        ("unsold stock", "SELECT o.account, o.item_id, SUM(o.remaining) FROM orders o WHERE o.side = 'sell' AND o.status IN ('open','matched') AND o.remaining > 0 AND o.dormant = 0 GROUP BY o.account, o.item_id".to_string(), 0),
        ("horizon", "SELECT item_id, COUNT(*) FROM orders WHERE account = ?1 AND status = 'cancelled' AND side = 'sell' AND cancel_reason = 'horizon' AND updated_clock >= ?2 GROUP BY item_id".to_string(), 2),
        ("fail and relist deals", format!("SELECT {deal_cols} FROM deals WHERE status NOT IN ('settled','failed') ORDER BY created_clock, deal_id"), 0),
        ("open deals of account", format!("SELECT {deal_cols} FROM deals WHERE (buyer = ?1 OR seller = ?1) AND status NOT IN ('settled','failed') ORDER BY created_clock"), 1),
        ("settled fills", "SELECT price, quantity, buyer, seller FROM deals WHERE item_id = ?1 AND status = 'settled' AND updated_clock >= ?2 AND buyer <> ?3 AND seller <> ?3 ORDER BY updated_clock DESC LIMIT ?4".to_string(), 4),
        ("report by id", "SELECT measure_version, account, levels_mode FROM task_reports WHERE report_id = ?1".to_string(), 1),
        // `account_reports` walks the rowid backwards and stops at its LIMIT (about
        // 76 rows per wanted one): no index, and none that would not hijack the
        // per-account GROUP BY of the window sums into a whole-index scan.
        ("report list by clock", crate::work_gains::WORK_TICKS_SQL.to_string(), 2),
        ("report flows by clock", crate::work_gains::WORK_FLOWS_SQL.to_string(), 2),
        ("report levels", "SELECT skill, level, MAX(report_id) FROM task_levels WHERE account = ?1 AND report_id <= ?2 GROUP BY skill ORDER BY skill".to_string(), 2),
    ];
    for (name, sql, parameters) in queries {
        let mut statement = store.connection.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
        let values: Vec<rusqlite::types::Value> = (0..parameters).map(|_| rusqlite::types::Value::Integer(1)).collect();
        let details: Vec<String> = statement.query_map(rusqlite::params_from_iter(values), |row| row.get::<_, String>(3))?.collect::<Result<_, _>>()?;
        for detail in &details {
            let bare_scan = detail.starts_with("SCAN ") && !detail.contains("USING");
            let table = detail.trim_start_matches("SCAN ").split_whitespace().next().unwrap_or("");
            assert!(!(bare_scan && ["orders", "o", "deals", "task_reports", "task_levels"].contains(&table)), "{name}: full scan: {detail}");
        }
    }
    Ok(())
}

/// Real-data proof, by hand on a copy of the v12 database:
/// `LS_REAL_DB=<copy> cargo test --release real_data_levels_check -- --ignored --nocapture`.
/// Keeps the old rows in `proof_levels`, migrates, and compares every
/// report's levels.
#[test]
#[ignore]
fn real_data_levels_check() -> Result<(), Error> {
    let path = std::path::PathBuf::from(std::env::var("LS_REAL_DB").map_err(|_| Error::Invalid("LS_REAL_DB unset".to_string()))?);
    {
        let raw = rusqlite::Connection::open(&path)?;
        if schema::current_version(&raw)? <= 12 {
            raw.execute_batch(
                "DROP TABLE IF EXISTS proof_levels; CREATE TABLE proof_levels AS SELECT report_id, skill, level FROM task_levels; \
                 CREATE INDEX proof_levels_report ON proof_levels(report_id, skill);",
            )?;
        }
    }
    let started = std::time::Instant::now();
    let store = Store::open(&path)?;
    println!("migration + open: {:?}", started.elapsed());
    let ids: Vec<i64> = {
        let mut statement = store.connection.prepare("SELECT report_id FROM task_reports ORDER BY report_id")?;
        let rows = statement.query_map([], |row| row.get::<_, i64>(0))?;
        rows.collect::<Result<_, _>>()?
    };
    let (mut compared, mut empty, mut mismatches, mut none) = (0, 0, 0, 0);
    let started = std::time::Instant::now();
    for id in &ids {
        let old: Vec<(String, i64)> = {
            let mut statement = store.connection.prepare_cached("SELECT skill, level FROM proof_levels WHERE report_id = ?1 ORDER BY skill")?;
            let rows = statement.query_map([id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;
            rows.collect::<Result<_, _>>()?
        };
        match store.report_measure(*id)? {
            None => {
                none += 1;
                assert!(old.is_empty(), "measure-1 report {id} had levels");
            }
            Some(measure) => {
                compared += 1;
                empty += usize::from(old.is_empty());
                if measure.levels != old {
                    mismatches += 1;
                    println!("MISMATCH report {id}");
                }
            }
        }
    }
    println!("reports {} measure2 {compared} (no level rows: {empty}) measure1 {none} mismatches {mismatches}; read-back {:?}", ids.len(), started.elapsed());
    let proof_total: i64 = store.connection.query_row("SELECT COUNT(*) FROM proof_levels", [], |row| row.get(0))?;
    println!("old rows {proof_total}, new rows {}", level_rows(&store)?);
    assert_eq!(mismatches, 0);
    Ok(())
}
