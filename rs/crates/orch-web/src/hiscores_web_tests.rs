//! The hiscores web routes (cached endpoint and page), split out of
//! `hiscores_tests` in step B5 (2026-11-23): the stats layer cannot reach
//! `web`; the test follows the web layer (B8).

use lifescape_json::{self as json, Value};

use crate::hiscores::{build, Context, Holder, COINS};
use crate::hiscores_service as service;

/// One test owns the global cache: the persisted fixture is served by
/// the web routes as is (page, script and API), `next_at` rewritten.
#[test]
fn web_serves_the_cached_snapshot_and_the_page() {
    let dir = std::env::temp_dir().join(format!("lifescape-hiscores-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("hiscores.json");
    let mut xp = [0i64; 19];
    xp[0] = 83;
    let anna = Holder { account: "anna".to_string(), iron: false, source: "live", xp, items: vec![(COINS, 5)] };
    let table: Vec<(i64, i64)> = Vec::new();
    let mut price_of = move |obj: i64| -> Option<i64> { Some(table.iter().find(|(id, _)| *id == obj).map(|(_, price)| *price).unwrap_or(0)) };
    let item_name = |obj: i64| format!("obj {obj}");
    let display_name = |account: &str| account.to_uppercase();
    let snapshot = build(&[anna], Context { generated_at: 1000, next_at: 2800, price_of: &mut price_of, item_name: &item_name, display_name: &display_name, errors: Vec::new() });
    crate::paths::write_json(&path, &snapshot).unwrap();
    assert_eq!(service::load_persisted(&path, 1500), Some(1000));
    let dispatch: crate::web::Dispatch = std::sync::Arc::new(|_request: Value| Value::Null);
    let reply = crate::web::handle_get("/api/hiscores", "", &dispatch);
    assert_eq!(reply.status, 200);
    assert_eq!(reply.content_type, "application/json");
    let served = json::parse(&String::from_utf8(reply.body).unwrap()).unwrap();
    let accounts: Vec<String> = served.get("players").as_arr().unwrap().iter().map(|row| row.get("account").as_str().unwrap().to_string()).collect();
    assert_eq!(accounts, vec!["anna"]);
    assert_eq!(served.get("next_at").as_i64(), Some(service::first_due(1500, Some(1000))));
    for (route, kind, needle) in [("/hiscores", "text/html; charset=utf-8", "hiscores.js"), ("/hiscores.js", "application/javascript", "Hiscores")] {
        let page = crate::web::handle_get(route, "", &dispatch);
        assert_eq!(page.status, 200, "{route}");
        assert_eq!(page.content_type, kind);
        assert!(String::from_utf8_lossy(&page.body).contains(needle), "{route}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
