//! `orchestrator rates-rebuild <db> [--rows] [--guide money|training
//! [k=v&...]]` (plan rendements R2.1, R4): replays the
//! contract-2 reports of a store COPY through the live admission and
//! prints what the measures view would publish (keys, rows), touching
//! neither the daemon nor `run/`. Offline check of a migration: run it
//! on a copy of `run/lifescape.db`, never on the live file. `--guide`
//! builds a guide on the rebuilt estimator (the API's JSON, `web_guides`).

#![forbid(unsafe_code)]

/// CLI entry; returns the process exit code.
pub fn rates_rebuild_cli(args: &[String]) -> i32 {
    let Some(path) = args.first() else {
        eprintln!("usage: orchestrator rates-rebuild <copy of lifescape.db> [--rows] [--guide money|training [skill=..&level=..]]");
        return 2;
    };
    let path = std::path::PathBuf::from(path);
    if path == crate::market_service::db_path() {
        eprintln!("rates-rebuild: {} is the live store; copy it first", path.display());
        return 1;
    }
    let store = match lifescape_store::Store::open(&path) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("rates-rebuild: store open: {error}");
            return 1;
        }
    };
    let snapshot = match crate::rates_service::rebuild_from_store(&store) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!("rates-rebuild: {error}");
            return 1;
        }
    };
    let book = snapshot.book();
    let exact = book.buckets.keys().filter(|key| !key.is_rollup()).count();
    let thin = book.buckets.iter().filter(|(key, bucket)| !key.is_rollup() && (bucket.runs as usize) < crate::rates_service::MIN_MEASURED_REPORTS).count();
    let mut activities: Vec<(&str, &str)> = book.buckets.keys().filter(|key| !key.is_rollup()).map(|key| (key.method.as_str(), key.style.as_str())).collect();
    activities.sort_unstable();
    activities.dedup();
    let mut prices: std::collections::BTreeMap<String, f64> = std::collections::BTreeMap::new();
    let mut price = |item: &str| *prices.entry(item.to_string()).or_insert_with(|| crate::rates_service::price_estimate(&store, item).0);
    let (flows, units, xp) = crate::rates_service::measure_rows(&snapshot, crate::rates_service::now_secs(), &mut price);
    let folded = flows.iter().filter(|row| row.get("item").as_str().is_some_and(|item| item.starts_with(lifescape_planner::guide_flows::OTHER_DROPS))).count();
    println!("buckets: {} ({exact} measured keys, {thin} under n>=3, {} internal roll-ups)", book.buckets.len(), book.buckets.len() - exact);
    println!("activities (method, style): {}", activities.len());
    println!("measure rows: {} flows ({folded} folded 'other drops' lines), {} units, {} xp", flows.len(), units.len(), xp.len());
    if let Some(at) = args.iter().position(|arg| arg == "--guide") {
        let kind = match args.get(at + 1).map(String::as_str) {
            Some("training") => crate::web_guides::GuideKind::Training,
            _ => crate::web_guides::GuideKind::Money,
        };
        let filter = crate::web_guides::filter_of(&crate::web::parse_query(args.get(at + 2).map(String::as_str).unwrap_or("")));
        match crate::web_guides::build_guide(&store, &snapshot, kind, &filter) {
            Ok(guide) => println!("{}", lifescape_json::stringify(&guide)),
            Err(error) => {
                eprintln!("rates-rebuild: guide: {error}");
                return 1;
            }
        }
        return 0;
    }
    if args.iter().any(|arg| arg == "--rows") {
        for row in &flows {
            let text = |key: &str| row.get(key).as_str().unwrap_or("").to_string();
            let number = |key: &str| row.get(key).as_f64().unwrap_or(0.0);
            println!("{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.2}\t{:.1}", text("method"), text("style"), text("site"), text("tool"), row.get("band").as_i64().unwrap_or(0), row.get("runs").as_i64().unwrap_or(0), text("item") + "|" + &text("cause"), number("per_6000"), number("value_6000"));
        }
    }
    0
}
