#![forbid(unsafe_code)]
//! Physical handover: ring tiles, staggered convocations, sweeps.
//!
//! Pure core (`build_ring`, `pair_tiles`, `stagger`, `convenable`) works
//! on caller-supplied values, so unit tests pin it without a game. The
//! service half owns the convene state the socket exposes (`pending`,
//! `convene`, `sweep`): tile allocation, state machine, ledger refunds
//! and priority-preserving relists. The loop half (`start_convene_loop`)
//! runs daemon-side: positions in, poses out, ETAs via the nav service.
//!
//! Division of knowledge (D22): the market knows both parties' positions
//! to arrange the meeting; each bot only ever receives its own tile and
//! its own departure tick in the pose args.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lifescape_json::{object, str_value, Value};
use lifescape_market::{Deal, DealStatus, FailReason, Fill, Order, Side};
use lifescape_nav::plan::Tile;

use crate::daemon::Orchestrator;
use crate::market_eta::{EtaCache, TickMeter};
use crate::market_service::MarketService;

/// Matched deals wait this long (server ticks) before a convocation, so
/// one trip can embarquer the other matches of the same bot.
pub const GROUPING_TICKS: u64 = 300;
/// Head start before the first departure (bank withdrawal, nearby walk).
pub const PREP_MARGIN_TICKS: u64 = 30;
/// Slack past the last expected arrival before the deal expires: covers
/// the bank dance plus both trade screens with room to spare.
pub const DEADLINE_MARGIN_TICKS: u64 = 600;
/// Convene loop cadence (wall clock): poses and sweeps, never plans past
/// one per unknown zone.
pub const CONVENE_EVERY_SECS: u64 = 10;

// -- ring ------------------------------------------------------------------

/// Inner rectangle around the basin: x 3210..=3216, z 3426..=3432.
fn inner_perimeter() -> Vec<(i64, i64)> {
    let mut tiles = Vec::new();
    for x in 3210..=3216 {
        tiles.push((x, 3426));
    }
    for z in 3427..=3432 {
        tiles.push((3216, z));
    }
    for x in (3210..=3215).rev() {
        tiles.push((x, 3432));
    }
    for z in (3427..=3431).rev() {
        tiles.push((3210, z));
    }
    tiles
}

/// Second ring when the first is full: x 3209..=3217, z 3425..=3433.
fn outer_perimeter() -> Vec<(i64, i64)> {
    let mut tiles = Vec::new();
    for x in 3209..=3217 {
        tiles.push((x, 3425));
    }
    for z in 3426..=3433 {
        tiles.push((3217, z));
    }
    for x in (3209..=3216).rev() {
        tiles.push((x, 3433));
    }
    for z in (3426..=3432).rev() {
        tiles.push((3209, z));
    }
    tiles
}

/// Basin water excluded from either perimeter (x 3211..=3215, z 3427..=3431).
fn in_basin(x: i64, z: i64) -> bool {
    (3211..=3215).contains(&x) && (3427..=3431).contains(&z)
}

/// Ordered handover ring: inner perimeter then outer, each filtered by
/// the caller's walkability predicate (walkable, no loc, no door, no
/// toll). Adjacent pairs in the ring are adjacent tiles by construction.
pub fn build_ring(tile_open: &dyn Fn(i32, i32) -> bool) -> Vec<(i64, i64)> {
    inner_perimeter()
        .into_iter()
        .chain(outer_perimeter())
        .filter(|(x, z)| !in_basin(*x, *z) && tile_open(*x as i32, *z as i32))
        .collect()
}

/// Ring from the daemon snapshot through the nav `tiles_flags` batch op:
/// walkable and free of busts, stalls and doors. Read-only nav use.
pub fn build_ring_from_snapshot(snapshot: &lifescape_nav::service::Snapshot) -> Vec<(i64, i64)> {
    let candidates: Vec<Tile> = inner_perimeter()
        .into_iter()
        .chain(outer_perimeter())
        .map(|(x, z)| Tile { x: x as i32, z: z as i32, level: 0 })
        .collect();
    let rows = lifescape_nav::service_queries::tiles_flags(snapshot, &candidates);
    let open: HashSet<(i32, i32)> = rows
        .iter()
        .filter(|row| {
            row.flags == lifescape_nav::collision::OPEN && row.loc.is_none() && row.door.is_none() && !row.toll
        })
        .map(|row| (row.tile.x, row.tile.z))
        .collect();
    build_ring(&|x, z| open.contains(&(x, z)))
}

/// First free adjacent pair in ring order, `None` when the square is
/// full (the deals queue: no random tile, no stacking on one case).
pub fn pair_tiles(ring: &[(i64, i64)], busy: &HashSet<(i64, i64)>) -> Option<((i64, i64), (i64, i64))> {
    let members: HashSet<(i64, i64)> = ring.iter().cloned().collect();
    for first in ring {
        if busy.contains(first) {
            continue;
        }
        for dx in -1..=1 {
            for dz in -1..=1 {
                if dx == 0 && dz == 0 {
                    continue;
                }
                let second = (first.0 + dx, first.1 + dz);
                if members.contains(&second) && !busy.contains(&second) {
                    return Some((*first, second));
                }
            }
        }
    }
    None
}

// -- convene math ------------------------------------------------------------

/// True once a matched deal waited out the grouping window. Wall
/// clock: session ticks are per-login epochs, never comparable across
/// bots, so grouping and sweeps run on the daemon wall.
pub fn convenable(since_wall: f64, now_wall: f64, tick_ms: u64) -> bool {
    now_wall - since_wall >= grouping_secs(tick_ms)
}

/// Grouping window in wall seconds: the tick span at the live rate.
pub fn grouping_secs(tick_ms: u64) -> f64 {
    GROUPING_TICKS as f64 * tick_ms as f64 / 1000.0
}

/// Wall seconds until a sweep may expire the deal: the tick span at
/// the live rate plus pose latency for an engaged bot (its gate only
/// poses at the next safe point).
pub fn sweep_after_secs(span_ticks: u64, tick_ms: u64) -> f64 {
    const POSE_SLACK_TICKS: u64 = 1200;
    (span_ticks + POSE_SLACK_TICKS) as f64 * tick_ms as f64 / 1000.0
}

/// Daemon wall clock in seconds.
pub fn wall_now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|span| span.as_secs_f64()).unwrap_or(0.0)
}


/// Staggered departures plus per-party deadlines: the farther bot
/// leaves first (both land together). Ticks are per-login epochs, so
/// each deadline rides its own bot's clock; the shared wall instant for
/// the sweep is computed caller-side from the same spans.
pub fn stagger(tick_a: u64, eta_a: u64, tick_b: u64, eta_b: u64) -> (u64, u64, u64, u64) {
    let longest = eta_a.max(eta_b);
    let span = PREP_MARGIN_TICKS + longest.saturating_mul(2) + DEADLINE_MARGIN_TICKS;
    let depart_a = tick_a + PREP_MARGIN_TICKS + longest.saturating_sub(eta_a);
    let depart_b = tick_b + PREP_MARGIN_TICKS + longest.saturating_sub(eta_b);
    (depart_a, tick_a + span, depart_b, tick_b + span)
}

// -- deal ops (state machine behind the socket) -------------------------------

/// Consumed book rows for one fill, stamped before the match: the
/// resting row keeps its original `seq` (priority survives a failed
/// handover), the taker row the rank it consumed. Ids are placeholders
/// (0), assigned at relist time.
pub(crate) fn relist_for_fill(
    pre: &HashMap<u64, lifescape_market::Order>,
    taker_seq: u64,
    side: Side,
    account: &str,
    price: i64,
    fill: &Fill,
) -> Vec<Order> {
    let mut rows = Vec::new();
    let resting_id = if side == Side::Buy { fill.sell_id } else { fill.buy_id };
    if let Some(resting) = pre.get(&resting_id) {
        let mut row = resting.clone();
        row.id = 0;
        row.quantity = fill.quantity;
        row.remaining = fill.quantity;
        rows.push(row);
    }
    rows.push(Order {
        id: 0,
        account: account.to_string(),
        item: fill.item,
        side,
        quantity: fill.quantity,
        remaining: fill.quantity,
        price,
        seq: taker_seq,
    });
    rows
}

impl MarketService {
    /// Persists one fill: execution row, ledger move, live-tail print and
    /// an auto-convened deal (meeting tile + deadline assigned now).
    pub(crate) fn record_fill(&self, fill: &lifescape_market::Fill, relist: Vec<Order>, clock: i64) -> Result<i64, String> {
        let store = self.lock_store();
        store
            .record_execution(&lifescape_store::NewExecution {
                item_id: fill.item,
                quantity: fill.quantity,
                price: fill.price,
                clock,
                server_tick: clock,
            })
            .map_err(|error| error.to_string())?;
        drop(store);
        {
            let mut engine = self.lock_engine();
            let cost = fill.quantity * fill.price;
            engine.ledger.credit_coins(&fill.buyer, -cost);
            engine.ledger.credit_stock(&fill.buyer, fill.item, fill.quantity);
            engine.ledger.credit_stock(&fill.seller, fill.item, -fill.quantity);
            engine.ledger.credit_coins(&fill.seller, cost);
            engine.matched += 1;
            engine.remember(crate::market_service::Print { item: fill.item, quantity: fill.quantity, price: fill.price, clock });
        }
        let (tile_x, tile_y) = {
            let engine = self.lock_engine();
            let slot = (engine.deals.len() % crate::market_service::RING.len()) as usize;
            crate::market_service::RING[slot]
        };
        let deal_id = {
            let store = self.lock_store();
            store
                .create_deal(&lifescape_store::NewDeal {
                    buyer: fill.buyer.clone(),
                    seller: fill.seller.clone(),
                    item_id: fill.item,
                    quantity: fill.quantity,
                    price: fill.price,
                    meeting_x: tile_x,
                    meeting_y: tile_y,
                    meeting_level: 0,
                    deadline_tick: clock + 3600,
                    clock,
                })
                .map_err(|error| error.to_string())?
        };
        self.lock_store()
            .set_deal_status(deal_id, lifescape_store::DealStatus::Matched, "", clock)
            .map_err(|error| error.to_string())?;
        let mut engine = self.lock_engine();
        if let Ok(deal) = Deal::fresh(
            deal_id as u64,
            &fill.buyer,
            &fill.seller,
            fill.item,
            fill.quantity,
            fill.price,
            tile_x,
            tile_y,
            clock + 3600,
        ) {
            // Fresh matches wait for a convocation (grouping window,
            // staggered departures); `op_convene` moves them on.
            engine.deals.insert(deal_id as u64, deal);
            engine.arrived.insert(deal_id as u64, (false, false));
            engine.relist_rows.insert(deal_id as u64, relist);
        }
        Ok(deal_id)
    }

    /// Writes post-match remaining quantities back to the store rows.
    pub(crate) fn refresh_order_rows(&self, incoming: u64, fills: &[lifescape_market::Fill], clock: i64) {
        let engine = self.lock_engine();
        let mut touched = vec![incoming];
        for fill in fills {
            touched.push(fill.buy_id);
            touched.push(fill.sell_id);
        }
        touched.sort_unstable();
        touched.dedup();
        let remaining: Vec<(u64, Option<i64>)> =
            touched.iter().map(|id| (*id, engine.books.values().filter_map(|book| book.remaining(*id)).next())).collect();
        drop(engine);
        let store = self.lock_store();
        for (id, left) in remaining {
            let (status, qty) = match left {
                // Untouched incoming order stays open; a resting order
                // drained by fills settles, a partial one shows matched.
                Some(left) if id == incoming && fills.is_empty() => (lifescape_store::OrderStatus::Open, left),
                Some(left) if left > 0 => (lifescape_store::OrderStatus::Matched, left),
                _ => (lifescape_store::OrderStatus::Settled, 0),
            };
            let _ = store.set_order_status(id as i64, status, qty, clock);
        }
    }

    pub(crate) fn op_my_deals(&self, request: &Value) -> Value {
        let account = request.get("account").as_str().unwrap_or("");
        let store = self.lock_store();
        let rows = store.open_deals_for_account(account).unwrap_or_default();
        let values: Vec<Value> = rows
            .iter()
            .map(|row| {
                object(vec![
                    ("deal", Value::Int(row.deal_id)),
                    ("buyer", str_value(&row.buyer)),
                    ("seller", str_value(&row.seller)),
                    ("item", Value::Int(row.item_id)),
                    ("qty", Value::Int(row.quantity)),
                    ("price", Value::Int(row.price)),
                    ("x", Value::Int(row.meeting_x)),
                    ("y", Value::Int(row.meeting_y)),
                    ("status", str_value(match row.status {
                        lifescape_store::DealStatus::Matched => "matched",
                        lifescape_store::DealStatus::Convened => "convened",
                        lifescape_store::DealStatus::BothArrived => "both_arrived",
                        lifescape_store::DealStatus::Trading => "trading",
                        lifescape_store::DealStatus::Settled => "settled",
                        lifescape_store::DealStatus::Failed => "failed",
                    })),
                    ("reason", str_value(&row.fail_reason)),
                    ("deadline", Value::Int(row.deadline_tick)),
                ])
            })
            .collect();
        object(vec![("ok", Value::Bool(true)), ("deals", Value::Arr(values))])
    }

    pub(crate) fn op_deal_arrived(&self, request: &Value) -> Value {
        let account = request.get("account").as_str().unwrap_or("");
        let deal_id = request.get("deal").as_i64().unwrap_or(-1);
        let (buyer, seller, status) = {
            let engine = self.lock_engine();
            let Some(deal) = engine.deals.get(&(deal_id as u64)) else {
                return Self::err(format!("unknown deal {deal_id}"));
            };
            if account != deal.buyer && account != deal.seller {
                return Self::err("not your deal".to_string());
            }
            if deal.status != DealStatus::Convened {
                return Self::err(format!("deal {deal_id} is {}", deal.status.as_str()));
            }
            (deal.buyer.clone(), deal.seller.clone(), deal.status.as_str().to_string())
        };
        let mut engine = self.lock_engine();
        let mut marks = engine.arrived.get(&(deal_id as u64)).cloned().unwrap_or((false, false));
        if account == buyer {
            marks.0 = true;
        } else if account == seller {
            marks.1 = true;
        }
        engine.arrived.insert(deal_id as u64, marks);
        let both = marks.0 && marks.1;
        drop(engine);
        if both {
            let mut engine = self.lock_engine();
            if let Some(deal) = engine.deals.get_mut(&(deal_id as u64)) {
                if deal.transition(DealStatus::BothArrived, None).is_ok() {
                    self.lock_store()
                        .set_deal_status(deal_id, lifescape_store::DealStatus::BothArrived, "", Self::now_clock())
                        .ok();
                }
            }
            return object(vec![("ok", Value::Bool(true)), ("deal", Value::Int(deal_id)), ("status", str_value("both_arrived"))]);
        }
        object(vec![("ok", Value::Bool(true)), ("deal", Value::Int(deal_id)), ("status", str_value(&status))])
    }

    pub(crate) fn op_deal_report(&self, request: &Value) -> Value {
        let account = request.get("account").as_str().unwrap_or("");
        let deal_id = request.get("deal").as_i64().unwrap_or(-1);
        let outcome = request.get("outcome").as_str().unwrap_or("").to_string();
        let reason = request.get("reason").as_str().unwrap_or("").to_string();
        let next = match outcome.as_str() {
            "trading" => DealStatus::Trading,
            "settled" => DealStatus::Settled,
            "failed" => DealStatus::Failed,
            _ => return Self::err(format!("bad outcome '{outcome}'")),
        };
        let fail = if next == DealStatus::Failed {
            match FailReason::parse(&reason) {
                Ok(why) => Some(why),
                Err(error) => return Self::err(error.to_string()),
            }
        } else if !reason.is_empty() {
            return Self::err("reason only with failed".to_string());
        } else {
            None
        };
        let clock = Self::now_clock();
        // Snapshot the parties first: the ledger refund below needs them
        // without holding the engine lock across the store calls.
        let (buyer, seller, item, qty, price) = {
            let engine = self.lock_engine();
            let Some(deal) = engine.deals.get(&(deal_id as u64)) else {
                return Self::err(format!("unknown deal {deal_id}"));
            };
            if account != deal.buyer && account != deal.seller {
                return Self::err("not your deal".to_string());
            }
            (deal.buyer.clone(), deal.seller.clone(), deal.item, deal.quantity, deal.price)
        };
        let moved = {
            let mut engine = self.lock_engine();
            let Some(deal) = engine.deals.get_mut(&(deal_id as u64)) else {
                return Self::err(format!("unknown deal {deal_id}"));
            };
            match deal.transition(next, fail) {
                Ok(()) => true,
                Err(error) => return Self::err(error.to_string()),
            }
        };
        assert!(moved);
        let store_status = match next {
            DealStatus::Trading => lifescape_store::DealStatus::Trading,
            DealStatus::Settled => lifescape_store::DealStatus::Settled,
            _ => lifescape_store::DealStatus::Failed,
        };
        self.lock_store().set_deal_status(deal_id, store_status, fail.map(|why| why.as_str()).unwrap_or(""), clock).ok();
        if next == DealStatus::Failed {
            let why = fail.unwrap_or(FailReason::Timeout);
            // Arrival-gated outcome: the parties that showed up relist
            // with priority and keep reliability; the absent lose both.
            let marks = self.lock_engine().arrived.get(&(deal_id as u64)).cloned().unwrap_or((false, false));
            let (relist_buyer, relist_seller, reliability) = match why {
                FailReason::Mismatch => {
                    let reporter_is_buyer = account == buyer;
                    (
                        reporter_is_buyer,
                        !reporter_is_buyer,
                        (Some(reporter_is_buyer), Some(!reporter_is_buyer)),
                    )
                }
                FailReason::NoShow => (marks.0, marks.1, (arrival_score(marks.0), arrival_score(marks.1))),
                _ => (true, true, (None, None)),
            };
            self.refund_and_relist(deal_id as u64, &buyer, &seller, item, qty, price, relist_buyer, relist_seller, reliability);
        }
        if next == DealStatus::Settled {
            self.release_convene(deal_id as u64);
            let store = self.lock_store();
            let _ = store.record_deal_outcome(&buyer, true);
            let _ = store.record_deal_outcome(&seller, true);
        }
        object(vec![("ok", Value::Bool(true)), ("deal", Value::Int(deal_id)), ("status", str_value(next.as_str()))])
    }

    /// Ledger refund plus selective relist after a failed handover. The
    /// relisted rows ride their original `seq`: the party that showed up
    /// returns to the carnet with its priority, the absent one re-places
    /// by hand. Reliability only moves on verdicts with a party at
    /// fault (no-show, mismatch); death, timeout and nav_down stay
    /// neutral.
    pub(crate) fn refund_and_relist(
        &self,
        deal_id: u64,
        buyer: &str,
        seller: &str,
        item: i64,
        qty: i64,
        price: i64,
        relist_buyer: bool,
        relist_seller: bool,
        reliability: (Option<bool>, Option<bool>),
    ) {
        {
            let mut engine = self.lock_engine();
            let cost = qty * price;
            engine.ledger.credit_coins(buyer, cost);
            engine.ledger.credit_stock(buyer, item, -qty);
            engine.ledger.credit_stock(seller, item, qty);
            engine.ledger.credit_coins(seller, -cost);
        }
        self.release_convene(deal_id);
        let rows = self.lock_engine().relist_rows.remove(&deal_id).unwrap_or_default();
        let clock = Self::now_clock();
        for row in rows {
            let wanted = (row.account == buyer && relist_buyer) || (row.account == seller && relist_seller);
            if !wanted || row.remaining <= 0 {
                continue;
            }
            let store_id = {
                let store = self.lock_store();
                store
                    .place_order(&lifescape_store::NewOrder {
                        account: row.account.clone(),
                        side: if row.side == Side::Buy {
                            lifescape_store::OrderSide::Buy
                        } else {
                            lifescape_store::OrderSide::Sell
                        },
                        item_id: row.item,
                        quantity: row.remaining,
                        price: row.price,
                        dormant: false,
                        clock,
                    })
                    .ok()
            };
            let Some(store_id) = store_id else { continue };
            let mut engine = self.lock_engine();
            let mut relisted = row;
            relisted.id = store_id as u64;
            relisted.quantity = relisted.remaining;
            if let Some(book) = engine.books.get_mut(&item) {
                book.relist(relisted);
            }
            engine.order_item.insert(store_id as u64, item);
        }
        let store = self.lock_store();
        if let Some(score) = reliability.0 {
            let _ = store.record_deal_outcome(buyer, score);
        }
        if let Some(score) = reliability.1 {
            let _ = store.record_deal_outcome(seller, score);
        }
    }

    /// Frees convene state (tiles, deadline, account holds) at a terminal
    /// deal, whatever the outcome.
    pub(crate) fn release_convene(&self, deal_id: u64) {
        let mut engine = self.lock_engine();
        if let Some(tiles) = engine.deal_tiles.remove(&deal_id) {
            engine.ring_busy.remove(&tiles.0);
            engine.ring_busy.remove(&tiles.1);
        }
        engine.deal_sweep_wall.remove(&deal_id);
        engine.pending_since_wall.remove(&deal_id);
        engine.arrived.remove(&deal_id);
        let parties = engine.deals.get(&deal_id).map(|deal| (deal.buyer.clone(), deal.seller.clone()));
        if let Some((buyer, seller)) = parties {
            engine.convened_accounts.remove(&buyer);
            engine.convened_accounts.remove(&seller);
        }
    }

    /// Matched deals awaiting convocation, with the grouping verdict and
    /// the square destination for ETA plans. Marks first sightings: a
    /// matched deal convenes once it waited out the grouping window.
    pub(crate) fn op_pending(&self, request: &Value) -> Value {
        let now_wall = request.get("now_wall").as_f64().unwrap_or(0.0);
        let tick_ms = request.get("tick_ms").as_i64().unwrap_or(60).max(1) as u64;
        let mut engine = self.lock_engine();
        let dest = engine.ring.first().cloned().unwrap_or(crate::market_service::RING[0]);
        let mut deals = Vec::new();
        let mut ids: Vec<u64> = engine.deals.iter().filter(|(_, deal)| deal.status == DealStatus::Matched).map(|(id, _)| *id).collect();
        ids.sort_unstable();
        for id in ids {
            let snapshot = engine.deals.get(&id).map(|deal| {
                (deal.buyer.clone(), deal.seller.clone(), deal.item, deal.quantity, deal.price)
            });
            let Some((buyer, seller, item, qty, price)) = snapshot else {
                continue;
            };
            let since = *engine.pending_since_wall.entry(id).or_insert(now_wall);
            deals.push(object(vec![
                ("deal", Value::Int(id as i64)),
                ("buyer", str_value(&buyer)),
                ("seller", str_value(&seller)),
                ("item", Value::Int(item)),
                ("qty", Value::Int(qty)),
                ("price", Value::Int(price)),
                ("convenable", Value::Bool(convenable(since, now_wall, tick_ms))),
            ]));
        }
        object(vec![
            ("ok", Value::Bool(true)),
            ("dest", Value::Arr(vec![Value::Int(dest.0), Value::Int(dest.1)])),
            ("deals", Value::Arr(deals)),
        ])
    }

    /// Convenes matched deals: adjacent tile pair, per-party departures,
    /// shared deadline. One convocation per bot: any deal touching an
    /// already convened account is refused. Replies carry the pose args
    /// per account (own tiles only, never the partner's position).
    pub(crate) fn op_convene(&self, request: &Value) -> Value {
        let wanted = request.get("deals").as_arr().cloned().unwrap_or_default();
        let mut engine = self.lock_engine();
        let mut poses: HashMap<String, Vec<Value>> = HashMap::new();
        let mut departs: HashMap<String, u64> = HashMap::new();
        let mut deadlines: HashMap<String, u64> = HashMap::new();
        let mut convened = Vec::new();
        // One convocation per bot, regrouped: accounts convened BEFORE
        // this batch refuse new deals, but several deals of one account
        // inside the same batch convene together in a single pose.
        let held: HashSet<String> = engine.convened_accounts.iter().cloned().collect();
        for entry in wanted {
            let deal_id = entry.get("deal").as_i64().unwrap_or(-1) as u64;
            let snapshot = engine.deals.get(&deal_id).map(|deal| {
                (deal.status, deal.buyer.clone(), deal.seller.clone())
            });
            let Some((status, buyer, seller)) = snapshot else {
                continue;
            };
            if status != DealStatus::Matched {
                continue;
            }
            if held.contains(&buyer) || held.contains(&seller) {
                continue;
            }
            let ring: Vec<(i64, i64)> =
                if engine.ring.is_empty() { crate::market_service::RING.to_vec() } else { engine.ring.clone() };
            let Some(pair) = pair_tiles(&ring, &engine.ring_busy) else {
                continue;
            };
            let depart_buyer = entry.get("depart_buyer").as_i64().unwrap_or(0).max(0) as u64;
            let depart_seller = entry.get("depart_seller").as_i64().unwrap_or(0).max(0) as u64;
            let deadline_buyer = entry.get("deadline_buyer").as_i64().unwrap_or(0).max(0) as u64;
            let deadline_seller = entry.get("deadline_seller").as_i64().unwrap_or(0).max(0) as u64;
            let sweep_wall = entry.get("sweep_wall").as_f64().unwrap_or(0.0);
            let Some(deal) = engine.deals.get_mut(&deal_id) else {
                continue;
            };
            if deal.transition(DealStatus::Convened, None).is_err() {
                continue;
            }
            deal.meeting_x = pair.0.0;
            deal.meeting_y = pair.0.1;
            engine.ring_busy.insert(pair.0);
            engine.ring_busy.insert(pair.1);
            engine.deal_tiles.insert(deal_id, pair);
            engine.deal_sweep_wall.insert(deal_id, sweep_wall);
            engine.convened_accounts.insert(buyer.clone());
            engine.convened_accounts.insert(seller.clone());
            engine.pending_since_wall.remove(&deal_id);
            convened.push(Value::Int(deal_id as i64));
            poses.entry(buyer.clone()).or_default().push(str_value(&format!("{deal_id}={},{}", pair.0.0, pair.0.1)));
            poses.entry(seller.clone()).or_default().push(str_value(&format!("{deal_id}={},{}", pair.1.0, pair.1.1)));
            departs.insert(buyer.clone(), depart_buyer);
            departs.insert(seller.clone(), depart_seller);
            // Regrouped deals share one pose: the latest deadline per
            // account covers every deal in it.
            for (party, own) in [(&buyer, deadline_buyer), (&seller, deadline_seller)] {
                let held = deadlines.get(party).cloned().unwrap_or(0);
                deadlines.insert(party.clone(), held.max(own));
            }
        }
        drop(engine);
        let clock = Self::now_clock();
        for deal_id in convened.iter().filter_map(|value| value.as_i64()) {
            self.lock_store().set_deal_status(deal_id, lifescape_store::DealStatus::Convened, "", clock).ok();
        }
        let mut out = Vec::new();
        let mut accounts: Vec<String> = poses.keys().cloned().collect();
        accounts.sort();
        for account in accounts {
            let args = poses.remove(&account).unwrap_or_default();
            let depart = departs.remove(&account).unwrap_or(0);
            let deadline = deadlines.remove(&account).unwrap_or(0);
            out.push(object(vec![
                ("account", str_value(&account)),
                ("script", str_value("market")),
                ("depart", Value::Int(depart as i64)),
                ("deadline", Value::Int(deadline as i64)),
                ("deals", Value::Arr(args)),
            ]));
        }
        object(vec![("ok", Value::Bool(true)), ("poses", Value::Arr(out))])
    }

    /// Expires convened deals past their deadline. Arrival-gated like the
    /// live reports: the party that showed up relists with priority and
    /// keeps reliability, the absent loses both. Without nav every
    /// expiry is `nav_down`, neutral, both relist: nobody travels blind.
    pub(crate) fn op_sweep(&self, request: &Value) -> Value {
        let now_wall = request.get("now_wall").as_f64().unwrap_or(0.0);
        let nav_up = request.get("nav_up").as_bool().unwrap_or(true);
        let expired: Vec<u64> = {
            let engine = self.lock_engine();
            let mut expired: Vec<u64> = engine
                .deal_sweep_wall
                .iter()
                .filter(|(id, wall)| {
                    **wall < now_wall
                        && engine.deals.get(*id).is_some_and(|deal| {
                            deal.status == DealStatus::Convened || deal.status == DealStatus::BothArrived
                        })
                })
                .map(|(id, _)| *id)
                .collect();
            expired.sort_unstable();
            expired
        };
        let mut failed = Vec::new();
        for deal_id in expired {
            let (buyer, seller, item, qty, price, status, marks) = {
                let engine = self.lock_engine();
                let Some(deal) = engine.deals.get(&deal_id) else {
                    continue;
                };
                (
                    deal.buyer.clone(),
                    deal.seller.clone(),
                    deal.item,
                    deal.quantity,
                    deal.price,
                    deal.status,
                    engine.arrived.get(&deal_id).cloned().unwrap_or((false, false)),
                )
            };
            let reason = if !nav_up {
                FailReason::NavDown
            } else if status == DealStatus::BothArrived {
                FailReason::Timeout
            } else {
                FailReason::NoShow
            };
            {
                let mut engine = self.lock_engine();
                let Some(deal) = engine.deals.get_mut(&deal_id) else {
                    continue;
                };
                if deal.transition(DealStatus::Failed, Some(reason)).is_err() {
                    continue;
                }
            }
            self.lock_store()
                .set_deal_status(deal_id as i64, lifescape_store::DealStatus::Failed, reason.as_str(), Self::now_clock())
                .ok();
            let (relist_buyer, relist_seller, reliability) = match reason {
                FailReason::NoShow => (marks.0, marks.1, (arrival_score(marks.0), arrival_score(marks.1))),
                FailReason::Timeout => (true, true, (None, None)),
                _ => (true, true, (None, None)),
            };
            self.refund_and_relist(deal_id, &buyer, &seller, item, qty, price, relist_buyer, relist_seller, reliability);
            failed.push(Value::Int(deal_id as i64));
        }
        object(vec![("ok", Value::Bool(true)), ("failed", Value::Arr(failed))])
    }
}

/// Reliability verdict for one side of a no-show: showing up counts,
/// standing the partner up does not.
fn arrival_score(arrived: bool) -> Option<bool> {
    Some(arrived)
}

// -- convene loop (daemon-side) -----------------------------------------------

/// One posed convocation: the worker runs `market deliver` inline at the
/// next safe point, then resumes where it was.
pub struct ConvocationPose {
    pub account: String,
    pub args: Vec<String>,
}

/// Daemon convene round: sweep expired deals, then convene what waits.
/// Positions map account to (own tick epoch, level, x, z); only listed
/// accounts convene, and each bot gets a single pose with all its deals
/// in its own tick epoch. Without nav nothing convenes and matches stay
/// put.
pub fn convene_round(
    service: &MarketService,
    nav_up: bool,
    parties: &HashMap<String, (u64, i32, i32, i32)>,
    eta: &mut EtaCache,
    tick_ms: u64,
) -> Vec<ConvocationPose> {
    let wall = wall_now();
    service.handle(&object(vec![
        ("op", str_value("sweep")),
        ("now_wall", Value::Float(wall)),
        ("nav_up", Value::Bool(nav_up)),
    ]));
    if !nav_up || parties.is_empty() {
        return Vec::new();
    }
    let pending = service.handle(&object(vec![
        ("op", str_value("pending")),
        ("now_wall", Value::Float(wall)),
        ("tick_ms", Value::Int(tick_ms as i64)),
    ]));
    if pending.get("ok").as_bool() != Some(true) {
        return Vec::new();
    }
    let dest = {
        let coords = pending.get("dest").as_arr().cloned().unwrap_or_default();
        Tile {
            x: coords.first().and_then(|value| value.as_i64()).unwrap_or(3212) as i32,
            z: coords.get(1).and_then(|value| value.as_i64()).unwrap_or(3430) as i32,
            level: 0,
        }
    };
    // One ETA plan per departure zone, cached across rounds.
    let mut want: Vec<Value> = Vec::new();
    for entry in pending.get("deals").as_arr().cloned().unwrap_or_default() {
        if entry.get("convenable").as_bool() != Some(true) {
            continue;
        }
        let buyer = entry.get("buyer").as_str().unwrap_or("").to_string();
        let seller = entry.get("seller").as_str().unwrap_or("").to_string();
        let (Some(buyer_party), Some(seller_party)) = (parties.get(&buyer), parties.get(&seller)) else {
            eprintln!("market-convene: deal {} parties missing", entry.get("deal").as_i64().unwrap_or(-1));
            continue;
        };
        let buyer_tile = Tile { x: buyer_party.2, z: buyer_party.3, level: buyer_party.1 };
        let seller_tile = Tile { x: seller_party.2, z: seller_party.3, level: seller_party.1 };
        let (Some(eta_buyer), Some(eta_seller)) = (eta.eta(buyer_tile, dest), eta.eta(seller_tile, dest)) else {
            eprintln!("market-convene: deal {} eta unknown", entry.get("deal").as_i64().unwrap_or(-1));
            continue;
        };
        let (depart_buyer, deadline_buyer, depart_seller, deadline_seller) =
            stagger(buyer_party.0, eta_buyer, seller_party.0, eta_seller);
        let span = PREP_MARGIN_TICKS + eta_buyer.max(eta_seller).saturating_mul(2) + DEADLINE_MARGIN_TICKS;
        want.push(object(vec![
            ("deal", entry.get("deal").clone()),
            ("depart_buyer", Value::Int(depart_buyer as i64)),
            ("depart_seller", Value::Int(depart_seller as i64)),
            ("deadline_buyer", Value::Int(deadline_buyer as i64)),
            ("deadline_seller", Value::Int(deadline_seller as i64)),
            ("sweep_wall", Value::Float(wall + sweep_after_secs(span, tick_ms))),
        ]));
    }
    if want.is_empty() {
        return Vec::new();
    }
    let reply = service.handle(&object(vec![("op", str_value("convene")), ("deals", Value::Arr(want))]));
    let mut poses = Vec::new();
    for pose in reply.get("poses").as_arr().cloned().unwrap_or_default() {
        let account = pose.get("account").as_str().unwrap_or("").to_string();
        let depart = pose.get("depart").as_i64().unwrap_or(0);
        let deadline = pose.get("deadline").as_i64().unwrap_or(0);
        let mut args = vec!["deliver".to_string(), depart.to_string(), deadline.to_string()];
        for deal in pose.get("deals").as_arr().cloned().unwrap_or_default() {
            args.push(deal.as_str().unwrap_or("").to_string());
        }
        if account.is_empty() || args.len() < 4 {
            continue;
        }
        poses.push(ConvocationPose { account, args });
    }
    poses.sort_by(|left, right| left.account.cmp(&right.account));
    poses
}

/// Daemon convene thread: every round sweeps expired deals, convokes
/// what waited out the grouping window, and poses one `market deliver`
/// per bot. Unposeable convokes retry next round; expired ones sweep.
pub fn start_convene_loop(orch: Arc<Orchestrator>) {
    std::thread::Builder::new()
        .name("market-convene".to_string())
        .spawn(move || {
            // The registry lands when the market starts (just above the
            // call site); wait a minute before giving up, never spin.
            let mut service = None;
            for _ in 0..12 {
                if let Some(running) = crate::market_service::shared() {
                    service = Some(running);
                    break;
                }
                std::thread::sleep(Duration::from_secs(5));
            }
            let Some(service) = service else {
                return;
            };
            let mut eta = EtaCache::new();
            let mut meter = TickMeter::new();
            let mut unposed: Vec<ConvocationPose> = Vec::new();
            loop {
                std::thread::sleep(Duration::from_secs(CONVENE_EVERY_SECS));
                let parties = orch.market_live_positions();
                // Live tick rate from the freshest snapshot: session
                // ticks are per-login epochs but advance at one per
                // server tick, so wall-per-tick calibrates grouping and
                // sweep with no env switch.
                let wall = wall_now();
                let Some(truth) = parties.values().map(|party| party.0).max() else {
                    continue;
                };
                let tick_ms = meter.observe(wall, truth);
                let nav_up = crate::plan_service::stats_json().is_some();
                for pose in convene_round(&service, nav_up, &parties, &mut eta, tick_ms) {
                    eprintln!("market-convene: pose {} {:?}", pose.account, pose.args);
                    unposed.push(pose);
                }
                unposed.retain(|pose| {
                    if orch.pose_market_script(&pose.account, &pose.args) {
                        false
                    } else {
                        true
                    }
                });
                if unposed.len() > 200 {
                    unposed.drain(..unposed.len() - 200);
                }
            }
        })
        .expect("market convene thread spawns");
}

#[cfg(test)]
#[path = "market_convene_tests.rs"]
mod convene_tests;

/// Daemon hook for the convene loop: live positions, worker pose,
/// loop starter. Lives here (not `daemon.rs`) so the supervisor file
/// keeps its shape; same crate, same `Orchestrator`.
impl Orchestrator {
    /// Live positions for the market convene loop: account to (own tick
    /// epoch, level, x, z). Ticks are per-login epochs, never comparable
    /// across bots: departs and deadlines ride each bot's own clock, the
    /// sweep rides the daemon wall. Only connected workers with a pushed
    /// snapshot appear, so offline bots never convene (their orders stay
    /// dormant, matches wait).
    pub fn market_live_positions(&self) -> HashMap<String, (u64, i32, i32, i32)> {
        let mut parties = HashMap::new();
        for job in self.jobs.lock().unwrap().values() {
            let inner = job.inner.lock().unwrap();
            let Some(live) = inner.live.as_ref() else {
                continue;
            };
            let ticks = live.get("ticks").as_i64().unwrap_or(0).max(0) as u64;
            let position = live.get("position").as_arr().cloned().unwrap_or_default();
            if position.len() < 3 {
                continue;
            }
            let (level, x, z) = (
                position[0].as_i64().unwrap_or(0) as i32,
                position[1].as_i64().unwrap_or(0) as i32,
                position[2].as_i64().unwrap_or(0) as i32,
            );
            parties.insert(job.account.clone(), (ticks, level, x, z));
        }
        parties
    }

    /// Pose one market convocation on a worker: the `market deliver`
    /// script runs inline at the next safe point, one convocation at a
    /// time (a newer pose replaces the older). True when the worker acked.
    pub fn pose_market_script(&self, account: &str, args: &[String]) -> bool {
        let request = object(vec![
            ("cmd", str_value("pose_market")),
            ("script", str_value("market")),
            ("args", Value::Arr(args.iter().map(|arg| str_value(arg)).collect())),
        ]);
        match self.send_worker(account, &request, Duration::from_secs(10)) {
            Ok(reply) => reply.get("ok").as_bool() == Some(true),
            Err(_) => false,
        }
    }

    /// Market convene thread: sweeps expired deals, convokes what waited
    /// out the grouping window, poses one delivery per bot. Started once
    /// beside the holdings sampler; the loop owns its ETA cache.
    pub fn start_market_convene(self: &Arc<Self>) {
        crate::market_convene::start_convene_loop(self.clone());
    }
}
