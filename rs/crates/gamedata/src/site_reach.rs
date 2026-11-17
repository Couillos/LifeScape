//! Tree and rock spawns no worker can work (`gamedata/data/
//! unreachable_sites.json`, plan P6 2026-11-17): no stand reaches the
//! loc (wall, fence, invisible blocker) or every stand that does is a
//! sealed pocket. Recomputed from the map by
//! `rs/crates/nav/tests/work_sites.rs`; `danger::sites` leaves them out
//! of the site catalogue and the scripts never pick one.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::OnceLock;

const RAW: &str = include_str!("../../../../gamedata/data/unreachable_sites.json");

fn table() -> &'static HashMap<(i64, i64, i64), &'static str> {
    static TABLE: OnceLock<HashMap<(i64, i64, i64), &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut out = HashMap::new();
        let Ok(doc) = lifescape_json::parse(RAW) else { return out };
        for row in doc.get("sites").as_arr().map(Vec::as_slice).unwrap_or(&[]) {
            let (Some(x), Some(z), Some(level)) = (row.get("x").as_i64(), row.get("z").as_i64(), row.get("level").as_i64()) else { continue };
            let reason = match row.get("reason").as_str() {
                Some("sealed") => "sealed",
                _ => "no_stand",
            };
            out.insert((x, z, level), reason);
        }
        out
    })
}

/// Why the work loc spawned at `(x, z, level)` is no work site (`sealed`,
/// `no_stand`), `None` for a workable one.
pub fn unreachable(x: i64, z: i64, level: i64) -> Option<&'static str> {
    table().get(&(x, z, level)).copied()
}

/// Number of tabled spawns (the dump parsed).
pub fn count() -> usize {
    table().len()
}

#[cfg(test)]
mod tests {
    use super::{count, unreachable};

    /// Live 2026-11-17: the river willow behind `inviswall_blockrange`
    /// is out; the Lumbridge castle trees stay.
    #[test]
    fn the_river_willow_is_no_work_site() {
        assert!(count() > 100, "dump parsed: {}", count());
        assert_eq!(unreachable(3168, 3350, 0), Some("sealed"));
        assert_eq!(crate::danger::sites("woodcut", "willow_logs").iter().find(|site| **site == (3168, 3350, 0)), None, "never a willow site");
        assert!(!crate::danger::sites("woodcut", "willow_logs").is_empty());
    }
}
