//! Classic F2P melee-and-loot spots.

/// Surface tile used to reach an underground spot.
#[derive(Debug, Clone, Copy)]
pub struct Entrance {
    pub surface: (i64, i64),
}

/// One training spot: NPC debugnames plus the melee level range.
#[derive(Debug, Clone, Copy)]
pub struct CombatSpot {
    pub key: &'static str,
    pub npc_names: &'static [&'static str],
    pub min_level: i64,
    pub max_level: i64,
    pub entrance: Option<Entrance>,
}

const EDGEVILLE_DUNGEON: Entrance = Entrance { surface: (3097, 3468) };
const VARROCK_SEWERS: Entrance = Entrance { surface: (3237, 3458) };

const SPOTS: [CombatSpot; 9] = [
    CombatSpot { key: "chickens", npc_names: &["chicken"], min_level: 1, max_level: 9, entrance: None },
    // Right after chickens so best_spot() progression never changes
    // (chickens keeps level 0, cows keeps level 1): explicit keys bypass
    // it anyway.
    CombatSpot { key: "goblins", npc_names: &["goblin"], min_level: 1, max_level: 12, entrance: None },
    CombatSpot { key: "cows", npc_names: &["cow"], min_level: 1, max_level: 12, entrance: None },
    // Weakest first: men (vislevel 2) sort before al_kharid_warriors
    // (vislevel 9) so best_spot() progression never shadows the
    // stronger camp (t02 live: melee 15 stuck on men).
    CombatSpot {
        key: "men",
        npc_names: &["man", "man2", "man3", "woman", "woman2", "woman3"],
        min_level: 8,
        max_level: 25,
        entrance: None,
    },
    CombatSpot {
        key: "al_kharid_warriors",
        npc_names: &["al_kharid_warrior"],
        min_level: 8,
        max_level: 20,
        entrance: None,
    },
    CombatSpot { key: "guards", npc_names: &["guard1", "guard2"], min_level: 20, max_level: 40, entrance: None },
    CombatSpot {
        key: "dark_wizards",
        npc_names: &["young_dark_wizard", "bearded_dark_wizard"],
        min_level: 20,
        max_level: 45,
        entrance: None,
    },
    CombatSpot {
        key: "hill_giants",
        npc_names: &["giant"],
        min_level: 30,
        max_level: 60,
        entrance: Some(EDGEVILLE_DUNGEON),
    },
    CombatSpot {
        key: "moss_giants",
        npc_names: &["mossgiant"],
        min_level: 40,
        max_level: 80,
        entrance: Some(VARROCK_SEWERS),
    },
];

/// Every spot, weakest first.
pub fn all_spots() -> Vec<CombatSpot> {
    SPOTS.to_vec()
}

/// Spot by key, or None.
pub fn by_key(key: &str) -> Option<CombatSpot> {
    SPOTS.iter().find(|spot| spot.key == key).copied()
}

/// Highest spot whose minimum level is reached; the first spot otherwise.
pub fn best_spot(melee_level: i64) -> CombatSpot {
    let mut chosen = SPOTS[0];
    for spot in SPOTS {
        if spot.min_level <= melee_level {
            chosen = spot;
        }
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::{best_spot, by_key};

    #[test]
    fn spot_progression() {
        // best_spot(1) == chickens, best_spot(25) == dark_wizards.
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
    }
}
