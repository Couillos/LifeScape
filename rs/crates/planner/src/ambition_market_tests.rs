//! User lever 2026-11-17 (« entraînement marché plus fréquent »): the
//! share of ambitions on transformation skills follows market_affinity,
//! inside the existing catalogue, from a share computed from the game
//! data (never a list of skills).

use crate::ambition_catalog::{specs, transform_share};
use crate::ambition_fleet::{affinity_tilt, assign_fleet, FleetBot};

/// A kind fed by buy-able inputs (its transformation share at least half).
fn transforms(kind: &str) -> bool {
    transform_share(kind) >= 0.5
}

/// The catalogue's kinds split from the game data: the skills a player
/// trains by transforming bought stock (bars, raw fish, logs, hides,
/// bones, essence, runes) against harvests and fights.
#[test]
fn transformation_kinds_come_from_the_game_data() {
    let shares: Vec<(&str, f64)> = specs().iter().map(|entry| (entry.kind, transform_share(entry.kind))).collect();
    println!("transformation shares: {shares:?}");
    for kind in ["smith", "cook", "firemaker", "crafter", "priest", "runecrafter", "mage"] {
        assert!(transforms(kind), "{kind}: {shares:?}");
    }
    for kind in ["warrior", "ranger", "woodcutter", "fisher", "miner"] {
        assert!(!transforms(kind), "{kind}: {shares:?}");
    }
    assert_eq!(transform_share("generalist"), 0.0, "no primary group");
}

/// The tilt: neutral at 0.5, a market bot leans on transformation kinds,
/// an autarkic one on harvests and fights, continuously.
#[test]
fn the_tilt_follows_the_trait() {
    assert!((affinity_tilt("smith", 0.5) - 1.0).abs() < 1e-12 && (affinity_tilt("miner", 0.5) - 1.0).abs() < 1e-12);
    assert!(affinity_tilt("smith", 0.8) > 1.5 && affinity_tilt("miner", 0.8) < 0.7);
    assert!(affinity_tilt("smith", 0.1) < 0.6 && affinity_tilt("miner", 0.1) > 1.5);
    assert!(affinity_tilt("cook", 0.9) > affinity_tilt("cook", 0.7) && affinity_tilt("cook", 0.7) > affinity_tilt("cook", 0.3));
}

/// A fabricated fleet of 76 fresh profiles (every level 1, affinities
/// spread over 0..1, distinct seeds): the share of transformation
/// ambitions rises with the trait (the coverage of the 12 groups kept).
#[test]
fn the_fleet_ambitions_follow_market_affinity() {
    let fleet: Vec<FleetBot> = (0..76u64)
        .map(|index| FleetBot {
            account: format!("bot{index:02}"),
            levels: Vec::new(),
            ironman: false,
            market_affinity: index as f64 / 75.0,
            seed: 0x9E37_79B9_7F4A_7C15u64.wrapping_mul(index + 1),
            kind: None,
            market_maker: false,
            operator: false,
        })
        .collect();
    let assigned = assign_fleet(&fleet, &[]);
    assert_eq!(assigned.len(), 76);
    let covered: std::collections::BTreeSet<String> = assigned.iter().filter(|row| row.why.starts_with("coverage:")).map(|row| row.why.clone()).collect();
    assert_eq!(covered.len(), 12, "every group covered first: {covered:?}");
    let affinity_of = |account: &str| fleet.iter().find(|bot| bot.account == account).map(|bot| bot.market_affinity).unwrap_or(0.5);
    let share = |low: f64, high: f64| {
        let tier: Vec<bool> = assigned.iter().filter(|row| (low..high).contains(&affinity_of(&row.account))).map(|row| transforms(&row.kind)).collect();
        tier.iter().filter(|hit| **hit).count() as f64 / tier.len().max(1) as f64
    };
    let (autarkic, middle, market) = (share(0.0, 0.4), share(0.4, 0.6), share(0.6, 1.01));
    println!("transformation ambitions: autarkic {autarkic:.2}, middle {middle:.2}, market {market:.2}");
    assert!(market >= 0.7, "market profiles: {market:.2}");
    assert!(autarkic <= 0.45, "autarkic profiles: {autarkic:.2}");
    assert!(market - autarkic >= 0.3, "{autarkic:.2} -> {market:.2}");
}
