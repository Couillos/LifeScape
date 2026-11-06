//! Free-to-play bounds: 8x8 zones from the engine `free2play.csv`.
//!
//! One tile is F2P when its zone (`tile >> 3`) is listed, like the
//! engine's own check (level ignored). Unset means no bounds: every
//! tile is inside.

use std::collections::HashSet;
use std::path::Path;

/// Zone keys `(x >> 3, z >> 3)` of the free-to-play map.
pub fn load_zones(csv: &Path) -> Result<HashSet<(i32, i32)>, String> {
    let text = std::fs::read_to_string(csv).map_err(|err| format!("read {}: {err}", csv.display()))?;
    let mut zones = HashSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        let parts: Vec<&str> = trimmed.split('_').collect();
        if parts.len() != 5 {
            continue;
        }
        let mut nums = [0i32; 5];
        let mut ok = true;
        for (slot, part) in parts.iter().enumerate() {
            match part.parse::<i32>() {
                Ok(num) => nums[slot] = num,
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        zones.insert((nums[1] * 8 + (nums[3] >> 3), nums[2] * 8 + (nums[4] >> 3)));
    }
    Ok(zones)
}
