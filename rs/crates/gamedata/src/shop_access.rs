//! Which NPC opens which shop on an F2P world (`gamedata/data/
//! shop_access.json`, written by `gamedata/tools/dump_plan.mts` from the
//! `[opnpcN,npc]` handlers evaluated with `map_members = false`).
//!
//! The old keeper join (`owned_shop` param + an F2P spawn) both missed
//! shops opened by explicit `~openshop(inv)` calls (Zaff's staffs) and
//! kept shops whose handlers are shut on F2P worlds (Shantay, the tea
//! seller, the fancy dress shop) or gated on a members quest (the cannon
//! shop). Each opening path carries its quest-varp gates.

#![forbid(unsafe_code)]

use std::sync::OnceLock;

const RAW: &str = include_str!("../../../../gamedata/data/shop_access.json");

/// One quest/state varp the opening path checks, with its owning quest
/// directory (`quest_dragon`) when the server declares it under a quest.
#[derive(Debug, Clone, PartialEq)]
pub struct Gate {
    pub varp: String,
    pub quest: Option<String>,
}

/// One shop an NPC opens on an F2P world.
#[derive(Debug, Clone)]
pub struct Opening {
    pub inv: String,
    pub gates: Vec<Gate>,
}

/// One shop NPC: its `owned_shop` param and the shops its handlers open.
#[derive(Debug, Clone)]
pub struct ShopNpc {
    pub npc: String,
    pub npc_id: usize,
    pub owned_shop: Option<String>,
    /// Trade handlers of its own (0 = generic category handler only).
    pub handlers: i64,
    pub f2p_opens: Vec<Opening>,
}

fn table() -> &'static Vec<ShopNpc> {
    static TABLE: OnceLock<Vec<ShopNpc>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut out = Vec::new();
        let Ok(doc) = lifescape_json::parse(RAW) else { return out };
        for row in doc.get("npcs").as_arr().cloned().unwrap_or_default() {
            let Some(npc_id) = row.get("npc_id").as_i64().and_then(|id| usize::try_from(id).ok()) else { continue };
            let mut opens = Vec::new();
            for open in row.get("f2p_opens").as_arr().cloned().unwrap_or_default() {
                let gates = open
                    .get("gates")
                    .as_arr()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|gate| Gate {
                        varp: gate.get("varp").as_str().unwrap_or("").to_string(),
                        quest: gate.get("quest").as_str().map(str::to_string),
                    })
                    .collect();
                opens.push(Opening { inv: open.get("inv").as_str().unwrap_or("").to_string(), gates });
            }
            out.push(ShopNpc {
                npc: row.get("npc").as_str().unwrap_or("").to_string(),
                npc_id,
                owned_shop: row.get("owned_shop").as_str().map(str::to_string),
                handlers: row.get("handlers").as_i64().unwrap_or(0),
                f2p_opens: opens,
            });
        }
        out
    })
}

/// Every shop NPC row.
pub fn all() -> &'static [ShopNpc] {
    table()
}

/// NPCs whose handlers open `shop` on an F2P world, with that path's gates.
pub fn openers(shop: &str) -> Vec<(&'static ShopNpc, &'static Opening)> {
    table()
        .iter()
        .flat_map(|npc| npc.f2p_opens.iter().filter(move |open| open.inv == shop).map(move |open| (npc, open)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{all, openers};

    #[test]
    fn handlers_decide_f2p_shops() {
        assert!(all().len() > 100, "shop npcs, got {}", all().len());
        let zaff: Vec<&str> = openers("staffshop").iter().map(|(npc, _)| npc.npc.as_str()).collect();
        assert!(zaff.contains(&"zaff"), "Zaff opens his staffs by ~openshop, got {zaff:?}");
        assert!(openers("memberstaffshop").is_empty(), "members variant shut in F2P");
        assert!(openers("shantayshop").is_empty(), "Shantay shut in F2P");
        assert!(openers("teashop").is_empty(), "tea seller shut in F2P");
        assert!(openers("fancyclothesstore").is_empty(), "fancy dress shut in F2P");
        let cannon = openers("mcannonshop");
        assert!(cannon.iter().all(|(_, open)| open.gates.iter().any(|gate| gate.quest.as_deref() == Some("quest_mcannon"))), "cannon shop gated on its quest");
        let plate = openers("runeplateshop");
        assert!(plate.iter().all(|(_, open)| open.gates.iter().any(|gate| gate.quest.as_deref() == Some("quest_dragon"))), "Oziach after Dragon Slayer");
        assert!(!openers("craftingshop_free").is_empty(), "Rommik's F2P variant");
    }
}
