//! Plan marché M3-M5 criteria on fixed seeds, new concession against
//! the wave-1 5 % step (same arrivals, same book).

use super::*;

const SEEDS: [u64; 5] = [11, 22, 33, 44, 55];
const SPREAD: u64 = 18_000;
const HORIZON: u64 = 36_000;

struct Measures {
    ratio: f64,
    error: f64,
    inversions: f64,
    raw_inversions: f64,
    fills: usize,
    mean_wait: u64,
    lowest: i64,
    amends: usize,
    withdrawn: usize,
}

fn measure(seed: u64, model: Model) -> Measures {
    let traders = continuous_market(seed, 20, 20, SPREAD);
    let (pairs, equilibrium_price) = equilibrium(&traders);
    let run = run_lifecycle(&traders, model, SPREAD + HORIZON);
    let limit_of = |name: &str| traders.iter().find(|trader| trader.name == name).map(|trader| trader.limit).unwrap_or(0);
    for fill in &run.fills {
        // A buyer never pays over its ceiling; a seller may sell under
        // its own cost (the 10 % walk, user 2026-11-19: no seller floor).
        assert!(limit_of(&fill.buyer) >= fill.price, "{model:?} seed {seed}: {fill:?} buyer {}", limit_of(&fill.buyer));
    }
    for trader in &traders {
        assert!(units_of(&run, &trader.name) <= trader.units, "{model:?} seed {seed}: {} traded {}", trader.name, units_of(&run, &trader.name));
    }
    let units: i64 = run.fills.iter().map(|fill| fill.quantity).sum();
    let price = final_price(&run.fills).unwrap_or(0);
    // Ticks from a buyer's arrival to its first fill (served buyers).
    let waits: Vec<u64> = traders
        .iter()
        .filter(|trader| !trader.sell)
        .filter_map(|trader| run.fills.iter().find(|fill| fill.buyer == trader.name).map(|fill| fill.tick - trader.arrives))
        .collect();
    Measures {
        ratio: units as f64 / pairs.max(1) as f64,
        error: (price - equilibrium_price).abs() as f64 / equilibrium_price.max(1) as f64,
        inversions: inversion_share(&run.fills),
        raw_inversions: raw_inversion_share(&run.fills),
        fills: run.fills.len(),
        mean_wait: waits.iter().sum::<u64>() / (waits.len().max(1) as u64),
        lowest: run.lowest_quote,
        amends: run.amends,
        withdrawn: run.withdrawn,
    }
}

/// M3 simulator: 20 buyers and 20 sellers with drawn limits, varied
/// patience, continuous arrivals. Fills >= 90 % of the compatible pairs
/// within the horizon, direction inversions of the current price <= 15 % of the fills over
/// the five seeds (one seed alone may jitter a coin around a stationary
/// equilibrium: <= 20 %), never a 1-coin price when every reserve is
/// >= 5. The table compares the wave-1 5 % step on the same arrivals.
/// The 10 % walk (user 2026-11-19) has no seller floor: sellers trade
/// under their own cost, so the price settles under the equilibrium of
/// the drawn limits (17-27 % on these seeds, printed, never asserted).
#[test]
fn concession_meets_the_m3_criteria() {
    let mut failures = Vec::new();
    let (mut inversions, mut prints) = (0.0, 0usize);
    eprintln!("seed | model      | fills/pairs | |p-eq|/eq | inversions (raw) | buyer wait | lowest | amends | withdrawn");
    for seed in SEEDS {
        for model in [Model::Concession, Model::OldStep] {
            let seen = measure(seed, model);
            eprintln!(
                "{seed:>4} | {:<10} | {:>11.2} | {:>9.3} | {:>6.3} ({:.3}) | {:>10} | {:>6} | {:>6} | {:>9}",
                format!("{model:?}"),
                seen.ratio,
                seen.error,
                seen.inversions,
                seen.raw_inversions,
                seen.mean_wait,
                seen.lowest,
                seen.amends,
                seen.withdrawn
            );
            if model == Model::Concession {
                if seen.ratio < 0.9 {
                    failures.push(format!("seed {seed}: fills {:.2} of the compatible pairs", seen.ratio));
                }
                if seen.inversions > 0.20 {
                    failures.push(format!("seed {seed}: inversions {:.3}", seen.inversions));
                }
                if seen.lowest <= 1 {
                    failures.push(format!("seed {seed}: a 1-coin quote with every reserve >= 50"));
                }
                inversions += seen.inversions * seen.fills as f64;
                prints += seen.fills;
            }
        }
    }
    let pooled = inversions / prints.max(1) as f64;
    eprintln!("pooled current-price inversions (concession): {pooled:.3} over {prints} prints");
    if pooled > 0.15 {
        failures.push(format!("pooled inversions {pooled:.3}"));
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

fn urgent_market(urgency: Urgency, patience: f64, horizon: u64) -> Vec<Trader> {
    vec![
        Trader { name: "fighter".to_string(), sell: false, limit: 130, units: 1, patience, urgency, horizon_ticks: horizon, arrives: 0, loot: false, anchor: None },
        Trader { name: "looter".to_string(), sell: true, limit: 90, units: 1, patience: 0.5, urgency: Urgency::Patient, horizon_ticks: 72_000, arrives: 1000, loot: true, anchor: Some(115) },
    ]
}

/// M4: an urgent buyer takes an ask that enters its limit within one
/// window; a patient one waits for its curve and pays the seller's
/// price, later.
#[test]
fn an_urgent_buyer_is_served_in_one_window() {
    let urgent = run_lifecycle(&urgent_market(Urgency::Pressed, 0.5, 6000), Model::Concession, 80_000);
    let fill = urgent.fills.first().expect("the urgent buyer is served");
    assert!(fill.tick <= 1000 + window_ticks(6000), "served at {} (ask listed at 1000)", fill.tick);
    assert_eq!(fill.price, 90, "at the seller's ask (its keep value over the resting bid)");
    let patient = run_lifecycle(&urgent_market(Urgency::Normal, 0.9, 36_000), Model::Concession, 80_000);
    let later = patient.fills.first().expect("the patient buyer is served too");
    assert!(later.tick > fill.tick + window_ticks(6000), "the patient waits: {} vs {}", later.tick, fill.tick);
    // The 10 % walk (user 2026-11-19): the ask walks down, the bid up,
    // they meet under the seller's opening.
    assert!(later.price < 90, "both sides walked: {}", later.price);
}

/// M4: a pressed seller (a full bank) hits the resting bid (no seller
/// floor since the 10 % walk, user 2026-11-19): the seller is the taker.
#[test]
fn a_pressed_seller_hits_the_bid() {
    let traders = vec![
        Trader { name: "smith".to_string(), sell: false, limit: 100, units: 5, patience: 0.5, urgency: Urgency::Normal, horizon_ticks: 36_000, arrives: 0, loot: false, anchor: None },
        Trader { name: "full_bank".to_string(), sell: true, limit: 60, units: 3, patience: 0.5, urgency: Urgency::Pressed, horizon_ticks: 72_000, arrives: 30_000, loot: true, anchor: Some(150) },
    ];
    let run = run_lifecycle(&traders, Model::Concession, 40_000);
    assert!(run.fills.iter().any(|fill| fill.seller_took), "{:?}", run.fills);
    assert_eq!(units_of(&run, "full_bank"), 3);
}

fn loot_market() -> Vec<Trader> {
    let mut rng = FixedRng::new(7);
    let mut traders: Vec<Trader> = (0..50)
        .map(|index| Trader {
            name: format!("looter{index}"),
            sell: true,
            limit: rng.between(5, 30),
            units: 10,
            patience: rng.between(5, 95) as f64 / 100.0,
            urgency: Urgency::Patient,
            horizon_ticks: 72_000,
            arrives: index as u64 * 400,
            loot: true,
            anchor: Some(21),
        })
        .collect();
    for (index, arrives) in [2000u64, 10_000, 20_000].into_iter().enumerate() {
        traders.push(Trader { name: format!("buyer{index}"), sell: false, limit: 40, units: 20, patience: 0.5, urgency: Urgency::Normal, horizon_ticks: 36_000, arrives, loot: false, anchor: None });
    }
    traders
}

/// M5: 50 loot sellers, 3 buyers. Since the 10 % walk (user
/// 2026-11-19: no seller floor, "sinon les prix ne sont pas réels"), 500
/// units against 60 wanted walk under every keep value: the price says
/// the glut. Every buyer is served.
#[test]
fn loot_sellers_walk_under_their_keep_value_in_a_glut() {
    let traders = loot_market();
    let lowest_keep = traders.iter().filter(|trader| trader.sell).map(|trader| trader.limit).min().unwrap_or(0);
    let new = run_lifecycle(&traders, Model::Concession, 100_000);
    let old = run_lifecycle(&traders, Model::OldStep, 100_000);
    let served = |run: &LifecycleRun| (0..3).map(|index| units_of(run, &format!("buyer{index}"))).sum::<i64>();
    eprintln!("loot | model      | lowest quote | lowest print | median print | served | amends");
    for (label, run) in [("Concession", &new), ("OldStep", &old)] {
        let lowest_print = run.fills.iter().map(|fill| fill.price).min().unwrap_or(0);
        eprintln!("loot | {label:<10} | {:>12} | {:>12} | {:>12} | {:>6} | {:>6}", run.lowest_quote, lowest_print, final_price(&run.fills).unwrap_or(0), served(run), run.amends);
    }
    assert!(new.lowest_quote < lowest_keep, "the glut walks under the keep values: {}", new.lowest_quote);
    assert_eq!(served(&new), 60, "every buyer served");
}

#[test]
fn equilibrium_counts_compatible_pairs() {
    let trader = |name: &str, sell: bool, limit: i64| Trader { name: name.to_string(), sell, limit, units: 1, patience: 0.5, urgency: Urgency::Normal, horizon_ticks: 36_000, arrives: 0, loot: false, anchor: None };
    let traders = vec![trader("b1", false, 120), trader("b2", false, 90), trader("b3", false, 60), trader("s1", true, 50), trader("s2", true, 80), trader("s3", true, 100)];
    assert_eq!(equilibrium(&traders), (2, 85), "120>=50, 90>=80; clearing range [80, 90]");
}

