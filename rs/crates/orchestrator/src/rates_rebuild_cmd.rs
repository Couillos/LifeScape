//! `orchestrator rates-rebuild <db> [--rows] [--guide money|training
//! [k=v&...]]` (plan rendements R2.1, R4): replays the
//! contract-2 reports of a store COPY through the live admission and
//! prints what the measures view would publish (keys, rows), touching
//! neither the daemon nor `run/`. Offline check of a migration: run it
//! on a copy of `run/lifescape.db`, never on the live file. `--guide`
//! builds a guide on the rebuilt estimator (the API's JSON, `web_guides`);
//! `--guides-store <dir> [money|training:query ...]` builds the whole
//! precomputed set as the `guides` thread does (`guides_store`), prints
//! its cost (wall, CPU, resident memory) and writes each read asked to
//! `<dir>/<n>.json` with its read time. `--view [query ...]` builds the
//! rates page index (`web_rates_view`) and answers each page query
//! (`group=key&q=..`) with its time and counts.

#![forbid(unsafe_code)]

/// CLI entry; returns the process exit code.
pub fn rates_rebuild_cli(args: &[String]) -> i32 {
    let Some(path) = args.first() else {
        eprintln!("usage: orchestrator rates-rebuild <copy of lifescape.db> [--rows] [--guide money|training [skill=..&level=..]] [--guides-store <dir> [money|training:query ...]] [--view [query ...]]");
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
    if let Some(at) = args.iter().position(|arg| arg == "--view") {
        return view_check(&store, &snapshot, args.get(at + 1..).unwrap_or(&[]));
    }
    if let Some(at) = args.iter().position(|arg| arg == "--guides-store") {
        return guides_store_check(&store, &snapshot, args.get(at + 1..).unwrap_or(&[]));
    }
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

/// Resident memory of the process in KiB (Linux `/proc/self/status`).
fn resident_kib() -> Option<i64> {
    let text = std::fs::read_to_string("/proc/self/status").ok()?;
    text.lines().find_map(|line| line.strip_prefix("VmRSS:")).and_then(|rest| rest.split_whitespace().next()).and_then(|value| value.parse().ok())
}

/// `--guides-store`: one set built and read as the daemon does.
fn guides_store_check(store: &lifescape_store::Store, snapshot: &crate::rates_service::Snapshot, args: &[String]) -> i32 {
    let Some(dir) = args.first() else {
        eprintln!("rates-rebuild: --guides-store <dir> [money|training:query ...]");
        return 2;
    };
    let waits_began = std::time::Instant::now();
    let mut tally = crate::guides_store::WaitsTally::default();
    if let Err(error) = tally.advance(store) {
        eprintln!("rates-rebuild: {error}");
        return 1;
    }
    let waits_ms = waits_began.elapsed().as_millis();
    let before = resident_kib().unwrap_or(0);
    let set = crate::guides_store::build_set(store, snapshot, &tally.waits());
    let after = resident_kib().unwrap_or(0);
    println!(
        "guides store: waits {waits_ms} ms (full tally), set {} ms wall, {} ms CPU, resident +{} MiB ({} -> {} MiB), {} quotes",
        set.cycle_ms,
        set.cycle_cpu_ms.map(|cpu| cpu.to_string()).unwrap_or_else(|| "?".to_string()),
        (after - before) / 1024,
        before / 1024,
        after / 1024,
        set.quotes.len()
    );
    println!("guides store stats: {}", lifescape_json::stringify(&set.stats));
    // The daemon's steady cycle: the process memos of the static game
    // data (priors' tables, tool tiers, XP, shops) already filled.
    drop(set);
    let set = crate::guides_store::build_set(store, snapshot, &tally.waits());
    println!(
        "guides store (second cycle, warm): set {} ms wall, {} ms CPU, resident {} MiB",
        set.cycle_ms,
        set.cycle_cpu_ms.map(|cpu| cpu.to_string()).unwrap_or_else(|| "?".to_string()),
        resident_kib().unwrap_or(0) / 1024
    );
    // The set's size: every view as the API serializes it (its rows in
    // memory hold about as much).
    let mut views: Vec<(crate::web_guides::GuideKind, lifescape_planner::guide_model::GuideFilter)> = vec![(crate::web_guides::GuideKind::Money, Default::default()), (crate::web_guides::GuideKind::Training, Default::default())];
    for level in crate::guides_store::band_levels() {
        for kind in [crate::web_guides::GuideKind::Money, crate::web_guides::GuideKind::Training] {
            views.push((kind, lifescape_planner::guide_model::GuideFilter { level: Some(level), ..Default::default() }));
        }
    }
    let bytes: usize = views.iter().map(|(kind, filter)| lifescape_json::stringify(&crate::web_guides::read_guide(&set, *kind, filter)).len()).sum();
    println!("guides store: {} views, {} MiB as JSON", views.len(), bytes / (1024 * 1024));
    let incremental = std::time::Instant::now();
    if let Err(error) = tally.advance(store) {
        eprintln!("rates-rebuild: {error}");
        return 1;
    }
    println!("guides store: incremental waits {} ms (no new report)", incremental.elapsed().as_millis());
    for (index, spec) in args.iter().skip(1).enumerate() {
        let (kind, query) = spec.split_once(':').unwrap_or((spec.as_str(), ""));
        let kind = if kind == "training" { crate::web_guides::GuideKind::Training } else { crate::web_guides::GuideKind::Money };
        let began = std::time::Instant::now();
        let filter = crate::web_guides::filter_of(&crate::web::parse_query(query));
        let value = crate::web_guides::read_guide(&set, kind, &filter);
        let text = lifescape_json::stringify(&value);
        let spent = began.elapsed().as_millis();
        let path = std::path::Path::new(dir).join(format!("{index}.json"));
        if let Err(error) = std::fs::write(&path, &text) {
            eprintln!("rates-rebuild: {}: {error}", path.display());
            return 1;
        }
        println!("read {spec}: {spent} ms (read + JSON, {} KiB) -> {}", text.len() / 1024, path.display());
    }
    0
}

/// `--view`: the rates page index built as the daemon does, then each
/// page query answered (time, counts, JSON size).
fn view_check(store: &lifescape_store::Store, snapshot: &crate::rates_service::Snapshot, queries: &[String]) -> i32 {
    let began = std::time::Instant::now();
    let index = crate::web_rates_view::index_of(store, snapshot);
    println!("rates view: index of {} keys built in {} ms", index.entries.len(), began.elapsed().as_millis());
    let defaults = [String::new(), "measured=1".to_string(), "group=key".to_string(), "group=key&measured=1&sort=net_value_6000".to_string(), "q=chickens&group=key".to_string()];
    let asked: &[String] = if queries.is_empty() { &defaults } else { queries };
    for query in asked {
        let began = std::time::Instant::now();
        let fields = crate::web_rates_view::activities_value(&index, &crate::web_rates_view::view_query_of(&crate::web::parse_query(query)));
        let value = lifescape_json::object(fields);
        let text = lifescape_json::stringify(&value);
        println!(
            "view '{query}': {:.2} ms (page + JSON, {} KiB), {} rows shown, {} matching of {}",
            began.elapsed().as_secs_f64() * 1000.0,
            text.len() / 1024,
            value.get("rows").as_arr().map(|rows| rows.len()).unwrap_or(0),
            value.get("matched").as_i64().unwrap_or(0),
            value.get("total").as_i64().unwrap_or(0)
        );
        let Some(first) = value.get("rows").as_arr().and_then(|rows| rows.first().cloned()) else { continue };
        let began = std::time::Instant::now();
        let mut params = std::collections::HashMap::new();
        for field in ["method", "style", "site", "tool"] {
            if let Some(text) = first.get(field).as_str() {
                params.insert(field.to_string(), text.to_string());
            }
        }
        if first.get("site").as_str().is_some() {
            params.insert("band".to_string(), first.get("band").as_i64().unwrap_or(0).to_string());
        }
        let detail = lifescape_json::stringify(&lifescape_json::object(crate::web_rates_view::detail_value(&index, &params)));
        println!("  detail of its first row: {:.2} ms ({} KiB)", began.elapsed().as_secs_f64() * 1000.0, detail.len() / 1024);
    }
    0
}
