//! Plan marché Q6 (wave 3) criteria on fixed seeds: the same sparse
//! market with and without dedicated market makers (wave 2 = the
//! concession alone), and a maker on an object whose value falls.

use super::*;
use crate::maker::MAKER_CAPS;
use crate::sim_maker::{falling_market, median, sparse_market, spreads, two_sided};

const SEEDS: [u64; 5] = [11, 22, 33, 44, 55];
const ITEMS: usize = 40;
const SPREAD: u64 = 30_000;
const END: u64 = SPREAD + 36_000;
/// A maker's capital in the simulator: the order of a slice's target
/// (`maker::capital_target`, ~100 000 coins for 54 objects).
const CAPITAL: i64 = 100_000;

struct Pooled {
    two_sided: usize,
    samples: usize,
    spreads: Vec<f64>,
    fills: usize,
    maker_pnl: i64,
}

fn pooled(seed: u64, with_maker: bool) -> Pooled {
    let mut out = Pooled { two_sided: 0, samples: 0, spreads: Vec::new(), fills: 0, maker_pnl: 0 };
    for item in sparse_market(seed, ITEMS, SPREAD) {
        let maker = with_maker.then(|| SimMaker::standard(CAPITAL, item.value, Some(item.outside)));
        // Q3 for every seller in both runs (wave 2 rule, `outlets`).
        let run = run_with_outlet(&item.traders, Model::Concession, END, maker, Some(item.outside));
        let (both, all) = two_sided(&run.samples);
        out.two_sided += both;
        out.samples += all;
        out.spreads.extend(spreads(&run.samples));
        out.fills += run.run.fills.len();
        if let Some(outcome) = run.maker {
            out.maker_pnl += outcome.value - CAPITAL;
        }
        // Every trader stays inside its own limit, makers or not.
        for fill in &run.run.fills {
            let limit = |name: &str| item.traders.iter().find(|trader| trader.name == name).map(|trader| trader.limit);
            if let Some(buyer) = limit(&fill.buyer) {
                assert!(fill.price <= buyer, "seed {seed}: {fill:?}");
            }
            if let Some(seller) = limit(&fill.seller) {
                assert!(fill.price >= seller, "seed {seed}: {fill:?}");
            }
        }
    }
    out
}

/// Q6: with makers the share of two-sided books and the median ask/bid
/// spread both beat wave 2 (concession alone) on every seed.
#[test]
fn makers_make_more_books_two_sided_and_tighter() {
    let mut failures = Vec::new();
    eprintln!("seed | model        | two-sided         | median spread | fills | maker P&L");
    for seed in SEEDS {
        let wave2 = pooled(seed, false);
        let wave3 = pooled(seed, true);
        let share = |pool: &Pooled| pool.two_sided as f64 / pool.samples.max(1) as f64;
        let spread = |pool: &Pooled| median(&pool.spreads).unwrap_or(f64::INFINITY);
        for (label, pool) in [("wave 2", &wave2), ("wave 3 maker", &wave3)] {
            eprintln!(
                "{seed:>4} | {label:<12} | {:>5}/{:<5} {:>5.1} % | x{:>12.3} | {:>5} | {:>9}",
                pool.two_sided,
                pool.samples,
                share(pool) * 100.0,
                spread(pool),
                pool.fills,
                pool.maker_pnl
            );
        }
        if share(&wave3) <= share(&wave2) {
            failures.push(format!("seed {seed}: two-sided {:.3} <= {:.3}", share(&wave3), share(&wave2)));
        }
        if spread(&wave3) >= spread(&wave2) {
            failures.push(format!("seed {seed}: median spread x{:.3} >= x{:.3}", spread(&wave3), spread(&wave2)));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// Q6: a maker on an object whose value falls 100 -> 10 keeps its loss
/// inside the object's risk share (10 % of its capital) and its stock
/// under the cap; the market still clears.
#[test]
fn a_maker_survives_a_falling_object() {
    for seed in SEEDS {
        let traders = falling_market(seed, 60_000);
        let run = run_with_maker(&traders, Model::Concession, 100_000, Some(SimMaker::standard(CAPITAL, 100, Some(5))));
        let outcome = run.maker.expect("maker outcome");
        eprintln!("falling seed {seed}: {outcome:?}, fills {}", run.run.fills.len());
        let loss = CAPITAL - outcome.value;
        assert!(loss <= (CAPITAL as f64 * MAKER_CAPS.max_item_capital_share) as i64, "seed {seed}: lost {loss} on one object: {outcome:?}");
        assert!(outcome.max_stock <= MAKER_CAPS.max_stock, "seed {seed}: {outcome:?}");
        assert!(outcome.coins >= 0, "seed {seed}: never short of coins");
        assert!(!run.run.fills.is_empty());
    }
}

/// The maker never moves a wave-2 market without it: `run_lifecycle`
/// equals the maker-less run.
#[test]
fn no_maker_is_the_wave_2_run() {
    let traders = continuous_market(11, 20, 20, 18_000);
    let plain = run_lifecycle(&traders, Model::Concession, 54_000);
    let none = run_with_maker(&traders, Model::Concession, 54_000, None);
    assert_eq!(plain.fills, none.run.fills);
    assert!(none.maker.is_none());
}

