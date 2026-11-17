//! Guides API (plan rendements R4): `/api/guides/money` and
//! `/api/guides/training`, the pure engine of `lifescape_planner::guide_*`
//! fed with the public data only (D22):
//! - yields: the live estimator (`rates_read::estimate_using`, n >= 3
//!   blended with the guides' game prior, else the prior alone), names
//!   turned into debug names for the engine;
//! - prices: the current price of each traded object (`rates_price`,
//!   D12) and the units its last five fills moved (the price holds for
//!   them only, user 2026-11-17); a never-traded object has none, only its resting book (the
//!   engine counts it for the units it holds, `quote_of`); the best F2P
//!   shop sale at a line's volume (Q3, `NPC_SALE_VOLUME`) and what the
//!   F2P shops sell in one window (Q7, finite stock, decision 2026-11-16
//!   §2: base stock unit by unit at the server price, then the window's
//!   restock, `gamedata::shop_buy`);
//! - the market wait per lot: mean pause of the contract-2 runs that
//!   paused for the market; a quest's duration: mean work of its
//!   finished contract-2 runs.
//!
//! Filters (query string): `skill`, `level` (a bot with every skill at
//! it; empty: each method at its own gate), `family`, `quests` (done,
//! comma separated), `held` (objects a bot holds, `item:units,...`,
//! debug names: a recipe on stock is evaluated on them). A build is cached `GUIDE_TTL` per query: the bots
//! never read these guides (R7 later), the page does.

#![forbid(unsafe_code)]

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use lifescape_json::{object, str_value, Value};
use lifescape_planner::guide_model::{Confidence, FlowLine, GuideFilter, GuideWorld, InputRoute, MeasuredKey, MoneyRow, Prereqs, Quote, RateAnswer, RateAsk, ShopNeed, TrainingRow, Unrated};
use lifescape_store::Store;

/// Longest a built guide serves before a rebuild.
pub const GUIDE_TTL: Duration = Duration::from_secs(60);
/// Store reports read per page while measuring waits and quests.
const MEASURE_PAGE: i64 = 2000;

/// Measured waits of the store: market pause per paused run, quest
/// durations by planner quest key.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StoreWaits {
    pub market_wait: Option<f64>,
    pub quests: HashMap<String, f64>,
}

/// Mean market pause of the runs that paused, mean ticks of each
/// quest's finished runs (contract 2).
pub fn store_waits(store: &Store) -> Result<StoreWaits, String> {
    let (mut paused, mut pauses) = (0.0, 0usize);
    let mut quests: HashMap<String, (f64, usize)> = HashMap::new();
    let mut after = 0;
    loop {
        let page = store.measures_after(after, MEASURE_PAGE).map_err(|error| format!("guides measures: {error}"))?;
        let Some(last) = page.last() else { break };
        after = last.report_id;
        for stored in &page {
            if stored.measure.pause_ticks > 0 {
                paused += stored.measure.pause_ticks as f64;
                pauses += 1;
            }
            if let Some(quest) = stored.measure.planner_method.strip_prefix("quest:") {
                if stored.stop == "goal_reached" && stored.measure.work_ticks > 0 {
                    let entry = quests.entry(quest.to_string()).or_insert((0.0, 0));
                    entry.0 += (stored.measure.work_ticks + stored.measure.pause_ticks.max(0)) as f64;
                    entry.1 += 1;
                }
            }
        }
    }
    Ok(StoreWaits { market_wait: (pauses > 0).then(|| paused / pauses as f64), quests: quests.into_iter().map(|(key, (ticks, runs))| (key, ticks / runs as f64)).collect() })
}

/// Units a shop is assumed to already hold when a guide line sells to
/// it: a line sells a window's output (tens to hundreds of units) and
/// each unit sold lowers the shop's price (`shop_delta`, up to
/// `MAX_SHOP_DELTA` per mille) down to the server's floor (10 % of the
/// object's cost) within a few dozen units; the guide prices that
/// volume, never the first unit's best case.
pub const NPC_SALE_VOLUME: i64 = 100;

/// The guide's condition of a shop (Q7, user 2026-11-16): its F2P quest
/// (directory `quest_dragon` -> planner key `dragon`) and its door's
/// quest points.
pub fn shop_need(quest: Option<&str>, quest_points: i64) -> ShopNeed {
    let quest = quest.and_then(|dir| lifescape_gamedata::f2p::F2P_QUESTS.iter().find(|(_, known)| *known == dir)).map(|(_, dir)| dir.strip_prefix("quest_").unwrap_or(dir));
    ShopNeed { quest, quest_points }
}

/// What the best reachable F2P shop pays per unit for a guide line's
/// volume (Q3), with its condition (Q7: a shop behind a quest or quest
/// points buys too, the row then needs them; an open shop first at an
/// equal value).
pub fn npc_volume_value(item: &str) -> Option<(f64, ShopNeed)> {
    lifescape_gamedata::shop_sale::npc_sale_shops_reachable(item)
        .iter()
        .filter_map(|shop| lifescape_gamedata::shop_sale::unit_value_floor(&shop.shop, item, NPC_SALE_VOLUME).map(|value| (value, shop.open(), shop_need(shop.quest.as_deref(), shop.quest_points))))
        .filter(|(value, _, _)| *value >= 1)
        .max_by_key(|(value, open, _)| (*value, *open))
        .map(|(value, _, need)| (value as f64, need))
}

/// What the reachable F2P shops sell of `item` in one window
/// (`WINDOW_TICKS`): (unit price, units, condition) steps cheapest first
/// (Q7, finite stock §2, a conditioned shop with its condition).
pub fn shop_window(item: &str) -> Vec<(f64, f64, ShopNeed)> {
    lifescape_gamedata::shop_buy::purchase_steps(item, lifescape_planner::guide_model::WINDOW_TICKS as i64).into_iter().map(|step| (step.price as f64, step.units as f64, shop_need(step.quest.as_deref(), step.quest_points))).collect()
}

/// The engine's quote of the object `item` (debug name; D12,
/// `lifescape_market::current_price` as `rates_price` reads it): the
/// current price, the median of the last five fills and how many there
/// are, and the resting (non-dormant) book of both sides as deep as the
/// store's impact walk reads (`IMPACT_ROWS` orders). Never traded: no
/// price, the book only. An object the market does not list: no price,
/// no book. Read by the object's id, never by its display: several
/// objects share one ("Sapphire amulet" strung and unstrung, live
/// 2026-11-16), each with its own book.
pub fn quote_of(store: &Store, item: &str) -> Quote {
    let unlisted = Quote { price: 1.0, ..Quote::default() };
    let Some(obj) = lifescape_gamedata::objs_by_name(item).into_iter().next() else { return unlisted };
    let item_id = obj.obj_id as i64;
    if !matches!(store.get_item(item_id), Ok(Some(_))) {
        return unlisted;
    }
    let last: Vec<lifescape_market::LastFill> = store
        .last_fills(item_id, lifescape_market::PRICE_FILLS as i64)
        .unwrap_or_default()
        .into_iter()
        .map(|(price, quantity)| lifescape_market::LastFill { price, quantity })
        .collect();
    let side = |side| store.book_levels_covering(item_id, side, i64::MAX).unwrap_or_default();
    let (bids, asks) = (side(lifescape_store::OrderSide::Buy), side(lifescape_store::OrderSide::Sell));
    let current = lifescape_market::current_price(&last, &bids, &asks);
    let prices: Vec<i64> = last.iter().map(|fill| fill.price).collect();
    let levels = |levels: Vec<(i64, i64)>| levels.into_iter().map(|(price, units)| (price as f64, units as f64)).collect();
    Quote {
        price: current.price as f64,
        traded: current.traded,
        median: lifescape_market::median_price(&prices).map(|median| median as f64).unwrap_or(0.0),
        fills: last.iter().filter(|fill| fill.price >= 1 && fill.quantity >= 1).count(),
        fill_units: last.iter().filter(|fill| fill.price >= 1 && fill.quantity >= 1).map(|fill| fill.quantity as f64).sum(),
        bids: levels(bids),
        asks: levels(asks),
    }
}

fn site_of(text: &str) -> Option<(i64, i64, i64)> {
    let parts: Vec<i64> = text.split(',').filter_map(|part| part.trim().parse().ok()).collect();
    (parts.len() == 3).then(|| (parts[0], parts[1], parts[2]))
}

/// Measured exact keys of the estimator (the roll-ups stay internal).
pub fn measured_keys(snapshot: &crate::rates_service::Snapshot) -> Vec<MeasuredKey> {
    snapshot
        .book()
        .buckets
        .keys()
        .filter(|key| !key.is_rollup())
        .map(|key| MeasuredKey { method: key.method.clone(), style: key.style.clone(), site: site_of(&key.site), tool: key.tool.clone(), band: key.band })
        .collect()
}

/// The engine's yield of one ask from the live estimator.
pub fn answer_of(snapshot: &crate::rates_service::Snapshot, ask: &RateAsk, now: i64) -> Result<RateAnswer, String> {
    let query = crate::rates_read::RateQuery { method: ask.method.clone(), style: ask.style.clone(), site: ask.site, tool: ask.tool.clone(), level: ask.level.max(1) };
    let estimate = crate::rates_read::estimate_using(snapshot.book(), &query, now, &crate::rates_read::cached_guide_prior).map_err(|error| error.to_string())?;
    // A display several objects share names the activity's own object.
    Ok(RateAnswer {
        flows: estimate.flows.iter().map(|(item, cause, qty)| (crate::rates_names::debug_in(&ask.method, item, *qty < 0.0), cause.clone(), *qty)).collect(),
        units: crate::rates_names::units_in_method(&ask.method, &estimate.units),
        pause_per_6000: estimate.pause_per_6000,
        confidence: Confidence {
            measured: !estimate.prior,
            runs: estimate.confidence.runs,
            age_secs: estimate.confidence.age_secs,
            dispersion: estimate.confidence.dispersion,
            failure_rate: estimate.confidence.failure_rate,
            read: format!("{:?}", estimate.fallback),
        },
    })
}

/// The filter of a query string.
pub fn filter_of(params: &HashMap<String, String>) -> GuideFilter {
    let text = |key: &str| params.get(key).map(|value| value.trim().to_string()).filter(|value| !value.is_empty());
    GuideFilter {
        skill: text("skill"),
        level: text("level").and_then(|value| value.parse::<u8>().ok()).map(|level| level.clamp(1, 99)),
        family: text("family"),
        quests_done: text("quests").map(|value| value.split(',').map(|quest| quest.trim().to_string()).filter(|quest| !quest.is_empty()).collect()).unwrap_or_default(),
        held: text("held").map(|value| held_of(&value)).unwrap_or_default(),
    }
}

/// `held=iron_ore:50,coal:500` (debug names): the objects a bot holds
/// (R7); an entry without a valid count is left out.
fn held_of(text: &str) -> Vec<(String, f64)> {
    text.split(',')
        .filter_map(|entry| {
            let (item, units) = entry.split_once(':')?;
            let units = units.trim().parse::<f64>().ok().filter(|units| units.is_finite() && *units > 0.0)?;
            let item = item.trim();
            (!item.is_empty()).then(|| (item.to_string(), units))
        })
        .collect()
}

/// Which guide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GuideKind {
    Money,
    Training,
}

/// One guide built from a store and a snapshot (pure engine inside).
pub fn build_guide(store: &Store, snapshot: &crate::rates_service::Snapshot, kind: GuideKind, filter: &GuideFilter) -> Result<Value, String> {
    let started = Instant::now();
    let waits = store_waits(store)?;
    let measured = measured_keys(snapshot);
    let now = crate::rates_service::now_secs();
    let prices: RefCell<HashMap<String, Quote>> = RefCell::new(HashMap::new());
    let rate = |ask: &RateAsk| answer_of(snapshot, ask, now);
    let price = |item: &str| {
        if let Some(known) = prices.borrow().get(item) {
            return known.clone();
        }
        let quote = quote_of(store, item);
        prices.borrow_mut().insert(item.to_string(), quote.clone());
        quote
    };
    let npc_sale = |item: &str| npc_volume_value(item);
    let shop_buy = |item: &str| shop_window(item);
    let quest_ticks = |key: &str| waits.quests.get(key).copied();
    let world = GuideWorld { rate: &rate, price: &price, npc_sale: &npc_sale, shop_buy: &shop_buy, market_wait_ticks: waits.market_wait, quest_ticks: &quest_ticks, measured: &measured };
    let engine = lifescape_planner::guide_engine::Engine::new(&world);
    let (rows, unrated) = match kind {
        GuideKind::Money => {
            let (rows, unrated) = lifescape_planner::guide_money::money_guide(&engine, filter);
            (rows.iter().map(money_value).collect::<Vec<Value>>(), unrated)
        }
        GuideKind::Training => {
            let (rows, unrated) = lifescape_planner::guide_training::training_guide(&engine, filter);
            (rows.iter().map(training_value).collect::<Vec<Value>>(), unrated)
        }
    };
    Ok(object(vec![
        ("ok", Value::Bool(true)),
        ("count", Value::Int(rows.len() as i64)),
        ("rows", Value::Arr(rows)),
        ("unrated", Value::Arr(unrated.iter().map(unrated_value).collect())),
        ("market_wait_ticks", waits.market_wait.map(Value::Float).unwrap_or(Value::Null)),
        ("built_ms", Value::Int(started.elapsed().as_millis() as i64)),
    ]))
}

type GuideCache = HashMap<(GuideKind, String), (Instant, Arc<Value>)>;

/// HTTP entry: the cached guide of `kind` for the query string.
pub fn guides_api(kind: GuideKind, query: &str) -> Arc<Value> {
    static CACHE: OnceLock<Mutex<GuideCache>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let filter = filter_of(&crate::web::parse_query(query));
    let key = (kind, format!("{filter:?}"));
    let mut slot = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((built, value)) = slot.get(&key) {
        if built.elapsed() < GUIDE_TTL {
            return value.clone();
        }
    }
    let built = match Store::open(&crate::market_service::db_path()) {
        Ok(store) => build_guide(&store, &crate::rates_service::live_snapshot(), kind, &filter).unwrap_or_else(crate::web::err_json),
        Err(error) => crate::web::err_json(format!("guides store: {error}")),
    };
    let value = Arc::new(built);
    if slot.len() > 64 {
        slot.clear();
    }
    slot.insert(key, (Instant::now(), value.clone()));
    value
}

fn site_text(site: Option<(i64, i64, i64)>) -> String {
    site.map(|(x, z, level)| format!("{x},{z},{level}")).unwrap_or_default()
}

fn pairs(rows: &[(String, f64)]) -> Value {
    let mut map = BTreeMap::new();
    for (name, value) in rows {
        map.insert(name.clone(), Value::Float(*value));
    }
    Value::Obj(map)
}

fn prereqs_value(prereqs: &Prereqs) -> Value {
    let mut levels = BTreeMap::new();
    for (skill, level) in &prereqs.levels {
        levels.insert(skill.clone(), Value::Int(i64::from(*level)));
    }
    object(vec![
        ("levels", Value::Obj(levels)),
        ("combat", Value::Int(i64::from(prereqs.combat))),
        ("quests", Value::Arr(prereqs.quests.iter().map(|quest| str_value(quest)).collect())),
        ("quest_points", Value::Int(prereqs.quest_points)),
        ("tools", Value::Arr(prereqs.tools.iter().map(|tool| str_value(tool)).collect())),
        ("capital", Value::Float(prereqs.capital)),
        ("ironman", Value::Bool(prereqs.ironman)),
        ("stock", pairs(&prereqs.stock)),
    ])
}

fn blocked_value(reasons: &[String]) -> Value {
    Value::Arr(reasons.iter().map(|reason| str_value(reason)).collect())
}

fn confidence_value(confidence: &Confidence) -> Value {
    object(vec![
        ("measured", Value::Bool(confidence.measured)),
        ("runs", Value::Int(confidence.runs as i64)),
        ("age_secs", Value::Int(confidence.age_secs)),
        ("dispersion", Value::Float(confidence.dispersion)),
        ("failure_rate", Value::Float(confidence.failure_rate)),
        ("read", str_value(&confidence.read)),
    ])
}

fn flow_value(line: &FlowLine) -> Value {
    object(vec![
        ("item", str_value(&line.item)),
        ("display", str_value(&crate::rates_names::display_of(&line.item))),
        ("cause", str_value(&line.cause)),
        ("per_6000", Value::Float(line.per_6000)),
        ("unit_price", Value::Float(line.unit_price)),
        ("value_6000", Value::Float(line.value_6000)),
        ("outlet", str_value(line.outlet.as_str())),
        ("traded", Value::Bool(line.traded)),
        ("basis", str_value(line.basis.as_str())),
    ])
}

fn optional(value: Option<f64>) -> Value {
    value.map(Value::Float).unwrap_or(Value::Null)
}

/// The way one input or consumable came by (decision 2026-11-16 §1/§3).
fn route_value(route: &InputRoute) -> Value {
    object(vec![
        ("item", str_value(&route.item)),
        ("display", str_value(&crate::rates_names::display_of(&route.item))),
        ("cause", str_value(&route.cause)),
        ("route", str_value(route.route.as_str())),
        ("source", str_value(&route.source)),
        ("units", Value::Float(route.units)),
        ("farm_ticks_per_unit", optional(route.farm_ticks_per_unit)),
        ("farm_coins_per_unit", optional(route.farm_coins_per_unit)),
        ("buy_coins_per_unit", optional(route.buy_coins_per_unit)),
    ])
}

fn unrated_value(gap: &Unrated) -> Value {
    object(vec![("method", str_value(&gap.method)), ("reason", str_value(&gap.reason))])
}

/// One money line as JSON.
pub fn money_value(row: &MoneyRow) -> Value {
    object(vec![
        ("family", str_value(row.family.as_str())),
        ("method", str_value(&row.method)),
        ("style", str_value(&row.style)),
        ("site", str_value(&site_text(row.site))),
        ("tool", str_value(&row.tool)),
        ("band", Value::Int(row.band)),
        ("profit_6000", Value::Float(row.profit_6000)),
        ("revenue_6000", Value::Float(row.revenue_6000)),
        ("cost_6000", Value::Float(row.cost_6000)),
        ("extra_ticks_6000", Value::Float(row.extra_ticks_per_6000)),
        ("xp_6000", pairs(&row.xp_6000)),
        ("prereqs", prereqs_value(&row.prereqs)),
        ("flows", Value::Arr(row.flows.iter().map(flow_value).collect())),
        (
            "other_drops",
            object(vec![("items", Value::Int(row.other_drops.items as i64)), ("units_6000", Value::Float(row.other_drops.units_per_6000)), ("value_6000", Value::Float(row.other_drops.value_per_6000))]),
        ),
        (
            "stages",
            Value::Arr(
                row.stages
                    .iter()
                    .map(|stage| object(vec![("method", str_value(&stage.method)), ("product", str_value(&stage.product)), ("ticks_per_unit", Value::Float(stage.ticks_per_unit)), ("profit_6000", Value::Float(stage.profit_6000))]))
                    .collect(),
            ),
        ),
        ("routes", Value::Arr(row.routes.iter().map(route_value).collect())),
        ("confidence", confidence_value(&row.confidence)),
        ("price_confidence", str_value(row.price_confidence)),
        ("blocked", blocked_value(&row.blocked)),
        ("accessible", Value::Bool(row.accessible)),
    ])
}

/// One training line as JSON.
pub fn training_value(row: &TrainingRow) -> Value {
    object(vec![
        ("skill", str_value(&row.skill)),
        ("band", Value::Int(row.band)),
        ("method", str_value(&row.method)),
        ("style", str_value(&row.style)),
        ("site", str_value(&site_text(row.site))),
        ("tool", str_value(&row.tool)),
        ("supply", str_value(row.supply.as_str())),
        ("xp_6000", Value::Float(row.xp_6000)),
        ("effective_xp_6000", Value::Float(row.effective_xp_6000)),
        ("coins_6000", Value::Float(row.coins_6000)),
        ("coins_per_xp", Value::Float(row.coins_per_xp)),
        ("converted_ticks_6000", Value::Float(row.converted_ticks_6000)),
        ("ticks_to_band_end", row.ticks_to_band_end.map(Value::Float).unwrap_or(Value::Null)),
        ("other_xp_6000", pairs(&row.other_xp_6000)),
        ("quest_xp", Value::Float(row.quest_xp)),
        ("quest_ticks", row.quest_ticks.map(Value::Float).unwrap_or(Value::Null)),
        ("prereqs", prereqs_value(&row.prereqs)),
        ("flows", Value::Arr(row.flows.iter().map(flow_value).collect())),
        ("routes", Value::Arr(row.routes.iter().map(route_value).collect())),
        ("confidence", confidence_value(&row.confidence)),
        ("price_confidence", str_value(row.price_confidence)),
        ("blocked", blocked_value(&row.blocked)),
        ("accessible", Value::Bool(row.accessible)),
    ])
}

#[cfg(test)]
#[path = "web_guides_tests.rs"]
mod tests;
