//! Experience table: server XP per level (levels 1-99).
//!
//! Units are server XP, ten times the display value (`Player.ts`
//! multiplies by 10 so fractional gains survive): level 2 starts at 830,
//! level 99 at 130344310. Formula, kept as the test oracle:
//! `acc += floor(level + 2^(level/7) * 300)`, `table = floor(acc/4) * 10`.

/// XP needed to reach each level; index *is* the level (`TABLE[1]` is 0).
/// Level 0 is unused padding so callers index directly.
pub const TABLE: [i64; 100] = [
    0,
    0, 830, 1740, 2760, 3880, 5120, 6500, 8010, 9690, 11540,
    13580, 15840, 18330, 21070, 24110, 27460, 31150, 35230, 39730, 44700,
    50180, 56240, 62910, 70280, 78420, 87400, 97300, 108240, 120310, 133630,
    148330, 164560, 182470, 202240, 224060, 248150, 274730, 304080, 336480, 372240,
    411710, 455290, 503390, 556490, 615120, 679830, 751270, 830140, 917210, 1013330,
    1119450, 1236600, 1365940, 1508720, 1666360, 1840400, 2032540, 2244660, 2478860, 2737420,
    3022880, 3338040, 3685990, 4070150, 4494280, 4962540, 5479530, 6050320, 6680510, 7376270,
    8144450, 8992570, 9928950, 10962780, 12104210, 13364430, 14755810, 16292000, 17988080, 19860680,
    21928180, 24210870, 26731140, 29513730, 32585940, 35977920, 39722940, 43857760, 48422950, 53463320,
    59028310, 65172530, 71956290, 79446140, 87715580, 96845770, 106926290, 118056060, 130344310,
];

/// XP needed to reach `level` (1-99), or None outside that range.
pub fn xp_for_level(level: u32) -> Option<i64> {
    if (1..=99).contains(&level) {
        Some(TABLE[level as usize])
    } else {
        None
    }
}

/// Highest level whose XP requirement `xp` meets (1-99, server units).
pub fn level_for_xp(xp: i64) -> u32 {
    let mut level = 1;
    for candidate in 2..=99u32 {
        if xp >= TABLE[candidate as usize] {
            level = candidate;
        } else {
            break;
        }
    }
    level
}

#[cfg(test)]
mod tests {
    use super::{level_for_xp, xp_for_level, TABLE};

    /// Raw server formula, mirrored from `Player.ts` (`Math.pow` is f64,
    /// like `f64::powf` here, so the oracle matches bit for bit).
    fn oracle(level: u32) -> i64 {
        let mut acc = 0i64;
        for completed in 1..=level {
            let delta = (completed as f64 + (2f64).powf(completed as f64 / 7.0) * 300.0).floor() as i64;
            acc += delta;
        }
        (acc / 4) * 10
    }

    #[test]
    fn xp_table_matches_the_server() {
        // Anchors (server units, 10x display): level 2 at 830,
        // level 99 at 130344310; level 1 needs nothing.
        assert_eq!(TABLE.len(), 100);
        assert_eq!(TABLE[1], 0);
        assert_eq!(xp_for_level(1), Some(0));
        assert_eq!(xp_for_level(2), Some(830));
        assert_eq!(xp_for_level(99), Some(130344310));
        assert_eq!(xp_for_level(0), None);
        assert_eq!(xp_for_level(100), None);
        for level in 2..=99u32 {
            assert_eq!(TABLE[level as usize], oracle(level - 1), "level {level}");
        }
        assert!(TABLE[1..100].windows(2).all(|pair| pair[0] < pair[1]), "strictly rising");
        // Boundaries: exact XP levels up, one less does not.
        assert_eq!(level_for_xp(0), 1);
        assert_eq!(level_for_xp(829), 1);
        assert_eq!(level_for_xp(830), 2);
        assert_eq!(level_for_xp(130344309), 98);
        assert_eq!(level_for_xp(130344310), 99);
        assert_eq!(level_for_xp(999_999_999), 99);
    }
}
