//! Typed requirements for the free-to-play quests. Item names resolve
//! against the object
//! table, completion varps against the varp table; unknown names fail
//! loudly, never silently.

use std::sync::OnceLock;

use crate::{objs_by_name, varps::id_by_name as varp_id_by_name};

/// One skill gate.
#[derive(Debug, Clone)]
pub struct SkillRequirement {
    pub skill: String,
    pub level: i64,
}

/// One item gate, resolved to its object id at build time.
#[derive(Debug, Clone)]
pub struct ItemRequirement {
    pub item: String,
    pub obj_id: usize,
    pub name: String,
    pub count: i64,
    pub worn_only: bool,
}

/// A cross-quest dependency (empty in revision 274; kept for members).
#[derive(Debug, Clone)]
pub struct QuestDependency {
    pub key: String,
    pub name: String,
    pub completion_varps: Vec<(i64, i64)>,
}

/// Full typed requirements for one F2P quest.
#[derive(Debug, Clone)]
pub struct QuestRequirements {
    pub key: String,
    pub name: String,
    pub members: bool,
    pub completion_varps: Vec<(i64, i64)>,
    pub skills: Vec<SkillRequirement>,
    pub quest_points: i64,
    pub items: Vec<ItemRequirement>,
    pub dependencies: Vec<QuestDependency>,
    pub combat_level: i64,
    pub recommended_combat: i64,
}

fn skill(skill: &str, level: i64) -> SkillRequirement {
    SkillRequirement { skill: skill.to_string(), level }
}

fn item(name: &str, count: i64, worn_only: bool) -> ItemRequirement {
    let found = objs_by_name(name);
    let entry = found.into_iter().next().unwrap_or_else(|| panic!("unknown quest item {name:?}"));
    ItemRequirement {
        item: name.to_string(),
        obj_id: entry.obj_id,
        name: entry.name.unwrap_or_else(|| name.to_string()),
        count,
        worn_only,
    }
}

fn completion(pairs: &[(&str, i64)]) -> Vec<(i64, i64)> {
    pairs
        .iter()
        .map(|(name, value)| {
            let varp = varp_id_by_name(name).unwrap_or_else(|| panic!("unknown quest varp {name:?}"));
            (varp, *value)
        })
        .collect()
}

fn build_f2p_quests() -> Vec<QuestRequirements> {
    vec![
        QuestRequirements {
            key: "blackknight".into(),
            name: "Black Knight's Fortress".into(),
            members: false,
            completion_varps: completion(&[("spy", 4)]),
            skills: vec![],
            quest_points: 12,
            items: vec![item("iron_chainbody", 1, true), item("bronze_med_helm", 1, true)],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
        },
        QuestRequirements {
            key: "cook".into(),
            name: "Cook's Assistant".into(),
            members: false,
            completion_varps: completion(&[("cookquest", 2)]),
            skills: vec![],
            quest_points: 0,
            items: vec![item("bucket_milk", 1, false), item("egg", 1, false), item("pot_flour", 1, false)],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
        },
        QuestRequirements {
            key: "demon".into(),
            name: "Demon Slayer".into(),
            members: false,
            completion_varps: completion(&[("demonstart", 30)]),
            skills: vec![],
            quest_points: 0,
            items: vec![item("coins", 1, false)],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
        },
        QuestRequirements {
            key: "doric".into(),
            name: "Doric's Quest".into(),
            members: false,
            completion_varps: completion(&[("doricquest", 100)]),
            skills: vec![],
            quest_points: 0,
            items: vec![item("clay", 6, false), item("copper_ore", 4, false), item("iron_ore", 2, false)],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
        },
        QuestRequirements {
            key: "dragon".into(),
            name: "Dragon Slayer".into(),
            members: false,
            completion_varps: completion(&[("dragonquest", 10)]),
            skills: vec![],
            quest_points: 32,
            items: vec![
                item("coins", 12000, false),
                item("woodplank", 3, false),
                item("nails", 12, false),
                item("hammer", 1, false),
                item("wizards_mind_bomb", 1, false),
                item("bowl_unfired", 1, false),
                item("lobster_pot", 1, false),
                item("silk", 1, false),
            ],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
        },
        QuestRequirements {
            key: "haunted".into(),
            name: "Ernest the Chicken".into(),
            members: false,
            completion_varps: completion(&[("haunted", 3)]),
            skills: vec![],
            quest_points: 0,
            items: vec![
                item("spade", 1, false),
                item("fish_food", 1, false),
                item("poison", 1, false),
                item("oil_can", 1, false),
                item("pressure_gauge", 1, false),
                item("rubber_tube", 1, false),
            ],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
        },
        QuestRequirements {
            key: "gobdip".into(),
            name: "Goblin Diplomacy".into(),
            members: false,
            completion_varps: completion(&[("goblinquest", 6)]),
            skills: vec![],
            quest_points: 0,
            items: vec![
                item("goblin_armour_orange", 1, false),
                item("goblin_armour_darkblue", 1, false),
                item("goblin_armour", 1, false),
            ],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
        },
        QuestRequirements {
            key: "squire".into(),
            name: "The Knight's Sword".into(),
            members: false,
            completion_varps: completion(&[("squire", 7)]),
            skills: vec![skill("mining", 10)],
            quest_points: 0,
            items: vec![],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 20,
        },
        QuestRequirements {
            key: "imp".into(),
            name: "Imp Catcher".into(),
            members: false,
            completion_varps: completion(&[("imp", 2)]),
            skills: vec![],
            quest_points: 0,
            items: vec![
                item("black_bead", 1, false),
                item("red_bead", 1, false),
                item("white_bead", 1, false),
                item("yellow_bead", 1, false),
            ],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
        },
        QuestRequirements {
            key: "hunt".into(),
            name: "Pirate's Treasure".into(),
            members: false,
            completion_varps: completion(&[("hunt", 4)]),
            skills: vec![],
            quest_points: 0,
            items: vec![item("banana", 10, false)],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
        },
        QuestRequirements {
            key: "prince".into(),
            name: "Prince Ali Rescue".into(),
            members: false,
            completion_varps: completion(&[("princequest", 110)]),
            skills: vec![],
            quest_points: 0,
            items: vec![],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
        },
        QuestRequirements {
            key: "priest".into(),
            name: "The Restless Ghost".into(),
            members: false,
            completion_varps: completion(&[("prieststart", 5)]),
            skills: vec![],
            quest_points: 0,
            items: vec![],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
        },
        QuestRequirements {
            key: "romeojuliet".into(),
            name: "Romeo & Juliet".into(),
            members: false,
            completion_varps: completion(&[("rjquest", 100)]),
            skills: vec![],
            quest_points: 0,
            items: vec![item("cadavaberries", 1, false)],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
        },
        QuestRequirements {
            key: "runemysteries".into(),
            name: "Rune Mysteries".into(),
            members: false,
            completion_varps: completion(&[("runemysteries", 6)]),
            skills: vec![],
            quest_points: 0,
            items: vec![],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
        },
        QuestRequirements {
            key: "sheep".into(),
            name: "Sheep Shearer".into(),
            members: false,
            completion_varps: completion(&[("sheep", 22)]),
            skills: vec![],
            quest_points: 0,
            items: vec![item("ball_of_wool", 20, false)],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
        },
        QuestRequirements {
            key: "blackarmgang".into(),
            name: "Shield of Arrav".into(),
            members: false,
            completion_varps: completion(&[("blackarmgang", 4), ("phoenixgang", 10)]),
            skills: vec![],
            quest_points: 0,
            items: vec![],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
        },
        QuestRequirements {
            key: "vampire".into(),
            name: "Vampire Slayer".into(),
            members: false,
            completion_varps: completion(&[("vampire", 3)]),
            skills: vec![],
            quest_points: 0,
            items: vec![item("stake", 1, false), item("garlic", 1, false), item("hammer", 1, false)],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
        },
        QuestRequirements {
            key: "hetty".into(),
            name: "Witch's Potion".into(),
            members: false,
            completion_varps: completion(&[("hetty", 3)]),
            skills: vec![],
            quest_points: 0,
            items: vec![
                item("onion", 1, false),
                item("rats_tail", 1, false),
                item("burnt_meat", 1, false),
                item("eye_of_newt", 1, false),
            ],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
        },
    ]
}

fn f2p_quests() -> &'static Vec<QuestRequirements> {
    static CACHE: OnceLock<Vec<QuestRequirements>> = OnceLock::new();
    CACHE.get_or_init(build_f2p_quests)
}

/// Every F2P quest with typed requirements.
pub fn all_f2p_quests() -> Vec<QuestRequirements> {
    f2p_quests().clone()
}

/// F2P quest by key, or None when the key is unknown.
pub fn by_key(key: &str) -> Option<QuestRequirements> {
    f2p_quests().iter().find(|quest| quest.key == key).cloned()
}

#[cfg(test)]
mod tests {
    use super::{all_f2p_quests, by_key};

    #[test]
    fn f2p_vectors() {
        // 18 quests; cook needs bucket_milk (obj 1927);
        // squire gates mining 10 with recommended combat 20.
        assert_eq!(all_f2p_quests().len(), 18);
        let cook = by_key("cook").expect("cook");
        assert_eq!(cook.items[0].item, "bucket_milk");
        assert_eq!(cook.items[0].obj_id, 1927);
        assert_eq!(cook.completion_varps, vec![(29, 2)]);
        let squire = by_key("squire").expect("squire");
        assert_eq!((squire.skills[0].skill.as_str(), squire.skills[0].level), ("mining", 10));
        assert_eq!(squire.recommended_combat, 20);
    }

    #[test]
    fn worn_and_heavy_gates() {
        // blackknight armour is worn-only; dragon needs 12000
        // coins (obj 995); blackarmgang spans two completion varps.
        let knight = by_key("blackknight").expect("blackknight");
        assert_eq!(knight.quest_points, 12);
        assert_eq!(knight.items[0].obj_id, 1101);
        assert!(knight.items.iter().all(|item| item.worn_only));
        let dragon = by_key("dragon").expect("dragon");
        assert_eq!((dragon.items[0].count, dragon.items[0].obj_id), (12000, 995));
        assert_eq!(by_key("blackarmgang").expect("arrav").completion_varps, vec![(145, 4), (146, 10)]);
        assert!(by_key("nope").is_none());
    }
}
