//! Static spawns: NPC positions and ground-item pops with cooldowns.

use crate::obj_by_id;

/// One static ground-item spawn row (build-time data).
pub(crate) struct StaticObjSpawn {
    pub level: i64,
    pub x: i64,
    pub z: i64,
    pub obj: i64,
    pub count: i64,
    pub members: Option<bool>,
}

mod generated {
    include!(concat!(env!("OUT_DIR"), "/spawns_obj_generated.rs"));
}

/// One static NPC spawn. `members` comes from the dump row.
#[derive(Debug, Clone)]
pub struct NpcSpawn {
    pub x: i64,
    pub z: i64,
    pub level: i64,
    pub npc_id: usize,
    pub members: bool,
}

/// One ground-item spawn, with the object type's respawn cooldown.
#[derive(Debug, Clone)]
pub struct ObjSpawn {
    pub x: i64,
    pub z: i64,
    pub level: i64,
    pub obj_id: usize,
    pub count: i64,
    pub respawn_ticks: i64,
    pub members: bool,
}

/// Every NPC spawn, optionally filtered to one NPC id.
pub fn all_npc_spawns(npc_id: Option<usize>) -> Vec<NpcSpawn> {
    let mut out = Vec::new();
    for (row_id, entry) in crate::npcs::ALL.iter().enumerate() {
        if npc_id.is_some_and(|wanted| wanted != row_id) {
            continue;
        }
        for spot in entry.spawns {
            out.push(NpcSpawn { x: spot.x, z: spot.z, level: spot.level, npc_id: row_id, members: spot.members });
        }
    }
    out
}

/// Every ground-item spawn, optionally filtered to one object id.
pub fn all_obj_spawns(obj_id: Option<usize>) -> Vec<ObjSpawn> {
    let mut out = Vec::new();
    for row in generated::ALL {
        let id = row.obj;
        if id < 0 {
            continue;
        }
        if obj_id.is_some_and(|wanted| wanted != id as usize) {
            continue;
        }
        let entry = obj_by_id(id as usize);
        let ticks = entry.map(|found| found.respawn_ticks).unwrap_or(100);
        // Dump flag wins; otherwise fall back to the object flag.
        let members = row.members.or_else(|| entry.and_then(|found| found.members)).unwrap_or(false);
        out.push(ObjSpawn {
            x: row.x,
            z: row.z,
            level: row.level,
            obj_id: id as usize,
            count: row.count,
            respawn_ticks: ticks,
            members,
        });
    }
    out
}

/// Ground-item spawns on this level within a Manhattan radius.
pub fn obj_spawns_near(x: i64, z: i64, level: i64, radius: i64) -> Vec<ObjSpawn> {
    all_obj_spawns(None)
        .into_iter()
        .filter(|spawn| spawn.level == level && (spawn.x - x).abs() + (spawn.z - z).abs() <= radius)
        .collect()
}

/// NPC spawns on this level within a Manhattan radius.
pub fn npc_spawns_near(x: i64, z: i64, level: i64, radius: i64) -> Vec<NpcSpawn> {
    all_npc_spawns(None)
        .into_iter()
        .filter(|spawn| spawn.level == level && (spawn.x - x).abs() + (spawn.z - z).abs() <= radius)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{all_npc_spawns, all_obj_spawns, npc_spawns_near, obj_spawns_near};

    #[test]
    fn spawn_vectors() {
        // 7322 NPC spawns, 1151 ground spawns; near
        // (3200, 3200, 0) +/-15: 16 NPCs, 1 ground spawn
        // (mind rune at 3206,3208).
        assert_eq!(all_npc_spawns(None).len(), 7322);
        assert_eq!(all_obj_spawns(None).len(), 1151);
        assert_eq!(npc_spawns_near(3200, 3200, 0, 15).len(), 16);
        let near = obj_spawns_near(3200, 3200, 0, 15);
        assert_eq!(near.len(), 1);
        assert_eq!((near[0].x, near[0].z, near[0].obj_id), (3206, 3208, 558));
        assert_eq!(near[0].respawn_ticks, 100);
        assert!(!near[0].members);
        assert!(all_npc_spawns(Some(0)).iter().all(|spawn| spawn.npc_id == 0));
    }
}
