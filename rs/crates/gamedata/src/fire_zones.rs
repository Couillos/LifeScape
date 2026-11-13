//! Zones where the server refuses a new fire (`bank_zones` table,
//! `content/scripts/skill_firemaking/configs/bank_zones.dbrow`, read by
//! `~area_allow_loc_add` in `firemaking.rs2`: "You can't light a fire
//! here."). Bank interiors only; the other refusals (blocked tile, an
//! interactable loc on the tile) are map facts the caller checks live.
//! Bounds are inclusive like the engine `inzone` (x, then level, then z).

#![forbid(unsafe_code)]

/// One inclusive box of the `bank_zones` table, levels included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FireZone {
    pub min_level: i32,
    pub max_level: i32,
    pub min_x: i32,
    pub max_x: i32,
    pub min_z: i32,
    pub max_z: i32,
}

/// The rev 274 table, decoded from `level_mx_mz_lx_lz` coord pairs
/// (x = mx*64+lx, z = mz*64+lz), in file order. A test re-reads the
/// server file when the checkout sits next to LifeScape.
pub const BANK_ZONES: [FireZone; 33] = [
    FireZone { min_level: 0, max_level: 0, min_x: 3253, max_x: 3253, min_z: 3425, max_z: 3427 },
    FireZone { min_level: 0, max_level: 1, min_x: 3250, max_x: 3257, min_z: 3416, max_z: 3424 },
    FireZone { min_level: 0, max_level: 1, min_x: 3180, max_x: 3190, min_z: 3433, max_z: 3447 },
    FireZone { min_level: 0, max_level: 0, min_x: 3265, max_x: 3272, min_z: 3161, max_z: 3173 },
    FireZone { min_level: 0, max_level: 0, min_x: 3088, max_x: 3097, min_z: 3240, max_z: 3246 },
    FireZone { min_level: 0, max_level: 0, min_x: 3011, max_x: 3014, min_z: 3357, max_z: 3358 },
    FireZone { min_level: 0, max_level: 0, min_x: 3009, max_x: 3021, min_z: 3353, max_z: 3356 },
    FireZone { min_level: 0, max_level: 0, min_x: 2945, max_x: 2947, min_z: 3366, max_z: 3373 },
    FireZone { min_level: 0, max_level: 0, min_x: 2948, max_x: 2949, min_z: 3366, max_z: 3369 },
    FireZone { min_level: 0, max_level: 0, min_x: 2946, max_x: 2948, min_z: 3362, max_z: 3365 },
    FireZone { min_level: 0, max_level: 0, min_x: 2945, max_x: 2948, min_z: 3359, max_z: 3361 },
    FireZone { min_level: 1, max_level: 1, min_x: 2945, max_x: 2949, min_z: 3362, max_z: 3369 },
    FireZone { min_level: 0, max_level: 0, min_x: 3091, max_x: 3098, min_z: 3488, max_z: 3499 },
    FireZone { min_level: 0, max_level: 1, min_x: 2721, max_x: 2730, min_z: 3490, max_z: 3497 },
    FireZone { min_level: 0, max_level: 0, min_x: 2724, max_x: 2727, min_z: 3487, max_z: 3489 },
    FireZone { min_level: 0, max_level: 0, min_x: 2806, max_x: 2812, min_z: 3438, max_z: 3445 },
    FireZone { min_level: 0, max_level: 0, min_x: 2609, max_x: 2616, min_z: 3088, max_z: 3097 },
    FireZone { min_level: 0, max_level: 0, min_x: 2612, max_x: 2614, min_z: 3335, max_z: 3335 },
    FireZone { min_level: 0, max_level: 0, min_x: 2619, max_x: 2621, min_z: 3335, max_z: 3335 },
    FireZone { min_level: 0, max_level: 0, min_x: 2612, max_x: 2621, min_z: 3330, max_z: 3334 },
    FireZone { min_level: 0, max_level: 2, min_x: 2649, max_x: 2658, min_z: 3280, max_z: 3287 },
    FireZone { min_level: 0, max_level: 0, min_x: 2843, max_x: 2845, min_z: 2952, max_z: 2955 },
    FireZone { min_level: 0, max_level: 0, min_x: 2846, max_x: 2858, min_z: 2953, max_z: 2955 },
    FireZone { min_level: 0, max_level: 0, min_x: 2859, max_x: 2861, min_z: 2952, max_z: 2955 },
    FireZone { min_level: 0, max_level: 0, min_x: 2850, max_x: 2854, min_z: 2951, max_z: 2957 },
    FireZone { min_level: 1, max_level: 1, min_x: 2843, max_x: 2861, min_z: 2951, max_z: 2957 },
    FireZone { min_level: 1, max_level: 1, min_x: 2443, max_x: 2448, min_z: 3415, max_z: 3434 },
    FireZone { min_level: 1, max_level: 1, min_x: 2440, max_x: 2453, min_z: 3416, max_z: 3496 },
    FireZone { min_level: 0, max_level: 0, min_x: 2529, max_x: 2546, min_z: 4711, max_z: 4723 },
    FireZone { min_level: 0, max_level: 0, min_x: 3308, max_x: 3309, min_z: 3119, max_z: 3121 },
    FireZone { min_level: 0, max_level: 0, min_x: 2586, max_x: 2587, min_z: 3418, max_z: 3422 },
    FireZone { min_level: 0, max_level: 0, min_x: 3148, max_x: 3155, min_z: 9573, max_z: 9582 },
    FireZone { min_level: 0, max_level: 0, min_x: 3380, max_x: 3384, min_z: 3267, max_z: 3271 },
];

impl FireZone {
    /// Engine `inzone`: inclusive on every axis.
    pub fn contains(&self, x: i32, z: i32, level: i32) -> bool {
        (self.min_x..=self.max_x).contains(&x) && (self.min_level..=self.max_level).contains(&level) && (self.min_z..=self.max_z).contains(&z)
    }
}

/// True when the server refuses a fire on this tile for being in a bank.
pub fn in_bank_zone(x: i32, z: i32, level: i32) -> bool {
    BANK_ZONES.iter().any(|zone| zone.contains(x, z, level))
}

#[cfg(test)]
mod tests {
    use super::{in_bank_zone, FireZone, BANK_ZONES};

    fn decode(raw: &str) -> (i32, i32, i32) {
        let parts: Vec<i32> = raw.split('_').map(|part| part.parse().expect("coord part")).collect();
        (parts[0], parts[1] * 64 + parts[3], parts[2] * 64 + parts[4])
    }

    /// The table matches the server file (when the checkout is next to
    /// LifeScape: the worktree layout may hide it, then only the
    /// hand-checked anchors below run).
    #[test]
    fn table_matches_the_server_dbrow() {
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let Some(file) = manifest
            .ancestors()
            .map(|dir| dir.join("lostcity/content/scripts/skill_firemaking/configs/bank_zones.dbrow"))
            .find(|candidate| candidate.is_file())
        else {
            return;
        };
        let text = std::fs::read_to_string(file).expect("dbrow reads");
        let decoded: Vec<FireZone> = text
            .lines()
            .filter_map(|line| line.trim().strip_prefix("data=coord_pair,"))
            .map(|pair| {
                let (low, high) = pair.split_once(',').expect("two coords");
                let ((min_level, min_x, min_z), (max_level, max_x, max_z)) = (decode(low.trim()), decode(high.trim()));
                FireZone { min_level, max_level, min_x, max_x, min_z, max_z }
            })
            .collect();
        assert_eq!(decoded, BANK_ZONES.to_vec());
    }

    /// Live refusal 2026-11-13: "You can't light a fire here." at the
    /// Draynor bank (3091-3097, 3242-3245); the square outside is free.
    #[test]
    fn draynor_bank_refuses_fire_outside_does_not() {
        assert!(in_bank_zone(3092, 3243, 0));
        assert!(in_bank_zone(3097, 3245, 0));
        assert!(!in_bank_zone(3092, 3248, 0), "north of the bank");
        assert!(!in_bank_zone(3092, 3243, 1), "level 0 box only");
        assert!(in_bank_zone(3254, 3420, 1), "Varrock east spans two levels");
    }
}
