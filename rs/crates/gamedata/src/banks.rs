//! Bank access points: booth loc spawns with location-derived members.

use crate::all_locs;

/// One bank booth spawn. `members` comes from the spawn position
/// (outside the F2P map counts as members-only).
#[derive(Debug, Clone)]
pub struct Bank {
    pub x: i64,
    pub z: i64,
    pub level: i64,
    pub loc_id: usize,
    pub name: String,
    pub members: bool,
}

/// Every bank booth spawn.
pub fn all_banks() -> Vec<Bank> {
    let mut found = Vec::new();
    for entry in all_locs() {
        if entry.name.unwrap_or("").to_lowercase() != "bankbooth" {
            continue;
        }
        for spot in entry.spawns {
            found.push(Bank {
                x: spot.x,
                z: spot.z,
                level: spot.level,
                loc_id: entry.loc_id,
                name: entry.name.unwrap_or("").to_string(),
                members: spot.members.unwrap_or(false),
            });
        }
    }
    found
}

/// Closest booth on the same level (Manhattan distance), or None.
pub fn nearest_bank(x: i64, z: i64, level: i64) -> Option<Bank> {
    let mut best: Option<(i64, Bank)> = None;
    for bank in all_banks() {
        if bank.level != level {
            continue;
        }
        let dist = (bank.x - x).abs() + (bank.z - z).abs();
        if best.as_ref().is_none_or(|current| dist < current.0) {
            best = Some((dist, bank));
        }
    }
    best.map(|(_, bank)| bank)
}

#[cfg(test)]
mod tests {
    use super::{all_banks, nearest_bank};

    #[test]
    fn booth_count_and_nearest() {
        // len(all_banks()) == 68,
        // nearest_bank(3200, 3200) is the Varrock west booth.
        assert_eq!(all_banks().len(), 68);
        let near = nearest_bank(3200, 3200, 0).expect("a bank");
        assert_eq!((near.x, near.z), (3268, 3169));
        assert_eq!(near.loc_id, 2213);
        assert!(!near.members);
    }

    #[test]
    fn members_booth_derived_from_position() {
        // Catherby-side booth stands outside the F2P map.
        let found = all_banks().into_iter().find(|bank| bank.x == 2721 && bank.z == 3494).expect("catherby booth");
        assert!(found.members);
    }
}
