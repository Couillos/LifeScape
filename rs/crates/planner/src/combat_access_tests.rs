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

