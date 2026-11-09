//! Fishing table: spot categories, level reqs, tools, bait, products, XP
//! (F2P rows only; XP is raw tenths).

use crate::npcs;

/// One fish tier: category matches the tail of the spot NPC debugnames.
#[derive(Debug, Clone, Copy)]
pub struct FishTier {
    pub category: &'static str,
    pub op: i64,
    pub action: &'static str,
    pub level: i64,
    pub tool: &'static str,
    pub bait: Option<&'static str>,
    pub products: &'static [&'static str],
    pub second_level: i64,
    pub exp: &'static [i64],
    pub product_members: bool,
}

const TIERS: [FishTier; 6] = [
    FishTier {
        category: "saltfish",
        op: 1,
        action: "Net",
        level: 1,
        tool: "net",
        bait: None,
        products: &["raw_shrimp", "raw_anchovies"],
        second_level: 15,
        exp: &[100, 400],
        product_members: false,
    },
    FishTier {
        category: "saltfish",
        op: 3,
        action: "Bait",
        level: 5,
        tool: "fishing_rod",
        bait: Some("fishing_bait"),
        products: &["raw_sardine", "raw_herring"],
        second_level: 10,
        exp: &[200, 300],
        product_members: false,
    },
    FishTier {
        category: "freshfish",
        op: 1,
        action: "Lure",
        level: 20,
        tool: "fly_fishing_rod",
        bait: Some("feather"),
        products: &["raw_trout", "raw_salmon"],
        second_level: 30,
        exp: &[500, 700],
        product_members: false,
    },
    FishTier {
        category: "freshfish",
        op: 3,
        action: "Bait",
        level: 25,
        tool: "fishing_rod",
        bait: Some("fishing_bait"),
        products: &["raw_pike"],
        second_level: 0,
        exp: &[600],
        product_members: false,
    },
    FishTier {
        category: "rarefish",
        op: 1,
        action: "Cage",
        level: 40,
        tool: "lobster_pot",
        bait: None,
        products: &["raw_lobster"],
        second_level: 0,
        exp: &[900],
        product_members: false,
    },
    FishTier {
        category: "rarefish",
        op: 3,
        action: "Harpoon",
        level: 35,
        tool: "harpoon",
        bait: None,
        products: &["raw_tuna", "raw_swordfish"],
        second_level: 50,
        exp: &[800, 1000],
        product_members: false,
    },
];

/// Every F2P fish tier.
pub fn all_fish_tiers() -> Vec<FishTier> {
    TIERS.to_vec()
}

/// Tiers whose level requirement is reached.
pub fn affordable_tiers(fishing_level: i64) -> Vec<FishTier> {
    TIERS.iter().filter(|tier| tier.level <= fishing_level).copied().collect()
}

/// Highest reachable tier, or None below level 1.
pub fn best_fish_tier(fishing_level: i64) -> Option<FishTier> {
    let mut best: Option<FishTier> = None;
    for tier in TIERS {
        if tier.level <= fishing_level && best.is_none_or(|current| tier.level > current.level) {
            best = Some(tier);
        }
    }
    best
}

/// NPC ids whose debugname ends with `_<category>`.
pub fn spot_npc_ids(category: &str) -> Vec<usize> {
    let tail = format!("_{category}");
    npcs::ALL
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.name.is_some_and(|name| name.ends_with(tail.as_str())))
        .map(|(npc_id, _)| npc_id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{affordable_tiers, best_fish_tier, spot_npc_ids};

    #[test]
    fn tier_progression() {
        // best(1) saltfish op 1, best(40) rarefish cage,
        // affordable(25) has 4 tiers, saltfish spots start at 316.
        let first = best_fish_tier(1).expect("tier");
        assert_eq!((first.category, first.op), ("saltfish", 1));
        let top = best_fish_tier(40).expect("tier");
        assert_eq!((top.category, top.tool), ("rarefish", "lobster_pot"));
        assert_eq!(affordable_tiers(25).len(), 4);
        assert!(best_fish_tier(0).is_none());
        let spots = spot_npc_ids("saltfish");
        assert_eq!(&spots[..3], &[316, 319, 320]);
    }
}
