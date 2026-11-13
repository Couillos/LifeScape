//! Typed requirements for the free-to-play quests (items read from the
//! rev 274 content scripts, quest points from `^<key>_questpoints`). Item names resolve
//! against the object
//! table, completion varps against the varp table; unknown names fail
//! loudly, never silently.

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
    /// Quest points the completion grants (`^<key>_questpoints`).
    pub reward_points: i64,
    /// Why no bot can run it solo today (`None`: plannable).
    pub blocker: Option<&'static str>,
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
        name: entry.name.map(str::to_string).unwrap_or_else(|| name.to_string()),
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
            // `sir_amik`/fortress scripts: the guard uniform worn, the
            // cabbage dropped down the chimney hole.
            items: vec![item("iron_chainbody", 1, true), item("bronze_med_helm", 1, true), item("cabbage", 1, false)],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
            reward_points: 3,
            blocker: None,
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
            reward_points: 1,
            blocker: None,
        },
        QuestRequirements {
            key: "demon".into(),
            name: "Demon Slayer".into(),
            members: false,
            completion_varps: completion(&[("demonstart", 30)]),
            skills: vec![],
            quest_points: 0,
            // Gypsy Aris takes a coin; Traiborn asks 25 bones
            // (`traiborn.rs2`); a bucket of water clears the drain key.
            items: vec![item("coins", 1, false), item("bones", 25, false), item("bucket_water", 1, false)],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
            reward_points: 3,
            blocker: None,
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
            reward_points: 1,
            blocker: None,
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
            reward_points: 2,
            blocker: None,
        },
        QuestRequirements {
            key: "haunted".into(),
            name: "Ernest the Chicken".into(),
            members: false,
            completion_varps: completion(&[("haunted", 3)]),
            skills: vec![],
            quest_points: 0,
            // Every part (fish food, poison, gauge, tube, oil can) is
            // found inside the manor during the quest: nothing to bring.
            items: vec![],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
            reward_points: 4,
            blocker: None,
        },
        QuestRequirements {
            key: "gobdip".into(),
            name: "Goblin Diplomacy".into(),
            members: false,
            completion_varps: completion(&[("goblinquest", 6)]),
            skills: vec![],
            quest_points: 0,
            // Three goblin mails (goblin drops) and Aggie's dyes: red +
            // yellow mix orange, blue dyes the dark blue mail (the quest
            // script mixes and dyes).
            items: vec![item("goblin_armour", 3, false), item("reddye", 1, false), item("yellowdye", 1, false), item("bluedye", 1, false)],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
            reward_points: 5,
            blocker: None,
        },
        QuestRequirements {
            key: "squire".into(),
            name: "The Knight's Sword".into(),
            members: false,
            completion_varps: completion(&[("squire", 7)]),
            skills: vec![skill("mining", 10)],
            quest_points: 0,
            // The Squire's cousin wants a redberry pie, Thurgo two iron bars
            // (`thurgo.rs2`: `iron_bar >= 2`); the blurite is mined in-quest.
            items: vec![item("redberry_pie", 1, false), item("iron_bar", 2, false)],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 20,
            reward_points: 1,
            blocker: None,
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
            reward_points: 1,
            blocker: None,
        },
        QuestRequirements {
            key: "hunt".into(),
            name: "Pirate's Treasure".into(),
            members: false,
            completion_varps: completion(&[("hunt", 4)]),
            skills: vec![],
            quest_points: 0,
            // The 10 bananas are picked on Karamja by the solver itself
            // (`quests::b::solve_hunt`, banana trees beside Luthas); the
            // coins pay the boat both ways (30 + 30) and Zambo's rum
            // (27): the solver refuses under 90 (`sailors.rs2`,
            // `customs_officer.rs2`, `zambo.rs2`).
            items: vec![item("coins", 90, false), item("spade", 1, false)],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
            reward_points: 2,
            blocker: None,
        },
        QuestRequirements {
            key: "prince".into(),
            name: "Prince Ali Rescue".into(),
            members: false,
            completion_varps: completion(&[("princequest", 110)]),
            skills: vec![],
            quest_points: 0,
            // Ned: wig from 3 balls of wool, rope for 4 more (`ned.rs2`),
            // Aggie dyes it yellow and mixes the paste (ashes, flour,
            // water, redberries, `aggie.rs2`), soft clay for the key print
            // and a bronze bar for Osman's key (`osman.rs2`), the pink
            // skirt, three beers for Joe (`joe_guard.rs2`).
            items: vec![
                item("ball_of_wool", 7, false),
                item("yellowdye", 1, false),
                item("ashes", 1, false),
                item("pot_flour", 1, false),
                item("bucket_water", 1, false),
                item("redberries", 1, false),
                item("softclay", 1, false),
                item("bronze_bar", 1, false),
                item("pink_skirt", 1, false),
                item("beer", 3, false),
            ],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
            reward_points: 3,
            blocker: None,
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
            reward_points: 1,
            blocker: None,
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
            reward_points: 5,
            blocker: None,
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
            reward_points: 1,
            blocker: None,
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
            reward_points: 1,
            blocker: None,
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
            reward_points: 1,
            blocker: Some("two players: one Phoenix and one Black Arm member trade the shield halves"),
        },
        QuestRequirements {
            key: "vampire".into(),
            name: "Vampire Slayer".into(),
            members: false,
            completion_varps: completion(&[("vampire", 3)]),
            skills: vec![],
            quest_points: 0,
            // Dr Harlow trades the stake for a beer (`harlow.rs2`).
            items: vec![item("beer", 1, false), item("garlic", 1, false), item("hammer", 1, false)],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
            reward_points: 3,
            blocker: None,
        },
        QuestRequirements {
            key: "hetty".into(),
            name: "Witch's Potion".into(),
            members: false,
            completion_varps: completion(&[("hetty", 3)]),
            skills: vec![],
            quest_points: 0,
            // The rat's tail drops only once the quest started: in-quest.
            items: vec![item("onion", 1, false), item("burnt_meat", 1, false), item("eye_of_newt", 1, false)],
            dependencies: vec![],
            combat_level: 0,
            recommended_combat: 0,
            reward_points: 1,
            blocker: None,
        },
    ]
}

/// Every F2P quest with typed requirements. Built per call (18 rows):
/// nothing is retained between calls.
pub fn all_f2p_quests() -> Vec<QuestRequirements> {
    build_f2p_quests()
}

/// F2P quest by key, or None when the key is unknown.
pub fn by_key(key: &str) -> Option<QuestRequirements> {
    build_f2p_quests().into_iter().find(|quest| quest.key == key)
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
        assert!(knight.items.iter().filter(|item| item.item != "cabbage").all(|item| item.worn_only), "the uniform is worn, the cabbage carried");
        let dragon = by_key("dragon").expect("dragon");
        // 39 quest points in F2P (`^<key>_questpoints`).
        assert_eq!(all_f2p_quests().iter().map(|quest| quest.reward_points).sum::<i64>(), 39);
        assert!(by_key("blackarmgang").expect("arrav").blocker.is_some(), "two players");
        assert_eq!((dragon.items[0].count, dragon.items[0].obj_id), (12000, 995));
        assert_eq!(by_key("blackarmgang").expect("arrav").completion_varps, vec![(145, 4), (146, 10)]);
        assert!(by_key("nope").is_none());
    }
}
