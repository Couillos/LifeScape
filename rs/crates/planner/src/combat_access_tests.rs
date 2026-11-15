//! Site gates in the site pick (audit 2026-11-13 item A5): guild doors,
//! keys, the Karamja boat, dungeons and the Wilderness.

use super::{min_combat_for_method, pick_site_gated, Fare, Pass, SitePick};

const FALADOR: (i64, i64, i64) = (2964, 3378, 0);

fn broke() -> Fare {
    Fare { wallet: 0, free_pass: false }
}

fn crafter(level: u8, held: &[&str]) -> Pass {
    Pass { levels: vec![("crafting".to_string(), level)], held: held.iter().map(|item| item.to_string()).collect() }
}

/// Gold from Falador: the Crafting Guild rocks only with crafting 40
/// and a brown apron; otherwise the open rocks (Rimmington), never the
/// guild interior.
#[test]
fn guild_rocks_need_the_level_and_the_uniform() {
    let inside = |pick: SitePick| match pick {
        SitePick::At((x, z, level)) => !lifescape_gamedata::access::gates_at(x, z, level).is_empty(),
        other => panic!("gold is workable, got {other:?}"),
    };
    assert!(!inside(pick_site_gated("gather:mining/gold_ore", 60, Some(FALADOR), broke(), &Pass::default())), "no gate passed");
    assert!(!inside(pick_site_gated("gather:mining/gold_ore", 60, Some(FALADOR), broke(), &crafter(40, &[]))), "no apron");
    assert!(inside(pick_site_gated("gather:mining/gold_ore", 60, Some(FALADOR), broke(), &crafter(40, &["brown_apron"]))), "guild open");
}

/// Karamja water is a boat site (`access::BOAT_ROUTED`, user goal
/// 2026-11-13): lobster, tuna and swordfish are workable, the round trip
/// (30 coins each way, from the pack) budgeted like the Al Kharid toll:
/// covered, the site is taken with `boat`; short, the round trip is the
/// coin need.
#[test]
fn the_boat_opens_karamja_with_its_fare() {
    for fish in ["raw_lobster", "raw_tuna", "raw_swordfish"] {
        let method = format!("gather:fishing/{fish}");
        match pick_site_gated(&method, 126, Some(FALADOR), Fare { wallet: 1_000, free_pass: false }, &Pass::default()) {
            SitePick::Boat { site, toll: false } => assert!(lifescape_gamedata::access::behind_boat(site.0, site.1, site.2), "{fish} at {site:?}"),
            other => panic!("{fish}: the fare is covered, got {other:?}"),
        }
        assert_eq!(pick_site_gated(&method, 126, Some(FALADOR), Fare { wallet: 59, free_pass: true }, &Pass::default()), SitePick::Fare(60), "{fish}: a round trip");
        assert!(min_combat_for_method(&method) < lifescape_gamedata::danger::NEVER, "{fish} is workable");
    }
    // East of the Al Kharid gate the trip pays the toll too.
    let al_kharid = (3290, 3180, 0);
    assert_eq!(pick_site_gated("gather:fishing/raw_lobster", 126, Some(al_kharid), broke(), &Pass::default()), SitePick::Fare(70));
    assert!(matches!(pick_site_gated("gather:fishing/raw_lobster", 126, Some(al_kharid), Fare { wallet: 70, free_pass: false }, &Pass::default()), SitePick::Boat { toll: true, .. }));
    // Mainland water always wins when it yields the fish.
    assert!(matches!(pick_site_gated("gather:fishing/raw_shrimp", 126, Some(FALADOR), Fare { wallet: 1_000, free_pass: false }, &Pass::default()), SitePick::At(_)));
}

/// Only the boat-armed scripts (gatherers, fights) take a Karamja site:
/// a station there stays out, and Crandor never opens.
#[test]
fn stations_never_sail() {
    for method in ["smelt:bronze_bar", "cook/raw_shrimp", "spin:ball_of_wool"] {
        if let SitePick::At(site) | SitePick::Boat { site, .. } = pick_site_gated(method, 126, Some(FALADOR), Fare { wallet: 1_000, free_pass: false }, &Pass::default()) {
            assert!(!lifescape_gamedata::access::behind_boat(site.0, site.1, site.2), "{method} sails to {site:?}");
        }
    }
    assert!(lifescape_gamedata::access::impassable(&lifescape_gamedata::access::Gate::Boat { area: "crandor", fare: 0 }));
}

/// Stations are sites too (args contract 2026-11-13): a smelter, smith,
/// cook, spinner, potter, jeweller and runecrafter are sent to a picked
/// station; the Cooks' Guild range only with cooking 32 and a chef's hat.
#[test]
fn stations_are_picked_sites() {
    for method in ["smelt:bronze_bar", "smith:bronze_sword", "cook/raw_shrimp", "spin:ball_of_wool", "pottery:pot_empty", "jewelry:gold_ring", "runecraft:airrune"] {
        match pick_site_gated(method, 3, Some(FALADOR), broke(), &Pass::default()) {
            SitePick::At(site) => assert!(lifescape_gamedata::access::gates_at(site.0, site.1, site.2).is_empty(), "{method} at {site:?}"),
            other => panic!("{method}: a station is picked, got {other:?}"),
        }
    }
    let cooks_guild = (3144, 3450, 1);
    let cook = Pass { levels: vec![("cooking".to_string(), 32)], held: vec!["chefs_hat".to_string()] };
    match pick_site_gated("cook/raw_trout", 3, Some(cooks_guild), broke(), &cook) {
        SitePick::At(site) => assert!(!lifescape_gamedata::access::gates_at(site.0, site.1, site.2).is_empty(), "the guild range: {site:?}"),
        other => panic!("ranges exist, got {other:?}"),
    }
}

/// Live 2026-11-13 (bd23dqpq, 53/53/53/53): `melee dark_wizards --want
/// cosmicrune 1` died at once (dark wizards cast: no melee exchange the
/// ladder can rate, `melee_ladder::RUNGS` leaves them out). A drop farm
/// hunts only a spot the melee executor rates, and only when the bot's
/// live stats survive it; otherwise the market (or another way) serves.
#[test]
fn drop_farms_hunt_only_rated_survivable_spots() {
    assert_eq!(crate::mapping::spot_for_drop("blackwizhat"), None, "only dark wizards drop it");
    assert_ne!(crate::mapping::spot_for_drop("cosmicrune").as_deref(), Some("dark_wizards"));
    assert_eq!(crate::mapping::spot_for_drop("bones").as_deref(), Some("chickens"));
    let mut fresh = crate::expand::ReadyCtx::blind(0.5);
    fresh.levels = ["attack", "strength", "defence"].iter().map(|skill| (skill.to_string(), 1)).chain([("hitpoints".to_string(), 10)]).collect();
    fresh.combat = 3;
    assert!(super::drop_farm_fit("farm:bones", &fresh), "chickens fit a fresh bot");
    assert!(!super::rung_fit("guards", &fresh), "guards kill a fresh bot");
    let mut strong = crate::expand::ReadyCtx::blind(0.5);
    strong.levels = ["attack", "strength", "defence", "hitpoints"].iter().map(|skill| (skill.to_string(), 53)).collect();
    strong.combat = 66;
    assert!(super::rung_fit("guards", &strong));
    assert!(super::rung_fit("guards", &crate::expand::ReadyCtx::blind(0.5)), "offline (no live levels): no stat gate");
    assert!(!crate::tools::method_feasible("farm:blackwizhat", &strong), "no rated spot drops it");
    assert!(crate::tools::method_feasible("farm:bones", &fresh));
}


fn miner(level: u8) -> Pass {
    Pass { levels: vec![("mining".to_string(), level)], held: Vec::new() }
}

fn in_mining_guild(pick: &SitePick) -> bool {
    matches!(pick, SitePick::At((x, z, level)) if lifescape_gamedata::access::gates_at(*x, *z, *level).iter().any(|gate| gate.area() == "mining_guild"))
}

/// User 2026-11-15: a miner who passes the Mining Guild door (mining 60)
/// works its coal and mithril, even from Edgeville where the Barbarian
/// Village coal is nearer; below 60, never inside.
#[test]
fn a_qualified_miner_works_the_mining_guild() {
    const EDGEVILLE: (i64, i64, i64) = (3094, 3491, 0);
    for ore in ["coal", "mithril_ore"] {
        let method = format!("gather:mining/{ore}");
        let pick = pick_site_gated(&method, 40, Some(EDGEVILLE), broke(), &miner(63));
        assert!(in_mining_guild(&pick), "{ore} at 63: {pick:?}");
        let below = pick_site_gated(&method, 40, Some(EDGEVILLE), broke(), &miner(59));
        assert!(!in_mining_guild(&below), "{ore} at 59: {below:?}");
    }
}

/// Lobsters and swordfish are Karamja water: a fisher with the fare goes.
#[test]
fn lobsters_and_swordfish_sail_to_karamja() {
    let rich = Fare { wallet: 10_000, free_pass: false };
    for fish in ["raw_lobster", "raw_swordfish"] {
        let pick = pick_site_gated(&format!("gather:fishing/{fish}"), 40, Some(FALADOR), rich, &Pass::default());
        assert!(matches!(pick, SitePick::Boat { .. }), "{fish}: {pick:?}");
    }
}

/// A crowded coal site reads farther: with a dozen fleet miners on the
/// nearest coal rocks a miner below the guild level picks other coal.
#[test]
fn a_crowded_coal_site_sends_the_next_miner_elsewhere() {
    const EDGEVILLE: (i64, i64, i64) = (3094, 3491, 0);
    let method = "gather:mining/coal";
    let SitePick::At(nearest) = pick_site_gated(method, 40, Some(EDGEVILLE), broke(), &miner(30)) else {
        panic!("coal is workable at 30");
    };
    let crowd = [(nearest, 12u32)];
    let pick = super::pick_site_crowded(method, 40, Some(EDGEVILLE), broke(), &miner(30), &crowd);
    match pick {
        SitePick::At(other) => assert!(super::crowd_penalty(other, &crowd) == 0, "crowded {nearest:?}, picked {other:?}"),
        other => panic!("another coal site exists: {other:?}"),
    }
    let lone = [(nearest, 1u32)];
    assert_eq!(super::pick_site_crowded(method, 40, Some(EDGEVILLE), broke(), &miner(30), &lone), SitePick::At(nearest), "one miner is no crowd");
}
