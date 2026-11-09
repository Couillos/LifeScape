//! Free-to-play index filters for planning: every table narrowed to what
//! a non-member bot can use. Rules: objects and recipes need
//! `members == Some(false)` (the dump leaves recipes null only for herblore,
//! so plain `false` also excludes every brew/fletch row); shops and drops
//! exclude only known-members rows (stalls carry no keeper flag); quests
//! have no server flag, so the 18 scripted quests below are the whitelist.

use crate::drops::{self, Drop};
use crate::quests::Quest;
use crate::recipes::Recipe;
use crate::shops::Shop;
use crate::spells::Spell;
use crate::spawns::{NpcSpawn, ObjSpawn};
use crate::{npc_by_id, obj_by_id, Obj};

/// Registry script name plus the `quests.json` dir it resolves to.
/// Source: the compiled script registry (18 F2P quests).
pub const F2P_QUESTS: [(&str, &str); 18] = [
    ("quest_black_knights_fortress", "quest_blackknight"),
    ("quest_cooks_assistant", "quest_cook"),
    ("quest_demon_slayer", "quest_demon"),
    ("quest_dorics_quest", "quest_doric"),
    ("quest_dragon_slayer", "quest_dragon"),
    ("quest_ernest_the_chicken", "quest_haunted"),
    ("quest_goblin_diplomacy", "quest_gobdip"),
    ("quest_imp_catcher", "quest_imp"),
    ("quest_knights_sword", "quest_squire"),
    ("quest_pirates_treasure", "quest_hunt"),
    ("quest_prince_ali_rescue", "quest_prince"),
    ("quest_restless_ghost", "quest_priest"),
    ("quest_romeo_and_juliet", "quest_romeojuliet"),
    ("quest_rune_mysteries", "quest_runemysteries"),
    ("quest_sheep_shearer", "quest_sheep"),
    ("quest_shield_of_arrav", "quest_blackarmgang"),
    ("quest_vampire_slayer", "quest_vampire"),
    ("quest_witchs_potion", "quest_hetty"),
];

/// The 18 whitelisted quests, resolved via `quests.json` (one row each).
/// Rows that fail to resolve are skipped; the test pins all 18.
pub fn f2p_quests() -> Vec<Quest> {
    let mut out = Vec::with_capacity(F2P_QUESTS.len());
    for (_, dir) in F2P_QUESTS {
        out.extend(crate::quests::by_name(dir));
    }
    out
}

/// Every object flagged free-to-play.
pub fn f2p_objs() -> Vec<&'static Obj> {
    crate::all_objs().iter().filter(|entry| entry.members == Some(false)).collect()
}

/// Market universe: F2P objects the server lets players trade.
pub fn f2p_tradeable_objs() -> Vec<&'static Obj> {
    f2p_objs().into_iter().filter(|entry| entry.tradeable).collect()
}

/// Craft recipes a non-member can use (no brew, no fletch: herblore and
/// fletching are members-only, and every null-flag row is herblore).
pub fn f2p_recipes() -> Vec<Recipe> {
    crate::recipes::all_recipes(None)
        .into_iter()
        .filter(|entry| entry.members == Some(false))
        .collect()
}

/// Shops that are not known members-only (stalls carry no keeper flag and
/// stay listed; interface inventories are filtered at the source level).
pub fn f2p_shops() -> Vec<Shop> {
    crate::shops::all_shops().into_iter().filter(|entry| entry.members != Some(true)).collect()
}

/// Combat spells a non-member can cast.
pub fn f2p_spells() -> Vec<Spell> {
    crate::spells::all_spells().into_iter().filter(|entry| !entry.members).collect()
}

/// Ground-item spawns outside members areas, optionally for one object.
pub fn f2p_obj_spawns(obj_id: Option<usize>) -> Vec<ObjSpawn> {
    crate::spawns::all_obj_spawns(obj_id).into_iter().filter(|entry| !entry.members).collect()
}

/// NPC spawns outside members areas, optionally for one NPC.
pub fn f2p_npc_spawns(npc_id: Option<usize>) -> Vec<NpcSpawn> {
    crate::spawns::all_npc_spawns(npc_id).into_iter().filter(|entry| !entry.members).collect()
}

/// Static drops from free-to-play NPCs of free-to-play tradeable objects.
pub fn f2p_drops() -> Vec<Drop> {
    drops::all_drops()
        .into_iter()
        .filter(|entry| {
            entry.obj_id < 0
                || obj_by_id(entry.obj_id as usize)
                    .is_some_and(|obj| obj.members == Some(false) && obj.tradeable)
        })
        .filter(|entry| {
            entry.npc_id < 0
                || npc_by_id(entry.npc_id as usize).is_some_and(|npc| npc.members == Some(false))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{f2p_drops, f2p_objs, f2p_quests, f2p_recipes, f2p_shops, f2p_spells, F2P_QUESTS};

    #[test]
    fn f2p_whitelist_resolves() {
        // All 18 registry scripts resolve to exactly one quest row each;
        // quests.json carries members=null, so the registry is the list.
        assert_eq!(F2P_QUESTS.len(), 18);
        let quests = f2p_quests();
        assert_eq!(quests.len(), 18, "quests resolve, got {}", quests.len());
        let names: Vec<&str> = quests.iter().map(|entry| entry.name.as_str()).collect();
        for (_, dir) in F2P_QUESTS {
            assert!(names.contains(&dir), "quest {dir} resolves");
        }
    }

    #[test]
    fn f2p_vectors() {
        // 1403 F2P objects; coins are F2P and tradeable, quest items
        // like karamja rum are F2P but not tradeable.
        assert_eq!(f2p_objs().len(), 1403);
        let coins = crate::objs_by_name("coins");
        assert_eq!(coins.len(), 1);
        assert_eq!(coins[0].members, Some(false));
        assert!(coins[0].tradeable);
        let rum = crate::objs_by_name("karamja_rum");
        assert_eq!(rum.len(), 1);
        assert_eq!(rum[0].members, Some(false));
        assert!(!rum[0].tradeable, "quest rum stays untradeable");
        // No member recipe and no herblore/fletching in the F2P set.
        let recipes = f2p_recipes();
        assert!(!recipes.is_empty());
        assert!(recipes.iter().all(|entry| entry.members != Some(true)), "no member recipe");
        assert!(
            recipes.iter().all(|entry| entry.kind != "brew" && entry.kind != "fletch"),
            "no brew/fletch rows"
        );
        // Shops keep F2P rows and stalls; spells keep the standard book.
        assert!(f2p_shops().iter().any(|entry| entry.name == "scimitarshop"));
        assert!(f2p_shops().iter().all(|entry| entry.members != Some(true)));
        let wind = f2p_spells().into_iter().find(|entry| entry.spell == "wind_strike").expect("wind strike");
        assert!(!wind.members);
        // Chicken drops survive the F2P cut.
        assert!(f2p_drops().iter().any(|entry| entry.npc == "chicken" && entry.obj == "feather"));
    }
}
