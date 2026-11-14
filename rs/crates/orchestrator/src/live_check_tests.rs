//! Offline check against a COPY of the live data (2026-11-14, gear plan,
//! producers answering bids, alchemy). Ignored by default; run with
//!
//! ```text
//! LIFESCAPE_CHECK_DB=<copy of run/lifescape.db> \
//! LIFESCAPE_CHECK_STATUS=<saved /api/status JSON> \
//! cargo test -p lifescape-orchestrator live_check -- --ignored --nocapture
//! ```
//!
//! For every bot of the status: the gear it wants (style of its life's
//! goal, 50 % of its coins, expected prices = median of the last five
//! fills of the copy, else the high alchemy value) and the bids they
//! open at; then, on the copy's book plus those bids, the bid each bot
//! would answer (training included, own time at 1 coin per tick, the
//! cold-start rates) and its best alchemy. Read-only on the copy.

use std::collections::HashMap;

use lifescape_gamedata::gear::Style;
use lifescape_json::Value;
use lifescape_planner::bid_response::{opportunity, preempts, Producer};
use lifescape_planner::market_fit::BookView;

fn debug_names(job: &Value, key: &str) -> Vec<(String, i64)> {
    job.get(key)
        .as_arr()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| Some((crate::brain_plan::live_debug_name(entry)?, entry.get("count").as_i64().unwrap_or(1))))
        .collect()
}

fn sim_units(method: &str, _item: &str) -> f64 {
    if method.starts_with("farm:") || method.starts_with("gather:") {
        crate::brain_bids::GATHER_UNITS_PER_6000
    } else {
        crate::brain_bids::MAKE_UNITS_PER_6000
    }
}

#[test]
#[ignore]
fn live_check() {
    let (Ok(db), Ok(status)) = (std::env::var("LIFESCAPE_CHECK_DB"), std::env::var("LIFESCAPE_CHECK_STATUS")) else {
        eprintln!("live_check: set LIFESCAPE_CHECK_DB and LIFESCAPE_CHECK_STATUS");
        return;
    };
    let store = lifescape_store::Store::open(std::path::Path::new(&db)).expect("db copy opens");
    let text = std::fs::read_to_string(&status).expect("status json");
    let root = lifescape_json::parse(&text).expect("status parses");
    let jobs = root.get("jobs").as_arr().cloned().unwrap_or_default();
    // The copy's resting book by debug name.
    let mut books: HashMap<String, BookView> = HashMap::new();
    let mut owners: Vec<(String, String, bool, i64, i64)> = Vec::new();
    for order in store.resting_orders().expect("orders") {
        let Some(name) = lifescape_gamedata::obj_by_id(order.item_id as usize).and_then(|obj| obj.name) else { continue };
        let name = name.strip_prefix("cert_").unwrap_or(name).to_string();
        owners.push((name, order.account.clone(), order.side == lifescape_store::OrderSide::Buy, order.price, order.remaining));
    }
    let price_of = |item: &str| -> i64 {
        let traded = store.item_id_by_name(&crate::brain_reserve::display_of(item)).ok().flatten().and_then(|id| lifescape_market::median_price(&store.last_prices(id, 5).unwrap_or_default()));
        traded.unwrap_or_else(|| lifescape_gamedata::objs_by_name(item).into_iter().next().map(|obj| obj.alch_high).unwrap_or(1)).max(1)
    };
    eprintln!("== gear each combat bot wants (budget = 50 % of its coins) ==");
    for job in &jobs {
        let account = job.get("account").as_str().unwrap_or("?").to_string();
        let levels_owned = crate::brain_market::live_levels(job);
        let levels: Vec<(&str, i64)> = levels_owned.iter().map(|(name, level)| (name.as_str(), i64::from(*level))).collect();
        let mut stacks = debug_names(job, "inv");
        stacks.extend(debug_names(job, "bank"));
        let coins: i64 = stacks.iter().filter(|(name, _)| name == "coins").map(|(_, qty)| qty).sum();
        let mut owned: Vec<String> = stacks.iter().map(|(name, _)| name.clone()).collect();
        owned.extend(debug_names(job, "worn").into_iter().map(|(name, _)| name));
        // Lives read from `LIFESCAPE_CHECK_LIVES` (read-only), else melee.
        let style = std::env::var("LIFESCAPE_CHECK_LIVES")
            .ok()
            .and_then(|dir| std::fs::read_to_string(std::path::Path::new(&dir).join(format!("{account}.json"))).ok())
            .and_then(|text| lifescape_json::parse(&text).ok())
            .and_then(|value| lifescape_planner::life_from_value(&value))
            .map(|life| crate::brain_gear::goal_style(&life))
            .unwrap_or(Style::Melee);
        let budget = lifescape_planner::gear_ladder::wallet_ceiling(coins, lifescape_planner::gear_ladder::GEAR_SHARE_PCT);
        let wants = lifescape_planner::gear_plan::plan_gear(style, &levels, &[], &owned, budget, &price_of);
        let shown: Vec<String> = wants.iter().map(|want| format!("{}@{}..{}", want.item, want.expected, want.ceiling)).collect();
        eprintln!("{account:<12} {style:?} coins {coins:>6} budget {budget:>6}: {}", shown.join(" "));
        for want in wants {
            owners.push((want.item.clone(), account.clone(), true, want.expected, 1));
        }
    }
    for (item, _, buy, price, remaining) in &owners {
        let view = books.entry(item.clone()).or_default();
        if *buy {
            view.bids.push((*price, *remaining));
        } else {
            view.asks.push((*price, *remaining));
        }
    }
    eprintln!("== bids each bot would answer (book + gear bids, training allowed) ==");
    for job in &jobs {
        let account = job.get("account").as_str().unwrap_or("?").to_string();
        let others: HashMap<String, BookView> = crate::brain_bids::views_from(&owners, &account);
        let market = |item: &str| others.get(item).cloned().or_else(|| books.get(item).map(|view| BookView { bids: Vec::new(), ..view.clone() }));
        let level_of = |skill: &str| crate::brain_succession::live_level(job, skill);
        let held = |item: &str| crate::brain_ambition::live_count(job, item);
        let avoid: Vec<String> = Vec::new();
        let producer = Producer { level_of: &level_of, held: &held, avoid: &avoid, quests_done: &[], coin_rate: 1.0, units_per_6000: &sim_units, ironman: false, seed: 1, tick: 0 };
        let mut found: Vec<String> = Vec::new();
        for (item, view) in others.iter().filter(|(_, view)| !view.bids.is_empty()) {
            if let Some(chance) = opportunity(item, view, &market, &producer) {
                if preempts(&chance, producer.coin_rate) {
                    found.push(format!("{} x{} @{} ({:.1}/tick, net {:.0}, train {:?})", chance.item, chance.count, chance.best_bid, chance.rate, chance.profit, chance.unit.train));
                }
            }
        }
        found.sort();
        let mage = lifescape_planner::alchemy::caster(level_of("magic"), held("staff_of_fire") > 0, &|rune| price_of(rune) as f64);
        let alch = mage.map(|mage| format!("caster {} (runes {:.0}/cast)", mage.spell.spell, mage.rune_cost)).unwrap_or_else(|| format!("no alchemy (magic {})", level_of("magic")));
        eprintln!("{account:<12} {alch}; answers: {}", if found.is_empty() { "-".to_string() } else { found.join(" | ") });
    }
}
