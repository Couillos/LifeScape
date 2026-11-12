//! NPC wander and chase ranges (`gamedata/data/npc_ranges.json`, written
//! by `orchestrator dump-npc-ranges` from the server's `.npc` configs;
//! the game dump carries neither field). Absent NPCs take the engine
//! defaults (`NpcType`: wanderrange 5, maxrange wanderrange + 2).

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::OnceLock;

/// Engine default (`NpcType.wanderrange = 5`).
pub const DEFAULT_WANDER: i64 = 5;

const RAW: &str = include_str!("../../../../gamedata/data/npc_ranges.json");

/// (wanderrange, maxrange) per NPC debug name.
fn table() -> &'static HashMap<String, (i64, i64)> {
    static TABLE: OnceLock<HashMap<String, (i64, i64)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut out = HashMap::new();
        let Ok(doc) = lifescape_json::parse(RAW) else { return out };
        if let Some(rows) = doc.get("npcs").as_obj() {
            for (name, row) in rows {
                let wander = row.get("wanderrange").as_i64().unwrap_or(DEFAULT_WANDER);
                let max = row.get("maxrange").as_i64().unwrap_or(wander + 2).max(wander);
                out.insert(name.clone(), (wander, max));
            }
        }
        out
    })
}

/// How far the NPC strays from its spawn.
pub fn wander_range(name: &str) -> i64 {
    table().get(name).map(|(wander, _)| *wander).unwrap_or(DEFAULT_WANDER)
}

/// How far the NPC chases from its spawn.
pub fn max_range(name: &str) -> i64 {
    table().get(name).map(|(_, max)| *max).unwrap_or(DEFAULT_WANDER + 2)
}

#[cfg(test)]
mod tests {
    use super::{max_range, wander_range, DEFAULT_WANDER};

    #[test]
    fn ranges_come_from_the_server_configs() {
        assert_eq!((wander_range("jailguard"), max_range("jailguard")), (11, 12), "draynor.npc");
        assert_eq!(wander_range("young_dark_wizard"), 3, "_unpack/225/all.npc");
        assert_eq!(wander_range("no_such_npc"), DEFAULT_WANDER, "engine default");
    }
}
