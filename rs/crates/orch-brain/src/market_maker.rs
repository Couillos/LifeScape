#![forbid(unsafe_code)]
//! Dedicated market makers, daemon side (plan marché Q6, wave 3).
//!
//! - Universe: every F2P object with a book of its own
//!   (`gamedata::f2p::market_universe`, 575 objects: the market catalogue
//!   minus banknotes and coins), split over `lifescape_market::maker_count`
//!   slices (slice capacity 54: F2P bank 60 + pack 28 minus 20 kept
//!   slots, 80 % fill) by `lifescape_market::partition`, BALANCED BY
//!   CAPITAL (deterministic, stable: few objects move when one joins).
//!   Each maker account owns one slice index for life.
//! - ONE capital target for every maker (`common_capital_target`):
//!   `lifescape_market::common_capital_target`, the smallest thousand
//!   covering every slice's need (one bid lot on every object at its
//!   public anchor, the shop value, each capped at 10 % of the capital),
//!   a few % over the balanced need (2026-11-16: 103 000 coins).
//! - Migration (`migrate_role`, at every life load): a role written
//!   before the balanced partition keeps its slice index and phase, takes
//!   the common target and the current slice count; a quoting maker's
//!   quotes on objects that left its slice are cancelled at its next pass
//!   (`stale_quotes`), their stock sells as a leftover (`brain_sales`).
//! - Switch (`maybe_switch`): a playing maker whose coins reach the
//!   target cancels its open buys and its listings of slice objects,
//!   then quotes for good (`lifescape_planner::become_quoting`).
//! - Quote pass (`quote_pass`, every `MAKER_PASS_TICKS`): per slice
//!   object, `lifescape_market::maker_quote` from public data (current
//!   price, the others' book, the others' purchases over the demand
//!   lookback) and the maker's own stock, cost and equity; its one bid
//!   and one ask (motive `market_make`, fixed price: no concession curve)
//!   are kept, amended, placed or cancelled; units nobody buys go to the
//!   NPC shop (Q3) through the sales pass (`take_dumps`).
//!
//! Information: public prints, the public book, the maker's own deals and
//! holdings only (D22). No cheat (rule 13): the stock comes from deals.

use std::collections::{BTreeMap, HashMap};

use lifescape_json::{object, str_value, Value};
use lifescape_market::{Leg, MakerInput, MAKER_CAPS};
use lifescape_planner::{Life, MakerRole};

use crate::jobs::Job;

/// Ticks between two quote passes of one maker (60 s at 60 ms).
pub const MAKER_PASS_TICKS: u64 = 1000;
/// Slice objects a maker holds at once: strictly under the bank's
/// janitor line (`lifescape_planner::BANK_TYPE_LIMIT`, 55 types) with
/// coins and the standing kit (axe, pickaxe, hammer, tinderbox) banked:
/// 55 - 1 - 4 - 1 = 49. No bid opens a new type past it, so the janitor
/// never fires on the maker's own stock.
pub const MAX_HELD_TYPES: usize = lifescape_planner::BANK_TYPE_LIMIT - 1 - 4 - 1;
/// Cancel reason of a maker's own requote (`orchestrator economy`).
pub const REQUOTE_REASON: &str = "market_make";
/// Cancel reason of the orders a role switch withdraws.
pub const ROLE_REASON: &str = "role";
/// Cancel reason of a retired maker's quotes (makers unused, `retire`).
pub const RETIRED_REASON: &str = "maker_retired";

/// One object of the universe: id, debug name, public anchor (shop
/// value) and what an F2P shop pays for it (Q3), if any.
#[derive(Clone, Debug)]
pub struct UniverseItem {
    pub id: i64,
    pub name: &'static str,
    pub anchor: i64,
    pub outside: Option<i64>,
}

/// The universe, from the dumps, once.
pub fn universe() -> &'static [UniverseItem] {
    static ALL: std::sync::OnceLock<Vec<UniverseItem>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        lifescape_gamedata::f2p::market_universe()
            .into_iter()
            .filter_map(|obj| {
                let name = obj.name?;
                Some(UniverseItem { id: obj.obj_id as i64, name, anchor: obj.price.max(1), outside: lifescape_gamedata::shop_sale::npc_sale_value(name) })
            })
            .collect()
    })
}

/// The universe as `(id, public anchor)`, the partition's input.
pub fn priced_universe() -> Vec<(i64, i64)> {
    universe().iter().map(|item| (item.id, item.anchor)).collect()
}

/// The slices (object ids), balanced by capital, once.
pub fn slices() -> &'static [Vec<i64>] {
    static ALL: std::sync::OnceLock<Vec<Vec<i64>>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        let priced = priced_universe();
        lifescape_market::partition(&MAKER_CAPS, &priced, lifescape_market::maker_count(priced.len()))
    })
}

/// Objects of one slice (empty past the last slice).
pub fn slice_items(slice: u32) -> Vec<i64> {
    slices().get(slice as usize).cloned().unwrap_or_default()
}

fn item_of(id: i64) -> Option<&'static UniverseItem> {
    universe().iter().find(|item| item.id == id)
}

fn slice_anchors(slice: u32) -> Vec<i64> {
    slice_items(slice).iter().filter_map(|id| item_of(*id)).map(|item| item.anchor).collect()
}

/// THE capital target of every maker (the switch to quoting), once:
/// from the balanced need of the universe over the slices, up to the
/// smallest thousand that covers every slice (`slice_need`).
pub fn common_capital_target() -> i64 {
    static TARGET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *TARGET.get_or_init(|| {
        let anchors: Vec<i64> = universe().iter().map(|item| item.anchor).collect();
        let balanced = lifescape_market::balanced_capital(&MAKER_CAPS, &anchors, slices().len());
        let per_slice: Vec<Vec<i64>> = (0..slices().len() as u32).map(slice_anchors).collect();
        lifescape_market::common_capital_target(&MAKER_CAPS, &per_slice, balanced)
    })
}

/// What quoting one slice asks at the common target (one bid lot on
/// every object, each capped at its risk share).
pub fn slice_need(slice: u32) -> i64 {
    lifescape_market::slice_need(&MAKER_CAPS, &slice_anchors(slice), common_capital_target())
}

/// A role as the current partition reads it: same slice index, phase and
/// baseline; the current slice count and the common target.
pub fn current_role(role: MakerRole) -> MakerRole {
    MakerRole { slices: slices().len() as u32, capital_target: common_capital_target(), ..role }
}

/// Load-time migration of a maker life (balanced partition, 2026-11-16):
/// true when the role changed (the next save writes it). A quoting maker
/// keeps its slice index (reassigned at this restart): objects that left
/// it are no longer quoted (`stale_quotes`) and sell as leftovers, none
/// of its stock is lost. Idempotent; a producer is untouched.
pub fn migrate_role(life: &mut Life) -> bool {
    let lifescape_planner::Archetype::MarketMaker(role) = life.profile.archetype else { return false };
    let current = current_role(role);
    if current == role {
        return false;
    }
    life.profile.archetype = lifescape_planner::Archetype::MarketMaker(current);
    true
}

/// First slice no maker holds yet.
pub fn first_free_slice(taken: &[u32], slices: usize) -> Option<u32> {
    (0..slices as u32).find(|slice| !taken.contains(slice))
}

/// Slices the maker lives in `dir` hold.
pub fn slices_in(dir: &std::path::Path) -> Vec<u32> {
    crate::market_maker_report::maker_lives(dir).into_iter().map(|(_, role)| role.slice).collect()
}

/// Slices the maker lives on disk hold (`create-user` only: a cheap scan).
pub fn taken_slices() -> Vec<u32> {
    slices_in(&crate::paths::lives_dir())
}

/// True when `account`'s life is a market maker.
pub fn is_maker(account: &str) -> bool {
    crate::brain::load_life(account).is_some_and(|life| lifescape_planner::maker_role(&life).is_some())
}

/// The brain's answers to the market engine (`market_facts`): lives and
/// the public anchors, read from the life files and the brain's tables.
pub(crate) struct LifeFacts;

impl crate::market_facts::AccountFacts for LifeFacts {
    fn is_maker(&self, account: &str) -> bool {
        is_maker(account)
    }

    fn price_patience(&self, account: &str) -> Option<f64> {
        crate::brain::load_life(account).map(|life| life.profile.price_patience)
    }

    fn public_anchors(&self, item: &str) -> Option<lifescape_market::discovery::Anchors> {
        // The market engine's callback: the daemon's handles, read now.
        Some(crate::market_discovery::public_anchors(item, &crate::brain_deps::BrainDeps::live()))
    }

    fn seller_quote(&self, service: &crate::market_service::MarketService, request: &lifescape_json::Value) -> lifescape_json::Value {
        crate::market_seller::seller_quote(service, request, &crate::brain_deps::BrainDeps::live())
    }
}

/// Registers the brain's facts with the market engine; `main` calls it
/// first thing, before any market request (tests that price a
/// never-traded bid or read a maker call it too).
pub fn register_account_facts() {
    crate::market_facts::register(std::sync::Arc::new(LifeFacts));
}

/// The role of a new maker account: the first free slice, the common
/// target.
/// A typed refusal when every slice has its maker.
pub fn new_role(taken: &[u32]) -> Result<MakerRole, String> {
    let count = slices().len();
    let slice = first_free_slice(taken, count).ok_or_else(|| format!("every slice already has its market maker ({count} slices)"))?;
    Ok(MakerRole::new(slice, count as u32, common_capital_target()))
}

/// One object position of a maker from its own deals (average cost).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Position {
    pub units: i64,
    /// Cost of the units held (average cost carried out on sales).
    pub cost: i64,
    pub bought_units: i64,
    pub bought_coins: i64,
    pub sold_units: i64,
    pub sold_coins: i64,
}

impl Position {
    /// Realized coins: sales minus purchases.
    pub fn cash(&self) -> i64 {
        self.sold_coins - self.bought_coins
    }
}

/// Positions of `account` from its non-failed deals, oldest first. A sale
/// of units it never bought as a maker (stock from its playing phase)
/// carries no cost.
pub fn positions(account: &str, deals: &[lifescape_store::Deal]) -> BTreeMap<i64, Position> {
    let mut out: BTreeMap<i64, Position> = BTreeMap::new();
    for deal in deals {
        let value = crate::market_units::deal_coins(deal.item_id, deal.quantity, deal.price);
        let position = out.entry(crate::market_audit::book_item(deal.item_id)).or_default();
        if deal.buyer == account {
            position.units += deal.quantity;
            position.cost += value;
            position.bought_units += deal.quantity;
            position.bought_coins += value;
        } else if deal.seller == account {
            if position.units > 0 {
                position.cost -= position.cost * deal.quantity.min(position.units) / position.units;
            }
            position.units = (position.units - deal.quantity).max(0);
            position.sold_units += deal.quantity;
            position.sold_coins += value;
        }
    }
    out
}

/// One move of the reconcile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Cancel(i64),
    Amend(i64, i64),
    Place(Leg),
}

/// Moves that bring one side of one object to `leg` from the maker's
/// resting rows `(order, price, remaining)`: no leg cancels them; one row
/// holding at least half the lot keeps it (amended when the price moved);
/// otherwise the rows go and the lot is placed again.
pub fn reconcile(leg: Option<Leg>, resting: &[(i64, i64, i64)]) -> Vec<Move> {
    let Some(leg) = leg else { return resting.iter().map(|(order, _, _)| Move::Cancel(*order)).collect() };
    let mut moves = Vec::new();
    let keep = resting.iter().find(|(_, _, remaining)| *remaining * 2 >= leg.qty).copied();
    for (order, _, _) in resting {
        if keep.is_none_or(|(kept, _, _)| kept != *order) {
            moves.push(Move::Cancel(*order));
        }
    }
    match keep {
        Some((order, price, _)) if price != leg.price => moves.push(Move::Amend(order, leg.price)),
        Some(_) => {}
        None => moves.push(Move::Place(leg)),
    }
    moves
}

/// Units the maker's sales pass hands to the NPC shop (Q3): `(name,
/// units, what the market pays)` per account, refreshed each quote pass.
fn dumps() -> &'static std::sync::Mutex<HashMap<String, Vec<(String, i64, i64)>>> {
    static TABLE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Vec<(String, i64, i64)>>>> = std::sync::OnceLock::new();
    TABLE.get_or_init(Default::default)
}

/// The maker's pending NPC shop lots (`brain_sales` plans the trip).
pub fn take_dumps(account: &str) -> Vec<(String, i64, i64)> {
    dumps().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(account).unwrap_or_default()
}

fn last_pass() -> &'static std::sync::Mutex<HashMap<String, f64>> {
    static TABLE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, f64>>> = std::sync::OnceLock::new();
    TABLE.get_or_init(Default::default)
}

fn wall_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs_f64()).unwrap_or(0.0)
}

fn ticks_as_secs(ticks: u64) -> f64 {
    ticks as f64 * crate::market_eta::live_tick_ms().max(1) as f64 / 1000.0
}

/// Live stock by base object id (pack and bank, notes folded).
fn live_stock(job: &Job) -> HashMap<i64, i64> {
    let mut stock = HashMap::new();
    for (name, qty) in crate::brain_plan::live_stacks(job) {
        let Some(obj) = lifescape_gamedata::objs_by_name(&name).into_iter().next() else { continue };
        *stock.entry(crate::market_audit::base_of(obj.obj_id as i64)).or_insert(0) += qty;
    }
    stock
}

pub(crate) fn cancel(service: &crate::market_service::MarketService, account: &str, order: i64, reason: &str) -> bool {
    let reply = service.handle(&object(vec![
        ("op", str_value("cancel")),
        ("account", str_value(account)),
        ("order", Value::Int(order)),
        ("reason", str_value(reason)),
    ]));
    reply.get("ok").as_bool() == Some(true)
}

// The switch of a playing maker at its capital target (plan marché Q6) and
// the retirement of a maker while makers are unused (user 2026-11-20) are
// decided by the planner (`upkeep_role::{switch_role, retire_due, ...}`) and
// driven by `brain_upkeep_drive::{maybe_switch, retire}`; the orders they
// cancel go through `cancel` below.

/// Units one shop trip of a maker carries for one object worth `value`
/// at the shop (plan équilibre D2, the outlet's lot): what its risk caps
/// let it hold, the capital share at that value and the stock cap.
pub fn outlet_units(equity: i64, value: i64) -> i64 {
    let by_capital = (MAKER_CAPS.max_item_capital_share * equity.max(0) as f64 / value.max(1) as f64).floor() as i64;
    by_capital.min(MAKER_CAPS.max_stock).max(1)
}

/// One quote pass of a quoting maker (rate-limited): returns the moves
/// made (placed, amended, cancelled).
pub(crate) fn quote_pass(job: &Job, life: &Life, deps: &crate::brain_deps::BrainDeps) -> usize {
    let Some(role) = lifescape_planner::maker_role(life).filter(|role| role.phase == lifescape_planner::MakerPhase::Quoting) else { return 0 };
    let wall = wall_now();
    // The previous pass (none after a daemon start: one pass back).
    let previous = {
        let mut table = last_pass().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if table.get(&job.account).is_some_and(|last| wall - last < ticks_as_secs(MAKER_PASS_TICKS)) {
            return 0;
        }
        table.insert(job.account.clone(), wall)
    };
    let Some(service) = deps.market.clone() else { return 0 };
    let account = job.account.as_str();
    // The maker's time value (D1) prices its shop trips (the outlet, D2);
    // read before any store lock (its refresh locks the store).
    let rate = crate::brain_d17::coin_rate(account);
    let coins = crate::brain_plan::holdings_count(job, "coins", deps);
    let stock = live_stock(job);
    let lookback = ticks_as_secs(lifescape_market::maker::DEMAND_LOOKBACK_TICKS) as i64;
    let now_clock = crate::market_service::MarketService::now_clock();
    let (deals, bought, rows) = {
        let store = service.read_store();
        (
            store.own_deals_since(account, role.switched_at).unwrap_or_default(),
            store.units_bought_since(now_clock - lookback, account).unwrap_or_default(),
            store.account_orders(account).unwrap_or_default(),
        )
    };
    let held = positions(account, &deals);
    let demand: HashMap<i64, i64> = bought.into_iter().collect();
    let items = slice_items(role.slice);
    let references: HashMap<i64, lifescape_market::Reference> = items.iter().map(|id| (*id, service.reference(*id))).collect();
    let equity = coins + items.iter().map(|id| stock.get(id).copied().unwrap_or(0) * references[id].price.max(1)).sum::<i64>();
    let held_types = items.iter().filter(|id| stock.get(id).copied().unwrap_or(0) > 0).count();
    let mut moves = 0;
    for order in stale_quotes(&rows, &items) {
        moves += usize::from(cancel(&service, account, order, REQUOTE_REASON));
    }
    // The shops' likely stock: the fleet's sales and trips, the other
    // bots' arbitrage lots in flight (`brain_npc_sale`).
    let shop_sales = crate::brain_npc_sale::fleet_sales_for(&service, account);
    let mut npc = Vec::new();
    for id in &items {
        let Some(item) = item_of(*id) else { continue };
        let resting = |side: lifescape_store::OrderSide| -> Vec<(i64, i64, i64)> {
            rows.iter()
                .filter(|row| row.side == side && row.motive == crate::market_lineage::MARKET_MAKE && crate::market_audit::book_item(row.item_id) == *id)
                .filter(|row| matches!(row.status, lifescape_store::OrderStatus::Open | lifescape_store::OrderStatus::Matched) && row.remaining > 0)
                .map(|row| (row.order_id, crate::market_units::to_unit(row.item_id, row.price), row.remaining))
                .collect()
        };
        let (bids, asks) = (resting(lifescape_store::OrderSide::Buy), resting(lifescape_store::OrderSide::Sell));
        let (others_bid, others_ask, bid_depth, ask_depth) = others_book(&service, *id, account);
        let units = stock.get(id).copied().unwrap_or(0);
        // M7: a never-traded object is quoted on its public anchor (the
        // published production cost); read only then (a traded object's
        // fair value is its current price).
        let make_cost = if references[id].traded { None } else { crate::market_discovery::public_anchors(item.name, deps).make_cost };
        let input = MakerInput {
            reference: references[id],
            best_bid: others_bid,
            best_ask: others_ask,
            bid_depth,
            ask_depth,
            recent_buys: demand.get(id).copied().unwrap_or(0),
            anchor: item.anchor,
            outside: item.outside,
            outlet: item.outside.and_then(|value| crate::brain_npc_sale::net_shop_outlet(item.name, &shop_sales, outlet_units(equity, value), rate)),
            make_cost,
            stock: units,
            stock_cost: held.get(id).map(|position| position.cost).unwrap_or(0),
            capital: equity,
            // Unserved since the previous pass (no buy of it since): the
            // bid climbs 5 % (`maker::quote`); bought, a fresh quote.
            own_bid: bids.first().map(|(_, price, _)| *price).filter(|_| !bought_since(account, *id, &deals, previous.unwrap_or(wall - ticks_as_secs(MAKER_PASS_TICKS)))),
            own_ask: asks.first().map(|(_, price, _)| *price),
        };
        let quote = lifescape_market::maker_quote(&MAKER_CAPS, &input);
        let bid = quote.bid.filter(|_| may_bid(held_types, units));
        // Plan prix lot D: the ask offers whole lots of the stock, none
        // under one lot (the market refuses it).
        let quote_lot = crate::market_units::lot_of(*id);
        let quote = lifescape_market::MakerQuote {
            ask: quote.ask.and_then(|leg| {
                let qty = lifescape_market::lots::whole_lots_down(leg.qty, quote_lot);
                (qty > 0).then_some(lifescape_market::Leg { qty, ..leg })
            }),
            ..quote
        };
        for (side, leg, rows_now) in [("buy", bid, &bids), ("sell", quote.ask, &asks)] {
            for planned in reconcile(leg, rows_now) {
                moves += apply(&service, account, *id, side, planned, item.name);
            }
        }
        if quote.npc_dump > 0 {
            npc.push((item.name.to_string(), quote.npc_dump, lifescape_market::market_pays(others_bid, references[id])));
        }
    }
    let mut table = dumps().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if npc.is_empty() {
        table.remove(account);
    } else {
        table.insert(account.to_string(), npc);
    }
    moves
}

/// True when `account` bought `item` (book object) in a deal opened at
/// or after `since` (unix seconds): its bid was served this pass.
pub fn bought_since(account: &str, item: i64, deals: &[lifescape_store::Deal], since: f64) -> bool {
    deals.iter().any(|deal| deal.buyer == account && crate::market_audit::book_item(deal.item_id) == item && deal.created_clock as f64 >= since)
}

/// Others' best bid, best ask and resting units per side on one book
/// (the maker's own orders left out: public data).
fn others_book(service: &crate::market_service::MarketService, item: i64, account: &str) -> (Option<i64>, Option<i64>, i64, i64) {
    let engine = service.lock_engine();
    let Some(book) = engine.books.get(&item) else { return (None, None, 0, 0) };
    let (mut bid, mut ask, mut bid_depth, mut ask_depth) = (None::<i64>, None::<i64>, 0, 0);
    // Per unit (plan prix lot D: the maker quotes in the brain's unit).
    for order in book.orders().into_iter().filter(|order| order.account != account && order.remaining > 0) {
        let price = crate::market_units::to_unit(item, order.price);
        if order.side == lifescape_market::Side::Buy {
            bid = Some(bid.map_or(price, |best| best.max(price)));
            bid_depth += order.remaining;
        } else {
            ask = Some(ask.map_or(price, |best| best.min(price)));
            ask_depth += order.remaining;
        }
    }
    (bid, ask, bid_depth, ask_depth)
}

/// True when the free coins cover a buy move (`free`, beyond every resting
/// bid; `resting`: the amended order's remaining units and per-lot price):
/// a placed leg whole, a raise by its extra coins (`affordable_raise`); a
/// cut, a sell or a cancel always (user 2026-11-19, point 1: a maker never
/// asks for coins it does not hold, never retries a refused bid a pass).
pub(crate) fn move_covered(item: i64, buying: bool, planned: &Move, free: i64, resting: Option<(i64, i64)>) -> bool {
    if !buying {
        return true;
    }
    match planned {
        Move::Cancel(_) => true,
        Move::Place(leg) => crate::market_units::deal_coins(item, leg.qty, crate::market_units::to_lot(item, leg.price)) <= free,
        Move::Amend(_, price) => {
            let wanted = crate::market_units::to_lot(item, *price);
            resting.is_some_and(|(remaining, old)| crate::market_audit::affordable_raise(item, remaining, old, wanted, free) >= wanted)
        }
    }
}

/// A sell move cut to the maker's free stock (`free`: held, net of its
/// resting asks and in-flight sales): a placed leg lists at most that,
/// in whole lots, nothing when none is free; a reprice or a cancel moves
/// no stock (user 2026-11-19: « un teneur ne doit mettre en vente que ce
/// qu'il possède »; live rae_stone 370 buckets listed with 131 free).
pub(crate) fn sell_leg_within(item: i64, planned: Move, free: i64) -> Option<Move> {
    match planned {
        Move::Place(leg) => {
            let qty = lifescape_market::lots::whole_lots_down(leg.qty.min(free.max(0)), crate::market_units::lot_of(item));
            (qty > 0).then_some(Move::Place(Leg { qty, ..leg }))
        }
        other => Some(other),
    }
}

fn apply(service: &crate::market_service::MarketService, account: &str, item: i64, side: &str, planned: Move, name: &str) -> usize {
    let planned = if side == "sell" {
        let (free, _) = service.free_coverage(account, lifescape_store::OrderSide::Sell, item);
        match sell_leg_within(item, planned, free) {
            Some(fitted) => fitted,
            // Nothing free to list: the leg waits for stock, silently.
            None => return 0,
        }
    } else {
        planned
    };
    if side == "buy" {
        let (_, free) = service.free_coverage(account, lifescape_store::OrderSide::Buy, item);
        let resting = match planned {
            Move::Amend(order, _) => service.read_store().get_order(order).ok().flatten().map(|row| (row.remaining, row.price)),
            _ => None,
        };
        if !move_covered(item, true, &planned, free, resting) {
            // The leg waits for coins, silently: no refused bid a pass.
            return 0;
        }
    }
    let reply = match planned {
        Move::Cancel(order) => return usize::from(cancel(service, account, order, REQUOTE_REASON)),
        Move::Amend(order, price) => {
            service.handle(&object(vec![("op", str_value("amend")), ("account", str_value(account)), ("order", Value::Int(order)), ("price", Value::Int(price))]))
        }
        Move::Place(leg) => service.handle(&object(vec![
            ("op", str_value("place")),
            ("account", str_value(account)),
            ("side", str_value(side)),
            ("item", Value::Int(item)),
            ("qty", Value::Int(leg.qty)),
            ("price", Value::Int(leg.price)),
            ("motive", str_value(crate::market_lineage::MARKET_MAKE)),
        ])),
    };
    if reply.get("ok").as_bool() == Some(true) {
        return 1;
    }
    crate::logln!("market maker {account}: {side} {name} {planned:?} refused: {}", reply.get("error").as_str().unwrap_or("?"));
    0
}

/// The maker's resting quotes (motive `market_make`) on objects outside
/// its slice `items`: after a repartition, cancelled by the next pass.
pub fn stale_quotes(rows: &[lifescape_store::Order], items: &[i64]) -> Vec<i64> {
    rows.iter()
        .filter(|row| row.motive == crate::market_lineage::MARKET_MAKE && matches!(row.status, lifescape_store::OrderStatus::Open | lifescape_store::OrderStatus::Matched) && row.remaining > 0)
        .filter(|row| !items.contains(&crate::market_audit::book_item(row.item_id)))
        .map(|row| row.order_id)
        .collect()
}

/// A bid may open a new held type only under `MAX_HELD_TYPES`; a bid on
/// an object already held always may.
pub fn may_bid(held_types: usize, stock: i64) -> bool {
    stock > 0 || held_types < MAX_HELD_TYPES
}

/// Debug names of a quoting maker's slice (the janitor's keep list).
pub fn slice_names(life: &Life) -> Vec<String> {
    match lifescape_planner::maker_role(life) {
        Some(role) if role.phase == lifescape_planner::MakerPhase::Quoting => slice_items(role.slice).iter().filter_map(|id| item_of(*id)).map(|item| item.name.to_string()).collect(),
        _ => Vec::new(),
    }
}

/// True when `item` (base id) belongs to the quoting maker's slice: its
/// stock is quoted by `quote_pass` only, never listed by the sales pass.
pub fn quoted_by_maker(life: &Life, item: i64) -> bool {
    lifescape_planner::maker_role(life)
        .filter(|role| role.phase == lifescape_planner::MakerPhase::Quoting)
        .is_some_and(|role| slice_items(role.slice).contains(&crate::market_audit::book_item(item)))
}

#[cfg(test)]
#[path = "market_maker_tests.rs"]
mod tests;
