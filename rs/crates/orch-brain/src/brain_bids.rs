#![forbid(unsafe_code)]
//! Producers answer the market, brain side (user-approved point 7,
//! 2026-11-14, extended to the whole market the same day). The pure
//! rules live in the planner (`bid_response`: unit cost with every
//! dependency, real trips, training share, spread; `market_value`: what
//! the market absorbs; `market_fit`: saturation, sale lots, coin rate);
//! this file reads the PUBLIC book (the market engine's resting orders,
//! the bot's own bids left out, current prices, 24 h volume, the public
//! claims of the order board), the bot's coin rate (what its work really
//! earns per tick, D1: `brain_d17::coin_rate`) and the bot's own state,
//! then hands the draw a production quota (`bid:<item>`) when a
//! lot in the bot's own trade beats the ambition (`planner::bid_trade`:
//! the primary skill of a need-drawn ambition makes the object; the D3
//! exceptions weigh the whole market), and posts the claim on the board, with the
//! units the lot buys from the resting asks (`market_claims::
//! claim_purchases`) and the gathering seats it takes (`site_crowd::
//! engage`): the next bots read them gone (2026-11-20). An answer whose
//! production every site refuses is given back (`brain_bid_release`).
//!
//! Called on EVERY goal draw (`brain_succession::draw_next`), before the
//! ambition's milestone: the ambition and its milestones are untouched,
//! the next draw weighs them again. The product sells through the
//! default listing path once the quota is done (`brain_sales`). CPU: the
//! whole market is weighed once per account per `CHOICE_TTL_SECS` (a
//! redraw retried every pass reuses the choice).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use lifescape_json::Value;
use lifescape_planner::bid_response::{best_opportunity, bid_goal, Opportunity, Producer};
use lifescape_planner::market_fit::BookView;
use lifescape_planner::{GoalKind, Life};

/// A market-wide choice is reused this long per account (wall seconds).
pub const CHOICE_TTL_SECS: u64 = 60;

/// Debug name of a market object (banknotes trade as their base).
fn debug_name(item_id: i64) -> Option<String> {
    let obj = lifescape_gamedata::obj_by_id(usize::try_from(item_id).ok()?)?;
    let name = obj.name?;
    Some(name.strip_prefix("cert_").unwrap_or(name).to_string())
}

/// Id of the base (non-banknote) object named `name`.
fn base_id(name: &str) -> Option<i64> {
    lifescape_gamedata::objs_by_name(name).into_iter().next().map(|obj| obj.obj_id as i64)
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
/// no data, no decision), with the market id of every object; 24 h
/// volume and current prices are read on demand (`MarketReader::view`).
fn live_views(account: &str) -> (HashMap<String, BookView>, HashMap<String, i64>) {
    let Some(service) = crate::market_service::shared() else { return (HashMap::new(), HashMap::new()) };
    let mut ids: HashMap<String, i64> = HashMap::new();
    let rows: Vec<Resting> = {
        let engine = service.lock_engine();
        let mut rows = Vec::new();
        for (item_id, book) in engine.books.iter() {
            let Some(name) = debug_name(*item_id) else { continue };
            // The base object's id, never its banknote's (both forms may
            // hold a book; the map's order is arbitrary): the current
            // price reads the base reference.
            ids.entry(name.clone()).or_insert_with(|| base_id(&name).unwrap_or(*item_id));
            for order in book.orders() {
                rows.push((name.clone(), order.account.clone(), order.side == lifescape_market::Side::Buy, crate::market_units::to_unit(*item_id, order.price), order.remaining));
            }
        }
        rows
    };
    (views_from(&rows, account), ids)
}

/// Units traded in the last 24 h for one object (every form).
fn volume_24h(item: &str) -> i64 {
    let Some(service) = crate::market_service::shared() else { return 0 };
    let clock = crate::market_service::MarketService::now_clock();
    let store = service.lock_store();
    lifescape_gamedata::objs_by_name(item)
        .into_iter()
        .chain(lifescape_gamedata::objs_by_name(&format!("cert_{item}")))
        .filter_map(|obj| store.volume_between(obj.obj_id as i64, clock - 86_400, clock + 1).ok())
        .sum()
}

/// Current price of one object when it traded (median of the last five
/// fills clamped by the impact prices), `None` otherwise.
fn current_of(item_id: i64) -> Option<i64> {
    let service = crate::market_service::shared()?;
    let reference = service.reference(item_id);
    reference.traded.then_some(reference.price.max(1))
}

/// A market reader over `views`, with the 24 h volume and the current
/// price read once per object on demand (the recursion asks the same
/// inputs again), and the public claims of other producers.
pub struct MarketReader {
    views: HashMap<String, BookView>,
    ids: HashMap<String, i64>,
    volumes: RefCell<HashMap<String, i64>>,
    currents: RefCell<HashMap<String, Option<i64>>>,
    claims: HashMap<String, i64>,
    /// Units of each object other producers' bid answers will buy from
    /// the resting asks (`market_claims::purchases_by_others`).
    purchases: HashMap<String, i64>,
    live: bool,
}

impl MarketReader {
    /// Offline reader (tests, sims): views as given (volume, current
    /// price and claims included).
    pub fn fixed(views: HashMap<String, BookView>) -> Self {
        MarketReader { views, ids: HashMap::new(), volumes: RefCell::new(HashMap::new()), currents: RefCell::new(HashMap::new()), claims: HashMap::new(), purchases: HashMap::new(), live: false }
    }

    /// The board other producers posted (sims): units claimed per object
    /// (supply on its way) and units their answers buy per object.
    pub fn with_board(mut self, claims: HashMap<String, i64>, purchases: HashMap<String, i64>) -> Self {
        self.claims = claims;
        self.purchases = purchases;
        self
    }

    pub fn live(account: &str) -> Self {
        let (views, ids) = live_views(account);
        let claims = crate::market_claims::claimed_by_others(account);
        MarketReader { views, ids, volumes: RefCell::new(HashMap::new()), currents: RefCell::new(HashMap::new()), claims, purchases: HashMap::new(), live: true }
    }

    /// `live` for a bid answer (the draw, `bid-why`): the asks other
    /// answers will buy are left out too (2026-11-20). Price caps (seller
    /// reserve, a buy's worth) read the plain `live` book.
    pub fn live_answering(account: &str) -> Self {
        let mut reader = MarketReader::live(account);
        reader.purchases = crate::market_claims::purchases_by_others(account);
        reader
    }

    fn current(&self, item: &str) -> Option<i64> {
        if !self.live {
            return self.views.get(item).and_then(|view| view.current);
        }
        if let Some(known) = self.currents.borrow().get(item) {
            return *known;
        }
        let id = self.ids.get(item).copied().or_else(|| lifescape_gamedata::objs_by_name(item).into_iter().next().map(|obj| obj.obj_id as i64));
        let price = id.and_then(current_of);
        self.currents.borrow_mut().insert(item.to_string(), price);
        price
    }

    /// The view of one object (`None`: nothing rests and nothing traded).
    pub fn view(&self, item: &str) -> Option<BookView> {
        let mut view = self.views.get(item).cloned();
        if self.live {
            let volume = *self.volumes.borrow_mut().entry(item.to_string()).or_insert_with(|| volume_24h(item));
            if volume > 0 {
                view.get_or_insert_with(BookView::default).volume_24h = volume;
            }
            if let Some(price) = self.current(item) {
                view.get_or_insert_with(BookView::default).current = Some(price);
            }
        }
        if let Some(claimed) = self.claims.get(item).filter(|units| **units > 0) {
            view.get_or_insert_with(BookView::default).claimed = *claimed;
        }
        // Asks other producers' bid answers will buy are not this bot's
        // to buy (2026-11-20: 14 answers counted the same 6 diamonds).
        if let (Some(bought), Some(book)) = (self.purchases.get(item).filter(|units| **units > 0), view.as_mut()) {
            book.without_bought(*bought);
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

    /// Every object with market value a producer can make: others' bids,
    /// or a current price with a day of volume, or (for a caster) an
    /// alchemy value. Views complete (volume, price, claims), sorted by
    /// name (deterministic). Objects no production leg makes are left
    /// out before any store read.
    pub fn value_books(&self, alch: &dyn Fn(&str) -> Option<f64>) -> Vec<(String, BookView)> {
        let mut names: Vec<&String> = self.views.keys().chain(self.ids.keys()).collect();
        names.sort();
        names.dedup();
        names
            .into_iter()
            .filter(|item| !lifescape_planner::obtain_legs::production_legs(item).is_empty())
            .filter_map(|item| {
                // Cheap reads first: the volume is a store read.
                let bids = self.views.get(item).is_some_and(|view| !view.bids.is_empty());
                if !bids && self.current(item).is_none() && alch(item).is_none() {
                    return None;
                }
                let view = self.view(item)?;
                let valued = !view.bids.is_empty() || (view.current.is_some() && view.volume_24h > 0) || alch(item).is_some();
                valued.then(|| (item.clone(), view))
            })
            .collect()
    }
}

/// Units of `item` per 6000 ticks of `method` for a bot at `level` with
/// `tool` (`rates_read`: n >= 3 blended with the game prior, else the
/// prior; nothing computable is a typed error).
pub fn units_per_6000_at(snapshot: &crate::rates_service::Snapshot, method: &str, item: &str, level: u8, tool: &str) -> Result<f64, crate::rates_read::RateError> {
    crate::rates_read::gained_per_6000(snapshot.book(), method, &display_of(item), level, tool)
}

/// Units per 6000 ticks of a production method for an object, read at
/// the method's skill gate (callers that know no bot level). A method
/// nothing estimates logs its typed error and reads NaN: every planner
/// consumer treats a non-finite rate as "no rate" and skips the leg,
/// never a default figure.
pub fn units_per_6000(snapshot: &crate::rates_service::Snapshot, method: &str, item: &str) -> f64 {
    match units_per_6000_at(snapshot, method, item, crate::rates_read::gate_level(method), "") {
        Ok(rate) => rate,
        Err(error) => {
            crate::rates_read::log_rate_error(&error);
            f64::NAN
        }
    }
}

/// What the scorer needs beyond the book, gathered by the caller.
pub struct BotState<'a> {
    pub level_of: &'a dyn Fn(&str) -> u8,
    pub held: &'a dyn Fn(&str) -> i64,
    pub avoid: Vec<String>,
    pub quests_done: Vec<String>,
    pub units: &'a dyn Fn(&str, &str) -> f64,
    pub tick: u64,
    pub combat: u8,
    pub position: Option<(i64, i64, i64)>,
    /// Highest level the bot's ambition climbs each skill to.
    pub planned: Vec<(String, u8)>,
    /// The bot as a caster (alchemy floor), when it casts.
    pub caster: Option<lifescape_planner::alchemy::Caster>,
    /// The planner's MEASURED rate reader (R7, `brain_plan_read::
    /// measured_rate`, memoised for the pass): the training a lot carries
    /// is sized on it (`bid_response::Producer::rate`).
    pub rate: lifescape_planner::work_rate::RateReader<'a>,
    /// Seats the other bots hold on gathering sites: working there now
    /// or engaged by a bid answer (`site_crowd::seats`, 2026-11-20).
    pub seats: Vec<lifescape_planner::bid_response::Seat>,
    /// Cooldown keys active for this bot (`brain_cool::active`): its
    /// cooled sites are no site for a lot.
    pub cooled: Vec<String>,
    /// Objects whose answer it gave back lately
    /// (`bid_release::shunned_items`): never answered again before the
    /// window ends.
    pub shunned: Vec<String>,
}

impl BotState<'_> {
    /// The planner's view of this bot at `rate` coins per tick; `use_it`
    /// runs with it (the closures live on this frame).
    pub fn with_producer<R>(&self, life: &Life, rate: f64, use_it: impl FnOnce(&Producer) -> R) -> R {
        self.with_producer_on(life, rate, &self.seats, use_it)
    }

    /// `with_producer` reading `seats` as the fleet's (none: a price cap
    /// asks what making costs, never whether a seat is free now).
    pub fn with_producer_on<R>(&self, life: &Life, rate: f64, seats: &[lifescape_planner::bid_response::Seat], use_it: impl FnOnce(&Producer) -> R) -> R {
        let planned = |skill: &str| self.planned.iter().filter(|(name, _)| name == skill).map(|(_, level)| *level).max().unwrap_or(0);
        let alch = |item: &str| self.caster.as_ref().and_then(|mage| mage.net(item));
        let producer = Producer {
            level_of: self.level_of,
            held: self.held,
            avoid: &self.avoid,
            quests_done: &self.quests_done,
            coin_rate: rate,
            units_per_6000: self.units,
            ironman: life.profile.ironman,
            seed: life.seed,
            tick: self.tick,
            combat: self.combat,
            position: self.position,
            free_pass: self.quests_done.iter().any(|quest| quest == lifescape_gamedata::toll::FREE_PASS_QUEST),
            planned_level: &planned,
            alch_net: &alch,
            rate: self.rate,
            seats,
            cooled: &self.cooled,
        };
        use_it(&producer)
    }
}

/// Highest level the life's ambition climbs each skill to NOW: its open
/// level milestones, never its ladder's top (`bid_reach::planned_levels`,
/// 2026-11-20: a smith at 73 answered adamant platebodies, smithing 88,
/// its training counted free because the ladder climbs to 90).
pub fn planned_levels(life: &Life) -> Vec<(String, u8)> {
    lifescape_planner::bid_reach::planned_levels(life)
}

/// `orchestrator bid-why <account> <item>`: every stage of the bid
/// response for one bot and one object (`bid_response::explain`), read
/// from the live book and the bot's own state.
pub fn explain_for(account: &str, item: &str) -> Result<Vec<String>, String> {
    // The operator may name the object by display name, banknote or id
    // (C13: "Holy symbol" is `blessedstar`, `stringstar` its input).
    let item = &crate::bid_why::resolve_item(item)?;
    let life = crate::brain::load_life(account).ok_or_else(|| format!("no life file for {account}"))?;
    let live = crate::jobs::live_of(account).ok_or_else(|| format!("{account}: no live snapshot (not supervised, or busy)"))?;
    if crate::market_service::shared().is_none() {
        return Err("market service down".to_string());
    }
    let reader = MarketReader::live_answering(account);
    let guides = crate::guides_store::current();
    Ok(with_bot_state(&life, &live, |state| {
        let key = key_of(&life, state);
        let best = guides.as_deref().and_then(|set| crate::brain_d17::best_row(set, &key));
        let rate = bot_rate(&life);
        state.with_producer(&life, rate, |producer| {
            let market = |name: &str| reader.view(name);
            let view = reader.view(item).unwrap_or_default();
            let mut lines = vec![crate::bid_why::identity_line(item), format!("{account}: coin rate {rate:.2} coins/tick")];
            if state.shunned.iter().any(|known| known == item) {
                lines.push(format!("its answer was given back less than {} ticks ago: not answered again yet", lifescape_planner::bid_release::BID_SHUN_TICKS));
            }
            if !lifescape_planner::bid_trade::answers_trade(&life, item) {
                let kind = life.ambition.as_ref().map(|held| held.kind.as_str()).unwrap_or("?");
                lines.push(format!("off the {kind}'s trade (bid_trade): its primary skill does not make it, never answered"));
            }
            if view.bids.is_empty() {
                lines.extend(crate::bid_why::related_bid_lines(item, &market));
            }
            // What sets that rate: what its work really earned (D1,
            // `time_value`); the D17, the best money guide line within
            // its reach, for comparison only.
            lines.push(time_value_line(account));
            lines.push(match (&guides, best) {
                (None, _) => "  D17 (display only): no money guide published yet".to_string(),
                (Some(_), None) => "  D17 (display only): no earning money guide line within its reach".to_string(),
                (Some(_), Some(row)) => format!("  D17 (display only): {} {} band {}: {:.0} coins per 6000 ticks", row.family.as_str(), row.method, row.band, row.profit_6000),
            });
            lines.extend(lifescape_planner::bid_response::explain(item, &view, &market, producer));
            lines
        })
    }))
}

/// What a D17 read knows of this bot (`brain_d17`): its live levels,
/// combat, quests and Ironman flag.
pub fn key_of<'a>(life: &Life, state: &'a BotState) -> crate::brain_d17::BotKey<'a> {
    crate::brain_d17::BotKey { level_of: state.level_of, combat: state.combat, quests_done: &state.quests_done, ironman: life.profile.ironman }
}

/// Coins one tick of this bot's time is worth: what its work really
/// earned per tick (`brain_d17::coin_rate`, plan équilibre D1).
pub fn bot_rate(life: &Life) -> f64 {
    crate::brain_d17::coin_rate(&life.account)
}

/// `bid-why`'s line on where the bot's rate comes from (D1).
fn time_value_line(account: &str) -> String {
    use crate::time_value::RateSource;
    let hours = crate::time_value::WINDOW_SECS / 3600;
    match crate::time_value::current().map(|values| values.source(account)) {
        Some(RateSource::Own(measured)) => format!("  time value (D1): own work, {:.0} coins over {} work ticks in {hours} h = {:.3}/tick", measured.value, measured.work_ticks, measured.per_tick()),
        Some(RateSource::Fleet(fleet)) => format!("  time value (D1): no measure of its own in {hours} h, the fleet's mean {:.3}/tick ({:.0} coins over {} work ticks)", fleet.per_tick(), fleet.value, fleet.work_ticks),
        Some(RateSource::Floor) | None => "  time value (D1): nothing measured, the floor".to_string(),
    }
}

/// Pure choice: the lot this life makes now, if any (planner rules), at
/// the bot's coin rate `rate` (`bot_rate`).
pub fn choose(life: &Life, reader: &MarketReader, rate: f64, state: &BotState) -> Option<(Opportunity, f64)> {
    choose_in(life, reader, rate, state, false)
}

/// `choose` over the whole market when `whole` (an answer while the
/// ambition waits on a buy, `planner::bid_trade` module doc), else the
/// bot's own trade.
pub fn choose_in(life: &Life, reader: &MarketReader, rate: f64, state: &BotState, whole: bool) -> Option<(Opportunity, f64)> {
    state.with_producer(life, rate, |producer| {
        let market = |item: &str| reader.view(item);
        let mut books = reader.value_books(producer.alch_net);
        // Its own trade only (`bid_trade`, 2026-11-20): the need-drawn
        // ambition runs, the consumers buy what they burn.
        books.retain(|(item, _)| !state.shunned.contains(item) && (whole || lifescape_planner::bid_trade::answers_trade(life, item)));
        if !whole {
            return best_opportunity(&books, &market, producer).map(|found| (found, rate));
        }
        lifescape_planner::bid_response::ranked_opportunities(&books, &market, producer)
            .into_iter()
            .find(|found| lifescape_planner::bid_trade::waiting_answer_fits(life, &found.item, &found.unit.train))
            .map(|found| (found, rate))
    })
}

/// Runs `use_state` over this bot's own state (live levels, free
/// holdings, bans, quests, rates, position, plans, caster): shared by the
/// bid draw, the seller reserve cap and the priced training, so all price
/// a unit the same way.
pub(crate) fn with_bot_state<R>(life: &Life, live: &Value, use_state: impl FnOnce(&BotState) -> R) -> R {
    let level_of = |skill: &str| crate::brain_succession::live_level(live, skill);
    let locks = crate::brain_locks::locked_items(&life.account);
    let held = |item: &str| crate::brain_locks::free_count(crate::brain_ambition::live_count(live, item), crate::brain_locks::locked_in(&locks, item));
    let snapshot = crate::rates_service::live_snapshot();
    let owned = crate::brain_plan::live_owned(live);
    // Read at the bot's own level in the method's skill and its best
    // tool tier (plan rendements R2), never a forced band.
    // A method that never launches for this bot now (quarantine, its own
    // cooldowns) makes nothing: no bid is answered with it (plan P2).
    let quarantined = crate::method_health::quarantined();
    let cooled = crate::brain_cool::active(life);
    let units = |method: &str, item: &str| {
        if quarantined.iter().any(|known| known == method) || lifescape_planner::cooldown::method_cooled(&cooled, method) {
            return 0.0;
        }
        let level = crate::rates_read::read_skill(method).map(|skill| level_of(&skill)).unwrap_or(1);
        let tool = crate::report_meta::best_tools(method, &owned).join("+");
        match units_per_6000_at(&snapshot, method, item, level, &tool) {
            Ok(rate) => rate,
            Err(error) => {
                crate::rates_read::log_rate_error(&error);
                f64::NAN
            }
        }
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0);
    // The planner's measured reader (R7), one estimate per (method,
    // level) for the whole pass: never a heavy read per candidate.
    let measured: std::cell::RefCell<HashMap<(String, u8), Option<lifescape_planner::work_rate::WorkRate>>> = Default::default();
    let rate = |method: &str, level: u8| {
        let key = (method.to_string(), level);
        if let Some(known) = measured.borrow().get(&key) {
            return known.clone();
        }
        let found = crate::brain_plan_read::measured_rate(snapshot.book(), method, level, now);
        measured.borrow_mut().insert(key, found.clone());
        found
    };
    let ticks_since = |stamp: i64| crate::brain_stuck::seconds_to_ticks(now - stamp);
    let position = live.get("position").as_arr().and_then(|cells| {
        let part = |slot: usize| cells.get(slot).and_then(|cell| cell.as_i64());
        Some((part(1)?, part(2)?, part(0)?))
    });
    let state = BotState {
        level_of: &level_of,
        held: &held,
        avoid: lifescape_planner::banned_skills(&life.history, &ticks_since),
        quests_done: crate::brain_rescue::quests_done(life),
        units: &units,
        tick: crate::brain_ambition::stamp_now().tick,
        combat: crate::brain_plan::live_combat(live),
        position,
        planned: planned_levels(life),
        caster: if life.profile.ironman { None } else { crate::brain_alch::caster_for(live) },
        rate: &rate,
        seats: crate::site_crowd::seats(&life.account),
        cooled: cooled.clone(),
        // Given back lately, or no bot can make it now (a tool nobody can
        // come by: the water talisman, `market_demand::unworkable_lots`).
        shunned: lifescape_planner::bid_release::shunned_items(&life.history, &ticks_since).into_iter().chain(crate::market_demand::unworkable_lots()).collect(),
    };
    use_state(&state)
}

/// What making one unit of each `(item, units)` costs this bot (coins +
/// its ticks at its coin rate, the lot's one-off walks and fares spread
/// over the units): `bid_response::production_cost`, the make path a
/// quota is priced with. The seller reserve never exceeds it
/// (`brain_reserve`, 2026-11-14). Items it cannot make now are absent.
/// One book read and one rates read for the whole pass.
pub fn production_costs_for(life: &Life, live: &Value, items: &[(String, i64)]) -> HashMap<String, f64> {
    let _phase = crate::cpu_profile::phase("bids.production_costs");
    if items.is_empty() || crate::market_service::shared().is_none() {
        return HashMap::new();
    }
    let reader = MarketReader::live(&life.account);
    with_bot_state(life, live, |state| {
        let rate = bot_rate(life);
        // A cap (seller reserve, a buy's worth) prices the make path
        // whatever the seats now: the fleet's seats only gate a bid answer.
        state.with_producer_on(life, rate, &[], |producer| {
            let market = |name: &str| reader.view(name);
            items
                .iter()
                .filter_map(|(item, units)| {
                    let unit = lifescape_planner::bid_response::production_cost(item, *units, &market, producer)?;
                    Some((item.clone(), unit.total_for(*units, rate)))
                })
                .collect()
        })
    })
}

/// Gathering seats a chosen lot takes: (method, site) of each made leg
/// whose site holds a limited number of workers
/// (`bid_response::UnitCost::seats`), posted on `site_crowd` so the
/// other bots read them taken.
pub fn engaged_seats(found: &Opportunity) -> Vec<(String, (i64, i64, i64))> {
    found.unit.seats.clone()
}

/// The gathering seats a lot of `count` `item` takes for this bot now
/// (the draw's make path, `bid_response::feasible_lot`, the others' seats
/// read): the engagement a bid goal restores after a daemon restart
/// (`brain_bid_release::reengage_after_restart`). Empty when no make path
/// has a free seat.
pub(crate) fn lot_seats(life: &Life, live: &Value, item: &str, count: i64) -> Vec<(String, (i64, i64, i64))> {
    let reader = MarketReader::live_answering(&life.account);
    with_bot_state(life, live, |state| {
        state.with_producer(life, bot_rate(life), |producer| {
            let market = |name: &str| reader.view(name);
            lifescape_planner::bid_response::feasible_lot(item, count.max(1), &market, producer).map(|(_, unit)| unit.seats).unwrap_or_default()
        })
    })
}

/// Last market-wide choice per account: (wall seconds, spread window,
/// the choice).
type Choice = Option<(Opportunity, f64)>;

fn choices() -> &'static Mutex<HashMap<String, (u64, u64, Choice)>> {
    static TABLE: OnceLock<Mutex<HashMap<String, (u64, u64, Choice)>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn wall_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs()).unwrap_or(0)
}

/// Cache key of a choice: the account, `#wait` for the whole market.
fn choice_key(account: &str, whole: bool) -> String {
    if whole {
        format!("{account}#wait")
    } else {
        account.to_string()
    }
}

/// The cached (or freshly weighed) choice of this life, own trade or
/// whole market.
fn cached_choice(life: &Life, live: &Value, whole: bool) -> Choice {
    if life.profile.ironman || crate::market_service::shared().is_none() {
        return None;
    }
    let window = crate::brain_ambition::stamp_now().tick / lifescape_planner::bid_response::BID_WINDOW_TICKS;
    let now = wall_secs();
    let key = choice_key(&life.account, whole);
    let cached = choices().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&key).cloned();
    match cached {
        Some((at, seen_window, choice)) if now.saturating_sub(at) < CHOICE_TTL_SECS && seen_window == window => choice,
        _ => {
            let reader = MarketReader::live_answering(&life.account);
            let choice = with_bot_state(life, live, |state| choose_in(life, &reader, bot_rate(life), state, whole));
            choices().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(key, (now, window, choice.clone()));
            choice
        }
    }
}

/// True when a lot of the whole market pays this bot now (the check
/// before its ambition goal is set aside, `brain_wait_answer`); the
/// choice is kept for the draw that follows.
pub(crate) fn whole_market_lot(life: &Life, live: &Value) -> bool {
    cached_choice(life, live, true).is_some()
}

/// The bid goal for this life on a draw (live book, rates, own state;
/// its own trade, the whole market when `whole`), logged with its
/// numbers, its claim posted on the board; `None` when no lot beats the
/// ambition.
pub fn bid_goal_in(life: &Life, live: &Value, whole: bool) -> Option<(GoalKind, String)> {
    let _phase = crate::cpu_profile::phase("bids.goal_for");
    let (found, rate) = cached_choice(life, live, whole)?;
    // Taken: the next draw weighs the market again.
    choices().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&choice_key(&life.account, whole));
    let secs = crate::market_claims::claim_secs(found.ticks, crate::market_eta::live_tick_ms());
    crate::market_claims::claim(&life.account, &found.item, found.count, secs);
    // What the lot buys and the gathering seats it takes are no other
    // producer's to count (2026-11-20).
    let purchases = found.purchases();
    crate::market_claims::claim_purchases(&life.account, &purchases, secs);
    let seats = engaged_seats(&found);
    crate::site_crowd::engage(&life.account, &seats, secs);
    if !purchases.is_empty() || !seats.is_empty() {
        let bought: Vec<String> = purchases.iter().map(|(item, units)| format!("{units} {item}")).collect();
        let held: Vec<String> = seats.iter().map(|(method, site)| format!("{method}@{},{},{}", site.0, site.1, site.2)).collect();
        crate::logln!("brain {}: bid answer reserves: buys [{}], seats [{}]", life.account, bought.join(", "), held.join(", "));
    }
    crate::logln!(
        "brain {}: answers the market: produce {} {} ({} to bids, best {}; {} at the current price; {} alched; revenue {:.0}, unit {:.0} coins + {:.0} ticks, lot {:.0} ticks, {:.1} coins/tick vs own {:.2}, net {:.0}, train {:?}; {})",
        life.account,
        found.count,
        found.item,
        found.from_bids,
        found.best_bid,
        found.from_sales,
        found.from_alch,
        found.revenue,
        found.unit.coins,
        found.unit.ticks,
        found.ticks,
        found.rate,
        rate,
        found.profit,
        found.unit.train,
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
