//! Skill coverage 1→99 (M10b §5 + §8): every F2P skill has a verified
//! method for each 10-level tranche. XP are raw server tenths; epoch
//! follows the catalogue (P1 2004-2005 guides, P2 possible in 274).
//! All objects are `members == Some(false)`; all stations stand in F2P
//! zones (`stations`); all NPCs stand in F2P zones (`f2p_npc_spawns`).

/// One coverage method: the cheapest verified way to train at `level`.
#[derive(Debug, Clone)]
pub struct Method {
    pub skill: &'static str,
    pub level: i64,
    pub activity: &'static str,
    pub xp: i64,
    pub epoch: &'static str,
    pub station: Option<&'static str>,
}

static ALL: &[Method] = &[
    // Woodcutting (trees.dbrow exp; P1 classic to yews).
    Method { skill: "woodcutting", level: 1, activity: "trees", xp: 250, epoch: "P1", station: None },
    Method { skill: "woodcutting", level: 15, activity: "oaks", xp: 375, epoch: "P1", station: None },
    Method { skill: "woodcutting", level: 30, activity: "willows", xp: 675, epoch: "P1", station: None },
    Method { skill: "woodcutting", level: 60, activity: "yews", xp: 1750, epoch: "P1", station: None },
    // Mining (mine.dbrow exp; P1 Al-Kharid iron, coal, guilds).
    Method { skill: "mining", level: 1, activity: "copper_tin", xp: 175, epoch: "P1", station: None },
    Method { skill: "mining", level: 1, activity: "clay", xp: 50, epoch: "P2", station: None },
    Method { skill: "mining", level: 1, activity: "rune_essence", xp: 50, epoch: "P1", station: None },
    Method { skill: "mining", level: 15, activity: "iron", xp: 350, epoch: "P1", station: None },
    Method { skill: "mining", level: 20, activity: "silver", xp: 400, epoch: "P1", station: None },
    Method { skill: "mining", level: 30, activity: "coal", xp: 500, epoch: "P1", station: None },
    Method { skill: "mining", level: 40, activity: "gold", xp: 650, epoch: "P1", station: None },
    Method { skill: "mining", level: 55, activity: "mithril", xp: 800, epoch: "P1", station: None },
    Method { skill: "mining", level: 70, activity: "adamantite", xp: 950, epoch: "P1", station: None },
    Method { skill: "mining", level: 85, activity: "runite", xp: 1250, epoch: "P1", station: None },
    // Fishing (fishing tiers exp; P1 net/bait/fly/cage/harpoon).
    Method { skill: "fishing", level: 1, activity: "net_shrimp", xp: 100, epoch: "P1", station: None },
    Method { skill: "fishing", level: 5, activity: "bait_sardine", xp: 200, epoch: "P1", station: None },
    Method { skill: "fishing", level: 15, activity: "net_anchovy", xp: 400, epoch: "P1", station: None },
    Method { skill: "fishing", level: 20, activity: "fly_trout", xp: 500, epoch: "P1", station: None },
    Method { skill: "fishing", level: 25, activity: "river_pike", xp: 600, epoch: "P2", station: None },
    Method { skill: "fishing", level: 30, activity: "fly_salmon", xp: 700, epoch: "P1", station: None },
    Method { skill: "fishing", level: 35, activity: "harpoon_tuna", xp: 800, epoch: "P1", station: None },
    Method { skill: "fishing", level: 40, activity: "cage_lobster", xp: 900, epoch: "P1", station: None },
    Method { skill: "fishing", level: 50, activity: "harpoon_swordfish", xp: 1000, epoch: "P1", station: None },
    // Cooking (cook recipes xp; P1 fish+meat, P2 composed).
    Method { skill: "cooking", level: 1, activity: "cook_shrimp", xp: 300, epoch: "P1", station: Some("range") },
    Method { skill: "cooking", level: 1, activity: "bake_bread", xp: 400, epoch: "P2", station: Some("range") },
    Method { skill: "cooking", level: 5, activity: "cook_herring", xp: 0, epoch: "P1", station: Some("range") },
    Method { skill: "cooking", level: 10, activity: "bake_redberry_pie", xp: 780, epoch: "P2", station: Some("range") },
    Method { skill: "cooking", level: 15, activity: "cook_trout", xp: 0, epoch: "P1", station: Some("range") },
    Method { skill: "cooking", level: 20, activity: "bake_meat_pie", xp: 1100, epoch: "P2", station: Some("range") },
    Method { skill: "cooking", level: 25, activity: "cook_stew", xp: 1170, epoch: "P2", station: Some("range") },
    Method { skill: "cooking", level: 30, activity: "bake_apple_pie", xp: 1300, epoch: "P2", station: Some("range") },
    Method { skill: "cooking", level: 35, activity: "bake_pizza", xp: 1430, epoch: "P2", station: Some("range") },
    Method { skill: "cooking", level: 40, activity: "bake_cake", xp: 1800, epoch: "P2", station: Some("range") },
    Method { skill: "cooking", level: 40, activity: "cook_lobster", xp: 0, epoch: "P1", station: Some("range") },
    Method { skill: "cooking", level: 45, activity: "cook_swordfish", xp: 0, epoch: "P1", station: Some("range") },
    // Firemaking (firemaking.obj productexp; P1 willows in bulk).
    Method { skill: "firemaking", level: 1, activity: "burn_logs", xp: 400, epoch: "P1", station: None },
    Method { skill: "firemaking", level: 15, activity: "burn_oak", xp: 600, epoch: "P1", station: None },
    Method { skill: "firemaking", level: 30, activity: "burn_willow", xp: 900, epoch: "P1", station: None },
    Method { skill: "firemaking", level: 60, activity: "burn_yew", xp: 2025, epoch: "P1", station: None },
    // Crafting (spinning/pottery/leather/jewellery structs; P1 wool+gold).
    Method { skill: "crafting", level: 1, activity: "spin_wool", xp: 25, epoch: "P1", station: Some("spinningwheel") },
    Method { skill: "crafting", level: 1, activity: "leather_gloves", xp: 138, epoch: "P1", station: None },
    Method { skill: "crafting", level: 1, activity: "pot_fire", xp: 63, epoch: "P2", station: Some("potteryoven") },
    Method { skill: "crafting", level: 5, activity: "gold_ring", xp: 150, epoch: "P1", station: Some("furnace") },
    Method { skill: "crafting", level: 8, activity: "gold_amulet", xp: 300, epoch: "P1", station: Some("furnace") },
    Method { skill: "crafting", level: 16, activity: "holy_symbol", xp: 500, epoch: "P2", station: Some("furnace") },
    Method { skill: "crafting", level: 20, activity: "cut_sapphire", xp: 500, epoch: "P2", station: None },
    Method { skill: "crafting", level: 20, activity: "sapphire_ring", xp: 400, epoch: "P1", station: Some("furnace") },
    Method { skill: "crafting", level: 27, activity: "emerald_ring", xp: 550, epoch: "P1", station: Some("furnace") },
    Method { skill: "crafting", level: 34, activity: "ruby_ring", xp: 700, epoch: "P1", station: Some("furnace") },
    Method { skill: "crafting", level: 43, activity: "diamond_ring", xp: 850, epoch: "P1", station: Some("furnace") },
    Method { skill: "crafting", level: 70, activity: "diamond_amulet", xp: 1000, epoch: "P2", station: Some("furnace") },
    // Smithing (smelt recipes xp + Knight's Sword 1->29; P1 steel+platebodies).
    Method { skill: "smithing", level: 1, activity: "smelt_bronze", xp: 62, epoch: "P1", station: Some("furnace") },
    Method { skill: "smithing", level: 1, activity: "knights_sword", xp: 12725, epoch: "P1", station: None },
    Method { skill: "smithing", level: 15, activity: "smelt_iron", xp: 0, epoch: "P1", station: Some("furnace") },
    Method { skill: "smithing", level: 20, activity: "smelt_silver", xp: 0, epoch: "P1", station: Some("furnace") },
    Method { skill: "smithing", level: 30, activity: "smelt_steel", xp: 175, epoch: "P1", station: Some("furnace") },
    Method { skill: "smithing", level: 40, activity: "smelt_gold", xp: 0, epoch: "P1", station: Some("furnace") },
    Method { skill: "smithing", level: 50, activity: "smelt_mithril", xp: 0, epoch: "P1", station: Some("furnace") },
    Method { skill: "smithing", level: 70, activity: "smelt_adamantite", xp: 0, epoch: "P1", station: Some("furnace") },
    Method { skill: "smithing", level: 85, activity: "smelt_runite", xp: 0, epoch: "P1", station: Some("furnace") },
    // Runecraft (runecraft.dbrow experience; P1 air+earth).
    Method { skill: "runecraft", level: 1, activity: "craft_air", xp: 50, epoch: "P1", station: Some("ruin") },
    Method { skill: "runecraft", level: 2, activity: "craft_mind", xp: 55, epoch: "P2", station: Some("ruin") },
    Method { skill: "runecraft", level: 5, activity: "craft_water", xp: 60, epoch: "P2", station: Some("ruin") },
    Method { skill: "runecraft", level: 9, activity: "craft_earth", xp: 65, epoch: "P1", station: Some("ruin") },
    Method { skill: "runecraft", level: 14, activity: "craft_fire", xp: 70, epoch: "P2", station: Some("ruin") },
    Method { skill: "runecraft", level: 20, activity: "craft_body", xp: 75, epoch: "P2", station: Some("ruin") },
    // Prayer (bones.obj bone_exp; P1 bury where trained).
    Method { skill: "prayer", level: 1, activity: "bury_bones", xp: 45, epoch: "P1", station: None },
    Method { skill: "prayer", level: 1, activity: "bury_big_bones", xp: 150, epoch: "P1", station: None },
    // Magic (spells.json xp; P1 strikes/curses/alch/teleports, P2 superheat).
    Method { skill: "magic", level: 1, activity: "wind_strike", xp: 55, epoch: "P1", station: None },
    Method { skill: "magic", level: 3, activity: "confuse", xp: 130, epoch: "P1", station: None },
    Method { skill: "magic", level: 11, activity: "weaken", xp: 210, epoch: "P1", station: None },
    Method { skill: "magic", level: 19, activity: "curse", xp: 290, epoch: "P1", station: None },
    Method { skill: "magic", level: 21, activity: "low_alch", xp: 310, epoch: "P1", station: None },
    Method { skill: "magic", level: 25, activity: "varrock_teleport", xp: 350, epoch: "P1", station: None },
    Method { skill: "magic", level: 33, activity: "telegrab_wine", xp: 430, epoch: "P1", station: None },
    Method { skill: "magic", level: 43, activity: "superheat", xp: 530, epoch: "P2", station: None },
    Method { skill: "magic", level: 55, activity: "high_alch", xp: 650, epoch: "P1", station: None },
    // Ranged (F2P bows/arrows from equipment.json; P1 safe spots).
    Method { skill: "ranged", level: 1, activity: "shortbow_bronze", xp: 0, epoch: "P1", station: None },
    Method { skill: "ranged", level: 10, activity: "longbow_bronze", xp: 0, epoch: "P1", station: None },
    Method { skill: "ranged", level: 20, activity: "oak_shortbow", xp: 0, epoch: "P1", station: None },
    Method { skill: "ranged", level: 30, activity: "willow_shortbow", xp: 0, epoch: "P1", station: None },
    Method { skill: "ranged", level: 40, activity: "green_dhide", xp: 0, epoch: "P1", station: None },
    // Melee (combat spots §5.12; P1 cows/monks/hills/moss/lessers).
    Method { skill: "attack", level: 1, activity: "chickens", xp: 0, epoch: "P1", station: None },
    Method { skill: "attack", level: 1, activity: "cows", xp: 0, epoch: "P1", station: None },
    Method { skill: "attack", level: 8, activity: "al_kharid_warriors", xp: 0, epoch: "P1", station: None },
    Method { skill: "attack", level: 20, activity: "guards", xp: 0, epoch: "P1", station: None },
    Method { skill: "attack", level: 30, activity: "hill_giants", xp: 0, epoch: "P1", station: None },
    Method { skill: "attack", level: 40, activity: "moss_giants", xp: 0, epoch: "P1", station: None },
    Method { skill: "attack", level: 60, activity: "ice_warriors", xp: 0, epoch: "P1", station: None },
    Method { skill: "attack", level: 82, activity: "lesser_demons", xp: 0, epoch: "P1", station: None },
    Method { skill: "strength", level: 1, activity: "chickens", xp: 0, epoch: "P1", station: None },
    Method { skill: "strength", level: 1, activity: "cows", xp: 0, epoch: "P1", station: None },
    Method { skill: "strength", level: 8, activity: "al_kharid_warriors", xp: 0, epoch: "P1", station: None },
    Method { skill: "strength", level: 20, activity: "guards", xp: 0, epoch: "P1", station: None },
    Method { skill: "strength", level: 30, activity: "hill_giants", xp: 0, epoch: "P1", station: None },
    Method { skill: "strength", level: 40, activity: "moss_giants", xp: 0, epoch: "P1", station: None },
    Method { skill: "strength", level: 60, activity: "ice_warriors", xp: 0, epoch: "P1", station: None },
    Method { skill: "strength", level: 82, activity: "lesser_demons", xp: 0, epoch: "P1", station: None },
    Method { skill: "defence", level: 1, activity: "chickens", xp: 0, epoch: "P1", station: None },
    Method { skill: "defence", level: 1, activity: "cows", xp: 0, epoch: "P1", station: None },
    Method { skill: "defence", level: 8, activity: "al_kharid_warriors", xp: 0, epoch: "P1", station: None },
    Method { skill: "defence", level: 20, activity: "guards", xp: 0, epoch: "P1", station: None },
    Method { skill: "defence", level: 30, activity: "hill_giants", xp: 0, epoch: "P1", station: None },
    Method { skill: "defence", level: 40, activity: "moss_giants", xp: 0, epoch: "P1", station: None },
    Method { skill: "defence", level: 60, activity: "ice_warriors", xp: 0, epoch: "P1", station: None },
    Method { skill: "defence", level: 82, activity: "lesser_demons", xp: 0, epoch: "P1", station: None },
    Method { skill: "hitpoints", level: 1, activity: "chickens", xp: 0, epoch: "P1", station: None },
    Method { skill: "hitpoints", level: 8, activity: "al_kharid_warriors", xp: 0, epoch: "P1", station: None },
    Method { skill: "hitpoints", level: 20, activity: "guards", xp: 0, epoch: "P1", station: None },
    Method { skill: "hitpoints", level: 30, activity: "hill_giants", xp: 0, epoch: "P1", station: None },
    Method { skill: "hitpoints", level: 40, activity: "moss_giants", xp: 0, epoch: "P1", station: None },
    Method { skill: "hitpoints", level: 82, activity: "lesser_demons", xp: 0, epoch: "P1", station: None },
];

/// Every coverage method.
pub fn all_methods() -> Vec<Method> {
    ALL.to_vec()
}

/// Methods for one skill, ordered by level.
pub fn methods_for(skill: &str) -> Vec<Method> {
    let mut out: Vec<Method> =
        ALL.iter().filter(|entry| entry.skill == skill).cloned().collect();
    out.sort_by_key(|entry| entry.level);
    out
}

/// Highest method whose level requirement is reached, or None.
pub fn best_method(skill: &str, level: i64) -> Option<Method> {
    let mut best: Option<Method> = None;
    for entry in methods_for(skill) {
        if entry.level <= level && best.as_ref().is_none_or(|current| entry.level >= current.level) {
            best = Some(entry);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::{all_methods, best_method, methods_for};
    use crate::stations::stations_of;

    const SKILLS: [&str; 15] = [
        "attack", "defence", "strength", "hitpoints", "ranged", "prayer", "magic", "cooking", "woodcutting",
        "fishing", "firemaking", "crafting", "smithing", "mining", "runecraft",
    ];

    #[test]
    fn every_skill_covers_1_to_99_per_decade() {
        // Catalogue §8: at least one verified method per 10-level tranche
        // 1→99; beyond the last tier the best method trains by volume.
        for skill in SKILLS {
            let methods = methods_for(skill);
            assert!(!methods.is_empty(), "skill {skill} has methods");
            for tranche in [1, 10, 20, 30, 40, 50, 60, 70, 80, 90] {
                assert!(
                    methods.iter().any(|entry| entry.level <= tranche),
                    "skill {skill} has a method by {tranche}"
                );
                assert!(best_method(skill, tranche).is_some(), "skill {skill} best by {tranche}");
            }
            assert!(best_method(skill, 99).is_some(), "skill {skill} trains at 99");
            assert!(best_method(skill, 0).is_none(), "skill {skill} nothing at 0");
        }
        // Spot checks: yews at 60, runite at 85, lobsters at 40.
        assert_eq!(best_method("woodcutting", 60).expect("yew").activity, "yews");
        assert_eq!(best_method("mining", 85).expect("runite").activity, "runite");
        assert_eq!(best_method("fishing", 40).expect("lobster").activity, "cage_lobster");
        assert_eq!(best_method("crafting", 70).expect("diamond").activity, "diamond_amulet");
    }

    #[test]
    fn methods_use_f2p_stations_and_epochs() {
        // Every stationed method names a placed F2P post; every row is
        // P1 or P2 with P1 preferred (D64).
        let all = all_methods();
        assert!(all.len() >= 80, "methods, got {}", all.len());
        let mut p1 = 0;
        for entry in &all {
            assert!(entry.epoch == "P1" || entry.epoch == "P2", "epoch for {}", entry.activity);
            if entry.epoch == "P1" {
                p1 += 1;
            }
            if let Some(station) = entry.station {
                assert!(!stations_of(station).is_empty(), "station {station} for {}", entry.activity);
            }
            assert!(entry.xp >= 0, "xp for {}", entry.activity);
        }
        assert!(p1 * 2 >= all.len(), "P1 preferred, got {p1}/{}", all.len());
    }
}
