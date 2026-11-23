//! F2P combat spots (catalogue §5.12, M10b §4): every target with
//! position, access and danger. Positions come from `f2p_npc_spawns`
//! (D63); the dump `members` flag is ignored.

use crate::f2p::f2p_npc_spawns;

/// Surface tile used to reach an underground spot.
#[derive(Debug, Clone, Copy)]
pub struct Entrance {
    pub surface: (i64, i64),
}

/// One training spot: NPC debugnames, melee range, access and danger.
/// `access` names what the bot needs (fare, toll, quest) or `None`;
/// `danger` is `safe`, `aggressive`, `wild` or `underground`.
#[derive(Debug, Clone, Copy)]
pub struct CombatSpot {
    pub key: &'static str,
    pub npc_names: &'static [&'static str],
    pub min_level: i64,
    pub max_level: i64,
    pub entrance: Option<Entrance>,
    pub access: Option<&'static str>,
    pub danger: &'static str,
}

const EDGEVILLE_DUNGEON: Entrance = Entrance { surface: (3097, 3468) };
const VARROCK_SEWERS: Entrance = Entrance { surface: (3237, 3458) };
/// The rope down the Karamja volcano (`volcano_entrance_358` (2856,3168)
/// -> (2856,9568); `climbing_rope2_0` back up).
const KARAMJA_VOLCANO: Entrance = Entrance { surface: (2856, 3168) };
/// The Mudskipper Point cellar ladder into the Asgarnian ice dungeon
/// (`ladder_cellar_inside_down_71` (3008,3150) -> (3008,9550)).
const MUDSKIPPER_POINT: Entrance = Entrance { surface: (3008, 3150) };
const KARAMJA_BOAT: &str = "boat 30gp port_sarim";
const WILD_DANGER: &str = "wild";

// Nine legacy spots (planner/scripts compatible): `all_spots()` and
// `best_spot()` stay on these so the `farm:bronze_axe → goblins`
// mapping and the t02 melee progression never change. The full
// catalogue (§5.12) lives in `CATALOGUE_SPOTS` for P2.3/P2.5.
const SPOTS: [CombatSpot; 9] = [
    CombatSpot { key: "chickens", npc_names: &["chicken"], min_level: 1, max_level: 9, entrance: None, access: None, danger: "safe" },
    // Right after chickens so best_spot() progression never changes
    // (chickens keeps level 0, cows keeps level 1): explicit keys bypass
    // it anyway.
    CombatSpot { key: "goblins", npc_names: &["goblin"], min_level: 1, max_level: 12, entrance: None, access: None, danger: "safe" },
    CombatSpot { key: "cows", npc_names: &["cow"], min_level: 1, max_level: 12, entrance: None, access: None, danger: "safe" },
    // Weakest first: men (vislevel 2) sort before al_kharid_warriors
    // (vislevel 9) so best_spot() progression never shadows the
    // stronger camp (t02 live: melee 15 stuck on men).
    CombatSpot {
        key: "men",
        npc_names: &["man", "man2", "man3", "woman", "woman2", "woman3"],
        min_level: 8,
        max_level: 25,
        entrance: None,
        access: None,
        danger: "safe",
    },
    CombatSpot {
        key: "al_kharid_warriors",
        npc_names: &["al_kharid_warrior"],
        min_level: 8,
        max_level: 20,
        entrance: None,
        access: Some("toll 10gp"),
        danger: "safe",
    },
    CombatSpot { key: "guards", npc_names: &["guard1", "guard2"], min_level: 20, max_level: 40, entrance: None, access: None, danger: "safe" },
    CombatSpot {
        key: "dark_wizards",
        npc_names: &["young_dark_wizard", "bearded_dark_wizard"],
        min_level: 20,
        max_level: 45,
        entrance: None,
        access: None,
        danger: "aggressive",
    },
    CombatSpot {
        key: "hill_giants",
        npc_names: &["giant"],
        min_level: 30,
        max_level: 60,
        entrance: Some(EDGEVILLE_DUNGEON),
        access: None,
        danger: "underground",
    },
    CombatSpot {
        key: "moss_giants",
        npc_names: &["mossgiant"],
        min_level: 40,
        max_level: 80,
        entrance: Some(VARROCK_SEWERS),
        access: None,
        danger: "underground",
    },
];

/// Full catalogue §5.12 (28 spots): legacy nine plus rats, imps, monks,
/// barbarians, crypt crawlers, bears, hobgoblins, red spiders, ice and
/// wilderness targets, the demons. Ordered weakest first; new low spots
/// sort where they never shadow the legacy `best_spot()` progression.
/// No green dragons: members NPCs never spawn on this F2P world
/// (`GameMap.ts`, `world.json` members false).
const CATALOGUE_SPOTS: [CombatSpot; 28] = [
    CombatSpot { key: "chickens", npc_names: &["chicken"], min_level: 1, max_level: 9, entrance: None, access: None, danger: "safe" },
    CombatSpot { key: "monks", npc_names: &["monk"], min_level: 5, max_level: 20, entrance: None, access: None, danger: "safe" },
    CombatSpot { key: "barbarians", npc_names: &["barbarian"], min_level: 8, max_level: 25, entrance: None, access: None, danger: "safe" },
    CombatSpot { key: "rats", npc_names: &["giantrat", "giantrat1", "rat"], min_level: 1, max_level: 12, entrance: None, access: None, danger: "safe" },
    CombatSpot { key: "imps", npc_names: &["imp"], min_level: 1, max_level: 15, entrance: None, access: None, danger: "safe" },
    CombatSpot { key: "goblins", npc_names: &["goblin"], min_level: 1, max_level: 12, entrance: None, access: None, danger: "safe" },
    CombatSpot { key: "cows", npc_names: &["cow"], min_level: 1, max_level: 12, entrance: None, access: None, danger: "safe" },
    CombatSpot {
        key: "men",
        npc_names: &["man", "man2", "man3", "woman", "woman2", "woman3"],
        min_level: 8,
        max_level: 25,
        entrance: None,
        access: None,
        danger: "safe",
    },
    CombatSpot {
        key: "al_kharid_warriors",
        npc_names: &["al_kharid_warrior"],
        min_level: 8,
        max_level: 20,
        entrance: None,
        access: Some("toll 10gp"),
        danger: "safe",
    },
    CombatSpot { key: "skeletons", npc_names: &["skeleton_unarmed", "skeleton_armed"], min_level: 20, max_level: 40, entrance: Some(VARROCK_SEWERS), access: None, danger: "underground" },
    CombatSpot { key: "zombies", npc_names: &["zombie2", "zombie_unarmed", "zombie_armed"], min_level: 20, max_level: 40, entrance: Some(VARROCK_SEWERS), access: None, danger: "underground" },
    CombatSpot { key: "giant_spiders", npc_names: &["giantspider1", "spider"], min_level: 20, max_level: 45, entrance: Some(VARROCK_SEWERS), access: None, danger: "aggressive" },
    CombatSpot { key: "scorpions", npc_names: &["scorpion"], min_level: 20, max_level: 40, entrance: None, access: None, danger: "safe" },
    CombatSpot { key: "ghosts", npc_names: &["ghost"], min_level: 20, max_level: 40, entrance: Some(VARROCK_SEWERS), access: None, danger: "underground" },
    // The surface undead (low Wilderness skeletons, `param=undead`): the
    // crumble undead target a magic run reaches on foot (D18, 2026-11-16;
    // the sewer camps above ride only with the melee ladder's clusters).
    CombatSpot { key: "wild_skeletons", npc_names: &["skeleton_unarmed"], min_level: 20, max_level: 40, entrance: None, access: None, danger: WILD_DANGER },
    CombatSpot { key: "guards", npc_names: &["guard1", "guard2"], min_level: 20, max_level: 40, entrance: None, access: None, danger: "safe" },
    CombatSpot {
        key: "dark_wizards",
        npc_names: &["young_dark_wizard", "bearded_dark_wizard"],
        min_level: 20,
        max_level: 45,
        entrance: None,
        access: None,
        danger: "aggressive",
    },
    CombatSpot {
        key: "hill_giants",
        npc_names: &["giant"],
        min_level: 30,
        max_level: 60,
        entrance: Some(EDGEVILLE_DUNGEON),
        access: None,
        danger: "underground",
    },
    CombatSpot { key: "bears", npc_names: &["brownbear", "darkbear"], min_level: 30, max_level: 45, entrance: None, access: None, danger: WILD_DANGER },
    CombatSpot { key: "hobgoblins", npc_names: &["hobgoblin_unarmed", "hobgoblin_armed"], min_level: 30, max_level: 50, entrance: Some(EDGEVILLE_DUNGEON), access: None, danger: "underground" },
    CombatSpot { key: "red_spiders", npc_names: &["deadly_red_spider"], min_level: 35, max_level: 60, entrance: Some(VARROCK_SEWERS), access: None, danger: "aggressive" },
    CombatSpot {
        key: "moss_giants",
        npc_names: &["mossgiant"],
        min_level: 40,
        max_level: 80,
        entrance: Some(VARROCK_SEWERS),
        access: None,
        danger: "underground",
    },
    CombatSpot { key: "ice_warriors", npc_names: &["icewarrior"], min_level: 50, max_level: 70, entrance: None, access: None, danger: WILD_DANGER },
    CombatSpot { key: "ice_giants", npc_names: &["icegiant"], min_level: 55, max_level: 80, entrance: None, access: None, danger: WILD_DANGER },
    // The two ice camps where warriors and giants share the ground (each
    // hunts the bot that came for the other): the Asgarnian ice dungeon
    // under Mudskipper Point and the deep Wilderness camp.
    CombatSpot { key: "ice_dungeon", npc_names: &["icewarrior", "icegiant"], min_level: 50, max_level: 80, entrance: Some(MUDSKIPPER_POINT), access: None, danger: "underground" },
    CombatSpot { key: "wild_ice", npc_names: &["icewarrior", "icegiant"], min_level: 55, max_level: 85, entrance: None, access: None, danger: WILD_DANGER },
    // The six of the Karamja volcano (the five past `dragonsecretdoor`
    // need Dragon Slayer: `melee_ladder` keeps the reachable area).
    CombatSpot {
        key: "lesser_demons",
        npc_names: &["lesser_demon"],
        min_level: 80,
        max_level: 99,
        entrance: Some(KARAMJA_VOLCANO),
        access: Some(KARAMJA_BOAT),
        danger: "underground",
    },
    // The deep Wilderness camp east of the lava (no boat: on foot).
    CombatSpot { key: "greater_demons", npc_names: &["greater_demon"], min_level: 90, max_level: 99, entrance: None, access: None, danger: WILD_DANGER },
];

/// Every legacy spot, weakest first (planner/scripts compatible).
pub fn all_spots() -> Vec<CombatSpot> {
    SPOTS.to_vec()
}

/// Full catalogue §5.12, weakest first (27 spots for P2.3/P2.5).
pub fn catalogue_spots() -> Vec<CombatSpot> {
    CATALOGUE_SPOTS.to_vec()
}

/// Spot by key, or None (searches the full catalogue).
pub fn by_key(key: &str) -> Option<CombatSpot> {
    CATALOGUE_SPOTS.iter().find(|spot| spot.key == key).copied()
}

/// True when the spot needs the Al Kharid toll (`access` names it).
pub fn toll_gated(spot: &CombatSpot) -> bool {
    spot.access.is_some_and(|access| access.starts_with("toll"))
}

/// True when the spot sits behind the Port Sarim boat (`access` names it).
pub fn boat_gated(spot: &CombatSpot) -> bool {
    spot.access.is_some_and(|access| access.starts_with("boat"))
}

/// Foe level from which a camp is a risk camp (user 2026-11-20): the
/// ice giants (49), ice warriors (57), lesser (82) and greater demons
/// (92). There a death is the price of the drops, never a reason to
/// leave: the bot respawns, re-gears at the bank and comes back; what it
/// dropped is destroyed (`death.rs2`), a sink the fleet makes again.
pub const RISK_FOE_LEVEL: i64 = 45;
/// Combat a risk camp asks under its foes' level: lesser demons (82)
/// from 80, greater demons (92) from 90 (user 2026-11-20).
pub const RISK_LEVEL_SLACK: i64 = 2;

/// Highest combat level (`vislevel`) among the spot's NPCs (0 unknown).
pub fn foe_level(spot: &CombatSpot) -> i64 {
    spot.npc_names.iter().filter_map(|name| crate::npcs_by_name(name).into_iter().next()).map(|npc| npc.vislevel).max().unwrap_or(0)
}

/// True when the spot `key` is a risk camp (`RISK_FOE_LEVEL`).
pub fn risk_spot(key: &str) -> bool {
    by_key(key).is_some_and(|spot| foe_level(&spot) >= RISK_FOE_LEVEL)
}

/// Combat level a risk camp asks (its foes' level less
/// `RISK_LEVEL_SLACK`); `None` for any other spot.
pub fn risk_floor(key: &str) -> Option<i64> {
    let spot = by_key(key)?;
    let level = foe_level(&spot);
    (level >= RISK_FOE_LEVEL).then_some(level - RISK_LEVEL_SLACK)
}

/// Legacy spots whose NPCs drop `item` (debug-name prefix match: the
/// `goblin_greenarmour` drop row meets the `goblins` spot hunting plain
/// `goblin`s), the spots a bot can farm: ungated ones first, then the
/// toll-gated ones (legal once the brain budgets the fare), weakest first
/// in each group; a camp of record underground (the Edgeville dungeon
/// hill giants) ranks after the surface ones of its group. A spot needs
/// a position: an underground camp counts only through the spawns the
/// navigation reaches (`positions`, user 2026-11-21: drop farms go
/// underground like fights). Never behind a key or a boat.
pub fn drop_spots(item: &str) -> Vec<CombatSpot> {
    let Some(obj) = crate::objs_by_name(item).into_iter().next() else { return Vec::new() };
    let npcs: Vec<String> = crate::drops::drops_for_obj(obj.obj_id)
        .into_iter()
        .filter(|drop| drop.npc_id >= 0)
        .map(|drop| drop.npc)
        .collect();
    let mut out: Vec<CombatSpot> = SPOTS
        .iter()
        .filter(|spot| spot.access.is_none() || toll_gated(spot))
        .filter(|spot| npcs.iter().any(|npc| spot.npc_names.contains(&npc.as_str())))
        .filter(|spot| !positions(spot.key).is_empty())
        .copied()
        .collect();
    out.sort_by_key(|spot| (toll_gated(spot), spot.entrance.is_some()));
    out
}

/// Highest legacy spot whose minimum level is reached; the first otherwise.
pub fn best_spot(melee_level: i64) -> CombatSpot {
    let mut chosen = SPOTS[0];
    for spot in SPOTS {
        if spot.min_level <= melee_level {
            chosen = spot;
        }
    }
    chosen
}

/// Highest catalogue spot whose minimum level is reached.
pub fn best_catalogue_spot(melee_level: i64) -> CombatSpot {
    let mut chosen = CATALOGUE_SPOTS[0];
    for spot in CATALOGUE_SPOTS {
        if spot.min_level <= melee_level {
            chosen = spot;
        }
    }
    chosen
}

/// F2P spawn positions for one spot (all NPC names, zone-filtered),
/// never a spawn no stand reaches (`site_reach::unreachable_spawn`: the
/// caged lesser demon of the Wizards' Tower, live 2026-11-21), nor one
/// no plan from Lumbridge enters and leaves (`no_route`: Crandor, Elvarg's lair).
pub fn positions(key: &str) -> Vec<(i64, i64, i64)> {
    raw_positions(key).into_iter().filter(|(x, z, level)| crate::site_reach::unreachable_spawn(*x, *z, *level).is_none()).collect()
}

/// True when (x, z, level) is an underground position of a catalogue
/// camp (`positions`, z ≥ 6400): the navigation reaches every one from
/// the surface and back (`rs/crates/nav/tests/fight_sites.rs` writes
/// `no_route` for the others), so a fight there passes the generic
/// underground gate (`access::fight_gates_at`, user 2026-11-21). Any
/// other underground tile (a rock, a stray spawn) is false.
pub fn routed_underground(x: i64, z: i64, level: i64) -> bool {
    static ROUTED: std::sync::OnceLock<std::collections::HashSet<(i64, i64, i64)>> = std::sync::OnceLock::new();
    let routed = ROUTED.get_or_init(|| {
        CATALOGUE_SPOTS.iter().flat_map(|spot| positions(spot.key)).filter(|(_, spawn_z, _)| crate::underground::is_underground(*spawn_z)).collect()
    });
    crate::underground::is_underground(z) && routed.contains(&(x, z, level))
}

/// Every F2P spawn of a spot, reachable or not (the reach dump's input,
/// `rs/crates/nav/tests/fight_sites.rs`).
pub fn raw_positions(key: &str) -> Vec<(i64, i64, i64)> {
    let Some(spot) = by_key(key) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for name in spot.npc_names {
        let Some(npc) = crate::npcs_by_name(name).into_iter().next() else {
            continue;
        };
        for spawn in f2p_npc_spawns(Some(npc.npc_id)) {
            out.push((spawn.x, spawn.z, spawn.level));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{all_spots, best_catalogue_spot, best_spot, by_key, catalogue_spots, positions};
    use crate::free_zones::is_free;

    /// Live 2026-11-23, Zezima: 12 h of `farm:naturerune` at the goblins
    /// for 0 nature runes. `goblin_greenarmour` drops them, the spot
    /// hunts `goblin` only: a prefix match credited the spot with a table
    /// of an NPC it never kills.
    #[test]
    fn drop_spots_match_the_hunted_npc_exactly() {
        let nature: Vec<&str> = super::drop_spots("naturerune").iter().map(|spot| spot.key).collect();
        assert!(!nature.contains(&"goblins"), "goblins drop no nature rune: {nature:?}");
        let feathers: Vec<&str> = super::drop_spots("feather").iter().map(|spot| spot.key).collect();
        assert!(feathers.contains(&"chickens"), "chickens still drop feathers: {feathers:?}");
    }

    #[test]
    fn spot_progression() {
        // best_spot(1) == cows, best_spot(25) == dark_wizards.
        assert_eq!(best_spot(0).key, "chickens");
        assert_eq!(best_spot(1).key, "cows");
        // t02 live: melee 15 hunted men (vislevel 2) instead of the
        // level-9 al-kharid warriors. Weakest first, so men sorts
        // before al_kharid_warriors and never shadows it.
        assert_eq!(best_spot(7).key, "cows");
        assert_eq!(best_spot(8).key, "al_kharid_warriors");
        assert_eq!(best_spot(15).key, "al_kharid_warriors");
        assert_eq!(best_spot(19).key, "al_kharid_warriors");
        assert_eq!(best_spot(25).key, "dark_wizards");
        assert_eq!(by_key("cows").expect("cows").npc_names, &["cow"]);
        assert_eq!(by_key("goblins").expect("goblins").npc_names, &["goblin"]);
        let hill = by_key("hill_giants").expect("hill giants");
        assert_eq!(hill.entrance.expect("entrance").surface, (3097, 3468));
        assert!(by_key("dragons").is_none());
        // Legacy progression stays frozen for planner/scripts.
        assert_eq!(best_spot(40).key, "moss_giants");
        // Catalogue high end: moss at 40, ice at 50, lessers at 82.
        assert_eq!(best_catalogue_spot(40).key, "moss_giants");
        assert_eq!(best_catalogue_spot(82).key, "lesser_demons");
        assert_eq!(best_catalogue_spot(90).key, "greater_demons");
    }

    #[test]
    fn every_spot_has_a_free_position_access_and_danger() {
        // Catalogue §5.12: all targets placed in F2P zones with access
        // (key, fare, toll) and danger; Wilderness spots flagged.
        // Legacy `all_spots()` stays at nine for planner compatibility;
        // the catalogue holds all 28.
        assert_eq!(all_spots().len(), 9, "legacy spots frozen");
        let spots = catalogue_spots();
        assert!(spots.len() >= 20, "spots, got {}", spots.len());
        for spot in &spots {
            let found = positions(spot.key);
            assert!(!found.is_empty(), "spot {} has an F2P spawn", spot.key);
            for (x, z, _) in &found {
                assert!(is_free(*x, *z), "spot {} at ({x}, {z}) stands free", spot.key);
            }
            assert!(!spot.danger.is_empty(), "spot {} names danger", spot.key);
        }
        assert!(positions("no_such_spot").is_empty());
        // Access extras from the catalogue.
        // The Edgeville trapdoor and the main dungeon reach the giants'
        // hall: the brass key is a shortcut, never a requirement
        // (golden route `geants_sans_cle`).
        assert_eq!(by_key("hill_giants").expect("hill").access, None);
        assert!(by_key("lesser_demons").expect("lesser").access.unwrap_or("").contains("30gp"));
        assert!(by_key("al_kharid_warriors").expect("warrior").access.unwrap_or("").contains("10gp"));
        let wild: Vec<&str> = spots.iter().filter(|entry| entry.danger == "wild").map(|entry| entry.key).collect();
        for key in ["bears", "wild_skeletons", "ice_warriors", "wild_ice", "greater_demons"] {
            assert!(wild.contains(&key), "{key} flagged wild: {wild:?}");
        }
    }

    /// User 2026-11-20 (F2P world, `world.json` members false): members
    /// NPCs never spawn, so no green dragon spot. The lesser demons hunt
    /// down the Karamja volcano rope behind the boat; the greater demons
    /// stand in the deep Wilderness, on foot (no boat).
    #[test]
    fn high_level_camps_follow_the_server() {
        assert!(by_key("green_dragons").is_none(), "members NPCs never spawn here");
        let lesser = by_key("lesser_demons").expect("lesser");
        assert_eq!(lesser.entrance.expect("volcano rope").surface, (2856, 3168));
        assert!(super::boat_gated(&lesser));
        let greater = by_key("greater_demons").expect("greater");
        assert!(greater.access.is_none() && greater.entrance.is_none() && greater.danger == "wild", "{greater:?}");
        assert!(!super::boat_gated(&greater));
        let dungeon = by_key("ice_dungeon").expect("ice dungeon");
        assert_eq!(dungeon.entrance.expect("ladder").surface, (3008, 3150));
        assert!(dungeon.npc_names.contains(&"icewarrior") && dungeon.npc_names.contains(&"icegiant"));
    }

    /// Risk camps (user 2026-11-20): foes from level 45; lesser demons
    /// from combat 80, greater demons from 90; the giants of the
    /// Edgeville dungeon stay an ordinary camp.
    #[test]
    fn risk_camps_and_their_floor() {
        use super::{risk_floor, risk_spot};
        for key in ["lesser_demons", "greater_demons", "ice_dungeon", "wild_ice"] {
            assert!(risk_spot(key), "{key} is a risk camp");
        }
        for key in ["hill_giants", "moss_giants", "cows", "guards"] {
            assert!(!risk_spot(key), "{key} is not");
        }
        assert_eq!(risk_floor("lesser_demons"), Some(80));
        assert_eq!(risk_floor("greater_demons"), Some(90));
        assert_eq!(risk_floor("hill_giants"), None);
    }

    /// Live 2026-11-21: the caged lesser demon at the top of the Wizards'
    /// Tower (3110,3157,2) was the camp's site the brain priced, and
    /// every walk there ended `Disconnected` (25 failures in 6 h). A spawn
    /// no stand reaches is no position of its camp
    /// (`site_reach::unreachable_spawn`); the volcano's six stay.
    #[test]
    fn a_sealed_spawn_is_no_position() {
        let lesser = positions("lesser_demons");
        assert!(!lesser.contains(&(3110, 3157, 2)), "the caged demon is out: {lesser:?}");
        assert_eq!(crate::site_reach::unreachable_spawn(3110, 3157, 2), Some("sealed"));
        let volcano = lesser.iter().filter(|(x, z, level)| *level == 0 && (2820..=2850).contains(x) && (9548..=9570).contains(z)).count();
        assert_eq!(volcano, 6, "the volcano's six: {lesser:?}");
    }
}
