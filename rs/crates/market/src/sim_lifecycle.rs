#![forbid(unsafe_code)]
//! Offline life of orders (plan marché M3-M5, fixed seeds): traders
//! arrive continuously, each with its own limit, patience and motive
//! horizon, and reprice on its own window phase. Two models on the same
//! book code and the same arrivals:
//! - `Model::Concession` (wave 2): `concession::step` toward the
//!   order's own limit over its horizon, ZIP back-off on own fills,
//!   public-print pull, pressed orders crossing, Q4 seller withdrawal;
//!   loot sellers join the best ask (`zip::found_open`);
//! - `Model::OldStep` (wave 1): one `zip::demand_step` (5 %, at least a
//!   coin) per synchronized 2000-tick window, loot sellers at ask - 1
//!   with a 1-coin floor.
//!
//! Openings follow the daemon: a buyer takes a covered ask at placement
//! (`opening_bid`), else rests at the current price (never above its
//! limit, `remembered_bid`); a seller opens at the current price or the
//! richer bid when traded, else at twice its limit (`seller_open`); a
//! loot seller per `found_open`. With public discovery anchors
//! (`run_full`, plan marché M7) a never-traded object opens on them
//! (`discovery`): sellers over the published cost and never under the
//! shop's offer, buyers under their value and over that floor.

use crate::book::{Book, Fill, Side};
use crate::concession::{concession_beta, step, window_offset, window_ticks, Concession, Step, Urgency, Window};
use crate::discovery::{bid_open, listing_open, loot_anchor, margin_of, Anchors};
use crate::price::{current_price, median_price, LastFill, Reference};
use crate::sim::FixedRng;
use crate::zip::{demand_step, found_open, opening_bid, seller_open, PATIENCE_TICKS};

pub use crate::sim_maker::{BookSample, MakerOutcome, MakerRun, SimMaker};
use crate::sim_maker::{MakerState, SAMPLE_TICKS};

const ITEM: i64 = 1;
/// Simulated time per round: windows land on their own phase inside it.
pub const ROUND_TICKS: u64 = 100;

/// Repricing model of a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Concession,
    OldStep,
}

/// One participant.
#[derive(Clone, Debug)]
pub struct Trader {
    pub name: String,
    pub sell: bool,
    /// Buyer: most it pays; seller: least it accepts (keep value for loot).
    pub limit: i64,
    pub units: i64,
    pub patience: f64,
    pub urgency: Urgency,
    pub horizon_ticks: u64,
    pub arrives: u64,
    /// Found stock (loot): opens per `found_open`, else `seller_open`.
    pub loot: bool,
    /// Loot anchor (high alchemy value) of an empty book.
    pub anchor: Option<i64>,
}

/// One print.
#[derive(Clone, Debug, PartialEq)]
pub struct SimFill {
    pub tick: u64,
    pub price: i64,
    pub quantity: i64,
    pub buyer: String,
    pub seller: String,
    pub seller_took: bool,
}

/// Outcome of one run.
#[derive(Clone, Debug, Default)]
pub struct LifecycleRun {
    pub fills: Vec<SimFill>,
    pub amends: usize,
    pub withdrawn: usize,
    /// Units sellers took to the NPC shop (Q3 outlet, `run_with_outlet`).
    pub npc_units: i64,
    /// Lowest price any ask rested or any print traded at.
    pub lowest_quote: i64,
}

#[derive(Clone, Debug)]
struct Live {
    trader: usize,
    id: u64,
    price: i64,
    seen_remaining: i64,
    curve: Concession,
    curve_start: u64,
    phase: u64,
    window: u64,
    /// Own last fill price (its public print is never a pull).
    own_fill: Option<i64>,
}

struct Engine {
    book: Book,
    prints: Vec<LastFill>,
    next_id: u64,
    live: Vec<Live>,
    run: LifecycleRun,
    /// Wave 3: the market maker quoting this object, if any.
    maker: Option<MakerState>,
    /// Q3: what an NPC shop pays per unit; a seller the market pays less
    /// sells there instead (`outlets::npc_sale_beats_market`).
    outlet: Option<i64>,
    /// M7: public anchors of the object (`None`: the openings of wave 2).
    anchors: Option<Anchors>,
}

impl Engine {
    /// A trader's limit: its own (no public price floor, user
    /// 2026-11-18).
    fn limit_of(&self, trader: &Trader) -> i64 {
        trader.limit
    }

    /// Q3 (wave 2 rule): the shop beats what the market pays the seller
    /// `owner` (others' best bid, the traded current price).
    fn shop_takes(&self, traders: &[Trader], owner: usize) -> bool {
        let Some(paid) = self.outlet else { return false };
        traders[owner].sell && crate::outlets::npc_sale_beats_market(paid, self.best_other(traders, owner, Side::Buy), self.reference())
    }
}

impl Engine {
    fn reference(&self) -> Reference {
        current_price(&self.prints, &self.book.levels_covering(Side::Buy, 1), &self.book.levels_covering(Side::Sell, 1))
    }

    fn best_other(&self, traders: &[Trader], owner: usize, side: Side) -> Option<i64> {
        let prices = self.book.orders().into_iter().filter(|order| order.side == side && order.remaining > 0 && order.account != traders[owner].name).map(|order| order.price);
        if side == Side::Sell {
            prices.min()
        } else {
            prices.max()
        }
    }

    fn record(&mut self, tick: u64, fills: Vec<Fill>) {
        for fill in fills {
            if let Some(maker) = self.maker.as_mut() {
                maker.settle(tick, &fill);
            }
            self.prints.push(LastFill { price: fill.price, quantity: fill.quantity });
            self.run.lowest_quote = self.run.lowest_quote.min(fill.price);
            self.run.fills.push(SimFill { tick, price: fill.price, quantity: fill.quantity, buyer: fill.buyer, seller: fill.seller, seller_took: fill.taker == Side::Sell });
        }
    }
}

fn opening(engine: &Engine, traders: &[Trader], owner: usize, model: Model) -> i64 {
    let trader = &traders[owner];
    let reference = engine.reference();
    let bid = engine.best_other(traders, owner, Side::Buy);
    let ask = engine.best_other(traders, owner, Side::Sell);
    let margin = margin_of(trader.patience);
    let limit = engine.limit_of(trader);
    if !trader.sell {
        // A covered ask is taken at placement (daemon `remembered_bid`),
        // else the bid rests at the current price, never over its limit;
        // never traded with anchors (M7): under its value, over the floor.
        let covered = ask.is_some_and(|ask| ask <= trader.limit);
        let discovered = engine.anchors.as_ref().filter(|_| !reference.traded && !covered).and_then(|anchors| bid_open(anchors, Some(trader.limit), trader.limit, margin));
        return discovered.unwrap_or_else(|| opening_bid(reference.price, ask, trader.limit));
    }
    if trader.loot {
        return match (model, engine.anchors.as_ref()) {
            (Model::Concession, Some(anchors)) => found_open(reference, bid, ask, limit, loot_anchor(reference, trader.anchor, limit, anchors, margin), None, trader.urgency == Urgency::Pressed).0,
            (Model::Concession, None) => found_open(reference, bid, ask, limit, trader.anchor, None, trader.urgency == Urgency::Pressed).0,
            (Model::OldStep, _) => old_found_open(reference, bid, ask, trader.anchor),
        };
    }
    match engine.anchors.as_ref() {
        Some(anchors) => listing_open(reference, bid, trader.limit, None, anchors, margin).0,
        None => seller_open(reference, bid, trader.limit, None).0,
    }
}

/// Wave-1 `found_open` (ask - 1, floor 1): the cascade the plan measured.
fn old_found_open(reference: Reference, best_bid: Option<i64>, best_ask: Option<i64>, anchor: Option<i64>) -> i64 {
    let bid = best_bid.unwrap_or(0);
    let base = if reference.traded {
        reference.price.max(bid)
    } else {
        match best_ask.filter(|ask| *ask >= 1) {
            Some(ask) => (ask - 1).max(bid),
            None if bid >= 1 => bid,
            None => anchor.unwrap_or(2),
        }
    };
    base.max(1)
}

/// Runs `traders` on one book until `end_tick`.
pub fn run_lifecycle(traders: &[Trader], model: Model, end_tick: u64) -> LifecycleRun {
    run_with_maker(traders, model, end_tick, None).run
}

/// Runs `traders` on one book until `end_tick`, with a wave-3 market
/// maker quoting the object when `maker` is set (same arrivals, same
/// book code); the book's best bid and ask are sampled every
/// `SAMPLE_TICKS` from the first arrival.
pub fn run_with_maker(traders: &[Trader], model: Model, end_tick: u64, maker: Option<SimMaker>) -> MakerRun {
    run_with_outlet(traders, model, end_tick, maker, None)
}

/// `run_with_maker` where every seller also has the Q3 outlet: a shop
/// paying `outlet` per unit takes its stock whenever the market pays
/// less (checked at placement and at each of its windows, like
/// `brain_sales`).
pub fn run_with_outlet(traders: &[Trader], model: Model, end_tick: u64, maker: Option<SimMaker>, outlet: Option<i64>) -> MakerRun {
    run_full(traders, model, end_tick, maker, outlet, None)
}

/// `run_with_outlet` on an object whose public discovery anchors are
/// `anchors` (M7): openings of a never-traded book on them, sellers'
/// floors never under the public outside options.
pub fn run_full(traders: &[Trader], model: Model, end_tick: u64, maker: Option<SimMaker>, outlet: Option<i64>, anchors: Option<Anchors>) -> MakerRun {
    let mut engine = Engine {
        book: Book::new(),
        prints: Vec::new(),
        next_id: 1,
        live: Vec::new(),
        run: LifecycleRun { lowest_quote: i64::MAX, ..LifecycleRun::default() },
        maker: maker.map(MakerState::new),
        outlet,
        anchors,
    };
    let first_arrival = traders.iter().map(|trader| trader.arrives).min().unwrap_or(0);
    let mut samples = Vec::new();
    let mut placed = vec![false; traders.len()];
    let mut tick = 0;
    while tick <= end_tick {
        for owner in 0..traders.len() {
            if placed[owner] || traders[owner].arrives > tick {
                continue;
            }
            placed[owner] = true;
            place(&mut engine, traders, owner, model, tick);
        }
        requote_maker(&mut engine, tick);
        reprice(&mut engine, traders, model, tick);
        if tick >= first_arrival && tick % SAMPLE_TICKS == 0 {
            let (bid, ask) = engine.book.best();
            samples.push(BookSample { tick, bid, ask });
        }
        tick += ROUND_TICKS;
    }
    let maker = engine.maker.take().map(|state| state.outcome(engine.reference().price));
    MakerRun { run: engine.run, samples, maker }
}

fn place(engine: &mut Engine, traders: &[Trader], owner: usize, model: Model, tick: u64) {
    let trader = &traders[owner];
    if engine.shop_takes(traders, owner) {
        engine.run.npc_units += trader.units;
        return;
    }
    let price = opening(engine, traders, owner, model).max(1);
    let id = engine.next_id;
    engine.next_id += 1;
    let side = if trader.sell { Side::Sell } else { Side::Buy };
    let Ok((fills, rests)) = engine.book.place(id, &trader.name, ITEM, side, trader.units, price) else { return };
    engine.record(tick, fills);
    if trader.sell {
        engine.run.lowest_quote = engine.run.lowest_quote.min(price);
    }
    if !rests {
        return;
    }
    let beta = concession_beta(trader.patience, trader.urgency);
    let window = match model {
        Model::Concession => window_ticks(trader.horizon_ticks),
        Model::OldStep => PATIENCE_TICKS,
    };
    let phase = match model {
        Model::Concession => window_offset(&trader.name, ITEM, window),
        Model::OldStep => 0,
    };
    engine.live.push(Live {
        trader: owner,
        id,
        price,
        seen_remaining: engine.book.remaining(id).unwrap_or(0),
        curve: Concession { opening: price, limit: engine.limit_of(trader), horizon_ticks: trader.horizon_ticks, beta },
        curve_start: tick,
        phase,
        window,
        own_fill: None,
    });
}

fn reprice(engine: &mut Engine, traders: &[Trader], model: Model, tick: u64) {
    let reference = engine.reference();
    let mut kept = Vec::new();
    for mut live in std::mem::take(&mut engine.live) {
        let Some(left) = engine.book.remaining(live.id) else { continue };
        let due = tick >= live.phase && (tick - live.phase) % live.window < ROUND_TICKS;
        if !due {
            kept.push(live);
            continue;
        }
        let trader = &traders[live.trader];
        if engine.shop_takes(traders, live.trader) {
            let _ = engine.book.cancel(live.id);
            engine.run.npc_units += left;
            continue;
        }
        let filled = left < live.seen_remaining;
        live.seen_remaining = left;
        let next = match model {
            Model::OldStep => {
                let (floor, ceiling) = if trader.sell { (engine.limit_of(trader), i64::MAX / 4) } else { (1, trader.limit) };
                let floor = if trader.loot { 1 } else { floor };
                let next = demand_step(live.price, trader.sell, filled, floor, ceiling);
                (next != live.price).then_some(next)
            }
            Model::Concession => {
                let opposite = engine.best_other(traders, live.trader, if trader.sell { Side::Buy } else { Side::Sell });
                let decision = step(&Window {
                    sell: trader.sell,
                    current: live.price,
                    curve: live.curve,
                    elapsed_ticks: tick - live.curve_start,
                    window_ticks: live.window,
                    filled,
                    anchor_price: reference.traded.then_some(reference.price),
                    own_fill: live.own_fill,
                    best_opposite: opposite,
                    pressed: trader.urgency == Urgency::Pressed,
                });
                match decision {
                    Step::Hold => None,
                    Step::Amend(price) => Some(price),
                    Step::Restart { opening } => {
                        live.curve = Concession { opening, ..live.curve };
                        live.curve_start = tick;
                        live.own_fill = Some(live.price);
                        Some(opening)
                    }
                }
            }
        };
        let Some(next) = next else {
            kept.push(live);
            continue;
        };
        let new_id = engine.next_id;
        engine.next_id += 1;
        let Ok((replacement, fills)) = engine.book.amend(live.id, next, new_id) else { continue };
        engine.run.amends += 1;
        engine.record(tick, fills);
        if replacement.remaining > 0 {
            if trader.sell {
                engine.run.lowest_quote = engine.run.lowest_quote.min(next);
            }
            kept.push(Live { id: new_id, price: next, seen_remaining: replacement.remaining, ..live });
        }
    }
    engine.live = kept;
}

/// The maker's pass (every `SimMaker::window_ticks`): its two orders are
/// withdrawn and quoted again from public data and its own stock
/// (`maker::quote`), the NPC shop takes what nobody buys (Q3).
fn requote_maker(engine: &mut Engine, tick: u64) {
    let Some(state) = engine.maker.as_ref() else { return };
    if tick % state.spec.window_ticks.max(ROUND_TICKS) != 0 {
        return;
    }
    let name = state.spec.name.clone();
    // The ask still resting since the last pass (unsold) walks down.
    let unsold_ask = state.ask_id.and_then(|ask| engine.book.orders().into_iter().find(|order| order.id == ask && order.remaining > 0).map(|order| order.price));
    for id in [state.bid_id, state.ask_id].into_iter().flatten() {
        let _ = engine.book.cancel(id);
    }
    let reference = engine.reference();
    let others = |side: Side| engine.book.orders().into_iter().filter(|order| order.side == side && order.remaining > 0 && order.account != name).collect::<Vec<_>>();
    let (bids, asks) = (others(Side::Buy), others(Side::Sell));
    let state = engine.maker.as_mut().expect("maker checked above");
    state.own_ask = unsold_ask;
    let input = state.input(tick, reference, &bids, &asks);
    let quote = crate::maker::quote(&state.spec.caps, &input);
    state.own_bid = quote.bid.map(|leg| leg.price);
    state.bid_id = None;
    state.ask_id = None;
    state.dump(quote.npc_dump);
    let affordable = |leg: crate::maker::Leg, coins: i64| crate::maker::Leg { qty: leg.qty.min(coins / leg.price.max(1)), ..leg };
    let bid = quote.bid.map(|leg| affordable(leg, state.coins)).filter(|leg| leg.qty > 0);
    for (side, leg) in [(Side::Buy, bid), (Side::Sell, quote.ask)] {
        let Some(leg) = leg else { continue };
        let id = engine.next_id;
        engine.next_id += 1;
        let Ok((fills, rests)) = engine.book.place(id, &name, ITEM, side, leg.qty, leg.price) else { continue };
        engine.record(tick, fills);
        if rests {
            let state = engine.maker.as_mut().expect("maker checked above");
            if side == Side::Buy {
                state.bid_id = Some(id);
            } else {
                state.ask_id = Some(id);
            }
        }
    }
}

/// Compatible unit pairs of a market (max matching: buyers by limit
/// descending against sellers by limit ascending) and the competitive
/// equilibrium price (midpoint of the clearing range).
pub fn equilibrium(traders: &[Trader]) -> (i64, i64) {
    let mut bids: Vec<i64> = traders.iter().filter(|trader| !trader.sell).flat_map(|trader| std::iter::repeat_n(trader.limit, trader.units as usize)).collect();
    let mut asks: Vec<i64> = traders.iter().filter(|trader| trader.sell).flat_map(|trader| std::iter::repeat_n(trader.limit, trader.units as usize)).collect();
    bids.sort_unstable_by(|left, right| right.cmp(left));
    asks.sort_unstable();
    let pairs = bids.iter().zip(asks.iter()).take_while(|(bid, ask)| bid >= ask).count();
    if pairs == 0 {
        return (0, 0);
    }
    let low = asks[pairs - 1].max(bids.get(pairs).copied().unwrap_or(0));
    let high = bids[pairs - 1].min(asks.get(pairs).copied().unwrap_or(i64::MAX));
    (pairs as i64, (low + high) / 2)
}

/// Direction changes of the CURRENT price (median of the last five
/// prints after each print, `price::median_price`: the price every bot
/// reads, plan §3.9) over the print count. Raw prints between different
/// counterparties zigzag by nature; the current price is what must not
/// oscillate.
pub fn inversion_share(fills: &[SimFill]) -> f64 {
    let prices: Vec<i64> = fills.iter().map(|fill| fill.price).collect();
    let current: Vec<i64> = (1..=prices.len()).filter_map(|end| median_price(&prices[..end])).collect();
    direction_changes(&current, fills.len())
}

/// Direction changes of consecutive raw prints over the print count.
pub fn raw_inversion_share(fills: &[SimFill]) -> f64 {
    let prices: Vec<i64> = fills.iter().map(|fill| fill.price).collect();
    direction_changes(&prices, fills.len())
}

fn direction_changes(series: &[i64], fills: usize) -> f64 {
    let mut last_direction = 0i64;
    let mut inversions = 0usize;
    for pair in series.windows(2) {
        let direction = (pair[1] - pair[0]).signum();
        if direction != 0 {
            if last_direction != 0 && direction != last_direction {
                inversions += 1;
            }
            last_direction = direction;
        }
    }
    if fills == 0 {
        0.0
    } else {
        inversions as f64 / fills as f64
    }
}

/// Final formed price: median of the last five prints.
pub fn final_price(fills: &[SimFill]) -> Option<i64> {
    median_price(&fills.iter().map(|fill| fill.price).collect::<Vec<_>>())
}

/// The plan's M3 market: `buyers` and `sellers` arriving over
/// `spread_ticks`, limits drawn around 100 (buyers 60-140, sellers
/// 50-130, two units each), patience 0.05-0.95, horizon 36 000 ticks.
pub fn continuous_market(seed: u64, buyers: usize, sellers: usize, spread_ticks: u64) -> Vec<Trader> {
    let mut rng = FixedRng::new(seed);
    let mut traders = Vec::new();
    for index in 0..buyers + sellers {
        let sell = index >= buyers;
        let limit = if sell { rng.between(50, 130) } else { rng.between(60, 140) };
        traders.push(Trader {
            name: format!("{}{index}", if sell { "seller" } else { "buyer" }),
            sell,
            limit,
            units: 2,
            patience: rng.between(5, 95) as f64 / 100.0,
            urgency: Urgency::Normal,
            horizon_ticks: 36_000,
            arrives: rng.between(0, spread_ticks as i64) as u64,
            loot: false,
            anchor: None,
        });
    }
    traders
}

/// Units each named trader received (buyers) or delivered (sellers).
pub fn units_of(run: &LifecycleRun, name: &str) -> i64 {
    run.fills.iter().filter(|fill| fill.buyer == name || fill.seller == name).map(|fill| fill.quantity).sum()
}

#[cfg(test)]
#[path = "sim_lifecycle_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "sim_maker_tests.rs"]
mod maker_tests;
