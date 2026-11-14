//! Offline check against a COPY of the live data (2026-11-14, gear plan,
//! the market-wide evaluation, derived input bids, ambition weights).
//! Ignored by default; run with
//!
//! ```text
//! LIFESCAPE_CHECK_DB=<copy of run/lifescape.db> \
//! LIFESCAPE_CHECK_STATUS=<saved /api/status JSON> \
//! LIFESCAPE_CHECK_LIVES=<run/lives> \
//! LIFESCAPE_CHECK_RATES=<copy of run/rates-snapshot.json> \
//! cargo test -p lifescape-orchestrator live_check -- --ignored --nocapture
//! ```
//!
//! For every bot of the status: the gear it wants (style of its life's
//! goal, 50 % of its coins, expected prices = median of the last five
//! fills of the copy, else the high alchemy value) and the bids they
//! open at; then, on the copy's book plus those bids, the lot each bot
//! would make (the planner's `choose`: every valued object, its own coin
//! rate from the copy's published profits, the snapshot's rates, its
//! position, its ambition's levels; claims of the bots before it on the
//! board), the input bids that lot would post (derived demand), and the
//! fleet's ambition weights. Read-only on the copies.

use std::collections::HashMap;

use lifescape_gamedata::gear::Style;
use lifescape_json::Value;

use crate::brain_bids::{BotState, MarketReader};

fn debug_names(job: &Value, key: &str) -> Vec<(String, i64)> {
    job.get(key)
        .as_arr()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| Some((crate::brain_plan::live_debug_name(entry)?, entry.get("count").as_i64().unwrap_or(1))))
        .collect()
}

fn life_of(account: &str) -> Option<lifescape_planner::Life> {
    let dir = std::env::var("LIFESCAPE_CHECK_LIVES").ok()?;
    let text = std::fs::read_to_string(std::path::Path::new(&dir).join(format!("{account}.json"))).ok()?;
    lifescape_planner::life_from_value(&lifescape_json::parse(&text).ok()?)
}

fn debug_of(item_id: i64) -> Option<String> {
    let name = lifescape_gamedata::obj_by_id(usize::try_from(item_id).ok()?)?.name?;
    Some(name.strip_prefix("cert_").unwrap_or(name).to_string())
}

/// Current price of every traded object of the copy (median of the last
/// five fills clamped by the impact prices of the copy's book) and its
/// 24 h volume.
fn prices_of(store: &lifescape_store::Store, owners: &[(String, String, bool, i64, i64)]) -> HashMap<String, (i64, i64)> {
    let clock = crate::market_service::MarketService::now_clock();
    let mut out = HashMap::new();
    for item_id in store.items_with_executions().unwrap_or_default() {
        let Some(name) = debug_of(item_id) else { continue };
        let fills: Vec<lifescape_market::LastFill> =
            store.last_fills(item_id, 5).unwrap_or_default().into_iter().map(|(price, quantity)| lifescape_market::LastFill { price, quantity }).collect();
        let mut bids: Vec<(i64, i64)> = owners.iter().filter(|row| row.0 == name && row.2).map(|row| (row.3, row.4)).collect();
        let mut asks: Vec<(i64, i64)> = owners.iter().filter(|row| row.0 == name && !row.2).map(|row| (row.3, row.4)).collect();
        bids.sort_by(|left, right| right.0.cmp(&left.0));
        asks.sort_by_key(|(price, _)| *price);
        let reference = lifescape_market::current_price(&fills, &bids, &asks);
        let volume = store.volume_between(item_id, clock - 86_400, clock + 1).unwrap_or(0);
        if reference.traded {
            let entry = out.entry(name).or_insert((reference.price, 0));
            entry.1 += volume;
        }
    }
    out
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
    let snapshot = std::env::var("LIFESCAPE_CHECK_RATES")
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| crate::rates_service::snapshot_from_json(&text))
        .unwrap_or_default();
    let published: Vec<(String, i64, f64)> = crate::rates_service::profit_rows(&store, &snapshot).into_iter().map(|row| (row.method, row.band, row.profit)).collect();
    // The copy's resting book by debug name.
    let mut owners: Vec<(String, String, bool, i64, i64)> = Vec::new();
    for order in store.resting_orders().expect("orders") {
        let Some(name) = debug_of(order.item_id) else { continue };
        owners.push((name, order.account.clone(), order.side == lifescape_store::OrderSide::Buy, order.price, order.remaining));
    }
    let traded = prices_of(&store, &owners);
    // The coin rates read the realizable profits (fleet view of the book).
    let mut fleet = crate::brain_bids::views_from(&owners, "");
    for (item, (price, volume)) in &traded {
        let view = fleet.entry(item.clone()).or_default();
        view.current = Some(*price);
        view.volume_24h = *volume;
    }
    let rows = crate::brain_bids::realizable_rows(&published, &snapshot.yield_rows(), &|item| fleet.get(item).cloned());
    for (label, table) in [("published", &published), ("realizable", &rows)] {
        let mut best = table.clone();
        best.sort_by(|left, right| right.2.total_cmp(&left.2));
        eprintln!("== best {label} profits (script, band, per 6000): {:?}", best.iter().take(6).map(|(script, band, profit)| format!("{script}/{band} {profit:.0}")).collect::<Vec<_>>());
    }
    let price_of = |item: &str| -> i64 {
        traded.get(item).map(|(price, _)| *price).unwrap_or_else(|| lifescape_gamedata::objs_by_name(item).into_iter().next().map(|obj| obj.alch_high).unwrap_or(1)).max(1)
    };
    eprintln!("== gear each bot wants (budget = 50 % of its coins) ==");
    for job in &jobs {
        let account = job.get("account").as_str().unwrap_or("?").to_string();
        let levels_owned = crate::brain_market::live_levels(job);
        let levels: Vec<(&str, i64)> = levels_owned.iter().map(|(name, level)| (name.as_str(), i64::from(*level))).collect();
        let mut stacks = debug_names(job, "inv");
        stacks.extend(debug_names(job, "bank"));
        let coins: i64 = stacks.iter().filter(|(name, _)| name == "coins").map(|(_, qty)| qty).sum();
        let mut owned: Vec<String> = stacks.iter().map(|(name, _)| name.clone()).collect();
        owned.extend(debug_names(job, "worn").into_iter().map(|(name, _)| name));
        let style = life_of(&account).map(|life| crate::brain_gear::goal_style(&life)).unwrap_or(Style::Melee);
        let budget = lifescape_planner::gear_ladder::wallet_ceiling(coins, lifescape_planner::gear_ladder::GEAR_SHARE_PCT);
        let wants = lifescape_planner::gear_plan::plan_gear(style, &levels, &[], &owned, budget, &price_of);
        let shown: Vec<String> = wants.iter().map(|want| format!("{}@{}..{}", want.item, want.expected, want.ceiling)).collect();
        eprintln!("{account:<12} {style:?} coins {coins:>6} budget {budget:>6}: {}", shown.join(" "));
        for want in wants {
            owners.push((want.item.clone(), account.clone(), true, want.expected, 1));
        }
    }
    eprintln!("== lots each bot would make (whole market, own rate, claims of the bots before it) ==");
    let mut claims: HashMap<String, i64> = HashMap::new();
    let mut picked: HashMap<String, usize> = HashMap::new();
    for job in &jobs {
        let account = job.get("account").as_str().unwrap_or("?").to_string();
        let Some(life) = life_of(&account) else { continue };
        let mut views = crate::brain_bids::views_from(&owners, &account);
        for (item, (price, volume)) in &traded {
            let view = views.entry(item.clone()).or_default();
            view.current = Some(*price);
            view.volume_24h = *volume;
        }
        for (item, units) in &claims {
            views.entry(item.clone()).or_default().claimed = *units;
        }
        let reader = MarketReader::fixed(views);
        let level_of = |skill: &str| crate::brain_succession::live_level(job, skill);
        let held = |item: &str| crate::brain_ambition::live_count(job, item);
        let units = |method: &str, item: &str| crate::brain_bids::units_per_6000(&snapshot, method, item);
        let position = job.get("position").as_arr().and_then(|cells| {
            let part = |slot: usize| cells.get(slot).and_then(|cell| cell.as_i64());
            Some((part(1)?, part(2)?, part(0)?))
        });
        let caster = lifescape_planner::alchemy::caster(level_of("magic"), held("staff_of_fire") > 0, &|rune| price_of(rune) as f64);
        let state = BotState {
            level_of: &level_of,
            held: &held,
            avoid: Vec::new(),
            quests_done: lifescape_planner::quests_done_in_graph(&life),
            units: &units,
            tick: 0,
            combat: crate::brain_plan::live_combat(job),
            position,
            planned: crate::brain_bids::planned_levels(&life),
            caster,
        };
        let rate = lifescape_planner::market_fit::coin_rate(&rows, state.level_of);
        // Every valued lot (spread ignored), best rate first; `*` preempts.
        let mut lots: Vec<lifescape_planner::bid_response::Opportunity> = state.with_producer(&life, rate, |producer| {
            let market = |item: &str| reader.view(item);
            reader
                .value_books(producer.alch_net)
                .iter()
                .filter_map(|(item, view)| lifescape_planner::bid_response::opportunity(item, view, &market, producer))
                .collect()
        });
        lots.sort_by(|left, right| right.rate.total_cmp(&left.rate));
        // One object traced for every bot (`LIFESCAPE_CHECK_ITEM`).
        if let Ok(traced) = std::env::var("LIFESCAPE_CHECK_ITEM") {
            state.with_producer(&life, rate, |producer| {
                let market = |item: &str| reader.view(item);
                let view = reader.view(&traced).unwrap_or_default();
                let unit = lifescape_planner::bid_response::unit_cost_training(&traced, 3, &market, producer);
                let found = lifescape_planner::bid_response::opportunity(&traced, &view, &market, producer);
                eprintln!("{account:<12}   trace {traced}: view {view:?}\n{:<14} unit {unit:?}\n{:<14} lot {found:?}", "", "");
            });
        }
        let kind = life.ambition.as_ref().map(|ambition| ambition.kind.clone()).unwrap_or_default();
        let shown: Vec<String> = lots
            .iter()
            .take(3)
            .map(|lot| {
                format!(
                    "{}{} x{} (bids {} best {}, sales {}, alch {}; rev {:.0} spent {:.0} ticks {:.0} → {:.2}/tick net {:.0} train {:?})",
                    if lifescape_planner::bid_response::preempts(lot, rate) { "*" } else { "" },
                    lot.item, lot.count, lot.from_bids, lot.best_bid, lot.from_sales, lot.from_alch, lot.revenue, lot.spent, lot.ticks, lot.rate, lot.profit, lot.unit.train
                )
            })
            .collect();
        let started = std::time::Instant::now();
        let chosen = crate::brain_bids::choose(&life, &reader, &rows, &state);
        let spent_ms = started.elapsed().as_secs_f64() * 1000.0;
        let chosen_text = match &chosen {
            Some((lot, _)) => {
                *claims.entry(lot.item.clone()).or_default() += lot.count;
                *picked.entry(lot.item.clone()).or_default() += 1;
                format!("PICKS {} x{}", lot.item, lot.count)
            }
            None => "keeps its ambition".to_string(),
        };
        eprintln!("{account:<12} {kind:<11} rate {rate:.2} ({spent_ms:.1} ms) {chosen_text}; top: {}", if shown.is_empty() { "-".to_string() } else { shown.join(" | ") });
        // Derived input bids of the chosen lot (its graph: product ← inputs).
        if let Some((lot, _)) = chosen {
            let method = lot.unit.plan.first().and_then(|step| step.strip_prefix("make ")).unwrap_or("").to_string();
            let leg = lifescape_planner::obtain_legs::production_legs(&lot.item).into_iter().find(|leg| leg.method == method);
            if let Some(leg) = leg {
                let mut top = lifescape_planner::Node::new("n1", lifescape_planner::NodeKind::ObtainItems { item: lot.item.clone(), count: lot.count }, Vec::new(), lot.count);
                top.method = Some(method.clone());
                let mut nodes = vec![top];
                for (index, (input, qty)) in leg.inputs.iter().enumerate() {
                    let id = format!("n{}", index + 2);
                    nodes[0].deps.push(id.clone());
                    nodes.push(lifescape_planner::Node::new(&id, lifescape_planner::NodeKind::ObtainItems { item: input.clone(), count: qty * lot.count }, Vec::new(), qty * lot.count));
                }
                let value_of = |item: &str| reader.view(item).and_then(|view| view.best_bid().max(view.current)).map(|price| price as f64);
                let input_price = |item: &str| reader.view(item).and_then(|view| view.best_ask().or(view.current)).unwrap_or(1) as f64;
                let prices = lifescape_planner::derived_demand::DerivedPrices { value_of: &value_of, input_price: &input_price, units_per_6000: &units, coin_rate: rate };
                let coins = held("coins");
                for index in 1..nodes.len() {
                    let lifescape_planner::NodeKind::ObtainItems { item, count } = &nodes[index].kind else { continue };
                    if let Some(reserve) = lifescape_planner::derived_demand::input_reserve(&nodes, index, &prices) {
                        let (open, ceiling) = lifescape_planner::derived_demand::derived_bounds(reserve, (coins / count.max(&1)).max(1));
                        eprintln!("{:<12}   input bid {count} {item}: reserve {reserve:.0}, opens ≥ {open}, ceiling {ceiling} (wallet {coins})", "");
                    }
                }
            }
        }
    }
    let mut picks: Vec<(String, usize)> = picked.into_iter().collect();
    picks.sort();
    eprintln!("== picks: {picks:?}");
    eprintln!("== ambition weights (fleet demand of the copy) ==");
    let name_of = |item_id: i64| debug_of(item_id);
    let orders = store.resting_orders().unwrap_or_default();
    let signals = crate::market_demand::signals_from(&orders, crate::market_service::MarketService::now_clock(), &name_of, &|secs| secs.max(0) as u64 * 10);
    let items = crate::market_demand::demand_items_with(&signals, &|item| price_of(item) as f64);
    let mut weights = lifescape_planner::kind_weights(&items, &[]);
    weights.sort_by(|left, right| right.weight.total_cmp(&left.weight));
    for row in weights {
        eprintln!("{:<12} weight {:.2} top {:?}", row.kind, row.weight, row.top_item);
    }
    let scimitars: Vec<(i64, i64)> = owners.iter().filter(|row| row.0 == "bronze_scimitar" && !row.2).map(|row| (row.3, row.4)).collect();
    eprintln!("== bronze scimitar: current {:?}, asks {scimitars:?}", traded.get("bronze_scimitar"));
}
