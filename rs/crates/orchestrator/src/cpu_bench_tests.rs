//! Offline CPU bench of the seller / brain market reads (ignored: needs
//! `LIFESCAPE_BENCH_ROOT`, a COPY of `run/` with lifescape.db, lives,
//! users and rates-snapshot.json — never the live directory).
//! `LIFESCAPE_BENCH_ROOT=/copy cargo test -p lifescape-orchestrator --lib
//! cpu_bench -- --ignored --nocapture`

use crate::cpu_profile::Probe;
use lifescape_json::{object, str_value, Value};

const ITEMS: &[&str] = &[
    "logs", "oak_logs", "raw_shrimps", "shrimps", "iron_ore", "copper_ore", "tin_ore", "bronze_bar", "iron_bar", "coal",
    "feather", "bones", "cowhide", "leather", "ball_of_wool",
];

fn timed<R>(label: &str, rounds: u32, mut work: impl FnMut() -> R) -> R {
    // One untimed warm-up call: lazy gamedata indexes build once per process.
    let _ = work();
    let probe = Probe::start();
    let mut last = None;
    for _ in 0..rounds {
        last = Some(work());
    }
    let (wall, cpu) = probe.elapsed();
    println!("{label:<40} {:>9.3} ms cpu/call {:>9.3} ms wall/call", cpu as f64 / 1e6 / f64::from(rounds), wall as f64 / 1e6 / f64::from(rounds));
    last.expect("one round")
}

#[test]
#[ignore]
fn cpu_bench() {
    let Ok(root) = std::env::var("LIFESCAPE_BENCH_ROOT") else { return };
    assert!(!root.contains("/home/josh/dev/LifeScape/run"), "bench on a copy only");
    std::env::set_var("LIFESCAPE_ROOT", &root);
    let service = crate::market_service::start().expect("market starts on the copy");
    let accounts: Vec<String> = std::fs::read_dir(crate::brain::lives_dir())
        .expect("lives")
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.strip_suffix(".json").map(str::to_string))
        .filter(|name| crate::paths::user_paths(name).0.exists())
        .take(4)
        .collect();
    timed("rates live_snapshot clone", 20, crate::rates_service::live_snapshot);
    {
        let store = lifescape_store::Store::open(&crate::market_service::db_path()).expect("store copy");
        let snapshot = crate::rates_service::live_snapshot();
        timed("store.list_items (old: once per price)", 20, || store.list_items().map(|rows| rows.len()).unwrap_or(0));
        timed("Store::open", 20, || lifescape_store::Store::open(&crate::market_service::db_path()).is_ok());
        // The brain reads its coin figures in the money guide (`brain_d17`):
        // one cycle built on the copy and published, as the daemon does.
        let set = timed("guides_store::build_set (one cycle)", 1, || crate::guides_store::build_set(&store, &snapshot, &crate::web_guides::StoreWaits::default()));
        crate::guides_store::publish(set);
    }
    {
        // Current price of every catalogue item (impact clamp 2026-11-14):
        // the live engine read and the store read of the rates.
        let items = service.lock_store().list_items().expect("items");
        println!("current price over {} items", items.len());
        let _ = items.iter().map(|item| service.reference(item.item_id).price).sum::<i64>();
        timed("reference(every item, rings warm)", 20, || items.iter().map(|item| service.reference(item.item_id).price).sum::<i64>());
        let store = lifescape_store::Store::open(&crate::market_service::db_path()).expect("store copy");
        timed("price_estimate(every item)", 5, || items.iter().map(|item| crate::rates_price::price_estimate(&store, &item.name).0).sum::<f64>());
    }
    for item in ["iron_bar", "logs", "bronze_bar", "cowhide", "coal", "steel_bar", "leather", "ball_of_wool", "shrimps", "bronze_dagger", "tin_ore"] {
        timed(&format!("production_legs({item})"), 50, || lifescape_planner::obtain_legs::production_legs(item).len());
    }
    let snapshot = crate::rates_service::live_snapshot();
    timed("units_per_6000(smelt iron_bar)", 200, || crate::brain_bids::units_per_6000(&snapshot, "smelt:iron_bar", "iron_bar"));
    timed("objs_by_name(iron_bar)", 2000, || lifescape_gamedata::objs_by_name("iron_bar").len());
    timed("tool_family(bronze_pickaxe)", 200, || lifescape_planner::tools::tool_family("bronze_pickaxe").len());
    for account in &accounts {
        println!("--- {account}");
        let life = crate::brain::load_life(account).expect("life");
        let live = crate::paths::read_json(&crate::paths::user_paths(account).0).unwrap_or(Value::Null);
        timed("MarketReader::live", 5, || crate::brain_bids::MarketReader::live(account));
        timed("OwnCosts::load", 5, || crate::brain_reserve::OwnCosts::load(&service.lock_store(), account, crate::brain_d17::coin_rate(account)));
        timed("brain_locks::locked_items", 5, || crate::brain_locks::locked_items(account));
        let all: Vec<(String, i64)> = ITEMS.iter().map(|item| (item.to_string(), 28)).collect();
        let costs = timed("production_costs_for(15 items)", 10, || crate::brain_bids::production_costs_for(&life, &live, &all));
        println!("  costs: {costs:?}");
        let ironish = { let mut copy = life.clone(); copy.profile.ironman = true; copy };
        timed("production_costs_for(15, as ironman)", 10, || crate::brain_bids::production_costs_for(&ironish, &live, &all).len());
        timed("live_count(logs)", 200, || crate::brain_ambition::live_count(&live, "logs"));
        timed("production_costs_for(iron_bar)", 20, || crate::brain_bids::production_costs_for(&life, &live, &[("iron_bar".to_string(), 28)]));
        timed("bid_goal_for", 10, || crate::brain_bids::bid_goal_for(&life, &live));
        let obtain = lifescape_planner::Node::new("bench", lifescape_planner::NodeKind::ObtainItems { item: "iron_bar".to_string(), count: 28 }, Vec::new(), 1);
        let money = lifescape_planner::Node::new("bench-money", lifescape_planner::NodeKind::MakeMoney, Vec::new(), 1);
        timed("ready_ctx(obtain iron_bar)", 5, || crate::brain_plan::ready_ctx(&life, &obtain, &lifescape_json::Value::Null));
        timed("ready_ctx(money)", 5, || crate::brain_plan::ready_ctx(&life, &money, &lifescape_json::Value::Null));
        let mut ctx = crate::brain_plan::ready_ctx(&life, &money, &lifescape_json::Value::Null);
        timed("fill_ctx(money)", 5, || crate::brain_market::fill_ctx(&mut ctx, &money, &live));
        timed("market_service locks_for(1)", 5, || service.locks_for(&[account.to_string()]));
        for item_id in [1511i64, 2349, 440] {
            let request = object(vec![("op", str_value("seller_quote")), ("account", str_value(account)), ("item", Value::Int(item_id)), ("qty", Value::Int(28))]);
            timed(&format!("op seller_quote {item_id} (no live)"), 5, || service.handle(&request));
            timed(&format!("public_view {item_id}"), 5, || service.public_view(account, item_id));
        }
    }
}
