use super::*;
use crate::rates_estimator::RunSample;

fn shrimps(qty: i64, ticks: i64) -> RunSample {
    RunSample {
        work_ticks: ticks,
        pause_ticks: 60,
        flows: vec![("Raw shrimps".to_string(), "product".to_string(), qty)],
        units: vec![("product".to_string(), "Raw shrimps".to_string(), qty)],
        clock: 100,
    }
}

fn query(site: Option<(i64, i64, i64)>) -> RateQuery {
    RateQuery { method: "gather:fishing/raw_shrimp".to_string(), style: String::new(), site, tool: "small_fishing_net".to_string(), level: 3 }
}

fn exact_key() -> YieldKey {
    key_of_query(&query(Some((3087, 3227, 0))))
}

/// Under three runs the game prior serves alone (flagged), its XP
/// derived from its units: shrimps × 10 XP.
#[test]
fn under_three_runs_the_prior_serves() {
    let mut book = YieldBook::default();
    book.record(&exact_key(), &shrimps(100, 1200));
    book.record(&exact_key(), &shrimps(100, 1200));
    let read = estimate(&book, &query(Some((3087, 3227, 0))), 200).expect("prior");
    assert!(read.prior && read.fallback == Fallback::PriorOnly);
    assert_eq!(read.confidence.runs, 2, "the thin bucket still shows its runs");
    let shrimp = read.gained("Raw shrimps");
    assert!(shrimp > 0.0);
    let fishing = read.xp.iter().find(|(skill, _)| skill == "fishing").map(|(_, xp)| *xp).unwrap_or(0.0);
    assert!((fishing - shrimp * 10.0).abs() < 1e-6, "XP derived: {fishing} vs {shrimp} shrimps");
}

/// From three runs the measure blends with the prior, at the exact key.
#[test]
fn three_runs_blend_with_the_prior() {
    let mut book = YieldBook::default();
    for _ in 0..3 {
        book.record(&exact_key(), &shrimps(120, 1200));
    }
    let read = estimate(&book, &query(Some((3087, 3227, 0))), 200).expect("measured");
    assert_eq!(read.fallback, Fallback::Exact);
    assert!(!read.prior);
    let prior = estimate(&YieldBook::default(), &query(Some((3087, 3227, 0))), 200).expect("prior").gained("Raw shrimps");
    let measured = 600.0;
    let bucket = book.get(&exact_key()).expect("bucket");
    let expected = (measured * bucket.work_ticks + prior * PRIOR_WEIGHT_TICKS) / (bucket.work_ticks + PRIOR_WEIGHT_TICKS);
    assert!((read.gained("Raw shrimps") - expected).abs() < 1e-6);
    assert!((read.pause_per_6000 - 300.0).abs() < 1e-6);
}

/// No site: the all-sites roll-up (explicit fallback); a thin exact key
/// falls to the roll-up too.
#[test]
fn fallback_goes_to_all_sites_then_all_tools() {
    let mut book = YieldBook::default();
    for (index, site) in ["1,1,0", "2,2,0", "3,3,0"].iter().enumerate() {
        book.record(&YieldKey { site: site.to_string(), ..exact_key() }, &shrimps(100 + index as i64, 1200));
    }
    assert_eq!(estimate(&book, &query(None), 200).expect("rollup").fallback, Fallback::AllSites);
    assert_eq!(estimate(&book, &query(Some((1, 1, 0))), 200).expect("thin").fallback, Fallback::AllSites);
    let other_tool = RateQuery { tool: "golden_net".to_string(), ..query(None) };
    assert_eq!(estimate(&book, &other_tool, 200).expect("tools").fallback, Fallback::AllSitesTools);
}

/// No measure and no prior: a typed error, never a figure.
#[test]
fn no_measure_no_prior_is_an_error() {
    let read = estimate(&YieldBook::default(), &RateQuery { method: "mill:pot_flour".to_string(), level: 1, ..RateQuery::default() }, 0);
    assert!(matches!(read, Err(RateError::NoPrior { .. })), "{read:?}");
}

/// A melee farm of gems is measured per combat style: the brain reads it
/// at the guides' fight style and the attack band, so the fleet's own
/// yield (not the level-1 prior) answers (live 2026-11-21: "0.00000
/// units a tick" for ~160 gems an hour).
#[test]
fn a_gem_farm_reads_its_measured_melee_yield() {
    let method = "farm:uncut_sapphire";
    assert_eq!(read_style(method), "melee:attack");
    assert_eq!(read_skill(method).as_deref(), Some("attack"));
    let run = RunSample {
        work_ticks: 6000,
        pause_ticks: 60,
        flows: vec![("Uncut sapphire".to_string(), "loot".to_string(), 3)],
        units: vec![("kill".to_string(), "giant".to_string(), 120)],
        clock: 100,
    };
    let query = RateQuery { method: method.to_string(), style: read_style(method).to_string(), site: None, tool: "adamant_battleaxe".to_string(), level: 80 };
    let mut book = YieldBook::default();
    for _ in 0..3 {
        book.record(&key_of_query(&query), &run);
    }
    let prior = gained_per_6000(&YieldBook::default(), method, "Uncut sapphire", 80, "adamant_battleaxe").expect("prior");
    let measured = gained_per_6000(&book, method, "Uncut sapphire", 80, "adamant_battleaxe").expect("measured");
    assert!(measured > 5.0 * prior, "the fleet's measure {measured} against the prior {prior}");
}
