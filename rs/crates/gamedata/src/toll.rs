//! Al Kharid toll fence (server `areas/area_alkharid/scripts/border_gate.rs2`,
//! rev 274): the border gate at `0_51_50_4_27` (3268, 3227) charges
//! 10 coins from the INVENTORY, both ways (`coordx(coord) < coordx(loc_coord)`
//! picks the Lumbridge or Al Kharid guard, same dialog), free once Prince
//! Ali Rescue reached `^prince_saved` (`%princequest >= ^prince_saved`).
//!
//! The fence test mirrors the scripts' (`kit::bank::toll_gate_x` +
//! `kit::bans::across_toll`): within `FENCE_REACH` tiles (Manhattan) of the
//! gate, a target whose x sits on the other side of the gate line is
//! "across" — the scripts refuse it unless the run carries `toll`.

/// Gate tile (x, z, level): its x is the fence line.
pub const GATE: (i64, i64, i64) = (3268, 3227, 0);
/// Coins the guard takes per crossing (`inv_del(inv, coins, 10)`).
pub const FARE: i64 = crate::melee_ladder::TOLL_COINS as i64;
/// Radius the scripts scan for the gate around the bot (`toll_gate_x`).
pub const FENCE_REACH: i64 = 150;
/// Quest key whose completion makes the crossing free.
pub const FREE_PASS_QUEST: &str = "prince";

/// True when the bot at `from` must cross the toll fence to reach `to`
/// (same rule the scripts apply live): the bot stands within reach of
/// the gate, both tiles on the surface level, on opposite sides of x = 3268.
pub fn crossing(from: (i64, i64, i64), to: (i64, i64, i64)) -> bool {
    let (gate_x, gate_z, gate_level) = GATE;
    if from.2 != gate_level || to.2 != gate_level {
        return false;
    }
    if (from.0 - gate_x).abs() + (from.1 - gate_z).abs() > FENCE_REACH {
        return false;
    }
    (from.0 >= gate_x) != (to.0 >= gate_x)
}

#[cfg(test)]
mod tests {
    use super::crossing;

    /// Lumbridge to the Al Kharid mine crosses; Rimmington does not; an
    /// Al Kharid bot crosses back west to the Lumbridge cows; a Varrock
    /// bot is out of the gate's reach (the scripts see no fence there).
    #[test]
    fn the_fence_splits_lumbridge_from_al_kharid() {
        let lumbridge = (3232, 3203, 0);
        assert!(crossing(lumbridge, (3294, 3287, 0)), "Al Kharid gold is across");
        assert!(!crossing(lumbridge, (2975, 3234, 0)), "Rimmington gold is on this side");
        assert!(crossing((3290, 3180, 0), (3255, 3275, 0)), "Al Kharid to the cows crosses back");
        assert!(!crossing((3210, 3420, 0), (3294, 3287, 0)), "Varrock is out of the gate's reach");
        assert!(!crossing(lumbridge, (3294, 9687, 1)), "never across levels");
    }
}
