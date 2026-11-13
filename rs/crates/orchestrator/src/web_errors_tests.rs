use super::*;

const NOW: i64 = 1_794_565_628;

fn scratch_run(tag: &str) -> std::path::PathBuf {
    let run = std::env::temp_dir().join(format!("lifescape-errors-web-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&run);
    std::fs::create_dir_all(run.join("failures")).unwrap();
    run
}

fn body(reply: &Response) -> Value {
    json::parse(&String::from_utf8(reply.body.clone()).expect("utf8 body")).expect("json body")
}

fn seed(run: &Path) {
    let store = Store::open(&run.join("lifescape.db")).unwrap();
    let mut nav = ErrorRecord {
        clock: NOW - 600,
        source: "nav".to_string(),
        kind: "NoRoute".to_string(),
        account: "t01".to_string(),
        signature: "travel: no route (#,#,#)".to_string(),
        message: "travel: no route (1,2,0)".to_string(),
        position: Some((3200, 3420, 0)),
        zone: "Varrock Square".to_string(),
        dossier: "t01-55.json".to_string(),
        source_ref: Some("nav:t01-55.json:1".to_string()),
        ..ErrorRecord::default()
    };
    let task = ErrorRecord {
        clock: NOW - 300,
        source: "task".to_string(),
        kind: "Failed".to_string(),
        account: "t02".to_string(),
        script: "fishing".to_string(),
        node_id: "n4".to_string(),
        signature: "failed: fish failed at (#,#,#)".to_string(),
        message: "failed: fish failed at (0, 1, 2)".to_string(),
        ..ErrorRecord::default()
    };
    let old = ErrorRecord { clock: NOW - 26 * 3600, source_ref: None, ..nav.clone() };
    store.record_errors(&[nav.clone(), task, old]).unwrap();
    nav.source_ref = Some("nav:t01-56.json:1".to_string());
    nav.clock = NOW - 30 * 3600;
    store.record_errors(&[nav]).unwrap();
    store
        .record_task_report(&lifescape_store::NewTaskReport {
            account: "t02".to_string(),
            node_id: "n4".to_string(),
            method: "fishing".to_string(),
            script: "fishing".to_string(),
            ticks: 5,
            stop: "failed".to_string(),
            context_level: 1,
            clock: NOW - 300,
        })
        .unwrap();
}

#[test]
fn percent_decoding() {
    assert_eq!(percent_decode("Varrock+Square"), "Varrock Square");
    assert_eq!(percent_decode("a%20b%2Cc"), "a b,c");
    assert_eq!(percent_decode("%C3%A9t%C3%A9"), "été");
    assert_eq!(percent_decode("100%"), "100%");
    assert_eq!(percent_decode("%zz"), "%zz");
}

#[test]
fn filter_parsing() {
    let (filter, hours) = parse_filter("", NOW).unwrap();
    assert_eq!(hours, 24);
    assert_eq!(filter.until, NOW);
    assert_eq!(filter.since, NOW - 24 * 3600);
    assert_eq!(filter.limit, 200);
    let (filter, hours) = parse_filter("hours=6&type=NoRoute&zone=Varrock+Square&account=t01", NOW).unwrap();
    assert_eq!(hours, 6);
    assert_eq!(filter.kind, "NoRoute");
    assert_eq!(filter.zone, "Varrock Square");
    assert_eq!(filter.account, "t01");
    let (filter, hours) = parse_filter("since=100&until=7300", NOW).unwrap();
    assert_eq!((filter.since, filter.until, hours), (100, 7300, 2));
    assert!(parse_filter("hours=0", NOW).is_err());
    assert!(parse_filter("hours=999", NOW).is_err());
    assert!(parse_filter("limit=abc", NOW).is_err());
    assert!(parse_filter("since=10&until=5", NOW).is_err());
}

#[test]
fn summary_shape() {
    let run = scratch_run("summary");
    seed(&run);
    let reply = errors_api(&["summary"], "hours=24", &run, NOW);
    assert_eq!(reply.status, 200);
    let payload = body(&reply);
    assert_eq!(payload.get("ok").as_bool(), Some(true), "{}", json::stringify(&payload));
    for key in ["since", "until", "hours", "kpis", "hourly", "by_source", "by_kind", "by_account", "by_script", "by_zone", "groups", "scripts", "options"] {
        assert!(payload.as_obj().unwrap().contains_key(key), "missing {key}");
    }
    let kpis = payload.get("kpis");
    assert_eq!(kpis.get("total").as_i64(), Some(2));
    assert_eq!(kpis.get("previous_total").as_i64(), Some(2));
    assert_eq!(kpis.get("trend_pct").as_f64(), Some(0.0));
    assert_eq!(kpis.get("runs_failed").as_i64(), Some(1));
    assert_eq!(kpis.get("run_failure_rate").as_f64(), Some(1.0));
    assert_eq!(kpis.get("deal_failure_rate"), &Value::Null, "no deal, no rate");
    assert_eq!(payload.get("hourly").as_arr().map(Vec::len), Some(25), "24 h window spans 25 hour slots");
    assert_eq!(payload.get("groups").as_arr().map(Vec::len), Some(2));
    // A filter narrows the KPIs, never the options.
    let filtered = body(&errors_api(&["summary"], "hours=24&source=nav", &run, NOW));
    assert_eq!(filtered.get("kpis").get("total").as_i64(), Some(1));
    assert_eq!(filtered.get("kpis").get("previous_total").as_i64(), Some(2));
    assert_eq!(filtered.get("options").get("sources").as_arr().map(Vec::len), Some(2));
    let bad = body(&errors_api(&["summary"], "hours=0", &run, NOW));
    assert_eq!(bad.get("ok").as_bool(), Some(false));
    let _ = std::fs::remove_dir_all(&run);
}

#[test]
fn list_shape_and_dossier() {
    let run = scratch_run("list");
    seed(&run);
    let payload = body(&errors_api(&["list"], "hours=24&zone=Varrock%20Square", &run, NOW));
    assert_eq!(payload.get("total").as_i64(), Some(1));
    let rows = payload.get("rows").as_arr().unwrap();
    let first = &rows[0];
    for key in ["id", "clock", "time", "source", "kind", "account", "script", "node_id", "signature", "message", "pos", "zone", "dossier"] {
        assert!(first.as_obj().unwrap().contains_key(key), "missing {key}");
    }
    assert_eq!(first.get("time").as_str(), Some("2026-11-13 10:17:08"));
    assert_eq!(first.get("pos").as_arr().unwrap()[0].as_i64(), Some(3200));
    let limited = body(&errors_api(&["list"], "hours=48&limit=1", &run, NOW));
    assert_eq!(limited.get("rows").as_arr().map(Vec::len), Some(1));
    assert_eq!(limited.get("total").as_i64(), Some(4));
    std::fs::write(run.join("failures").join("t01-55.json"), r#"{"account":"t01"}"#).unwrap();
    let dossier = errors_api(&["dossier", "t01-55.json"], "", &run, NOW);
    assert_eq!(dossier.status, 200);
    assert_eq!(body(&dossier).get("account").as_str(), Some("t01"));
    assert_eq!(errors_api(&["dossier", "t01-99.json"], "", &run, NOW).status, 404);
    assert_eq!(body(&errors_api(&["dossier", "..json"], "", &run, NOW)).get("ok").as_bool(), Some(false));
    assert_eq!(body(&errors_api(&["nope"], "", &run, NOW)).get("ok").as_bool(), Some(false));
    let _ = std::fs::remove_dir_all(&run);
}

#[test]
fn dossier_names_are_strict() {
    assert!(dossier_name_ok("fiona_park-39560.json"));
    assert!(!dossier_name_ok("../x.json"));
    assert!(!dossier_name_ok("a/b.json"));
    assert!(!dossier_name_ok(".hidden.json"));
    assert!(!dossier_name_ok("x.txt"));
}
