//! Al Kharid toll fence (server `areas/area_alkharid/scripts/border_gate.rs2`,
//! rev 274): the border gate at `0_51_50_4_27` (3268, 3227) charges
//! 10 coins from the INVENTORY, both ways (`coordx(coord) < coordx(loc_coord)`
//! picks the Lumbridge or Al Kharid guard, same dialog), free once Prince
//! Ali Rescue reached `^prince_saved` (`%princequest >= ^prince_saved`).
//!
//! Which side of the fence a tile stands on is a region, never a line
//! (2026-11-14, live bd23dqpq: the south coast fishing spot (3267,3148)
//! read "west of x = 3268", launched without `toll`, NoRoute, fishing
//! banned). The fence zigzags (x 3266 at z 3240, x 3270 near its top)
//! and the Al Kharid shore reaches x 3259 in the south.
//! `data/toll_region.json` holds the Al Kharid side, derived from the
//! served map's connectivity (`lifescape_nav::toll_side::derive_region`:
//! walk flood, ordinary doors open, the toll gate shut; a water, rock or
//! tree tile takes the side of its nearest walkable tiles) and checked
//! against it by the golden test `nav/tests/toll_region.rs`. Its window
//! stops at the fence's northern end (z 3329): above it both sides meet on
//! foot (the free detour). Three classes follow: behind the toll (the
//! region), the toll-free side (the rest of the window: Lumbridge,
//! Draynor, Port Sarim, Rimmington south of z 3330) and open ground
//! outside the window (Varrock, Falador, the north), never across.
//!
//! The crossing test mirrors the scripts' (`kit::bank::toll_gate_x` +
//! `kit::bans::across_toll`): within `FENCE_REACH` tiles (Manhattan) of the
//! gate, a target behind the toll from a bot on the toll-free side (or
//! the reverse) is "across" — the scripts refuse it unless the run
//! carries `toll`.

use std::sync::OnceLock;

const RAW: &str = include_str!("../../../../gamedata/data/toll_region.json");

/// Gate tile (x, z, level), the Al Kharid side of the gate.
pub const GATE: (i64, i64, i64) = (3268, 3227, 0);
/// Coins the guard takes per crossing (`inv_del(inv, coins, 10)`).
pub const FARE: i64 = crate::melee_ladder::TOLL_COINS as i64;
/// Radius the scripts scan for the gate around the bot (`toll_gate_x`).
pub const FENCE_REACH: i64 = 150;
/// Quest key whose completion makes the crossing free.
pub const FREE_PASS_QUEST: &str = "prince";

/// The Al Kharid side, row by row: `spans[z - min_z]` lists inclusive
/// x spans; `window` (min_x, min_z, max_x, max_z) bounds both sides.
struct Region {
    window: (i64, i64, i64, i64),
    min_z: i64,
    spans: Vec<Vec<(i64, i64)>>,
}

fn region() -> &'static Region {
    static REGION: OnceLock<Region> = OnceLock::new();
    REGION.get_or_init(|| parse_region(RAW).unwrap_or_else(|error| panic!("toll_region.json: {error}")))
}

fn parse_region(raw: &str) -> Result<Region, String> {
    let value = lifescape_json::parse(raw)?;
    let window: Vec<i64> = value.get("window").as_arr().ok_or("window")?.iter().filter_map(|cell| cell.as_i64()).collect();
    let (min_x, min_z, max_x, max_z) = match window.as_slice() {
        [min_x, min_z, max_x, max_z] => (*min_x, *min_z, *max_x, *max_z),
        _ => return Err("window needs [min_x, min_z, max_x, max_z]".to_string()),
    };
    let mut spans = vec![Vec::new(); usize::try_from(max_z - min_z + 1).map_err(|_| "window rows")?];
    for row in value.get("rows").as_arr().ok_or("rows")? {
        let cells: Vec<i64> = row.as_arr().ok_or("row")?.iter().filter_map(|cell| cell.as_i64()).collect();
        let Some((row_z, bounds)) = cells.split_first() else { return Err("empty row".to_string()) };
        if bounds.len() % 2 != 0 || !(min_z..=max_z).contains(row_z) {
            return Err(format!("row {row_z} malformed"));
        }
        let slot = &mut spans[(row_z - min_z) as usize];
        for pair in bounds.chunks(2) {
            slot.push((pair[0], pair[1]));
        }
    }
    Ok(Region { window: (min_x, min_z, max_x, max_z), min_z, spans })
}

/// True when the tile stands behind the toll (the Al Kharid side). An
/// upper floor (levels 1-3) stands over its surface tile; underground
/// (z ≥ 6400) and every tile outside the region is toll-free.
pub fn behind(x: i64, z: i64, level: i64) -> bool {
    if !(0..=3).contains(&level) {
        return false;
    }
    let region = region();
    let Some(row) = usize::try_from(z - region.min_z).ok().and_then(|index| region.spans.get(index)) else { return false };
    row.iter().any(|(start, end)| (*start..=*end).contains(&x))
}

/// True when the tile stands on the toll-free side of the fence: inside
/// the region's window (south of the fence top) but not behind it.
pub fn toll_free_side(x: i64, z: i64, level: i64) -> bool {
    let (min_x, min_z, max_x, max_z) = region().window;
    (0..=3).contains(&level) && (min_x..=max_x).contains(&x) && (min_z..=max_z).contains(&z) && !behind(x, z, level)
}

/// True when the toll fence stands between the two surface tiles (x, z):
/// one behind it, the other on its toll-free side. Open ground (north of
/// the fence top, far west or east) is never across.
pub fn across(first: (i64, i64), second: (i64, i64)) -> bool {
    (behind(first.0, first.1, 0) && toll_free_side(second.0, second.1, 0)) || (toll_free_side(first.0, first.1, 0) && behind(second.0, second.1, 0))
}

/// True when no toll fence stands between the two tiles (`!across`).
pub fn same_side(first: (i64, i64), second: (i64, i64)) -> bool {
    !across(first, second)
}

/// True when the bot at `from` must cross the toll fence to reach `to`
/// (same rule the scripts apply live): the bot stands within reach of
/// the gate, the target on the surface level, the fence between them
/// (`across`). An upper floor (levels 1-3) stands over its surface tile: a
/// bot on the Lumbridge castle floors walks down and crosses like one on
/// the ground (never "not crossing" because it stands upstairs).
pub fn crossing(from: (i64, i64, i64), to: (i64, i64, i64)) -> bool {
    let (gate_x, gate_z, gate_level) = GATE;
    if !(gate_level..=3).contains(&from.2) || to.2 != gate_level {
        return false;
    }
    if (from.0 - gate_x).abs() + (from.1 - gate_z).abs() > FENCE_REACH {
        return false;
    }
    across((from.0, from.1), (to.0, to.1))
}

#[cfg(test)]
mod tests {
    use super::{across, behind, crossing, parse_region, same_side, toll_free_side};

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
        assert!(crossing((3212, 3223, 1), (3294, 3287, 0)), "the castle's first floor crosses like its ground floor");
    }

    /// Live bd23dqpq 2026-11-14: the south coast fishing spot (3267,3148),
    /// west of x = 3268, sits behind the toll with its shore; so does the
    /// fence's zigzag (x 3266 at z 3240). The gate's west tile, the fence
    /// top's west side and everything north of the fence are toll-free.
    #[test]
    fn the_region_follows_the_fence_not_a_line() {
        let lumbridge = (3222, 3218, 0);
        assert!(behind(3267, 3148, 0) && behind(3267, 3149, 0), "south coast spot and shore");
        assert!(crossing(lumbridge, (3267, 3148, 0)), "Lumbridge to the coast spot pays the toll");
        assert!(crossing((3218, 3238, 0), (3267, 3148, 0)), "bd23dqpq's launch tile crosses too");
        assert!(behind(3266, 3240, 0), "the fence zigzags west of 3268");
        assert!(behind(3268, 3227, 0) && !behind(3267, 3227, 0), "the gate splits its two tiles");
        assert!(!behind(3268, 3329, 0), "west of the fence top (it runs at x 3270 there)");
        assert!(!behind(3290, 3340, 0), "north of the fence: the free detour");
        assert!(behind(3293, 3170, 2), "a palace floor stands over its ground");
        assert!(!behind(3293, 9570, 0), "underground is never behind");
        assert!(same_side((3269, 3167), (3267, 3148)), "the bank and the coast spot together");
        assert!(!same_side((3222, 3218), (3267, 3148)), "Lumbridge and the coast spot apart");
        assert!(toll_free_side(3092, 3243, 0), "Draynor is on the toll-free side");
        assert!(across((3275, 3227), (3092, 3243)), "Al Kharid to Draynor crosses");
        assert!(!toll_free_side(3270, 3470, 0) && !behind(3270, 3470, 0), "north of Varrock: open ground");
        assert!(same_side((3275, 3227), (3270, 3470)), "Al Kharid to the north walks the free detour");
        assert!(same_side((3222, 3218), (3270, 3470)), "Lumbridge to the north too");
    }

    #[test]
    fn malformed_regions_fail_loud() {
        assert!(parse_region("{\"window\": [0, 0, 9], \"rows\": []}").is_err(), "short window");
        assert!(parse_region("{\"window\": [0, 0, 9, 9], \"rows\": [[3, 1]]}").is_err(), "odd span");
        assert!(parse_region("{\"window\": [0, 0, 9, 9], \"rows\": [[12, 1, 2]]}").is_err(), "row outside the window");
        let region = parse_region("{\"window\": [0, 5, 9, 9], \"rows\": [[6, 1, 2, 4, 4]]}").expect("valid");
        assert_eq!((region.min_z, region.spans[1].clone()), (5, vec![(1, 2), (4, 4)]));
    }
}
