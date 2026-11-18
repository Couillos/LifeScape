#![forbid(unsafe_code)]
//! Wave-3 market makers in the lifecycle simulator (plan marché Q6):
//! the maker's own state (coins, stock, resting legs), the book samples
//! the criteria read (two-sided share, ask/bid spread), and the scenario
//! markets: a sparse multi-object market shaped like the live constat
//! (§1.1: most books one-sided, asks only above all) and an object whose
//! value falls (the maker must not be ruined).
//!
//! The maker quotes with `maker::quote` from public data (current price,
//! the others' book, the others' purchases lately) and its own stock and
//! equity, exactly like the daemon pass; the NPC shop takes its dumps at
//! the outside price (Q3).

use crate::book::{Fill, Order};
use crate::concession::Urgency;
use crate::maker::{MakerCaps, MakerInput, MAKER_CAPS};
use crate::price::Reference;
use crate::sim::FixedRng;
use crate::sim_lifecycle::Trader;

/// Book samples: one every 1000 ticks.
pub const SAMPLE_TICKS: u64 = 1000;

/// One simulated maker on one object.
#[derive(Clone, Debug)]
pub struct SimMaker {
    pub name: String,
    /// Starting coins (its capital target).
    pub capital: i64,
    /// Public anchor of the object (shop value).
    pub anchor: i64,
    /// What an NPC shop pays per unit (Q3).
    pub outside: Option<i64>,
    /// Discovery anchor (M7): published production cost.
    pub make_cost: Option<i64>,
    /// Ticks between two quote passes.
    pub window_ticks: u64,
    pub caps: MakerCaps,
}

impl SimMaker {
    /// The standard maker: `MAKER_CAPS`, a pass every 1000 ticks.
    pub fn standard(capital: i64, anchor: i64, outside: Option<i64>) -> SimMaker {
        SimMaker { name: "maker".to_string(), capital, anchor, outside, make_cost: None, window_ticks: 1000, caps: MAKER_CAPS }
    }
}

/// The book's best bid and ask at one tick (everyone's orders).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BookSample {
    pub tick: u64,
    pub bid: Option<i64>,
    pub ask: Option<i64>,
}

/// What the maker ends with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MakerOutcome {
    pub coins: i64,
    pub stock: i64,
    pub max_stock: i64,
    pub dumped: i64,
    pub dump_coins: i64,
    pub final_price: i64,
    /// Coins plus stock at the final current price.
    pub value: i64,
}

/// One run with or without a maker.
#[derive(Clone, Debug)]
pub struct MakerRun {
    pub run: crate::sim_lifecycle::LifecycleRun,
    pub samples: Vec<BookSample>,
    pub maker: Option<MakerOutcome>,
}

#[derive(Clone, Debug)]
pub(crate) struct MakerState {
    pub(crate) spec: SimMaker,
    pub(crate) coins: i64,
    pub(crate) stock: i64,
    /// Coins paid for the stock held (average cost carried on sales).
    stock_cost: i64,
    pub(crate) bid_id: Option<u64>,
    pub(crate) ask_id: Option<u64>,
    pub(crate) own_bid: Option<i64>,
    pub(crate) own_ask: Option<i64>,
    max_stock: i64,
    dumped: i64,
    dump_coins: i64,
    /// Others' purchases (tick, units): public prints.
    others_buys: Vec<(u64, i64)>,
}

impl MakerState {
    pub(crate) fn new(spec: SimMaker) -> MakerState {
        MakerState { coins: spec.capital, spec, stock: 0, stock_cost: 0, bid_id: None, ask_id: None, own_bid: None, own_ask: None, max_stock: 0, dumped: 0, dump_coins: 0, others_buys: Vec::new() }
    }

    /// One fill on the book: the maker's own moves its coins and stock;
    /// another buyer's is public demand.
    pub(crate) fn settle(&mut self, tick: u64, fill: &Fill) {
        let value = fill.price.saturating_mul(fill.quantity);
        if fill.buyer == self.spec.name {
            self.stock += fill.quantity;
            self.stock_cost += value;
            self.coins -= value;
            self.max_stock = self.max_stock.max(self.stock);
        } else {
            if fill.seller == self.spec.name {
                self.release_cost(fill.quantity);
                self.stock -= fill.quantity;
                self.coins += value;
            }
            self.others_buys.push((tick, fill.quantity));
        }
    }

    /// The quote inputs at `tick`: public book of the others, public
    /// purchases lately, own stock and equity.
    pub(crate) fn input(&self, tick: u64, reference: Reference, bids: &[Order], asks: &[Order]) -> MakerInput {
        let since = tick.saturating_sub(crate::maker::DEMAND_LOOKBACK_TICKS);
        MakerInput {
            reference,
            best_bid: bids.iter().map(|order| order.price).max(),
            best_ask: asks.iter().map(|order| order.price).min(),
            bid_depth: bids.iter().map(|order| order.remaining).sum(),
            ask_depth: asks.iter().map(|order| order.remaining).sum(),
            recent_buys: self.others_buys.iter().filter(|(at, _)| *at >= since).map(|(_, units)| *units).sum(),
            anchor: self.spec.anchor,
            outside: self.spec.outside,
            make_cost: self.spec.make_cost,
            stock: self.stock,
            stock_cost: self.stock_cost,
            capital: self.coins + self.stock * reference.price.max(1),
            own_bid: self.own_bid,
            own_ask: self.own_ask,
        }
    }

    /// Units leaving the stock carry their average cost out.
    fn release_cost(&mut self, units: i64) {
        if self.stock > 0 {
            self.stock_cost -= self.stock_cost * units.min(self.stock) / self.stock;
        }
    }

    /// Q3: units nobody buys go to the NPC shop at the outside price.
    pub(crate) fn dump(&mut self, units: i64) {
        let units = units.min(self.stock).max(0);
        let Some(paid) = self.spec.outside else { return };
        if units == 0 {
            return;
        }
        self.release_cost(units);
        self.stock -= units;
        self.coins += units * paid;
        self.dumped += units;
        self.dump_coins += units * paid;
    }

    pub(crate) fn outcome(self, final_price: i64) -> MakerOutcome {
        MakerOutcome {
            coins: self.coins,
            stock: self.stock,
            max_stock: self.max_stock,
            dumped: self.dumped,
            dump_coins: self.dump_coins,
            final_price,
            value: self.coins + self.stock * final_price.max(1),
        }
    }
}

/// (two-sided samples, samples).
pub fn two_sided(samples: &[BookSample]) -> (usize, usize) {
    (samples.iter().filter(|sample| sample.bid.is_some() && sample.ask.is_some()).count(), samples.len())
}

/// Ask over bid of every two-sided sample.
pub fn spreads(samples: &[BookSample]) -> Vec<f64> {
    samples.iter().filter_map(|sample| Some(sample.ask? as f64 / sample.bid?.max(1) as f64)).collect()
}

/// Median of a list (`None` when empty).
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    Some(if sorted.len() % 2 == 1 { sorted[middle] } else { (sorted[middle - 1] + sorted[middle]) / 2.0 })
}

/// One object of a scenario market: its public value (shop value), what
/// a shop pays for it, its traders.
#[derive(Clone, Debug)]
pub struct SimItem {
    pub value: i64,
    pub outside: i64,
    pub traders: Vec<Trader>,
}

#[allow(clippy::too_many_arguments)]
fn trader(name: String, sell: bool, limit: i64, units: i64, rng: &mut FixedRng, horizon_ticks: u64, arrives: u64, loot: Option<i64>) -> Trader {
    Trader {
        name,
        sell,
        limit: limit.max(1),
        units,
        patience: rng.between(5, 95) as f64 / 100.0,
        urgency: if loot.is_some() { Urgency::Patient } else { Urgency::Normal },
        horizon_ticks,
        arrives,
        loot: loot.is_some(),
        anchor: loot,
    }
}

/// A sparse market of `items` objects shaped like the live constat
/// (plan §1.1): 60 % loot books (looters listing what they found, keep
/// values 30-80 % of the value, a buyer now and then), 20 % sparse
/// two-sided books, 20 % buyers only. Values 5-400 coins; a shop pays a
/// third of the value; arrivals over `spread_ticks`.
pub fn sparse_market(seed: u64, items: usize, spread_ticks: u64) -> Vec<SimItem> {
    let mut rng = FixedRng::new(seed);
    let mut out = Vec::new();
    for item in 0..items {
        let value = rng.between(5, 400);
        let shape = rng.between(0, 99);
        let pct = |rng: &mut FixedRng, low: i64, high: i64| value * rng.between(low, high) / 100;
        let mut traders = Vec::new();
        let at = |rng: &mut FixedRng| rng.between(0, spread_ticks as i64) as u64;
        if shape < 60 {
            for index in 0..4 {
                let limit = pct(&mut rng, 30, 80);
                let units = rng.between(5, 30);
                let arrives = at(&mut rng);
                traders.push(trader(format!("i{item}looter{index}"), true, limit, units, &mut rng, 72_000, arrives, Some(value * 6 / 10)));
            }
            if rng.between(0, 1) == 1 {
                let limit = pct(&mut rng, 90, 130);
                let arrives = at(&mut rng);
                traders.push(trader(format!("i{item}buyer"), false, limit, 3, &mut rng, 36_000, arrives, None));
            }
        } else if shape < 80 {
            for index in 0..2 {
                let (buy, sell) = (pct(&mut rng, 90, 130), pct(&mut rng, 70, 110));
                let (buy_at, sell_at) = (at(&mut rng), at(&mut rng));
                traders.push(trader(format!("i{item}buyer{index}"), false, buy, 2, &mut rng, 36_000, buy_at, None));
                traders.push(trader(format!("i{item}seller{index}"), true, sell, 2, &mut rng, 36_000, sell_at, None));
            }
        } else {
            for index in 0..2 {
                let limit = pct(&mut rng, 80, 120);
                let arrives = at(&mut rng);
                traders.push(trader(format!("i{item}buyer{index}"), false, limit, 2, &mut rng, 36_000, arrives, None));
            }
        }
        out.push(SimItem { value, outside: (value / 3).max(1), traders });
    }
    out
}

/// An object whose value falls from 100 to 10 over `span_ticks`: a
/// seller every 1500 ticks at the falling value (10 units, pressed), a
/// buyer every 4500 ticks at 110 % of it (3 units). The maker buys on
/// the way down; its caps must keep it alive.
pub fn falling_market(seed: u64, span_ticks: u64) -> Vec<Trader> {
    let mut rng = FixedRng::new(seed);
    let value_at = |tick: u64| 100 - (90 * tick.min(span_ticks) / span_ticks.max(1)) as i64;
    let mut traders = Vec::new();
    let mut tick = 0;
    let mut index = 0;
    while tick <= span_ticks {
        let mut seller = trader(format!("dumper{index}"), true, value_at(tick), 10, &mut rng, 36_000, tick, None);
        seller.urgency = Urgency::Pressed;
        traders.push(seller);
        if index % 3 == 0 {
            traders.push(trader(format!("taker{index}"), false, value_at(tick) * 11 / 10, 3, &mut rng, 36_000, tick + 700, None));
        }
        tick += 1500;
        index += 1;
    }
    traders
}
