//! Mining table: rock loc names, level reqs, ore products, XP
//! (XP is raw tenths).

/// One static mining tier (build-time data, mirrored into [`RockTier`] per call).
pub(crate) struct StaticRockTier {
    pub rocks: &'static [&'static str],
    pub level: i64,
    pub product: &'static str,
    pub exp: i64,
    pub ore_name: &'static str,
    pub product_members: Option<bool>,
}

mod generated {
    include!(concat!(env!("OUT_DIR"), "/rocks_generated.rs"));
}

/// One mining tier.
#[derive(Debug, Clone)]
pub struct RockTier {
    pub rocks: Vec<String>,
    pub level: i64,
    pub product: String,
    pub exp: i64,
    pub ore_name: String,
    pub product_members: Option<bool>,
}

fn tier_from_static(row: &StaticRockTier) -> RockTier {
    RockTier {
        rocks: row.rocks.iter().map(|entry| (*entry).to_string()).collect(),
        level: row.level,
        product: row.product.to_string(),
        exp: row.exp,
        ore_name: row.ore_name.to_string(),
        product_members: row.product_members,
    }
}

/// Every rock tier.
pub fn all_rock_tiers() -> Vec<RockTier> {
    generated::ALL.iter().map(tier_from_static).collect()
}

/// Highest tier whose level requirement is reached, or None.
pub fn rock_tier_for_level(mining_level: i64) -> Option<RockTier> {
    let mut best: Option<RockTier> = None;
    for tier in all_rock_tiers() {
        if tier.level <= mining_level && best.as_ref().is_none_or(|current| tier.level > current.level) {
            best = Some(tier);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::{all_rock_tiers, rock_tier_for_level};

    #[test]
    fn rock_vectors() {
        // 17 tiers; level 1 mines blankrune, level 30 coal
        // (500 xp, product is F2P).
        assert_eq!(all_rock_tiers().len(), 17);
        assert_eq!(rock_tier_for_level(1).expect("tier").product, "blankrune");
        let coal = rock_tier_for_level(30).expect("coal");
        assert_eq!(coal.product, "coal");
        assert_eq!(coal.exp, 500);
        assert_eq!(coal.product_members, Some(false));
        assert!(rock_tier_for_level(-1).is_none());
    }
}
