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

/// Closest F2P booth to `(x, z, level)`: same level first, else any
/// level (Manhattan), reached through the stairs and ladders the router
/// rides. No F2P bank stands above the ground in rev 274, so a bot on an
/// upper floor (the Lumbridge mill top, the castle's first floor) banks on
/// the surface instead of finding none (live 2026-11-14: four bots
/// stranded at (3165,3306,2), "no reachable bank").
pub fn nearest_free_bank(x: i64, z: i64, level: i64) -> Option<Bank> {
    all_banks()
        .into_iter()
        .filter(|bank| !bank.members)
        .min_by_key(|bank| (bank.level != level, (bank.x - x).abs() + (bank.z - z).abs()))
}

#[cfg(test)]
mod tests {
    use super::{all_banks, nearest_bank, nearest_free_bank};

    /// Upper floors bank on the surface: the mill top (level 2) and the
    /// Lumbridge spinning wheel floor (level 1) get a free ground bank,
    /// never a members booth that happens to share the level.
    #[test]
    fn upper_floor_banks_on_the_surface() {
        for (x, z, level) in [(3165, 3306, 2), (3209, 3212, 1)] {
            let bank = nearest_free_bank(x, z, level).expect("a free bank");
            assert!(!bank.members && bank.level == 0, "({x},{z},{level}) -> {bank:?}");
        }
        let ground = nearest_free_bank(3092, 3245, 0).expect("Draynor");
        assert!(!ground.members && ground.level == 0 && (ground.x - 3092).abs() + (ground.z - 3245).abs() <= 4, "same level, nearest: {ground:?}");
    }

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
