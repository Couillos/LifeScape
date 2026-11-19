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
//! debug names: a recipe on stock is evaluated on them). Profile of the
//! feasibility (D22, `guide_feasible`): `level`, `levels`
//! (`skill:level,...`, overriding `level`), `quests`, `held` and `coins`
//! (free coins; empty: a lot's capital unjudged). Every row carries its
//! `conditions` (structured list) and its `feasibility` for that profile
//! (`feasible` | `after_prereqs` | `infeasible`, pending and reasons);
//! nothing is ever blocked. No profile (none of those keys): no line is
//! judged, `feasibility` null on every row and `judged` false (never a
//! judgement for a made-up bot: each bot judges itself, D22). No request
//! builds a guide: the API reads the set the `guides` thread publishes
//! (`guides_store`, filters applied by reading); `build_guide` builds one
//! guide directly for the offline check (`rates-rebuild --guide`).

#![forbid(unsafe_code)]

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use lifescape_json::{object, str_value, Value};
use lifescape_planner::guide_feasible::{conditions, feasible_for, BotState, Condition, Feasibility};
use lifescape_planner::guide_model::{Confidence, FlowLine, GuideFilter, GuideWorld, InputRoute, MeasuredKey, MoneyRow, Prereqs, Quote, RateAnswer, RateAsk, ShopNeed, TrainingRow, Unrated};
use lifescape_store::Store;

/// Store reports read per page while measuring waits and quests.
pub const MEASURE_PAGE: i64 = 2000;

/// Measured waits of the store: market pause per paused run, quest
/// durations by planner quest key.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StoreWaits {
    pub market_wait: Option<f64>,
    pub quests: HashMap<String, f64>,
}

/// Mean market pause of the runs that paused, mean ticks of each
/// quest's finished runs (contract 2): every report tallied
/// (`guides_store::WaitsTally` from the first).
pub fn store_waits(store: &Store) -> Result<StoreWaits, String> {
    let mut tally = crate::guides_store::WaitsTally::default();
    tally.advance(store)?;
    Ok(tally.waits())
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
    let newest = store.newest_fill_clock(item_id).ok().flatten();
    let current = lifescape_market::current_price_at(&last, &bids, &asks, newest, crate::market_service::MarketService::now_clock());
    let prices: Vec<i64> = last.iter().map(|fill| fill.price).collect();
    // Per unit, exact (plan prix lot D: the book holds coins per lot).
    let lot = crate::market_units::lot_of(item_id) as f64;
    let levels = |levels: Vec<(i64, i64)>| levels.into_iter().map(|(price, units)| (price as f64 / lot, units as f64)).collect();
    Quote {
        price: current.price as f64 / lot,
        traded: current.traded,
        median: lifescape_market::median_price(&prices).map(|median| median as f64 / lot).unwrap_or(0.0),
        fills: last.iter().filter(|fill| fill.price >= 1 && fill.quantity >= 1).count(),
        fill_units: last.iter().filter(|fill| fill.price >= 1 && fill.quantity >= 1).map(|fill| fill.quantity as f64).sum(),
        bids: levels(bids),
        asks: levels(asks),
        // The order board is read by the guides' build (`guides_store`).
        claimed: 0.0,
    }
}

/// `quote_of` with the units producers claimed on the order board
/// (`market_claims::claimed_total`, plan équilibre step 2): what every
/// build of the guides reads, the brain's set and the CLI's rebuild.
pub fn quote_on_board(store: &Store, item: &str, claims: &HashMap<String, i64>) -> Quote {
    let mut quote = quote_of(store, item);
    quote.claimed = claims.get(item).copied().unwrap_or(0).max(0) as f64;
    quote
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
    // The guides' prior, unmemoised here: the engine asks each yield
    // once per build, and a process memo (`rates_read::cached_guide_prior`,
    // 4096 entries shared with the brain's) would be emptied over and
    // over by the ~50 000 asks of a set (one per method and level).
    let estimate = crate::rates_read::estimate_quiet(snapshot.book(), &query, now, &lifescape_planner::method_rate::guide_prior).map_err(|error| error.to_string())?;
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
        levels: text("levels").map(|value| held_of(&value).into_iter().map(|(skill, level)| (skill, level.clamp(1.0, 99.0) as u8)).collect()).unwrap_or_default(),
        coins: text("coins").and_then(|value| value.parse::<f64>().ok()).filter(|coins| coins.is_finite() && *coins >= 0.0),
    }
}

/// `held=iron_ore:50,coal:500` (debug names): the objects a bot holds
/// (R7); an entry without a valid count is left out (also `levels`).
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
    let waits_ms = started.elapsed().as_millis() as i64;
    let measured = measured_keys(snapshot);
    let now = crate::rates_service::now_secs();
    let prices: RefCell<HashMap<String, Quote>> = RefCell::new(HashMap::new());
    // The order board as the brain's set reads it (plan équilibre step 2;
    // a process without the daemon's board reads none).
    let claims = crate::market_claims::claimed_total();
    let timers: RefCell<BTreeMap<&'static str, (u64, Duration)>> = RefCell::new(BTreeMap::new());
    let timed = |name: &'static str, began: Instant| {
        let mut slot = timers.borrow_mut();
        let entry = slot.entry(name).or_insert((0, Duration::ZERO));
        entry.0 += 1;
        entry.1 += began.elapsed();
    };
    let rate = |ask: &RateAsk| {
        let began = Instant::now();
        let found = answer_of(snapshot, ask, now);
        timed("rate", began);
        found
    };
    let price = |item: &str| {
        if let Some(known) = prices.borrow().get(item) {
            return known.clone();
        }
        let began = Instant::now();
        let quote = quote_on_board(store, item, &claims);
        timed("price", began);
        prices.borrow_mut().insert(item.to_string(), quote.clone());
        quote
    };
    let npc_sale = |item: &str| {
        let began = Instant::now();
        let found = npc_volume_value(item);
        timed("npc_sale", began);
        found
    };
    let shop_buy = |item: &str| {
        let began = Instant::now();
        let found = shop_window(item);
        timed("shop_buy", began);
        found
    };
    let quest_ticks = |key: &str| waits.quests.get(key).copied();
    let world = GuideWorld { rate: &rate, price: &price, npc_sale: &npc_sale, shop_buy: &shop_buy, market_wait_ticks: waits.market_wait, quest_ticks: &quest_ticks, measured: &measured };
    let engine = lifescape_planner::guide_engine::Engine::new(&world);
    let engine_began = Instant::now();
    // D22 Q1 (R7): the same order as a read of the set (feasible first
    // for a profile).
    let profiled = filter.has_profile();
    let (rows, unrated) = lifescape_planner::guide_feasible::with_filter_bot(filter, |bot| match kind {
        GuideKind::Money => {
            let (mut rows, unrated) = lifescape_planner::guide_money::money_guide(&engine, filter);
            if profiled {
                lifescape_planner::guide_feasible::feasible_first(&mut rows, bot);
            }
            (rows.iter().map(|row| money_value(row, profiled.then_some(bot))).collect::<Vec<Value>>(), unrated)
        }
        GuideKind::Training => {
            let (mut rows, unrated) = lifescape_planner::guide_training::training_guide(&engine, filter);
            if profiled {
                lifescape_planner::guide_feasible::feasible_first(&mut rows, bot);
            }
            (rows.iter().map(|row| training_value(row, profiled.then_some(bot))).collect::<Vec<Value>>(), unrated)
        }
    });
    let mut stats = engine_stats(&engine);
    if let Value::Obj(map) = &mut stats {
        for (name, (calls, spent)) in timers.borrow().iter() {
            map.insert(format!("{name}_calls"), Value::Int(*calls as i64));
            map.insert(format!("{name}_ms"), Value::Int(spent.as_millis() as i64));
        }
        map.insert("waits_ms".to_string(), Value::Int(waits_ms));
        map.insert("engine_and_json_ms".to_string(), Value::Int(engine_began.elapsed().as_millis() as i64));
    }
    Ok(object(vec![
        ("ok", Value::Bool(true)),
        ("count", Value::Int(rows.len() as i64)),
        ("judged", Value::Bool(profiled)),
        ("rows", Value::Arr(rows)),
        ("unrated", Value::Arr(unrated.iter().map(unrated_value).collect())),
        ("market_wait_ticks", waits.market_wait.map(Value::Float).unwrap_or(Value::Null)),
        ("built_ms", Value::Int(started.elapsed().as_millis() as i64)),
        ("stats", stats),
    ]))
}

/// Counters of one build: engine events and the time of its steps.
pub fn engine_stats(engine: &lifescape_planner::guide_engine::Engine) -> Value {
    let mut map = BTreeMap::new();
    for (name, count) in engine.stats() {
        map.insert(name.to_string(), Value::Int(count as i64));
    }
    for (name, spent) in engine.step_times() {
        map.insert(format!("step_{name}_ms"), Value::Int(spent.as_millis() as i64));
    }
    Value::Obj(map)
}

/// One guide read from a published set as the API serves it, with the
/// set's age, cost and what the read approximates.
pub fn read_guide(set: &crate::guides_store::GuideSet, kind: GuideKind, filter: &GuideFilter) -> Value {
    // D22 Q1 (R7): a profile given, the lines it can work come first.
    let profiled = filter.has_profile();
    let read = lifescape_planner::guide_feasible::with_filter_bot(filter, |bot| match kind {
        GuideKind::Money => set.money(filter).map(|(mut rows, unrated)| {
            if profiled {
                lifescape_planner::guide_feasible::feasible_first(&mut rows, bot);
            }
            (rows.iter().map(|row| money_value(row, profiled.then_some(bot))).collect::<Vec<Value>>(), unrated)
        }),
        GuideKind::Training => {
            let (mut rows, unrated) = set.training(filter);
            if profiled {
                lifescape_planner::guide_feasible::feasible_first(&mut rows, bot);
            }
            Ok((rows.iter().map(|row| training_value(row, profiled.then_some(bot))).collect::<Vec<Value>>(), unrated))
        }
    });
    let (rows, unrated) = match read {
        Ok(found) => found,
        Err(error) => return crate::web::err_json(error),
    };
    let view_level = match kind {
        GuideKind::Money => set.money_view(filter.level).level,
        GuideKind::Training => set.training_view(filter.level).level,
    };
    // What the read does not rebuild (`guide_read`): said, never hidden.
    let mut approximate: Vec<Value> = Vec::new();
    if let (Some(level), Some(built)) = (filter.level, view_level) {
        if level != built {
            approximate.push(str_value(&format!("rates of level {built}, the band's last level (activities above {level} left out, accessibility at {level})")));
        }
    }
    if !filter.quests_done.is_empty() {
        approximate.push(str_value(match kind {
            GuideKind::Money => "chain lines keep the farm-or-buy choices of a bot without quests (its D17 outside the chains)",
            GuideKind::Training => "D17 re-read from the money view with the quests (its chain lines as for a bot without quests)",
        }));
    }
    object(vec![
        ("ok", Value::Bool(true)),
        ("count", Value::Int(rows.len() as i64)),
        ("judged", Value::Bool(profiled)),
        ("rows", Value::Arr(rows)),
        ("unrated", Value::Arr(unrated.iter().map(unrated_value).collect())),
        ("market_wait_ticks", set.market_wait_ticks.map(Value::Float).unwrap_or(Value::Null)),
        ("view_level", view_level.map(|level| Value::Int(i64::from(level))).unwrap_or(Value::Null)),
        ("approximate", Value::Arr(approximate)),
        ("built_at_ms", Value::Int(set.built_unix_ms)),
        ("age_ms", Value::Int(set.age().as_millis() as i64)),
        ("cycle_ms", Value::Int(set.cycle_ms as i64)),
        ("cycle_cpu_ms", set.cycle_cpu_ms.map(|cpu| Value::Int(cpu as i64)).unwrap_or(Value::Null)),
        ("refresh_s", Value::Int(crate::guides_store::REFRESH_PERIOD.as_secs() as i64)),
    ])
}

/// HTTP entry: the guide of `kind` for the query string, read from the
/// last published set (never built here); pending before the first.
pub fn guides_api(kind: GuideKind, query: &str) -> Value {
    let filter = filter_of(&crate::web::parse_query(query));
    match crate::guides_store::current() {
        Some(set) => read_guide(&set, kind, &filter),
        None => object(vec![("ok", Value::Bool(false)), ("pending", Value::Bool(true)), ("error", str_value("guides: first build running"))]),
    }
}

pub(crate) fn site_text(site: Option<(i64, i64, i64)>) -> String {
    site.map(|(x, z, level)| format!("{x},{z},{level}")).unwrap_or_default()
}

fn pairs(rows: &[(String, f64)]) -> Value {
    let mut map = BTreeMap::new();
    for (name, value) in rows {
        map.insert(name.clone(), Value::Float(*value));
    }
    Value::Obj(map)
}

/// One condition (D22) as JSON: `kind`, `text`, `impossible` (no bot
/// meets it today) and its own fields.
fn condition_value(condition: &Condition) -> Value {
    let mut fields: Vec<(&str, Value)> = vec![("kind", str_value(condition.kind())), ("text", str_value(&condition.text())), ("impossible", Value::Bool(condition.impossible()))];
    match condition {
        Condition::Level { skill, level } => fields.extend([("skill", str_value(skill)), ("level", Value::Int(i64::from(*level)))]),
        Condition::Combat { level } => fields.push(("level", Value::Int(i64::from(*level)))),
        Condition::NoSite | Condition::NotIronman => {}
        Condition::Quest { quest } => fields.push(("quest", str_value(quest))),
        Condition::QuestPoints { points } => fields.push(("points", Value::Int(*points))),
        Condition::Tool { tool, shop } => fields.extend([("tool", str_value(tool)), ("shop", Value::Bool(*shop))]),
        Condition::Stock { item, units } => fields.extend([("item", str_value(item)), ("units", Value::Float(*units))]),
        Condition::Capital { coins } => fields.push(("coins", Value::Float(*coins))),
        Condition::Market(need) => fields.extend([
            ("item", str_value(&need.item)),
            ("cause", str_value(&need.cause)),
            ("per_6000", Value::Float(need.per_6000)),
            ("per_lot", Value::Float(need.per_lot)),
            ("asks", Value::Float(need.asks)),
            ("recent", Value::Float(need.recent)),
            ("shops", Value::Float(need.shops)),
            ("priced", Value::Bool(need.priced)),
        ]),
        Condition::Source(gap) => fields.extend([("item", str_value(&gap.item)), ("reason", str_value(&gap.reason))]),
    }
    object(fields)
}

fn conditions_value(prereqs: &Prereqs) -> Value {
    Value::Arr(conditions(prereqs).iter().map(condition_value).collect())
}

/// A row's feasibility for the request's profile (D22); no profile:
/// null (not judged).
fn judged_value(row: &impl lifescape_planner::guide_feasible::GuideRow, bot: Option<&BotState>) -> Value {
    bot.map(|bot| feasibility_value(&feasible_for(row, bot))).unwrap_or(Value::Null)
}

fn feasibility_value(found: &Feasibility) -> Value {
    let (pending, reasons): (&[lifescape_planner::guide_feasible::Pending], &[String]) = match found {
        Feasibility::Feasible => (&[], &[]),
        Feasibility::AfterPrereqs(pending) => (pending, &[]),
        Feasibility::Infeasible(reasons, pending) => (pending, reasons),
    };
    object(vec![
        ("state", str_value(found.state())),
        ("within_reach", Value::Bool(found.within_reach())),
        ("pending", Value::Arr(pending.iter().map(|need| str_value(&need.text())).collect())),
        ("reasons", Value::Arr(reasons.iter().map(|reason| str_value(reason)).collect())),
    ])
}

/// A figure that means nothing on an unpriced row (D22): null.
fn priced_value(priced: bool, value: f64) -> Value {
    if priced {
        Value::Float(value)
    } else {
        Value::Null
    }
}

pub(crate) fn confidence_value(confidence: &Confidence) -> Value {
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

/// One money line as JSON, its feasibility for `bot` (D22; none: null).
pub fn money_value(row: &MoneyRow, bot: Option<&BotState>) -> Value {
    object(vec![
        ("family", str_value(row.family.as_str())),
        ("method", str_value(&row.method)),
        ("style", str_value(&row.style)),
        ("site", str_value(&site_text(row.site))),
        ("tool", str_value(&row.tool)),
        ("band", Value::Int(row.band)),
        ("priced", Value::Bool(row.priced)),
        ("profit_6000", priced_value(row.priced, row.profit_6000)),
        ("revenue_6000", priced_value(row.priced, row.revenue_6000)),
        ("cost_6000", priced_value(row.priced, row.cost_6000)),
        ("extra_ticks_6000", Value::Float(row.extra_ticks_per_6000)),
        ("xp_6000", pairs(&row.xp_6000)),
        ("conditions", conditions_value(&row.prereqs)),
        ("feasibility", judged_value(row, bot)),
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
    ])
}

/// One training line as JSON, its feasibility for `bot` (D22; none: null).
pub fn training_value(row: &TrainingRow, bot: Option<&BotState>) -> Value {
    object(vec![
        ("skill", str_value(&row.skill)),
        ("band", Value::Int(row.band)),
        ("method", str_value(&row.method)),
        ("style", str_value(&row.style)),
        ("site", str_value(&site_text(row.site))),
        ("tool", str_value(&row.tool)),
        ("supply", str_value(row.supply.as_str())),
        ("priced", Value::Bool(row.priced)),
        ("xp_6000", Value::Float(row.xp_6000)),
        ("effective_xp_6000", priced_value(row.priced, row.effective_xp_6000)),
        ("coins_6000", priced_value(row.priced, row.coins_6000)),
        ("coins_per_xp", priced_value(row.priced, row.coins_per_xp)),
        ("converted_ticks_6000", priced_value(row.priced, row.converted_ticks_6000)),
        ("ticks_to_band_end", row.ticks_to_band_end.map(Value::Float).unwrap_or(Value::Null)),
        ("other_xp_6000", pairs(&row.other_xp_6000)),
        ("quest_xp", Value::Float(row.quest_xp)),
        ("quest_ticks", row.quest_ticks.map(Value::Float).unwrap_or(Value::Null)),
        ("conditions", conditions_value(&row.prereqs)),
        ("feasibility", judged_value(row, bot)),
        ("flows", Value::Arr(row.flows.iter().map(flow_value).collect())),
        ("routes", Value::Arr(row.routes.iter().map(route_value).collect())),
        ("confidence", confidence_value(&row.confidence)),
        ("price_confidence", str_value(row.price_confidence)),
    ])
}

#[cfg(test)]
#[path = "web_guides_tests.rs"]
mod tests;
