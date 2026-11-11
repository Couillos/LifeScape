//! Free-to-play index filters for planning: every table narrowed to what
//! a non-member bot can use. Rules: objects and recipes need
//! `members == Some(false)` (the dump leaves recipes null only for herblore,
//! so plain `false` also excludes every brew/fletch row); positions follow
//! the F2P map zones (`free_zones`, from `maps/free2play.csv`) and never
//! the dump spawn flags, which mark members-area spawns as F2P; shops
//! follow their keepers (a shop is F2P when a keeper NPC stands in an
//! F2P zone, plus the explicit `_free` world variants); quests have no
//! server flag, so the 18 scripted quests below are the whitelist.

use crate::drops::{self, Drop};
use crate::free_zones::is_free;
use crate::quests::Quest;
use crate::recipes::Recipe;
use crate::shops::{Keeper, Shop};
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

/// True when this keeper NPC stands somewhere a non-member can walk
/// (unknown or spawn-less keeper NPCs never count).
fn keeper_has_free_spawn(keeper: &Keeper) -> bool {
    if keeper.npc_id < 0 {
        return false;
    }
    npc_by_id(keeper.npc_id as usize)
        .is_some_and(|npc| npc.spawns.iter().any(|spot| is_free(spot.x, spot.z)))
}

/// Shops a non-member can use: a keeper NPC with an F2P-zone spawn, or
/// an explicit `_free` world variant (which carries no keeper row).
pub fn f2p_shops() -> Vec<Shop> {
    crate::shops::all_shops()
        .into_iter()
        .filter(|entry| entry.name.ends_with("_free") || entry.keepers.iter().any(keeper_has_free_spawn))
        .collect()
}

/// Combat spells a non-member can cast.
pub fn f2p_spells() -> Vec<Spell> {
    crate::spells::all_spells().into_iter().filter(|entry| !entry.members).collect()
}

/// Ground-item spawns standing in an F2P zone, optionally for one object.
/// The dump `members` flag is ignored: it follows the object type, not
/// the spawn position.
pub fn f2p_obj_spawns(obj_id: Option<usize>) -> Vec<ObjSpawn> {
    crate::spawns::all_obj_spawns(obj_id)
        .into_iter()
        .filter(|entry| is_free(entry.x, entry.z) && !is_unreachable_spawn(entry.x, entry.z, entry.level))
        .collect()
}

/// Static spawns no live bot can pick up (proven blocked tiles): the
/// Rimmington bronze axe upstairs (2970,3376,1) and the Varrock
/// bronze pickaxe (3009,3342,0). Both stand on tiles the router calls
/// disconnected/blocked live, so D51 never proposes them (axe falls back
/// to `axeshop`, pickaxe to its 4 reachable spawns or `pickaxeshop`).
pub const UNREACHABLE_OBJ_SPAWNS: [(i64, i64, i64); 2] = [(2970, 3376, 1), (3009, 3342, 0)];

fn is_unreachable_spawn(x: i64, z: i64, level: i64) -> bool {
    UNREACHABLE_OBJ_SPAWNS.contains(&(x, z, level))
}

/// NPC spawns standing in an F2P zone, optionally for one NPC. The dump
/// `members` flag is ignored: it follows the NPC type, not the spawn
/// position.
pub fn f2p_npc_spawns(npc_id: Option<usize>) -> Vec<NpcSpawn> {
    crate::spawns::all_npc_spawns(npc_id).into_iter().filter(|entry| is_free(entry.x, entry.z)).collect()
}

/// True when this NPC stands somewhere a non-member can walk.
fn npc_has_free_spawn(npc_id: usize) -> bool {
    npc_by_id(npc_id).is_some_and(|npc| npc.spawns.iter().any(|spot| is_free(spot.x, spot.z)))
}

/// Static drops of free-to-play tradeable objects from NPCs that stand
/// in an F2P zone (wildcard `npc_id < 0` rows carry no position and
/// stay listed, like before).
pub fn f2p_drops() -> Vec<Drop> {
    drops::all_drops()
        .into_iter()
        .filter(|entry| {
            entry.obj_id < 0
                || obj_by_id(entry.obj_id as usize)
                    .is_some_and(|obj| obj.members == Some(false) && obj.tradeable)
        })
        .filter(|entry| entry.npc_id < 0 || (entry.npc_id >= 0 && npc_has_free_spawn(entry.npc_id as usize)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{f2p_drops, f2p_npc_spawns, f2p_obj_spawns, f2p_objs, f2p_quests, f2p_recipes, f2p_shops, f2p_spells, F2P_QUESTS};
    use crate::free_zones::is_free;
    use crate::spawns::{all_npc_spawns, all_obj_spawns};

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
        // Spells keep the standard book; chicken drops survive the F2P cut.
        let wind = f2p_spells().into_iter().find(|entry| entry.spell == "wind_strike").expect("wind strike");
        assert!(!wind.members);
        assert!(f2p_drops().iter().any(|entry| entry.npc == "chicken" && entry.obj == "feather"));
    }

    #[test]
    fn obj_spawns_come_from_free_zones() {
        // The dump flags 799 spawns F2P; only the 452 standing in an
        // F2P zone survive (verified counts from the catalogue), minus
        // the 2 live-blocked boot spawns (axe upstairs, Varrock pickaxe).
        assert_eq!(all_obj_spawns(None).len(), 1151);
        let free = f2p_obj_spawns(None);
        assert_eq!(free.len(), 450, "f2p ground spawns, got {}", free.len());
        assert!(free.iter().all(|entry| is_free(entry.x, entry.z)), "no f2p spawn outside the zones");
        for (x, z, level) in super::UNREACHABLE_OBJ_SPAWNS {
            assert!(
                !free.iter().any(|entry| entry.x == x && entry.z == z && entry.level == level),
                "blocked spawn ({x},{z},{level}) never proposed"
            );
        }
        let count = |name: &str| {
            let id =
                crate::objs_by_name(name).into_iter().next().unwrap_or_else(|| panic!("{name} in dumps")).obj_id;
            f2p_obj_spawns(Some(id)).len()
        };
        assert_eq!(count("bronze_pickaxe"), 4, "bronze pick spawns minus the blocked Varrock tile");
        assert_eq!(count("bronze_axe"), 0, "blocked Rimmington axe falls back to axeshop");
        assert_eq!(count("iron_pickaxe"), 0, "no iron pick spawn");
        assert_eq!(count("fishing_rod"), 0, "no fishing rod spawn");
        assert_eq!(count("coins"), 39, "coin piles");
        assert_eq!(count("cake_tin"), 2, "cake mould spawns only");
    }

    #[test]
    fn npc_spawns_come_from_free_zones() {
        // The dump flags 7088 spawns F2P; only the 1779 standing in an
        // F2P zone survive.
        assert_eq!(all_npc_spawns(None).len(), 7322);
        let free = f2p_npc_spawns(None);
        assert_eq!(free.len(), 1779, "f2p npc spawns, got {}", free.len());
        assert!(free.iter().all(|entry| is_free(entry.x, entry.z)), "no f2p npc outside the zones");
        let hans = crate::npcs_by_name("hans").into_iter().next().expect("hans in dumps");
        assert!(!f2p_npc_spawns(Some(hans.npc_id)).is_empty(), "lumbridge hans stays");
        let druids = crate::npcs_by_name("chaos_druid").into_iter().next().expect("chaos druid in dumps");
        assert!(f2p_npc_spawns(Some(druids.npc_id)).is_empty(), "no f2p chaos druid");
    }

    #[test]
    fn shops_follow_their_keepers() {
        // Keepered shops need a keeper standing in an F2P zone; the two
        // `_free` rows are the explicit F2P-world variants.
        let free = f2p_shops();
        assert_eq!(free.len(), 45, "f2p shops, got {}", free.len());
        let names: Vec<&str> = free.iter().map(|entry| entry.name.as_str()).collect();
        for name in ["scimitarshop", "axeshop", "pickaxeshop", "fishingshop", "craftingshop_free", "craftingshop2_free"] {
            assert!(names.contains(&name), "shop {name} stays f2p");
        }
        for name in ["khazardshop", "fishingguild", "regicide_general_shop", "memberstaffshop", "foodshop"] {
            assert!(!names.contains(&name), "shop {name} leaves f2p");
        }
        for shop in &free {
            if shop.name.ends_with("_free") {
                continue;
            }
            assert!(
                shop.keepers.iter().any(|keeper| {
                    keeper.npc_id >= 0
                        && crate::npc_by_id(keeper.npc_id as usize)
                            .is_some_and(|npc| npc.spawns.iter().any(|spot| is_free(spot.x, spot.z)))
                }),
                "shop {} has a free keeper",
                shop.name
            );
        }
        for shop in crate::shops::all_shops() {
            if names.contains(&shop.name.as_str()) || shop.keepers.is_empty() {
                continue;
            }
            assert!(
                !shop.keepers.iter().any(|keeper| {
                    keeper.npc_id >= 0
                        && crate::npc_by_id(keeper.npc_id as usize)
                            .is_some_and(|npc| npc.spawns.iter().any(|spot| is_free(spot.x, spot.z)))
                }),
                "shop {} excluded for a reason",
                shop.name
            );
        }
    }

    #[test]
    fn drops_need_a_free_npc() {
        // Chaos druids never stand in an F2P zone, so their drops leave
        // the F2P set even though the NPC type flag says F2P.
        assert!(!f2p_drops().iter().any(|entry| entry.npc == "chaos_druid"), "no chaos druid drops");
        for drop in f2p_drops() {
            if drop.npc_id < 0 {
                continue;
            }
            assert!(
                crate::npc_by_id(drop.npc_id as usize)
                    .is_some_and(|npc| npc.spawns.iter().any(|spot| is_free(spot.x, spot.z))),
                "drop npc {} stands free",
                drop.npc
            );
        }
    }

    #[test]
    fn no_f2p_source_outside_the_zones() {
        // M10a acceptance: no F2P source (ground spawn, NPC spawn, shop
        // keeper, bank booth) stands outside `free2play.csv`.
        assert!(f2p_obj_spawns(None).iter().all(|entry| is_free(entry.x, entry.z)));
        assert!(f2p_npc_spawns(None).iter().all(|entry| is_free(entry.x, entry.z)));
        for shop in f2p_shops() {
            for keeper in &shop.keepers {
                if keeper.npc_id < 0 {
                    continue;
                }
                let npc = crate::npc_by_id(keeper.npc_id as usize)
                    .unwrap_or_else(|| panic!("keeper {} resolves", keeper.npc));
                assert!(
                    npc.spawns.iter().any(|spot| is_free(spot.x, spot.z)),
                    "keeper {} stands free",
                    keeper.npc
                );
            }
        }
        for bank in crate::banks::all_banks() {
            if !bank.members {
                assert!(is_free(bank.x, bank.z), "bank ({}, {}) stands free", bank.x, bank.z);
            }
        }
    }
}
