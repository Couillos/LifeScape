//! F2P NPC services (M10b §3): tanners, Aubury, Aggie, Wyson, Grum,
//! Port Sarim boat (30 gp), Al-Kharid toll (10 gp) and guild entries.
//! Costs come from the 274 server scripts; positions are F2P spawns
//! (`f2p_npc_spawns`), never the dump `members` flag (D63).

use crate::f2p::f2p_npc_spawns;

/// One NPC service: who, where, what it costs and needs.
#[derive(Debug, Clone)]
pub struct Service {
    pub key: &'static str,
    pub npc: &'static str,
    pub npc_id: usize,
    pub x: i64,
    pub z: i64,
    pub level: i64,
    pub cost_gp: i64,
    pub requires: &'static str,
}

/// Resolve the first F2P spawn of one NPC debugname.
fn first_spawn(name: &str) -> Option<(usize, i64, i64, i64)> {
    let found = crate::npcs_by_name(name).into_iter().next()?;
    let spawn = f2p_npc_spawns(Some(found.npc_id)).into_iter().next()?;
    Some((found.npc_id, spawn.x, spawn.z, spawn.level))
}

fn service(key: &'static str, npc: &'static str, cost_gp: i64, requires: &'static str) -> Option<Service> {
    first_spawn(npc).map(|(npc_id, x, z, level)| Service { key, npc, npc_id, x, z, level, cost_gp, requires })
}

/// Every F2P service with an F2P position (boat/toll are loc-based extras).
pub fn all_services() -> Vec<Service> {
    let mut out = Vec::new();
    // Tanner Ellis (Al-Kharid): first F2P spawn; Crafting Guild tanner
    // shares the same debugname and is the second F2P spawn.
    if let Some(tanner) = service("tan_soft", "tanner", 1, "cow_hide") {
        out.push(tanner);
    }
    if let Some(tanner_hard) = service("tan_hard", "tanner", 3, "cow_hide") {
        let mut hard = tanner_hard;
        hard.key = "tan_hard";
        out.push(hard);
    }
    for (key, npc, cost, requires) in [
        ("aubury_teleport", "aubury", 0, "quest_rune_mysteries"),
        ("aggie_red", "aggie", 5, "redberries"),
        ("aggie_yellow", "aggie", 5, "onion"),
        ("aggie_blue", "aggie", 5, "woadleaf"),
        ("wyson_woad", "wyson", 15, "coins"),
        ("grum_jewellery", "grum", 0, "coins"),
        ("boat_karamja", "seaman_lorris", 30, "coins"),
        ("boat_karamja2", "seaman_thresnor", 30, "coins"),
    ] {
        if let Some(entry) = service(key, npc, cost, requires) {
            out.push(entry);
        }
    }
    out
}

/// Port Sarim ↔ Karamja fare (30 gp) and Al-Kharid toll (10 gp).
pub const BOAT_FARE_GP: i64 = 30;
pub const TOLL_GP: i64 = 10;

/// Guild entries (level + uniform, from the 274 scripts). The uniform
/// is empty when the script checks no item: the Mining Guild dwarf only
/// asks for mining 60, so the entry carries no pickaxe.
pub const GUILDS: [(&str, i64, &str); 3] = [
    ("cooks_guild", 32, "chefs_hat"),
    ("crafting_guild", 40, "brown_apron"),
    ("mining_guild", 60, ""),
];

#[cfg(test)]
mod tests {
    use super::{all_services, BOAT_FARE_GP, GUILDS, TOLL_GP};
    use crate::free_zones::is_free;

    #[test]
    fn services_are_free_and_priced() {
        // Tanners (soft 1 gp, hard 3 gp), Aubury, Aggie, Wyson, Grum and
        // both seamen stand in F2P zones; boat 30, toll 10.
        let services = all_services();
        assert!(services.len() >= 8, "services, got {}", services.len());
        assert_eq!(BOAT_FARE_GP, 30);
        assert_eq!(TOLL_GP, 10);
        for entry in &services {
            assert!(is_free(entry.x, entry.z), "service {} at ({}, {}) stands free", entry.key, entry.x, entry.z);
        }
        let tan = services.iter().find(|entry| entry.key == "tan_soft").expect("tanner");
        assert_eq!((tan.npc, tan.cost_gp), ("tanner", 1));
        assert_eq!((tan.x, tan.z), (3276, 3193));
        let hard = services.iter().find(|entry| entry.key == "tan_hard").expect("hard tan");
        assert_eq!(hard.cost_gp, 3);
        let aubury = services.iter().find(|entry| entry.key == "aubury_teleport").expect("aubury");
        assert_eq!((aubury.x, aubury.z), (3253, 3402));
        let boat = services.iter().find(|entry| entry.key == "boat_karamja").expect("boat");
        assert_eq!(boat.cost_gp, 30);
        assert_eq!(GUILDS.len(), 3);
        assert_eq!(GUILDS[0], ("cooks_guild", 32, "chefs_hat"));
        assert_eq!(GUILDS[2], ("mining_guild", 60, ""), "no item gate on the mining guild");
    }
}
