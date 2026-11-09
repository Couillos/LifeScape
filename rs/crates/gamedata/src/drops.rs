//! Static NPC death drops scraped from the compiled drop-table scripts
//! (`gamedata/data/drops.json`, via `gamedata/tools/dump_plan.mts`).
//! One row per (NPC, object) pair; counts and chances are not modeled.

/// One static drop row (build-time data, mirrored into [`Drop`] per call).
pub(crate) struct StaticDrop {
    pub npc: &'static str,
    pub npc_id: i64,
    pub obj: &'static str,
    pub obj_id: i64,
}

mod generated {
    include!(concat!(env!("OUT_DIR"), "/drops_generated.rs"));
}

/// One static (NPC, object) drop pair, with resolved ids (`-1` unknown).
#[derive(Debug, Clone)]
pub struct Drop {
    pub npc: String,
    pub npc_id: i64,
    pub obj: String,
    pub obj_id: i64,
}

fn drop_from_static(row: &StaticDrop) -> Drop {
    Drop {
        npc: row.npc.to_string(),
        npc_id: row.npc_id,
        obj: row.obj.to_string(),
        obj_id: row.obj_id,
    }
}

/// Every drop pair.
pub fn all_drops() -> Vec<Drop> {
    generated::ALL.iter().map(drop_from_static).collect()
}

/// Drops of one object id.
pub fn drops_for_obj(obj_id: usize) -> Vec<Drop> {
    all_drops().into_iter().filter(|entry| entry.obj_id == obj_id as i64).collect()
}

/// Drops of one NPC id.
pub fn drops_for_npc(npc_id: usize) -> Vec<Drop> {
    all_drops().into_iter().filter(|entry| entry.npc_id == npc_id as i64).collect()
}

#[cfg(test)]
mod tests {
    use super::{all_drops, drops_for_npc, drops_for_obj};

    #[test]
    fn drop_vectors() {
        // Chickens (npc 41) drop feathers (314) and raw chicken (2138);
        // goblins keep their table apart from the village one (herbs).
        let all = all_drops();
        assert!(all.len() > 3000, "drop pairs, got {}", all.len());
        let chicken: Vec<String> = drops_for_npc(41).iter().map(|entry| entry.obj.clone()).collect();
        assert!(chicken.contains(&"feather".to_string()), "chicken feathers: {chicken:?}");
        assert!(chicken.contains(&"raw_chicken".to_string()), "chicken meat: {chicken:?}");
        let feather_npcs: Vec<i64> = drops_for_obj(314).iter().map(|entry| entry.npc_id).collect();
        assert!(feather_npcs.contains(&41), "feather from chicken");
        let goblin: Vec<String> = drops_for_npc(100).iter().map(|entry| entry.obj.clone()).collect();
        assert!(goblin.contains(&"coins".to_string()), "goblin coins: {goblin:?}");
        assert!(!all.iter().any(|entry| entry.obj_id < 0 || entry.npc_id < 0), "all ids resolve");
    }
}
