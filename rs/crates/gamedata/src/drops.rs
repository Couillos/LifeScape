//! Static NPC death drops scraped from the compiled drop-table scripts
//! (`gamedata/data/drops.json`, via `gamedata/tools/dump_plan.mts`).
//! One row per (NPC, object) pair, with the F2P-world drop rate per
//! kill and the quantity range evaluated from the RuneScript sources
//! (`gamedata/tools/drop_rates.mts`). `rate` 0 = members-only branch.

/// One static drop row (build-time data, mirrored into [`Drop`] per call).
pub(crate) struct StaticDrop {
    pub npc: &'static str,
    pub npc_id: i64,
    pub obj: &'static str,
    pub obj_id: i64,
    pub rate: f64,
    pub qty_min: i64,
    pub qty_max: i64,
    pub exact: bool,
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
    /// Expected drops of this object per kill on an F2P world (0 when
    /// only a members branch reaches it; -1 unknown).
    pub rate: f64,
    /// Quantity range of one drop (-1 when computed at runtime).
    pub qty_min: i64,
    pub qty_max: i64,
    /// False when the evaluator met a condition it could not decide.
    pub exact: bool,
}

impl Drop {
    /// Expected objects per kill: rate × mean quantity (min..max, 1
    /// when the quantity is computed at runtime).
    pub fn expected_per_kill(&self) -> f64 {
        let mean = if self.qty_min >= 0 && self.qty_max >= self.qty_min { (self.qty_min + self.qty_max) as f64 / 2.0 } else { 1.0 };
        self.rate.max(0.0) * mean
    }
}

fn drop_from_static(row: &StaticDrop) -> Drop {
    Drop {
        npc: row.npc.to_string(),
        npc_id: row.npc_id,
        obj: row.obj.to_string(),
        obj_id: row.obj_id,
        rate: row.rate,
        qty_min: row.qty_min,
        qty_max: row.qty_max,
        exact: row.exact,
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

    /// Rates evaluated from the rev 274 sources on an F2P world: cow
    /// hides every kill, goblin coins 27/128, the members-gated goblin
    /// spear never, bones from the default death handler.
    #[test]
    fn drop_rates_follow_the_server_tables() {
        let find = |npc: &str, obj: &str| all_drops().into_iter().find(|entry| entry.npc == npc && entry.obj == obj);
        assert_eq!(find("cow", "cow_hide").expect("cow hide").rate, 1.0);
        assert_eq!(find("cow", "bones").expect("cow bones").rate, 1.0, "death_drop param");
        let coins = find("goblin", "coins").expect("goblin coins");
        assert!((coins.rate - 27.0 / 128.0).abs() < 1e-5, "goblin coins {}", coins.rate);
        assert_eq!(find("goblin", "bronze_spear").expect("spear pair").rate, 0.0, "members-only branch");
        assert!((find("chicken", "feather").expect("feather").rate - 0.5).abs() < 1e-9);
        assert!(all_drops().iter().all(|entry| entry.rate >= 0.0), "every pair rated");
    }
}
