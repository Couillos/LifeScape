//! The draw of R7 (D10, D14) on fabricated candidates: deterministic per
//! seed, the 15 % band, the weights, the 5 % exploration measured over
//! many fabricated draws, profiles that draw differently.

use super::{draw, first_class, in_band, merit_money, merit_training, seed_for, Candidate, EXPLORE_SHARE, MEASURED_RUNS};

fn line(score: f64, runs: u64) -> Candidate {
    Candidate { score, weight: score, runs }
}

/// Draws over `count` seeds of one bot (its counter 0..count), how often
/// each candidate came, and how many draws explored.
fn tally(candidates: &[Candidate], bot: u64, count: u64) -> (Vec<u64>, u64) {
    let mut hits = vec![0u64; candidates.len()];
    let mut explored = 0;
    for counter in 0..count {
        let drawn = draw(candidates, seed_for(bot, "n42", counter)).expect("a draw");
        hits[drawn.index] += 1;
        explored += u64::from(drawn.explored);
    }
    (hits, explored)
}

#[test]
fn the_same_seed_draws_the_same_line() {
    let candidates = vec![line(1000.0, 9), line(950.0, 9), line(900.0, 9)];
    for counter in 0..50 {
        let seed = seed_for(7, "n3", counter);
        assert_eq!(draw(&candidates, seed), draw(&candidates, seed), "replayable");
    }
    assert_ne!(seed_for(7, "n3", 0), seed_for(8, "n3", 0), "another bot");
    assert_ne!(seed_for(7, "n3", 0), seed_for(7, "n4", 0), "another node");
    assert_ne!(seed_for(7, "n3", 0), seed_for(7, "n3", 1), "the next draw");
    assert_eq!(draw(&[], 1), None, "nothing to draw");
}

/// Only the lines within 15 % of the best are drawn (all measured: no
/// exploration), each in proportion to its weight.
#[test]
fn only_lines_within_fifteen_percent_of_the_best_are_drawn_by_weight() {
    assert!(in_band(850.0, 1000.0) && !in_band(849.0, 1000.0));
    assert!(in_band(-1.15, -1.0) && !in_band(-1.2, -1.0), "a negative best (training scores are minus ticks per XP)");
    let candidates = vec![line(1000.0, 9), line(860.0, 9), line(840.0, 9), line(10.0, 9)];
    let (hits, explored) = tally(&candidates, 11, 20_000);
    assert_eq!(explored, 0, "every line measured: no exploration");
    assert_eq!(hits[2] + hits[3], 0, "out of the band: never drawn ({hits:?})");
    let share = hits[0] as f64 / 20_000.0;
    let expected = 1000.0 / 1860.0;
    assert!((share - expected).abs() < 0.02, "weighted by the score: {share:.3} vs {expected:.3}");
    let drawn = draw(&candidates, seed_for(11, "n42", 0)).expect("a draw");
    assert_eq!((drawn.band, drawn.candidates), (2, 4));
}

/// D14: the unmeasured lines outside the band share ~5 % of the draws,
/// uniformly; a measured or worthless line never explores; measured on
/// 40 000 fabricated draws.
#[test]
fn unmeasured_lines_receive_about_five_percent_of_the_draws() {
    let candidates = vec![line(1000.0, 9), line(400.0, 0), line(300.0, MEASURED_RUNS - 1), line(200.0, MEASURED_RUNS), Candidate { score: 100.0, weight: 0.0, runs: 0 }];
    let count = 40_000;
    let (hits, explored) = tally(&candidates, 5, count);
    let share = explored as f64 / count as f64;
    assert!((share - EXPLORE_SHARE).abs() < 0.006, "exploration share {share:.4}");
    assert_eq!(hits[3] + hits[4], 0, "measured, or worth nothing: never explored ({hits:?})");
    let split = hits[1] as f64 / (hits[1] + hits[2]) as f64;
    assert!((split - 0.5).abs() < 0.05, "uniform among the unmeasured: {split:.3}");
    assert_eq!(hits[0] + explored, count, "the rest goes to the band");
    // Every line measured: the whole draw goes to the band.
    let measured: Vec<Candidate> = candidates.iter().map(|known| Candidate { runs: 9, ..known.clone() }).collect();
    assert_eq!(tally(&measured, 5, 2000).1, 0);
}

/// Two profiles on the same lines draw differently: the market bot's
/// weight doubles a market line, the autarkic one's shrinks it.
#[test]
fn different_profiles_draw_different_lines() {
    let farm = 1000.0;
    let market = 900.0;
    let lines = |affinity: f64| vec![Candidate { score: farm, weight: merit_money(farm, false, affinity), runs: 9 }, Candidate { score: market, weight: merit_money(market, true, affinity), runs: 9 }];
    let (trader, _) = tally(&lines(1.0), 3, 10_000);
    let (hermit, _) = tally(&lines(0.0), 3, 10_000);
    assert!(trader[1] > 6000, "a market bot takes the market line most often: {trader:?}");
    assert!(hermit[1] < 4000, "an autarkic bot rarely: {hermit:?}");
    // Same seed, different profile: some draws differ.
    let differ = (0..200).filter(|counter| draw(&lines(1.0), seed_for(3, "n42", *counter)).map(|found| found.index) != draw(&lines(0.0), seed_for(3, "n42", *counter)).map(|found| found.index)).count();
    assert!(differ > 20, "{differ} of 200 draws differ");
}

#[test]
fn training_merit_reads_the_score_lower_is_better() {
    assert!(merit_training(2.0) > merit_training(4.0), "fewer ticks per XP weigh more");
    assert!(merit_training(-3.0) > merit_training(-1.0), "a line earning more per XP weighs more");
}

#[test]
fn the_first_non_empty_class_draws() {
    assert_eq!(first_class(&[2, 1, 2, 1]), (Some(1), vec![1, 3]));
    assert_eq!(first_class(&[2, 2]), (Some(2), vec![0, 1]));
    assert_eq!(first_class(&[9, 9]), (None, Vec::new()), "a class outside the order never draws");
}
