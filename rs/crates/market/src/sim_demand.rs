#![forbid(unsafe_code)]
//! Offline check of demand-driven prices (2026-11-13, user rules: 1 coin
//! is only the bootstrap, prices follow supply and demand, buy orders
//! must make production worth it). Same book as the daemon, same pure
//! rules as `market_reprice`:
//! - listings open at the reference (`opening_ask`), bids at the
//!   reference or a covered ask (`opening_bid`);
//! - once per patience window (one round here) every resting quote takes
//!   one `demand_step`: a filled ask climbs, an idle ask falls, an idle
//!   bid climbs to its wallet cap, a filled bid falls to its opening;
//! - a producer lists a unit only while its margin at the price it can
//!   get now (`unit_margin(opening_ask)`) is positive: the margin comes
//!   from the caller (the brain plugs `planner::profit::chain_profit`).
//!
//! No floor tied to input cost anywhere: a listing that rests walks down
//! to 1 coin like any other.

use crate::book::{Book, Fill, Side};
use crate::price::{reference_price, PricedFill};
use crate::zip::{demand_step, opening_ask, opening_bid};

const ITEM: i64 = 1;
/// A seller's walk-up has no cap of its own (the daemon's 5x guard band
/// bounds it live).
const SELL_CAP: i64 = i64::MAX / 4;

/// One participant of a run.
#[derive(Clone, Debug)]
pub enum Agent {
    /// Holds `stock` units and lists them all at once.
    Seller { name: String, stock: i64 },
    /// Wants `units`, pays at most `ceiling` (its wallet cap), shows up
    /// at `arrives`.
    Buyer { name: String, units: i64, ceiling: i64, arrives: usize },
    /// Makes one unit every `every` rounds while the margin is positive.
    Producer { name: String, every: usize },
}

/// Outcome of one run.
#[derive(Clone, Debug, Default)]
pub struct DemandRun {
    /// `(round, price, quantity)` of every fill, in order.
    pub prints: Vec<(usize, i64, i64)>,
    /// Round of the producer's first listing (`None`: never worth it).
    pub first_listing: Option<usize>,
    /// Best bid when the producer first listed.
    pub bid_at_first_listing: Option<i64>,
    /// Producer margin of each unit it sold, at the fill price.
    pub producer_margins: Vec<f64>,
    /// Reference after the last round.
    pub final_reference: i64,
    /// Units every buyer received.
    pub served: i64,
}

#[derive(Clone, Debug)]
struct Resting {
    owner: usize,
    id: u64,
    price: i64,
    seen_remaining: i64,
    floor: i64,
    ceiling: i64,
    sell: bool,
}

struct Engine {
    book: Book,
    fills: Vec<PricedFill>,
    next_id: u64,
    resting: Vec<Resting>,
}

impl Engine {
    fn reference(&self) -> i64 {
        let recent: Vec<PricedFill> = self.fills.iter().rev().take(20).cloned().collect();
        let (bid, ask) = self.book.best();
        reference_price(&recent, self.fills.last().map(|fill| fill.price), bid, ask).price
    }

    fn place(&mut self, owner: usize, name: &str, side: Side, qty: i64, price: i64, floor: i64, ceiling: i64) -> Vec<Fill> {
        let id = self.next_id;
        self.next_id += 1;
        let Ok((fills, rests)) = self.book.place(id, name, ITEM, side, qty, price) else { return Vec::new() };
        if rests {
            let left = self.book.remaining(id).unwrap_or(0);
            self.resting.push(Resting { owner, id, price, seen_remaining: left, floor, ceiling, sell: side == Side::Sell });
        }
        fills
    }

    /// One patience window for every resting quote.
    fn walk_all(&mut self) -> Vec<Fill> {
        let mut out = Vec::new();
        let mut kept = Vec::new();
        for mut quote in std::mem::take(&mut self.resting) {
            let Some(left) = self.book.remaining(quote.id) else { continue };
            let filled = left < quote.seen_remaining;
            let next = demand_step(quote.price, quote.sell, filled, quote.floor, quote.ceiling);
            quote.seen_remaining = left;
            if next == quote.price {
                kept.push(quote);
                continue;
            }
            let new_id = self.next_id;
            self.next_id += 1;
            let Ok((replacement, fills)) = self.book.amend(quote.id, next, new_id) else { continue };
            out.extend(fills);
            if replacement.remaining > 0 {
                kept.push(Resting { id: new_id, price: next, seen_remaining: replacement.remaining, ..quote });
            }
        }
        self.resting = kept;
        out
    }

    fn rests_for(&self, owner: usize) -> bool {
        self.resting.iter().any(|quote| quote.owner == owner && self.book.remaining(quote.id).is_some())
    }
}

/// Runs `rounds` rounds of `agents` trading one item. `unit_margin` maps
/// a sale price to the producer's margin per unit (sale minus inputs at
/// market prices); sellers and buyers ignore it.
pub fn run_demand(agents: &[Agent], unit_margin: &dyn Fn(i64) -> f64, rounds: usize) -> DemandRun {
    let mut engine = Engine { book: Book::new(), fills: Vec::new(), next_id: 1, resting: Vec::new() };
    let mut run = DemandRun::default();
    let mut wants: Vec<i64> = agents.iter().map(|agent| if let Agent::Buyer { units, .. } = agent { *units } else { 0 }).collect();
    let mut listed = vec![false; agents.len()];
    let names: Vec<String> = agents
        .iter()
        .map(|agent| match agent {
            Agent::Seller { name, .. } | Agent::Buyer { name, .. } | Agent::Producer { name, .. } => name.clone(),
        })
        .collect();
    for round in 0..rounds {
        let mut fills = engine.walk_all();
        for (owner, agent) in agents.iter().enumerate() {
            let reference = engine.reference();
            let (bid, ask) = engine.book.best();
            match agent {
                Agent::Seller { stock, .. } if !listed[owner] => {
                    listed[owner] = true;
                    let price = opening_ask(reference, bid);
                    fills.extend(engine.place(owner, &names[owner], Side::Sell, *stock, price, 1, SELL_CAP));
                }
                Agent::Buyer { ceiling, arrives, .. } if *arrives <= round && !listed[owner] => {
                    listed[owner] = true;
                    let price = opening_bid(reference, ask, *ceiling);
                    fills.extend(engine.place(owner, &names[owner], Side::Buy, wants[owner], price, price, *ceiling));
                }
                Agent::Producer { every, .. } if round % (*every).max(1) == 0 && !engine.rests_for(owner) => {
                    let price = opening_ask(reference, bid);
                    if unit_margin(price) > 0.0 {
                        if run.first_listing.is_none() {
                            run.first_listing = Some(round);
                            run.bid_at_first_listing = bid;
                        }
                        fills.extend(engine.place(owner, &names[owner], Side::Sell, 1, price, 1, SELL_CAP));
                    }
                }
                _ => {}
            }
        }
        for fill in fills {
            run.prints.push((round, fill.price, fill.quantity));
            engine.fills.push(PricedFill { quantity: fill.quantity, price: fill.price, clock: round as i64 });
            if let Some(buyer) = names.iter().position(|name| *name == fill.buyer) {
                wants[buyer] -= fill.quantity;
                run.served += fill.quantity;
            }
            if let Some(seller) = names.iter().position(|name| *name == fill.seller) {
                if matches!(agents[seller], Agent::Producer { .. }) {
                    run.producer_margins.push(unit_margin(fill.price));
                }
            }
        }
    }
    run.final_reference = engine.reference();
    run
}

#[cfg(test)]
mod tests {
    use super::{run_demand, Agent};

    fn buyer(name: &str, units: i64, ceiling: i64, arrives: usize) -> Agent {
        Agent::Buyer { name: name.to_string(), units, ceiling, arrives }
    }

    /// The user's loop: a wanted object nobody makes (reference 1) gets a
    /// bid that walks up; once it covers the producer's inputs the
    /// producer starts and sells at a profit, and the price stays well
    /// above the 1-coin bootstrap.
    #[test]
    fn rising_bid_makes_production_profitable() {
        let cost = 20.0;
        let margin = |price: i64| price as f64 - cost;
        let agents = vec![buyer("fighter", 5, 60, 0), Agent::Producer { name: "smith".to_string(), every: 3 }];
        let run = run_demand(&agents, &margin, 200);
        eprintln!("prints {:?} first {:?} margins {:?} ref {}", run.prints, run.first_listing, run.producer_margins, run.final_reference);
        let first = run.first_listing.expect("the bid makes production worth it");
        assert!(first > 0, "nothing worth making at the 1-coin bootstrap");
        assert!(run.bid_at_first_listing.is_some_and(|bid| bid as f64 > cost), "production starts once the bid covers the inputs");
        assert_eq!(run.served, 5, "every unit wanted is made and sold");
        let average = run.producer_margins.iter().sum::<f64>() / run.producer_margins.len() as f64;
        assert!(average > 0.0, "the producer earns on average: {:?}", run.producer_margins);
        assert!(run.prints.iter().all(|(_, price, _)| *price <= 60), "never above the buyer's cap");
        assert!(run.final_reference as f64 >= cost, "price formed far above 1: {}", run.final_reference);
    }

    /// Without enough wallet behind the bid the producer never starts:
    /// the bid parks at its cap, under the inputs.
    #[test]
    fn a_poor_bid_draws_no_production() {
        let margin = |price: i64| price as f64 - 20.0;
        let agents = vec![buyer("fighter", 1, 12, 0), Agent::Producer { name: "smith".to_string(), every: 3 }];
        let run = run_demand(&agents, &margin, 200);
        assert_eq!(run.first_listing, None);
        assert!(run.prints.is_empty());
    }

    /// An ask that keeps selling (a buyer every patience window) climbs
    /// from the 1-coin bootstrap up to what buyers pay; never past their
    /// caps. With buyers rarer than the window, the idle windows pull it
    /// back down: the price reads the demand rate.
    #[test]
    fn asks_that_sell_climb_toward_what_buyers_pay() {
        let mut agents = vec![Agent::Seller { name: "miner".to_string(), stock: 200 }];
        for index in 0..90 {
            agents.push(buyer(&format!("buyer{index}"), 2, 10, index));
        }
        let run = run_demand(&agents, &|_| 0.0, 100);
        eprintln!("prints {:?} ref {}", run.prints, run.final_reference);
        assert!(run.final_reference >= 6, "steady demand lifts the 1-coin listing, got {}", run.final_reference);
        assert!(run.prints.iter().all(|(_, price, _)| *price <= 10), "never past the buyers' caps");
        let mut sparse = vec![Agent::Seller { name: "miner".to_string(), stock: 200 }];
        for index in 0..30 {
            sparse.push(buyer(&format!("buyer{index}"), 2, 10, index * 3));
        }
        let quiet = run_demand(&sparse, &|_| 0.0, 100);
        assert!(quiet.final_reference < run.final_reference, "rarer buyers, lower price: {} vs {}", quiet.final_reference, run.final_reference);
    }

    /// A glut stays cheap: many listings at 1, thin demand.
    #[test]
    fn a_glut_stays_cheap() {
        let mut agents: Vec<Agent> = (0..12).map(|index| Agent::Seller { name: format!("seller{index}"), stock: 50 }).collect();
        for index in 0..6 {
            agents.push(buyer(&format!("buyer{index}"), 1, 10, index * 10));
        }
        let run = run_demand(&agents, &|_| 0.0, 80);
        eprintln!("glut prints {:?} ref {}", run.prints, run.final_reference);
        assert_eq!(run.served, 6);
        assert!(run.final_reference <= 3, "supply keeps it near 1, got {}", run.final_reference);
    }
}
