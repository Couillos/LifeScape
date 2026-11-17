//! User lever 2026-11-17 (« entraînement marché plus fréquent »): a
//! market training line buys its band's inputs at once (need bids at the
//! tranche's volume), one square round trip and one market wait per
//! purchase, never one per pack of 28. The money guide's 100 % market
//! line keeps a pack per lot.

use super::{level, row, training_row, Fixture};
use crate::guide_model::{Family, GuideFilter, Supply};
use crate::guide_money::money_guide;
use crate::guide_training::{band_xp, training_guide};

/// Extra ticks per 6000 a line pays beyond its own work: 6000 × (raw XP
/// / scaled XP − 1).
fn extra_of(raw_xp: f64, xp_6000: f64) -> f64 {
    6000.0 * (raw_xp / xp_6000 - 1.0)
}

/// Firemaking 56-60 on logs (300 burnt per 6000 ticks of work, 40 XP
/// each): the band asks `band_xp(60)` XP, `band_xp / 40` logs. The
/// market line pays the wait once per band (12 000 XP / band XP purchases
/// per 6000 ticks of work), not once per 28 logs (10.7 purchases).
#[test]
fn a_market_climb_pays_one_wait_per_band_not_per_pack() {
    let filter = GuideFilter { skill: Some("firemaking".to_string()), level: Some(60), ..GuideFilter::default() };
    let raw_xp = 300.0 * 40.0;
    let mut extras = Vec::new();
    for wait in [100.0, 1100.0] {
        let mut fixture = Fixture::new();
        fixture.wait = Some(wait);
        let (rows, _) = fixture.run(|engine| training_guide(engine, &filter));
        let market = training_row(&rows, "fire:logs", Supply::Market);
        extras.push(extra_of(raw_xp, market.xp_6000));
    }
    let purchases = (extras[1] - extras[0]) / 1000.0;
    let band = raw_xp / band_xp(60);
    assert!((purchases - band).abs() < 1e-6, "one purchase per band: {purchases} vs {band}");
    assert!(purchases < 300.0 / 28.0 / 5.0, "{purchases}");
}

/// A band smaller than one pack (firemaking 1-5 burns ~13 logs) still
/// buys a pack per purchase (D15's lot), and the money guide's 100 %
/// market line keeps one purchase per pack whatever the band.
#[test]
fn a_small_band_and_the_money_line_keep_a_pack_per_purchase() {
    let filter = GuideFilter { skill: Some("firemaking".to_string()), level: Some(5), ..GuideFilter::default() };
    let raw_xp = 300.0 * 40.0;
    assert!(band_xp(5) / 40.0 < 28.0, "the band holds less than a pack");
    let mut extras = Vec::new();
    for wait in [100.0, 1100.0] {
        let mut fixture = Fixture::new();
        fixture.wait = Some(wait);
        let (rows, _) = fixture.run(|engine| training_guide(engine, &filter));
        extras.push(extra_of(raw_xp, training_row(&rows, "fire:logs", Supply::Market).xp_6000));
    }
    let purchases = (extras[1] - extras[0]) / 1000.0;
    assert!((purchases - 300.0 / 28.0).abs() < 1e-6, "a pack per purchase: {purchases}");
    let mut fixture = Fixture::new();
    fixture.wait = Some(100.0);
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(60)));
    let near = row(&rows, "smelt:steel_bar", Family::Market).extra_ticks_per_6000;
    fixture.wait = Some(1100.0);
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(60)));
    let far = row(&rows, "smelt:steel_bar", Family::Market).extra_ticks_per_6000;
    assert!(((far - near) / 1000.0 - 300.0 / 28.0).abs() < 1e-6, "100 % market: a pack per lot");
}
