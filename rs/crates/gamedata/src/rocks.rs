//! Mining table: rock loc names, level reqs, ore products, XP
//! (XP is raw tenths).

use crate::{num, opt_flag, opt_str, str_list, table};

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

/// Every rock tier.
pub fn all_rock_tiers() -> Vec<RockTier> {
    table("rocks")
        .iter()
        .filter(|row| **row != crate::Json::Null)
        .map(|row| RockTier {
            rocks: str_list(row, "rocks"),
            level: num(row, "level", 0),
            product: opt_str(row, "product").unwrap_or_default(),
            exp: num(row, "exp", 0),
            ore_name: opt_str(row, "ore_name").unwrap_or_default(),
            product_members: opt_flag(row, "product_members"),
        })
        .collect()
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
