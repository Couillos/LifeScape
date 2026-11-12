//! Wearable stats and level requirements.

/// One static wearable row (build-time data, mirrored into [`Wearable`] per call).
pub(crate) struct StaticWearable {
    pub name: &'static str,
    pub wearpos: i64,
    pub wearpos2: i64,
    pub wearpos3: i64,
    pub category: Option<&'static str>,
    pub attack: &'static [(&'static str, i64)],
    pub defence: &'static [(&'static str, i64)],
    pub strength: i64,
    pub prayer: i64,
    pub requirements: &'static [(&'static str, i64)],
    pub quest: Option<&'static str>,
    pub tier: i64,
    pub rangebonus: i64,
}

mod generated {
    include!(concat!(env!("OUT_DIR"), "/equipment_generated.rs"));
}

/// Server wearpos slots: 0 head, 2 amulet, 3 right hand, 4 body,
/// 5 left hand, 7 legs, 9 hands, 10 feet, 12 ring.
#[derive(Debug, Clone)]
pub struct Wearable {
    pub name: String,
    pub wearpos: i64,
    pub wearpos2: i64,
    pub wearpos3: i64,
    pub category: Option<String>,
    pub attack: Vec<(String, i64)>,
    pub defence: Vec<(String, i64)>,
    pub strength: i64,
    pub prayer: i64,
    pub requirements: Vec<(String, i64)>,
    /// Quest directory the wield check needs first (`quest_dragon` for
    /// rune platebodies), from the levelrequire labels.
    pub quest: Option<String>,
    /// `levelrequire` param: bow/arrow tier (ammo rule) or tool level
    /// (pickaxe mining level); -1 when absent.
    pub tier: i64,
    /// Arrow ranged strength (`rangebonus`); -1 when absent.
    pub rangebonus: i64,
}

/// Level-gated stats, mirroring `equipment.STATS`.
pub const STATS: [(&str, usize); 7] = [
    ("attack", 0),
    ("defence", 1),
    ("strength", 2),
    ("hitpoints", 3),
    ("ranged", 4),
    ("prayer", 5),
    ("magic", 6),
];

/// Index of a level-gated stat, or None for unknown stats.
pub fn stat_index(stat: &str) -> Option<usize> {
    STATS.iter().find(|(name, _)| *name == stat).map(|(_, index)| *index)
}

fn pair_list(pairs: &[(&'static str, i64)]) -> Vec<(String, i64)> {
    pairs.iter().map(|(name, value)| ((*name).to_string(), *value)).collect()
}

fn wearable_from_static(row: &StaticWearable) -> Wearable {
    Wearable {
        name: row.name.to_string(),
        wearpos: row.wearpos,
        wearpos2: row.wearpos2,
        wearpos3: row.wearpos3,
        category: row.category.map(str::to_string),
        attack: pair_list(row.attack),
        defence: pair_list(row.defence),
        strength: row.strength,
        prayer: row.prayer,
        requirements: pair_list(row.requirements),
        quest: row.quest.map(str::to_string),
        tier: row.tier,
        rangebonus: row.rangebonus,
    }
}

/// Every wearable, debugname order (the dump object already iterates sorted).
pub fn all_wearables() -> Vec<Wearable> {
    generated::ALL.iter().map(wearable_from_static).collect()
}

/// Wearable by exact debugname (case-sensitive, like the dump keys), or None.
pub fn by_name(name: &str) -> Option<Wearable> {
    generated::ALL
        .binary_search_by(|row| row.name.cmp(name))
        .ok()
        .map(|position| wearable_from_static(&generated::ALL[position]))
}

/// True when every level-gated requirement is met
/// (`levels` maps stat name to current level, missing counts as 1).
pub fn requirements_met(entry: &Wearable, levels: &[(&str, i64)]) -> bool {
    for (stat, needed) in &entry.requirements {
        if stat_index(stat).is_none() {
            return false;
        }
        let have = levels.iter().find(|(name, _)| *name == stat).map(|(_, level)| *level).unwrap_or(1);
        if have < *needed {
            return false;
        }
    }
    true
}

/// True when the wearable occupies the given wearpos slot.
pub fn covers_slot(entry: &Wearable, slot: i64) -> bool {
    slot == entry.wearpos || slot == entry.wearpos2 || slot == entry.wearpos3
}

#[cfg(test)]
mod tests {
    use super::{by_name, covers_slot, requirements_met};

    #[test]
    fn excalibur_gates() {
        // excalibur wearpos 3, requires attack 20.
        let excal = by_name("excalibur").expect("excalibur");
        assert_eq!(excal.wearpos, 3);
        assert!(excal.requirements.contains(&("attack".to_string(), 20)));
        assert!(requirements_met(&excal, &[("attack", 20)]));
        assert!(!requirements_met(&excal, &[("attack", 19)]));
        assert!(covers_slot(&excal, 3));
        assert!(!covers_slot(&excal, 4));
    }

    /// PRD3 lot 5: ranged and magic gates, combo labels in argument
    /// order, quest gates and tiers come from the server labels.
    #[test]
    fn server_labels_decide_the_gates() {
        let req = |name: &str| by_name(name).expect(name).requirements;
        assert!(req("oak_shortbow").contains(&("ranged".to_string(), 5)));
        assert!(req("coif").contains(&("ranged".to_string(), 20)));
        let staff = req("battlestaff");
        assert!(staff.contains(&("magic".to_string(), 30)) && staff.contains(&("attack".to_string(), 30)));
        let hide = req("blue_dragonhide_body");
        assert!(hide.contains(&("ranged".to_string(), 50)) && hide.contains(&("defence".to_string(), 40)), "ranged first, then defence: {hide:?}");
        assert_eq!(by_name("rune_platebody").expect("plate").quest.as_deref(), Some("quest_dragon"));
        assert_eq!(by_name("rune_arrow").expect("arrow").tier, 40);
        assert!(by_name("rune_arrow").expect("arrow").rangebonus > by_name("bronze_arrow").expect("arrow").rangebonus);
        assert_eq!(by_name("rune_pickaxe").expect("pick").tier, 41);
    }

    #[test]
    fn table_shape() {
        // 678 wearables; unknown stats fail the gate.
        assert_eq!(super::all_wearables().len(), 678);
        let mut gated = by_name("excalibur").expect("excalibur");
        gated.requirements.push(("quest_points".to_string(), 1));
        assert!(!requirements_met(&gated, &[("attack", 99)]));
        assert!(super::by_name("no_such_item").is_none());
    }
}
