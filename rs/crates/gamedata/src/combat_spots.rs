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
/// `access` names what the bot needs (key, fare, toll, quest) or `None`;
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
        access: Some("brass key"),
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

/// Full catalogue §5.12 (26 spots): legacy nine plus rats, imps, monks,
/// barbarians, crypt crawlers, bears, hobgoblins, red spiders, ice and
/// wilderness targets. Ordered weakest first; new low spots sort where
/// they never shadow the legacy `best_spot()` progression.
const CATALOGUE_SPOTS: [CombatSpot; 26] = [
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
        access: Some("brass key"),
        danger: "underground",
    },
    CombatSpot { key: "bears", npc_names: &["brownbear", "darkbear"], min_level: 30, max_level: 45, entrance: None, access: None, danger: WILD_DANGER },
    CombatSpot { key: "hobgoblins", npc_names: &["hobgoblin_unarmed", "hobgoblin_armed"], min_level: 30, max_level: 50, entrance: Some(EDGEVILLE_DUNGEON), access: Some("brass key"), danger: "underground" },
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
    CombatSpot { key: "green_dragons", npc_names: &["green_dragon"], min_level: 75, max_level: 99, entrance: None, access: None, danger: WILD_DANGER },
    CombatSpot { key: "lesser_demons", npc_names: &["lesser_demon"], min_level: 82, max_level: 99, entrance: None, access: Some(KARAMJA_BOAT), danger: "aggressive" },
    CombatSpot { key: "greater_demons", npc_names: &["greater_demon"], min_level: 90, max_level: 99, entrance: None, access: Some(KARAMJA_BOAT), danger: WILD_DANGER },
];

/// Every legacy spot, weakest first (planner/scripts compatible).
pub fn all_spots() -> Vec<CombatSpot> {
    SPOTS.to_vec()
}

/// Full catalogue §5.12, weakest first (26 spots for P2.3/P2.5).
pub fn catalogue_spots() -> Vec<CombatSpot> {
    CATALOGUE_SPOTS.to_vec()
}

/// Spot by key, or None (searches the full catalogue).
pub fn by_key(key: &str) -> Option<CombatSpot> {
    CATALOGUE_SPOTS.iter().find(|spot| spot.key == key).copied()
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

/// F2P spawn positions for one spot (all NPC names, zone-filtered).
pub fn positions(key: &str) -> Vec<(i64, i64, i64)> {
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
        // the catalogue holds all 26.
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
        assert_eq!(by_key("hill_giants").expect("hill").access, Some("brass key"));
        assert!(by_key("lesser_demons").expect("lesser").access.unwrap_or("").contains("30gp"));
        assert!(by_key("al_kharid_warriors").expect("warrior").access.unwrap_or("").contains("10gp"));
        let wild: Vec<&str> = spots.iter().filter(|entry| entry.danger == "wild").map(|entry| entry.key).collect();
        for key in ["bears", "ice_warriors", "green_dragons", "greater_demons"] {
            assert!(wild.contains(&key), "{key} flagged wild: {wild:?}");
        }
    }
}
