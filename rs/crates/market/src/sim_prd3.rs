#![forbid(unsafe_code)]
//! Offline check of the PRD3 price formation (lot 2), on the real book:
//! every object starts at 1 coin (never traded), buyers open at the
//! reference (or take a covered ask) and walk up to their ceiling, sellers
//! ask the reference (or hit a richer bid) and walk down to their floor,
//! one `step_toward` per round. Deterministic on the seed. Fill-driven
//! steps (`demand_step`) are exercised by `sim_demand`.
//!
//! Questions it answers (see the tests): does a wanted, scarce object get
//! bid up from 1? Does an abundant one stay cheap? Does anything run
//! away past what buyers will pay?

use crate::book::{Book, Side};
use crate::price::{reference_price, PricedFill};
use crate::sim::FixedRng;
use crate::zip::step_toward;

/// One synthetic participant: wants (buyer) or holds (seller) units,
/// with its private bound (buyer ceiling = value, seller floor = cost).
#[derive(Clone, Debug)]
pub struct Trader {
    pub name: String,
    pub buying: bool,
    pub units: i64,
    pub bound: i64,
}

/// Outcome of one run.
#[derive(Clone, Debug)]
pub struct Formation {
    /// Trade prices in order.
    pub prints: Vec<i64>,
    /// Reference after the last round.
    pub final_reference: i64,
    /// Highest bound any buyer had (nothing may trade above it).
    pub max_ceiling: i64,
    /// Units traded.
    pub volume: i64,
}

fn opening(buying: bool, reference: i64, bid: Option<i64>, ask: Option<i64>, bound: i64) -> i64 {
    if buying {
        match ask {
            Some(best) if best <= bound => best,
            _ => reference.min(bound).max(1),
        }
    } else {
        crate::zip::opening_ask(reference, bid).max(bound)
    }
}

/// Runs `rounds` rounds. `arrivals` adds (round, trader) as production
/// or demand appears over time.
pub fn run_formation(mut traders: Vec<Trader>, arrivals: Vec<(usize, Trader)>, rounds: usize, seed: u64) -> Formation {
    let mut rng = FixedRng::new(seed);
    let mut book = Book::new();
    let mut fills: Vec<PricedFill> = Vec::new();
    let mut prints = Vec::new();
    // Resting order per trader: (order id, price).
    let mut resting: Vec<Option<(u64, i64)>> = vec![None; traders.len()];
    let mut next_id: u64 = 1;
    let max_ceiling = traders.iter().chain(arrivals.iter().map(|(_, trader)| trader)).filter(|trader| trader.buying).map(|trader| trader.bound).max().unwrap_or(1);
    for round in 0..rounds {
        for (at, trader) in &arrivals {
            if *at == round {
                traders.push(trader.clone());
                resting.push(None);
            }
        }
        // Shuffle the turn order deterministically.
        let mut order: Vec<usize> = (0..traders.len()).collect();
        for index in (1..order.len()).rev() {
            let swap = rng.between(0, index as i64) as usize;
            order.swap(index, swap);
        }
        for who in order {
            if traders[who].units <= 0 {
                continue;
            }
            let recent: Vec<PricedFill> = fills.iter().rev().take(20).cloned().collect();
            let reference = reference_price(&recent, prints.last().copied(), None, None).price;
            let (bid, ask) = book.best();
            let price = match resting[who] {
                // Unfilled since last round: one step toward the market.
                Some((id, current)) => {
                    let next = step_toward(current, !traders[who].buying, if traders[who].buying { 1 } else { traders[who].bound }, if traders[who].buying { traders[who].bound } else { i64::MAX / 4 });
                    if next == current {
                        continue;
                    }
                    let _ = book.cancel(id);
                    next
                }
                None => opening(traders[who].buying, reference, bid, ask, traders[who].bound),
            };
            let id = next_id;
            next_id += 1;
            let side = if traders[who].buying { Side::Buy } else { Side::Sell };
            let Ok((matched, rests)) = book.place(id, &traders[who].name, 1, side, traders[who].units, price) else { continue };
            for fill in &matched {
                prints.push(fill.price);
                fills.push(PricedFill { quantity: fill.quantity, price: fill.price, clock: round as i64 });
                for (index, trader) in traders.iter_mut().enumerate() {
                    if trader.name == fill.buyer || trader.name == fill.seller {
                        trader.units -= fill.quantity;
                        if trader.units <= 0 {
                            resting[index] = None;
                        }
                    }
                }
            }
            resting[who] = if rests && traders[who].units > 0 { Some((id, price)) } else { None };
        }
    }
    let recent: Vec<PricedFill> = fills.iter().rev().take(20).cloned().collect();
    let volume = fills.iter().map(|fill| fill.quantity).sum();
    Formation { final_reference: reference_price(&recent, prints.last().copied(), None, None).price, prints, max_ceiling, volume }
}

fn trader(name: &str, buying: bool, units: i64, bound: i64) -> Trader {
    Trader { name: name.to_string(), buying, units, bound }
}

/// Scarce and wanted: eight buyers worth 40-80 each want one, two
/// sellers hold one each (cost 5), one more unit is produced every 10
/// rounds.
pub fn scarce_scenario(seed: u64) -> Formation {
    let mut rng = FixedRng::new(seed);
    let mut traders: Vec<Trader> = (0..8).map(|index| trader(&format!("buyer{index}"), true, 1, rng.between(40, 80))).collect();
    traders.push(trader("seller0", false, 1, 5));
    traders.push(trader("seller1", false, 1, 5));
    let arrivals = (1..=6).map(|index| (index * 10, trader(&format!("producer{index}"), false, 1, 5))).collect();
    run_formation(traders, arrivals, 80, seed)
}

/// Abundant: three buyers worth 10-15 want two each, twelve sellers hold
/// five each at cost 1.
pub fn abundant_scenario(seed: u64) -> Formation {
    let mut rng = FixedRng::new(seed);
    let mut traders: Vec<Trader> = (0..3).map(|index| trader(&format!("buyer{index}"), true, 2, rng.between(10, 15))).collect();
    for index in 0..12 {
        traders.push(trader(&format!("seller{index}"), false, 5, 1));
    }
    run_formation(traders, Vec::new(), 60, seed)
}

#[cfg(test)]
mod tests {
    use super::{abundant_scenario, scarce_scenario};

    #[test]
    fn scarce_wanted_items_get_bid_up_from_one_coin() {
        for seed in [1u64, 7, 42] {
            let run = scarce_scenario(seed);
            eprintln!("scarce seed {seed}: prints {:?} final {}", run.prints, run.final_reference);
            assert!(run.prints.len() >= 6, "most produced units sell, got {:?}", run.prints);
            assert!(run.final_reference >= 20, "demand lifts the price well above 1, got {}", run.final_reference);
            assert!(run.prints.iter().all(|price| *price <= run.max_ceiling), "never above what buyers pay");
        }
    }

    #[test]
    fn abundant_items_stay_cheap() {
        for seed in [1u64, 7, 42] {
            let run = abundant_scenario(seed);
            eprintln!("abundant seed {seed}: prints {:?} final {}", run.prints, run.final_reference);
            assert_eq!(run.volume, 6, "every buyer served");
            assert!(run.final_reference <= 3, "supply keeps it near 1, got {}", run.final_reference);
        }
    }
}
