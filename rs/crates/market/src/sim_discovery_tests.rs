//! Plan marché M7 : objet neuf, 5 vendeurs à coûts 20-40, 5 acheteurs à
//! valeurs 30-90 ; premier fill dans [20, 90] en ≤ 3 fenêtres, aucune
//! vente sous l'offre de la boutique, avec et sans teneur de marché.
//! Le tableau compare les ouvertures de la vague 2 (sans ancrage).

use super::*;

const SEEDS: [u64; 5] = [11, 22, 33, 44, 55];

#[test]
fn a_new_object_is_discovered_on_its_public_anchors() {
    let mut failures = Vec::new();
    eprintln!("seed | maker | anchors | met at | first fill (tick, price, windows) | fills | lowest | npc units");
    for seed in SEEDS {
        for maker in [false, true] {
            for anchored in [false, true] {
                let (traders, run) = run(seed, maker, anchored.then(anchors));
                let fills = &run.run.fills;
                let met = sides_meet_at(&traders, maker);
                let first = met.and_then(|met| first_fill(fills, met));
                let lowest = fills.iter().map(|fill| fill.price).min().unwrap_or(0);
                eprintln!(
                    "{seed:>4} | {:>5} | {:>7} | {:>6} | {:>32} | {:>5} | {:>6} | {:>9}",
                    maker,
                    anchored,
                    met.map(|tick| tick.to_string()).unwrap_or_else(|| "never".to_string()),
                    first.as_ref().map(|(fill, windows)| format!("{} @ {} in {windows}", fill.tick, fill.price)).unwrap_or_else(|| "none".to_string()),
                    fills.len(),
                    lowest,
                    run.run.npc_units
                );
                if !anchored {
                    continue;
                }
                match (met, first) {
                    // Every seller came before any buyer: the shop took them.
                    (None, _) => {}
                    (Some(_), Some((fill, windows))) => {
                        if !(20..=90).contains(&fill.price) {
                            failures.push(format!("seed {seed} maker {maker}: first fill {} outside [20, 90]", fill.price));
                        }
                        if windows > 3 {
                            failures.push(format!("seed {seed} maker {maker}: first fill {windows} windows after both sides met"));
                        }
                    }
                    (Some(met), None) => failures.push(format!("seed {seed} maker {maker}: no fill though both sides met at {met}")),
                }
                if let Some(under) = fills.iter().find(|fill| fill.price < SHOP_SALE) {
                    failures.push(format!("seed {seed} maker {maker}: sale under the shop's offer {SHOP_SALE}: {under:?}"));
                }
                let limit_of = |name: &str| traders.iter().find(|trader| trader.name == name).map(|trader| trader.limit);
                for fill in fills {
                    if limit_of(&fill.buyer).is_some_and(|value| fill.price > value) {
                        failures.push(format!("seed {seed} maker {maker}: a buyer paid over its value: {fill:?}"));
                    }
                    if limit_of(&fill.seller).is_some_and(|cost| fill.price < cost) {
                        failures.push(format!("seed {seed} maker {maker}: a seller sold under its cost: {fill:?}"));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn the_scenario_is_the_plans() {
    for seed in SEEDS {
        let traders = new_object_market(seed);
        assert_eq!(traders.iter().filter(|trader| trader.sell).count(), 5);
        assert!(traders.iter().all(|trader| if trader.sell { (20..=40).contains(&trader.limit) } else { (30..=90).contains(&trader.limit) }));
    }
}
