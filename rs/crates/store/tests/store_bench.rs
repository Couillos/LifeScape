#![forbid(unsafe_code)]
//! Timing of the hot store paths on a COPY of the real database:
//! `LS_REAL_DB=<copy> cargo test --release --test store_bench -- --ignored --nocapture`.
//! Writes into the copy. Public API only, so it also runs on older builds.

use lifescape_store::*;
use std::time::Instant;

fn timed<T>(label: &str, runs: u32, mut work: impl FnMut() -> T) {
    let started = Instant::now();
    for _ in 0..runs {
        std::hint::black_box(work());
    }
    println!("{label:<52} {:>10.3} ms/run ({runs} runs)", started.elapsed().as_secs_f64() * 1000.0 / f64::from(runs));
}

#[test]
#[ignore]
fn store_bench() {
    let path = std::path::PathBuf::from(std::env::var("LS_REAL_DB").expect("LS_REAL_DB"));
    let started = Instant::now();
    let store = Store::open(&path).expect("open (migrates a v12 copy)");
    println!("first open (migration included if pending)           {:?}", started.elapsed());
    timed("Store::open (already migrated)", 20, || Store::open(&path).is_ok());
    timed("resting_orders", 5, || store.resting_orders().expect("rows").len());
    let resting = store.resting_orders().expect("rows");
    let account = resting.first().map(|order| order.account.clone()).unwrap_or_default();
    let item = resting.first().map(|order| order.item_id).unwrap_or(0);
    println!("resting rows: {}", resting.len());
    timed("account_orders", 20, || store.account_orders(&account).expect("rows").len());
    timed("open_book", 20, || store.open_book(item, OrderSide::Sell).expect("rows").len());
    timed("open_deals_for_account", 20, || store.open_deals_for_account(&account).expect("rows").len());
    timed("seller_learning", 20, || store.seller_learning(&account, 0).expect("rows").len());
    let newest = store.newest_report_clock().expect("clock").unwrap_or(0);
    timed("work_flows_since 6 h", 5, || store.work_flows_since(newest - 6 * 3600, &["traded", "unattributed"]).expect("rows").len());
    let ids: Vec<i64> = (1..=219_100).step_by(219).collect();
    let started = Instant::now();
    for id in &ids {
        let _ = store.report_measure(*id);
    }
    println!("report_measure, {} spread reports                  {:?}", ids.len(), started.elapsed());
    let started = Instant::now();
    let page = store.measures_after(100_000, 2000).expect("page");
    println!("measures_after (one page of 2000 reports)            {:?} ({} rows)", started.elapsed(), page.len());
    // One report write, as the ingest does it, for a real account and the
    // levels of a real measured report.
    let mut after = 200_000;
    let mut newest = None;
    loop {
        let more = store.measures_after(after, 5000).expect("page");
        let Some(tail) = more.last().cloned() else { break };
        after = tail.report_id;
        newest = Some(tail);
    }
    let last = &newest.expect("a measured report");
    let measure = last.measure.clone();
    let make = |store: &Store| -> i64 {
        let id = store
            .record_task_report(&NewTaskReport {
                account: last.account.clone(),
                node_id: "n".to_string(),
                method: "woodcut".to_string(),
                script: "woodcut".to_string(),
                ticks: 600,
                stop: "goal_reached".to_string(),
                context_level: 5,
                clock: 4_000_000_000,
            })
            .expect("report");
        for index in 0..8 {
            store
                .record_task_yield(&NewTaskYield { report_id: id, item_name: format!("Item {index}"), provenance: "produced".to_string(), quantity: 3, skill: String::new(), xp: 0 })
                .expect("yield");
        }
        store.record_report_measure(id, &measure).expect("measure");
        id
    };
    timed("report write, one statement per commit (old ingest)", 100, || make(&store));
    timed("report write, one transaction (new ingest)", 100, || {
        store.begin_batch().expect("batch");
        let id = make(&store);
        store.commit_batch().expect("commit");
        id
    });
    let id = make(&store);
    println!("levels of a written report read back equal: {}", store.report_measure(id).expect("read").map(|read| read.levels) == Some(measure.levels.clone()));
}
