//! Woodcutting table: tree loc names, level reqs, log products, XP
//! (XP is raw tenths).

use crate::{num, opt_flag, opt_str, str_list, table};

/// One woodcutting tier. `product_members` is the product obj flag:
/// tree locs themselves carry no server members flag.
#[derive(Debug, Clone)]
pub struct TreeTier {
    pub trees: Vec<String>,
    pub level: i64,
    pub product: String,
    pub exp: i64,
    pub product_members: Option<bool>,
}

/// Every tree tier.
pub fn all_tiers() -> Vec<TreeTier> {
    table("trees")
        .iter()
        .filter(|row| !row.is_null())
        .map(|row| TreeTier {
            trees: str_list(row, "trees"),
            level: num(row, "level", 0),
            product: opt_str(row, "product").unwrap_or_default(),
            exp: num(row, "exp", 0),
            product_members: opt_flag(row, "product_members"),
        })
        .collect()
}

/// Highest F2P tier whose level requirement is reached, or None.
/// Members tiers never qualify (same rule as the spot picker).
pub fn tier_for_level(woodcutting_level: i64) -> Option<TreeTier> {
    let mut best: Option<TreeTier> = None;
    for tier in all_tiers() {
        if tier.product_members == Some(true) {
            continue;
        }
        if tier.level <= woodcutting_level && best.as_ref().is_none_or(|current| tier.level > current.level) {
            best = Some(tier);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::{all_tiers, tier_for_level};

    #[test]
    fn tree_vectors() {
        // 6 tiers; level 0 cuts logs, level 30 willow_logs
        // (675 xp, product is F2P).
        assert_eq!(all_tiers().len(), 6);
        assert_eq!(tier_for_level(0).expect("tier").product, "logs");
        let willow = tier_for_level(30).expect("willow");
        assert_eq!(willow.product, "willow_logs");
        assert_eq!(willow.exp, 675);
        assert_eq!(willow.product_members, Some(false));
        assert_eq!(tier_for_level(99).expect("capped at yew").product, "yew_logs");
        assert!(tier_for_level(-1).is_none());
    }
}
