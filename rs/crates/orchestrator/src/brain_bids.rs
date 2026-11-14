#![forbid(unsafe_code)]
//! Producers respond to bids, brain side (user-approved point 7,
//! 2026-11-14). The pure rules live in the planner
//! (`bid_response`: unit cost, depth split, preempt rule, spread;
//! `market_fit`: saturation, sale lots, own downstream use, coin rate);
//! this file reads the PUBLIC book (the market engine's resting orders,
//! the bot's own bids left out, 24 h volume from the executions), the
//! published rates and the bot's own state, then hands the draw a
//! production quota (`bid:<item>`) when a bid beats the ambition.
//!
//! Called on EVERY goal draw (`brain_succession::draw_next`), before the
//! ambition's milestone: the ambition and its milestones are untouched,
//! the next draw weighs them again. The product sells through the
//! default listing path once the quota is done (`brain_sales`).

use std::cell::RefCell;
use std::collections::HashMap;

use lifescape_json::Value;
use lifescape_planner::bid_response::{best_opportunity, bid_goal, Opportunity, Producer};
use lifescape_planner::market_fit::{coin_rate, BookView};
use lifescape_planner::{GoalKind, Life, Node, NodeKind};

/// Units per 6000 ticks of a make recipe nobody measured yet (one action
/// every ~5 ticks with the bank trips: smithing, smelting, cooking).
pub const MAKE_UNITS_PER_6000: f64 = 600.0;
/// Units per 6000 ticks of a gather nobody measured yet.
pub const GATHER_UNITS_PER_6000: f64 = 200.0;

/// Debug name of a market object (banknotes trade as their base).
fn debug_name(item_id: i64) -> Option<String> {
    let obj = lifescape_gamedata::obj_by_id(usize::try_from(item_id).ok()?)?;
    let name = obj.name?;
    Some(name.strip_prefix("cert_").unwrap_or(name).to_string())
}

fn display_of(item: &str) -> String {
    lifescape_gamedata::objs_by_name(item).into_iter().next().and_then(|obj| obj.display.or(obj.name)).unwrap_or(item).to_string()
}

/// Resting orders as `(item debug name, account, buy?, price, remaining)`.
type Resting = (String, String, bool, i64, i64);

/// Pure: every object's public view from resting orders, `account`'s own
/// bids left out (a bot never answers its own bids), its own asks kept
/// (they are supply like any other). Bids best first, asks cheapest first.
pub fn views_from(orders: &[Resting], account: &str) -> HashMap<String, BookView> {
    let mut out: HashMap<String, BookView> = HashMap::new();
    for (item, owner, buy, price, remaining) in orders.iter().filter(|row| row.4 > 0) {
        let view = out.entry(item.clone()).or_default();
        if *buy {
            if !owner.eq_ignore_ascii_case(account) {
                view.bids.push((*price, *remaining));
            }
        } else {
            view.asks.push((*price, *remaining));
        }
    }
    for view in out.values_mut() {
        view.bids.sort_by(|left, right| right.0.cmp(&left.0));
        view.asks.sort_by_key(|(price, _)| *price);
    }
    out
}

/// The live book as `account` reads it (empty while the market is down:
/// no data, no decision). 24 h volume is filled by `with_volume`.
pub fn live_views(account: &str) -> HashMap<String, BookView> {
    let Some(service) = crate::market_service::shared() else { return HashMap::new() };
    let rows: Vec<Resting> = {
        let engine = service.lock_engine();
        engine
            .books
            .iter()
            .filter_map(|(item_id, book)| Some((debug_name(*item_id)?, book.orders())))
            .flat_map(|(name, orders)| {
                orders
                    .into_iter()
                    .map(move |order| (name.clone(), order.account.clone(), order.side == lifescape_market::Side::Buy, order.price, order.remaining))
            })
            .collect()
    };
    views_from(&rows, account)
}

/// Units traded in the last 24 h for one object (every form).
fn volume_24h(item: &str) -> i64 {
    let Some(service) = crate::market_service::shared() else { return 0 };
    let clock = crate::market_service::MarketService::now_clock();
    let store = service.lock_store();
    lifescape_gamedata::objs_by_name(item)
        .into_iter()
        .chain(lifescape_gamedata::objs_by_name(&format!("cert_{item}")))
        .filter_map(|obj| store.executions_for_item(obj.obj_id as i64, clock - 86_400, clock + 1).ok())
        .map(|fills| fills.iter().map(|fill| fill.quantity).sum::<i64>())
        .sum()
}

/// A market reader over `views`, with the 24 h volume read once per
/// object on demand (the recursion asks the same inputs again).
pub struct MarketReader {
    views: HashMap<String, BookView>,
    volumes: RefCell<HashMap<String, i64>>,
    live_volume: bool,
}

impl MarketReader {
    /// Offline reader (tests, sims): volumes as given in the views.
    pub fn fixed(views: HashMap<String, BookView>) -> Self {
        MarketReader { views, volumes: RefCell::new(HashMap::new()), live_volume: false }
    }

    pub fn live(account: &str) -> Self {
        MarketReader { views: live_views(account), volumes: RefCell::new(HashMap::new()), live_volume: true }
    }

    /// The view of one object (`None`: nothing rests and nothing traded).
    pub fn view(&self, item: &str) -> Option<BookView> {
        let mut view = self.views.get(item).cloned();
        if self.live_volume {
            let volume = *self.volumes.borrow_mut().entry(item.to_string()).or_insert_with(|| volume_24h(item));
            if volume > 0 {
                view.get_or_insert_with(BookView::default).volume_24h = volume;
            }
        }
        view
    }

    /// Objects some other account bids for, heaviest bid value first.
    pub fn bid_books(&self) -> Vec<(String, BookView)> {
        let mut out: Vec<(String, BookView)> = self.views.iter().filter(|(_, view)| !view.bids.is_empty()).map(|(item, view)| (item.clone(), view.clone())).collect();
        out.sort_by(|left, right| {
            let value = |view: &BookView| view.bids.iter().map(|(price, qty)| price * qty).sum::<i64>();
            value(&right.1).cmp(&value(&left.1)).then_with(|| left.0.cmp(&right.0))
        });
        out
    }
}

/// Published `(script, band, profit per 6000)` rows (rates page).
pub fn rate_rows() -> Vec<(String, i64, f64)> {
    let Ok(store) = lifescape_store::Store::open(&crate::market_service::db_path()) else { return Vec::new() };
    let snapshot = crate::rates_service::live_snapshot();
    crate::rates_service::profit_rows(&store, &snapshot).into_iter().map(|row| (row.method, row.band, row.profit)).collect()
}

/// Units per 6000 ticks of a production method for an object: measured
/// (any band), else the cold-start prior, else the make/gather default.
pub fn units_per_6000(snapshot: &crate::rates_service::Snapshot, method: &str, item: &str) -> f64 {
    let probe = Node::new("probe", NodeKind::ObtainItems { item: item.to_string(), count: 1 }, Vec::new(), 1);
    let display = display_of(item);
    let script = crate::brain_plan::rate_key(method, &probe).unwrap_or_default();
    crate::rates_service::measured_yield_any_band(snapshot, &script, &display)
        .or_else(|| crate::rates_service::prior_for(&script, &display.to_lowercase()).map(|(units, _)| units))
        .unwrap_or(if method.starts_with("farm:") || method.starts_with("gather:") { GATHER_UNITS_PER_6000 } else { MAKE_UNITS_PER_6000 })
}

/// What the scorer needs beyond the book, gathered by the caller.
pub struct BotState<'a> {
    pub level_of: &'a dyn Fn(&str) -> u8,
    pub held: &'a dyn Fn(&str) -> i64,
    pub avoid: Vec<String>,
    pub quests_done: Vec<String>,
    pub units: &'a dyn Fn(&str, &str) -> f64,
    pub tick: u64,
}

/// Pure choice: the bid this life answers now, if any (planner rules).
pub fn choose(life: &Life, reader: &MarketReader, rows: &[(String, i64, f64)], state: &BotState) -> Option<(Opportunity, f64)> {
    let rate = coin_rate(rows, state.level_of);
    let producer = Producer {
        level_of: state.level_of,
        held: state.held,
        avoid: &state.avoid,
        quests_done: &state.quests_done,
        coin_rate: rate,
        units_per_6000: state.units,
        ironman: life.profile.ironman,
        seed: life.seed,
        tick: state.tick,
    };
    let market = |item: &str| reader.view(item);
    best_opportunity(&reader.bid_books(), &market, &producer).map(|found| (found, rate))
}

/// Runs `use_state` over this bot's own state (live levels, free
/// holdings, bans, quests, rates): shared by the bid draw and the seller
/// reserve cap, so both price a unit the same way.
fn with_bot_state<R>(life: &Life, live: &Value, use_state: impl FnOnce(&BotState) -> R) -> R {
    let level_of = |skill: &str| crate::brain_succession::live_level(live, skill);
    let locks = crate::brain_locks::locked_items(&life.account);
    let held = |item: &str| crate::brain_locks::free_count(crate::brain_ambition::live_count(live, item), crate::brain_locks::locked_in(&locks, item));
    let snapshot = crate::rates_service::live_snapshot();
    let units = |method: &str, item: &str| units_per_6000(&snapshot, method, item);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0);
    let ticks_since = |stamp: i64| crate::brain_stuck::seconds_to_ticks(now - stamp);
    let state = BotState {
        level_of: &level_of,
        held: &held,
        avoid: lifescape_planner::banned_skills(&life.history, &ticks_since),
        quests_done: crate::brain_rescue::quests_done(life),
        units: &units,
        tick: crate::brain_ambition::stamp_now().tick,
    };
    use_state(&state)
}

/// What making one unit of each `(item, units)` costs this bot (coins +
/// its ticks at its coin rate): `bid_response::production_cost`, the
/// make path a bid quota is priced with. The seller reserve never
/// exceeds it (`brain_reserve`, 2026-11-14). Items it cannot make now
/// are absent. One book read and one rates read for the whole pass.
pub(crate) fn production_costs_for(life: &Life, live: &Value, items: &[(String, i64)]) -> HashMap<String, f64> {
    if items.is_empty() || crate::market_service::shared().is_none() {
        return HashMap::new();
    }
    let rows = rate_rows();
    let reader = MarketReader::live(&life.account);
    with_bot_state(life, live, |state| {
        let rate = coin_rate(&rows, state.level_of);
        let producer = Producer {
            level_of: state.level_of,
            held: state.held,
            avoid: &state.avoid,
            quests_done: &state.quests_done,
            coin_rate: rate,
            units_per_6000: state.units,
            ironman: life.profile.ironman,
            seed: life.seed,
            tick: state.tick,
        };
        let market = |name: &str| reader.view(name);
        items
            .iter()
            .filter_map(|(item, units)| {
                let unit = lifescape_planner::bid_response::production_cost(item, *units, &market, &producer)?;
                Some((item.clone(), unit.total(rate)))
            })
            .collect()
    })
}

/// The bid goal for this life on a draw (live book, rates, own state),
/// logged with its numbers; `None` when no bid beats the ambition.
pub(crate) fn bid_goal_for(life: &Life, live: &Value) -> Option<(GoalKind, String)> {
    if life.profile.ironman || crate::market_service::shared().is_none() {
        return None;
    }
    let reader = MarketReader::live(&life.account);
    let (found, rate) = with_bot_state(life, live, |state| choose(life, &reader, &rate_rows(), state))?;
    crate::logln!(
        "brain {}: answers bids: produce {} {} (best bid {}, unit {:.0} coins + {:.0} ticks, {:.1} coins/tick vs own {:.2}, net {:.0}; {})",
        life.account,
        found.count,
        found.item,
        found.best_bid,
        found.unit.coins,
        found.unit.ticks,
        found.rate,
        rate,
        found.profit,
        found.unit.plan.join(" < ")
    );
    Some(bid_goal(&found))
}

#[cfg(test)]
mod tests {
    use super::views_from;

    /// The bot's own bids never reach its view; its own asks are supply.
    #[test]
    fn views_leave_out_own_bids_only() {
        let rows = vec![
            ("bronze_scimitar".to_string(), "fighter".to_string(), true, 2961, 1),
            ("bronze_scimitar".to_string(), "smith".to_string(), true, 3000, 1),
            ("bronze_scimitar".to_string(), "fighter".to_string(), true, 2876, 2),
            ("bronze_bar".to_string(), "smith".to_string(), false, 5, 146),
            ("bronze_bar".to_string(), "other".to_string(), false, 4, 0),
        ];
        let views = views_from(&rows, "smith");
        let scimitar = &views["bronze_scimitar"];
        assert_eq!(scimitar.bids, vec![(2961, 1), (2876, 2)]);
        assert_eq!(views["bronze_bar"].asks, vec![(5, 146)], "own asks count, empty rows do not");
    }
}
