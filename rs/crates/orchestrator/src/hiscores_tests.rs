//! Hiscores: ranking, valuation, player selection, sources, schedule
//! and the cached endpoint (fixture snapshot, no daemon).

use std::collections::BTreeSet;

use lifescape_json::{self as json, Value};

use super::*;
use crate::hiscores_service as service;

fn holder(account: &str, xp: &[(usize, i64)], items: &[(i64, i64)]) -> Holder {
    let mut table = [0i64; 19];
    for (slot, amount) in xp {
        table[*slot] = *amount;
    }
    Holder { account: account.to_string(), iron: false, source: "live", xp: table, items: items.to_vec() }
}

fn build_with(holders: &[Holder], prices: &[(i64, i64)], market_up: bool) -> Value {
    let table: Vec<(i64, i64)> = prices.to_vec();
    let mut price_of = move |obj: i64| -> Option<i64> {
        if !market_up {
            return None;
        }
        Some(table.iter().find(|(id, _)| *id == obj).map(|(_, price)| *price).unwrap_or(0))
    };
    let item_name = |obj: i64| format!("obj {obj}");
    let display_name = |account: &str| account.to_uppercase();
    build(
        holders,
        Context { generated_at: 1000, next_at: 2800, price_of: &mut price_of, item_name: &item_name, display_name: &display_name, errors: Vec::new() },
    )
}

fn accounts_in_order(snapshot: &Value) -> Vec<String> {
    snapshot.get("players").as_arr().unwrap().iter().map(|row| row.get("account").as_str().unwrap().to_string()).collect()
}

fn ints(value: &Value) -> Vec<i64> {
    value.as_arr().unwrap().iter().map(|cell| cell.as_i64().unwrap()).collect()
}

#[test]
fn levels_come_from_display_xp() {
    assert_eq!(base_level(0), 1);
    assert_eq!(base_level(82), 1, "level 2 starts at 83 display XP");
    assert_eq!(base_level(83), 2);
    assert_eq!(base_level(1154), 10, "starting hitpoints");
    assert_eq!(base_level(13_034_431), 99);
    assert_eq!(base_level(200_000_000), 99);
    let player = holder("a", &[(0, 83), (3, 1154)], &[]);
    // 17 skills at 1, attack 2, hitpoints 10.
    assert_eq!(player.total_level(), 17 + 2 + 10);
    assert_eq!(player.total_xp(), 83 + 1154);
}

#[test]
fn skills_follow_the_hiscores_order() {
    assert_eq!(SKILLS.len(), 19);
    assert_eq!(skill_slot(0), Some(0));
    assert_eq!(skill_slot(17), Some(17));
    assert_eq!(skill_slot(18), None, "unused stat");
    assert_eq!(skill_slot(19), None, "unused stat");
    assert_eq!(skill_slot(20), Some(18), "runecraft last");
    assert_eq!(SKILLS[18].1, "runecraft");
}

#[test]
fn valuation_merges_stacks_and_prices_once() {
    let mut calls = Vec::new();
    let mut price_of = |obj: i64| {
        calls.push(obj);
        match obj {
            COINS => 1,
            1511 => 5,
            1205 => 40,
            _ => 0,
        }
    };
    let items = [(COINS, 100), (1511, 10), (COINS, 50), (1205, 1), (9999, 3), (1511, 0)];
    let (total, top) = valuation(&items, &mut price_of);
    assert_eq!(total, 150 + 50 + 40);
    assert_eq!(calls, vec![COINS, 1205, 1511, 9999], "one lookup per id");
    assert_eq!(top.iter().map(|item| (item.obj, item.count, item.value)).collect::<Vec<_>>(), vec![(COINS, 150, 150), (1511, 10, 50), (1205, 1, 40)]);
    assert!(top.iter().all(|item| item.value > 0), "worthless stacks are left out");
}

#[test]
fn valuation_keeps_the_ten_best() {
    let items: Vec<(i64, i64)> = (1..=15).map(|obj| (obj, obj)).collect();
    let (total, top) = valuation(&items, &mut |_| 2);
    assert_eq!(total, (1..=15).sum::<i64>() * 2);
    assert_eq!(top.len(), TOP_ITEMS);
    assert_eq!(top[0].obj, 15);
    assert_eq!(top[9].obj, 6);
}

#[test]
fn ranks_are_unique_and_break_ties_by_name() {
    let holders = vec![holder("carl", &[], &[]), holder("anna", &[], &[]), holder("bob", &[], &[])];
    let scores = [10, 10, 30];
    assert_eq!(ranks(&holders, |index| scores[index]), vec![3, 2, 1]);
}

#[test]
fn overall_is_total_level_then_total_xp() {
    // anna: 2 skills at level 2 (more total level, less XP);
    // bob: one big skill (more XP, less total level); carl ties bob's
    // level with less XP.
    let anna = holder("anna", &[(0, 83), (1, 83)], &[]);
    let bob = holder("bob", &[(0, 150)], &[]);
    let carl = holder("carl", &[(0, 100)], &[]);
    let snapshot = build_with(&[carl, bob, anna], &[], true);
    assert_eq!(accounts_in_order(&snapshot), vec!["anna", "bob", "carl"]);
    let first = &snapshot.get("players").as_arr().unwrap()[0];
    assert_eq!(ints(first.get("overall")), vec![1, 19 + 2, 166]);
    assert_eq!(first.get("name").as_str(), Some("ANNA"));
    // Attack ranking: bob 150 > carl 100 > anna 83.
    let attack_rank = |account: &str| {
        let row = snapshot.get("players").as_arr().unwrap().iter().find(|row| row.get("account").as_str() == Some(account)).unwrap().clone();
        ints(&row.get("skills").as_arr().unwrap()[0])
    };
    assert_eq!(attack_rank("bob"), vec![1, 2, 150]);
    assert_eq!(attack_rank("carl"), vec![2, 2, 100]);
    assert_eq!(attack_rank("anna"), vec![3, 2, 83]);
    assert_eq!(snapshot.get("skills").as_arr().unwrap().len(), 19);
    assert_eq!(snapshot.get("next_at").as_i64(), Some(2800));
}

#[test]
fn wealth_ranks_coins_and_total_value() {
    // anna: many coins, nothing else; bob: few coins and a rich bank;
    // carl: an untradeable (price 0) only.
    let anna = holder("anna", &[], &[(COINS, 500)]);
    let bob = holder("bob", &[], &[(COINS, 20), (1205, 30)]);
    let carl = holder("carl", &[], &[(4000, 1)]);
    let snapshot = build_with(&[anna, bob, carl], &[(1205, 40)], true);
    let row = |account: &str| snapshot.get("players").as_arr().unwrap().iter().find(|row| row.get("account").as_str() == Some(account)).unwrap().clone();
    assert_eq!(ints(row("anna").get("coins")), vec![1, 500]);
    assert_eq!(ints(row("bob").get("coins")), vec![2, 20]);
    assert_eq!(ints(row("carl").get("coins")), vec![3, 0]);
    assert_eq!(ints(row("bob").get("value")), vec![1, 20 + 1200]);
    assert_eq!(ints(row("anna").get("value")), vec![2, 500]);
    assert_eq!(ints(row("carl").get("value")), vec![3, 0]);
    let top = row("bob").get("top").as_arr().unwrap().clone();
    assert_eq!(top[0].get("id").as_i64(), Some(1205));
    assert_eq!(top[0].get("price").as_i64(), Some(40));
    assert_eq!(top[0].get("value").as_i64(), Some(1200));
    assert_eq!(top[1].get("id").as_i64(), Some(COINS), "coins count at 1");
    assert_eq!(snapshot.get("market").as_bool(), Some(true));
}

#[test]
fn market_down_leaves_value_unranked_and_says_so() {
    let anna = holder("anna", &[], &[(COINS, 500), (1205, 1)]);
    let snapshot = build_with(&[anna], &[], false);
    let first = &snapshot.get("players").as_arr().unwrap()[0];
    assert!(first.get("value").is_null());
    assert_eq!(ints(first.get("coins")), vec![1, 500], "coins never need the market");
    assert_eq!(snapshot.get("market").as_bool(), Some(false));
    assert!(snapshot.get("errors").as_arr().unwrap()[0].as_str().unwrap().contains("market down"));
}

#[test]
fn players_are_jobs_and_recent_lives_minus_exclusions() {
    let excluded: BTreeSet<String> = ["admin", "bite", "olivia_king"].iter().map(|name| name.to_string()).collect();
    let jobs = vec!["t01".to_string(), "admin".to_string(), "Ironxmt9nxcy".to_string()];
    let lives = vec![
        ("t01".to_string(), 5),
        ("leo_king".to_string(), 3600),
        ("t02".to_string(), LIFE_ACTIVE_SECONDS + 1),
        ("proof-a".to_string(), 10),
        ("olivia_king".to_string(), 10),
        ("ironxmt9nxcy".to_string(), 10),
    ];
    assert_eq!(service::select_accounts(&jobs, &lives, &excluded), vec!["Ironxmt9nxcy", "leo_king", "t01"]);
    assert!(service::is_excluded("proof_b", &excluded));
    assert!(service::is_excluded("ADMIN", &excluded));
    assert!(!service::is_excluded("t01", &excluded));
}

#[test]
fn live_snapshot_feeds_xp_and_stacks() {
    let live = json::parse(
        r#"{"skills":[{"id":0,"level":50,"xp":71203},{"id":18,"level":1,"xp":0},{"id":20,"level":3,"xp":200}],
            "inv":[{"id":995,"count":10,"name":"Coins"},{"id":347,"count":1}],
            "worn":[{"id":1139,"count":1}],
            "bank":[{"id":995,"count":1000},{"id":1617,"count":0}]}"#,
    )
    .unwrap();
    let player = service::holder_from_live("t01", true, &live, Some(vec![(1, 99)])).unwrap();
    assert_eq!(player.xp[0], 71203);
    assert_eq!(player.xp[18], 200, "runecraft slot");
    assert_eq!(player.coins(), 1010);
    assert!(player.items.contains(&(1139, 1)), "worn counts in the value");
    assert!(!player.items.contains(&(1, 99)), "a known bank ignores the save");
    assert!(player.iron);
    assert_eq!(player.source, "live");

    let unopened = json::parse(r#"{"skills":[{"id":0,"xp":10}],"inv":[],"worn":[],"bank":[]}"#).unwrap();
    let borrowed = service::holder_from_live("t01", false, &unopened, Some(vec![(COINS, 77)])).unwrap();
    assert_eq!(borrowed.coins(), 77, "unopened bank borrows the save's");

    let logging_in = json::parse(r#"{"skills":[],"inv":[]}"#).unwrap();
    assert!(service::holder_from_live("t01", false, &logging_in, None).is_none());
}

#[test]
fn save_xp_is_server_units() {
    let mut save = lifescape_sdk::save::Save::default();
    save.stats.insert(0, (40, 377_480));
    save.stats.insert(20, (5, 3880));
    save.stats.insert(18, (1, 999_999));
    save.inv.insert(995, 12);
    save.bank.insert(995, 30);
    save.worn.insert(1139, 1);
    let player = service::holder_from_save("t01", false, &save);
    assert_eq!(player.xp[0], 37_748);
    assert_eq!(base_level(player.xp[0]), 40);
    assert_eq!(player.xp[18], 388);
    assert_eq!(player.total_xp(), 37_748 + 388, "stat 18 is not ranked");
    assert_eq!(player.coins(), 42);
    assert_eq!(player.source, "save");
}

#[test]
fn first_snapshot_waits_for_the_restart_and_the_interval() {
    assert_eq!(service::first_due(10_000, None), 10_000 + START_DELAY_SECONDS);
    assert_eq!(service::first_due(10_000, Some(1_000)), 10_000 + START_DELAY_SECONDS, "old snapshot: soon after start");
    assert_eq!(service::first_due(10_000, Some(9_500)), 9_500 + INTERVAL_SECONDS, "fresh snapshot: not before 30 min");
}

/// One test owns the global cache: the persisted fixture is served by
/// the web routes as is (page, script and API), `next_at` rewritten.
#[test]
fn web_serves_the_cached_snapshot_and_the_page() {
    let dir = std::env::temp_dir().join(format!("lifescape-hiscores-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("hiscores.json");
    let snapshot = build_with(&[holder("anna", &[(0, 83)], &[(COINS, 5)])], &[], true);
    crate::paths::write_json(&path, &snapshot).unwrap();
    assert_eq!(service::load_persisted(&path, 1500), Some(1000));
    let dispatch: crate::web::Dispatch = std::sync::Arc::new(|_request: Value| Value::Null);
    let reply = crate::web::handle_get("/api/hiscores", "", &dispatch);
    assert_eq!(reply.status, 200);
    assert_eq!(reply.content_type, "application/json");
    let served = json::parse(&String::from_utf8(reply.body).unwrap()).unwrap();
    assert_eq!(accounts_in_order(&served), vec!["anna"]);
    assert_eq!(served.get("next_at").as_i64(), Some(service::first_due(1500, Some(1000))));
    for (route, kind, needle) in [("/hiscores", "text/html; charset=utf-8", "hiscores.js"), ("/hiscores.js", "application/javascript", "Hiscores")] {
        let page = crate::web::handle_get(route, "", &dispatch);
        assert_eq!(page.status, 200, "{route}");
        assert_eq!(page.content_type, kind);
        assert!(String::from_utf8_lossy(&page.body).contains(needle), "{route}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
