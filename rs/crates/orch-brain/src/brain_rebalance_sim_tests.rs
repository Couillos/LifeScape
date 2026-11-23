//! Offline simulation of the A4 rebalance on a COPY of the live state
//! (never the live files): `SIM_LIVES` (a copy of `run/lives`), `SIM_DB`
//! (a copy of `lifescape.db`), `LIFESCAPE_SAVE_DIR` (engine saves, read
//! only). Run alone: `cargo test --release -p lifescape-orchestrator
//! simulate_rebalance -- --ignored --nocapture`.

use lifescape_planner::{rebalance, ValueContext};

#[test]
#[ignore]
fn simulate_rebalance_on_the_live_state() {
    let lives_dir = std::path::PathBuf::from(std::env::var("SIM_LIVES").expect("SIM_LIVES"));
    let db = std::path::PathBuf::from(std::env::var("SIM_DB").expect("SIM_DB"));
    let store = lifescape_store::Store::open(&db).expect("store copy");
    let excluded = crate::economy_guard::excluded_accounts();
    let mut bots = Vec::new();
    let mut names: Vec<_> =
        std::fs::read_dir(&lives_dir).expect("lives").flatten().map(|entry| entry.path()).filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json")).collect();
    names.sort();
    for path in names {
        let Some(life) = crate::brain::read_life_file(&path) else { continue };
        if excluded.contains(&life.account.to_lowercase()) {
            continue;
        }
        let levels = crate::brain_ambition::account_levels(&life.account, None);
        bots.push(crate::brain_ambition::fleet_bot(&life, levels));
    }
    println!("fleet: {} bots", bots.len());
    let orders = store.resting_orders().expect("orders");
    let holdings = store.holdings_at_or_before(i64::MAX).expect("holdings").map(|(_, rows)| rows).unwrap_or_default();
    let surplus = crate::market_demand::fleet_surplus_from(&holdings, &orders, &crate::market_demand::name_of);
    let volumes = store.volumes_since(crate::market_service::MarketService::now_clock() - 86_400).expect("volumes");
    let days = lifescape_planner::investment::INVESTMENT_HORIZON_TICKS * 60.0 / 86_400_000.0;
    let inputs = crate::market_demand::value_inputs_from(&orders, None, &surplus, &volumes, days, &|wanted: &[String]| crate::market_demand::price_table(&store, wanted));
    println!("unserved bids: {}, alch offers: {}, prices: {}", inputs.bids.len(), inputs.alch_offers.len(), inputs.prices.len());
    let rate = |method: &str, level: u8| lifescape_planner::work_rate::prior_rate(method, level);
    let price = |item: &str| inputs.price_of(item);
    let context = ValueContext { bids: &inputs.bids, alch_offers: &inputs.alch_offers, surplus: &surplus, price: &price, xp_rate: &lifescape_planner::investment::best_xp_rate, rate: &rate, roster: None, targets: &[] };
    let moved = rebalance(&bots, &context);
    let mut before: std::collections::BTreeMap<String, usize> = Default::default();
    let mut after: std::collections::BTreeMap<String, usize> = Default::default();
    for bot in &bots {
        *before.entry(bot.kind.clone().unwrap_or_default()).or_default() += 1;
    }
    for bot in &bots {
        let kind = moved.iter().find(|row| row.account == bot.account).map(|row| row.pick.kind.clone()).or_else(|| bot.kind.clone()).unwrap_or_default();
        *after.entry(kind).or_default() += 1;
    }
    for bot in &bots {
        match moved.iter().find(|row| row.account == bot.account) {
            Some(row) => println!("ROW {} | {} | {} | {}", row.account, row.from.clone().unwrap_or_default(), row.pick.kind, row.pick.numbers()),
            None => println!("KEPT {} | {} | protected or operator", bot.account, bot.kind.clone().unwrap_or_default()),
        }
    }
    let changed = moved.iter().filter(|row| row.from.as_deref() != Some(row.pick.kind.as_str())).count();
    let by_value = moved.iter().filter(|row| row.pick.value_6000 > 0.0).count();
    let kept = moved.iter().filter(|row| row.pick.kept).count();
    println!("COUNTS movable {} | changed {changed} | kept by margin {kept} | by value {by_value} | by tie-break {}", moved.len(), moved.len() - by_value);
    for row in moved.iter().filter(|row| ["firemaker", "priest", "cook", "ranger", "warrior", "fisher", "mage"].contains(&row.pick.kind.as_str())).take(40) {
        println!("CONSUMER {} | {} | {}", row.account, row.pick.kind, row.pick.numbers());
    }
    println!("BEFORE {before:?}");
    println!("AFTER  {after:?}");
}
