//! Site gates in the site pick (audit 2026-11-13 item A5): guild doors,
//! keys, the Karamja boat, dungeons and the Wilderness.

use super::{min_combat_for_method, pick_site_gated, Fare, Pass, SitePick};

const FALADOR: (i64, i64, i64) = (2964, 3378, 0);

fn broke() -> Fare {
    Fare { wallet: 0, free_pass: false }
}

fn crafter(level: u8, held: &[&str]) -> Pass {
    Pass { levels: vec![("crafting".to_string(), level)], held: held.iter().map(|item| item.to_string()).collect(), quest_points: 0 }
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
    let cook = Pass { levels: vec![("cooking".to_string(), 32)], held: vec!["chefs_hat".to_string()], quest_points: 0 };
    // The guild range opens with the level and the hat; the fastest
    // range (bank loads weighed, `bank_weight`) may still be another.
    let guild = |site: (i64, i64, i64)| lifescape_gamedata::access::gates_at(site.0, site.1, site.2).iter().any(|gate| gate.area().contains("cook"));
    match pick_site_gated("cook/raw_trout", 3, Some(cooks_guild), broke(), &cook) {
        SitePick::At(_) => {}
        other => panic!("ranges exist, got {other:?}"),
    }
    match pick_site_gated("cook/raw_trout", 3, Some(cooks_guild), broke(), &Pass::default()) {
        SitePick::At(site) => assert!(!guild(site), "no hat, no guild: {site:?}"),
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
    Pass { levels: vec![("mining".to_string(), level)], held: Vec::new(), quest_points: 0 }
}

fn in_mining_guild(pick: &SitePick) -> bool {
    matches!(pick, SitePick::At((x, z, level)) if lifescape_gamedata::access::gates_at(*x, *z, *level).iter().any(|gate| gate.area() == "mining_guild"))
}

/// The Mining Guild door opens at mining 60 (never below), and the guild
/// is a site like any other (user decision D2, 2026-11-17): a qualified
/// miner standing at Falador, by the guild ladder, works it; read from
/// its surface entrance, never 6 000 tiles away underground.
#[test]
fn a_qualified_miner_works_the_mining_guild() {
    for ore in ["coal", "mithril_ore"] {
        let method = format!("gather:mining/{ore}");
        let pick = pick_site_gated(&method, 40, Some(FALADOR), broke(), &miner(63));
        assert!(in_mining_guild(&pick), "{ore} at 63 from Falador: {pick:?}");
        let below = pick_site_gated(&method, 40, Some(FALADOR), broke(), &miner(59));
        assert!(!in_mining_guild(&below), "{ore} at 59: {below:?}");
    }
}

/// Plan P7a red test (D2): six fleet miners on the guild's mithril
/// rocks fill it; the next qualified miner goes to another mithril site.
#[test]
fn a_full_guild_sends_the_next_miner_elsewhere() {
    let method = "gather:mining/mithril_ore";
    let SitePick::At(guild) = pick_site_gated(method, 40, Some(FALADOR), broke(), &miner(63)) else { panic!("mithril workable") };
    assert!(in_mining_guild(&SitePick::At(guild)));
    let capacity = super::site_capacity(method, guild).expect("a rock field has a capacity");
    assert!(capacity >= 1 && capacity < 12, "guild mithril rocks: {capacity}");
    let crowd = [(guild, 6u32.max(capacity as u32))];
    assert!(super::site_full(method, guild, &crowd));
    let pick = super::pick_site_crowded(method, 40, Some(FALADOR), broke(), &miner(63), &crowd);
    assert!(!in_mining_guild(&pick), "a full guild is no pick while other mithril is open: {pick:?}");
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
    // A rock field whose respawns feed several miners: one is no crowd.
    let tin = "gather:mining/tin_ore";
    let SitePick::At(field) = pick_site_gated(tin, 40, Some(EDGEVILLE), broke(), &miner(30)) else { panic!("tin is workable at 30") };
    assert!(super::site_capacity(tin, field).is_some_and(|capacity| capacity >= 2), "tin respawns in 10 ticks: {:?}", super::site_capacity(tin, field));
    let lone = [(field, 1u32)];
    assert_eq!(super::pick_site_crowded(tin, 40, Some(EDGEVILLE), broke(), &miner(30), &lone), SitePick::At(field), "one miner is no crowd");
}

/// Lot 3, live 2026-11-19: 39 % of the filler time mined coal and two
/// coal fields jammed (`contended`) 38 and 19 times, the second miner
/// drawn there by a crowd of one to three fleet bots (the rock count
/// read the four Barbarian Village rocks as four seats). A coal rock
/// respawns in 100 ticks (`mine.dbrow` `rock_respawnrate`): those four
/// rocks feed ONE miner, the next one goes elsewhere; a big field (the
/// Dwarven Mine) feeds several.
#[test]
fn a_slow_rock_field_holds_the_miners_its_respawns_feed() {
    const VARROCK_WEST: (i64, i64, i64) = (3185, 3436, 0);
    let method = "gather:mining/coal";
    let barbarian = (3089, 3434, 0);
    assert_eq!(super::site_capacity(method, barbarian), Some(1), "four coal rocks, 100-tick respawn");
    assert!(super::site_full(method, barbarian, &[(barbarian, 1u32)]), "one miner fills it");
    // The fastest coal (bank loads weighed): filled to its capacity, the
    // next miner goes elsewhere.
    let SitePick::At(field) = super::pick_site_crowded(method, 40, Some(VARROCK_WEST), broke(), &miner(60), &[]) else { panic!("coal is open") };
    let seats = super::site_capacity(method, field).expect("a rock field") as u32;
    let crowd = [(field, seats)];
    assert!(super::site_full(method, field, &crowd), "{seats} miners fill {field:?}");
    match super::pick_site_crowded(method, 40, Some(VARROCK_WEST), broke(), &miner(60), &crowd) {
        SitePick::At(other) | SitePick::Tolled(other) => assert!(other != field && !super::site_full(method, other, &crowd), "the next miner goes to a free seat: {other:?}"),
        other => panic!("other coal is open: {other:?}"),
    }
    let dwarven = (3028, 9737, 0);
    let big = super::site_capacity(method, dwarven).expect("a rock field");
    assert!(big >= 2 && big < 21, "21 coal rocks feed a few miners: {big}");
}

/// Review lot 3: the seats are counted for the pickaxe a miner at the
/// rock's level wields (`pickaxe_checker.rs2`: rune from 41, never at
/// iron's 15 or tin's 1): a slower swing leaves the rocks time to
/// respawn, a fast-respawn field seats several miners.
#[test]
fn a_rock_field_seats_miners_with_the_pickaxe_of_its_level() {
    assert_eq!(crate::guide_methods::tool_at("gather:mining/iron_ore", "", 15), "steel_pickaxe");
    let rimmington_iron = super::site_capacity("gather:mining/iron_ore", (2968, 3239, 0)).expect("iron field");
    assert!(rimmington_iron >= 3, "six iron rocks, 20-tick respawn, a steel pickaxe at 15: {rimmington_iron}");
    let varrock_tin = super::site_capacity("gather:mining/tin_ore", (3172, 3366, 0)).expect("tin field");
    assert!(varrock_tin >= 6, "tin respawns in 10 ticks: {varrock_tin}");
}

/// Review lot 3: only the bots on the same resource fill a site. A
/// fighter on the Barbarian Village barbarians, beside the coal rocks,
/// lengthens the walk like any crowd but never fills the coal seats:
/// the next miner still mines there; a coal miner there fills them.
#[test]
fn a_crowd_of_another_method_never_fills_the_coal_seats() {
    let method = "gather:mining/coal";
    let barbarian = (3089, 3434, 0);
    let mut ctx = crate::expand::ReadyCtx::blind(0.5);
    ctx.combat = 40;
    ctx.position = Some((3185, 3436, 0));
    ctx.levels = vec![("mining".to_string(), 60)];
    let _ = barbarian;
    let SitePick::At(field) = super::pick_site_for(method, &ctx) else { panic!("coal is open") };
    let seats = super::site_capacity(method, field).expect("a rock field") as u32;
    ctx.seats = vec![(field, "fight:melee/barbarians".to_string(), seats)];
    assert_eq!(super::pick_site_for(method, &ctx), SitePick::At(field), "fighters are no miners");
    ctx.seats = vec![(field, "farm:coal".to_string(), seats)];
    assert!(super::pick_site_for(method, &ctx) != SitePick::At(field), "coal miners take the seats");
}

/// Tour 2 P2-3a red test: cooling one rock cools its site. lily_brook
/// cooled (3242,3160) on the Lumbridge swamp and was sent to (3243,3166),
/// (3243,3167), (3243,3158)... seven "sites" in ten tiles. A cooled site
/// covers every row within `CROWD_RADIUS` on its level.
#[test]
fn a_cooled_rock_cools_its_whole_site() {
    let method = "gather:mining/mithril_ore";
    let table = super::method_site_table(method).expect("mithril sites");
    let swamp: Vec<(i64, i64, i64)> =
        table.iter().map(|row| row.site).filter(|site| site.2 == 0 && (site.0 - 3243).abs() <= 6 && (site.1 - 3162).abs() <= 8).collect();
    assert!(swamp.len() >= 2, "several swamp mithril rows: {swamp:?}");
    let avoid = [swamp[0]];
    let pick = super::pick_site_avoiding(method, 40, Some((3244, 3165, 0)), broke(), &miner(63), &[], &avoid);
    let site = match pick {
        SitePick::At(site) | SitePick::Tolled(site) | SitePick::Boat { site, .. } => site,
        other => panic!("another mithril site serves: {other:?}"),
    };
    assert!(
        site.2 != avoid[0].2 || (site.0 - avoid[0].0).abs().max((site.1 - avoid[0].1).abs()) > super::CROWD_RADIUS,
        "cooled {:?}, picked {site:?} in the same site",
        avoid[0]
    );
}

/// Tour 2 P2-3b red test (D6): every site of a method full (the fleet
/// works every node) is `Full`, never a forced full site; the node's next
/// method serves.
#[test]
fn a_method_full_everywhere_is_full() {
    let method = "gather:mining/mithril_ore";
    let table = super::method_site_table(method).expect("mithril sites");
    let crowd: Vec<((i64, i64, i64), u32)> = table.iter().map(|row| (row.site, 50u32)).collect();
    let rich = Fare { wallet: 10_000, free_pass: true };
    assert_eq!(super::pick_site_crowded(method, 99, Some((3244, 3165, 0)), rich, &miner(99), &crowd), SitePick::Full);
}
