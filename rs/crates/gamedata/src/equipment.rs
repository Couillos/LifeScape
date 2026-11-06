//! Wearable stats and level requirements.

use crate::{Json, num, opt_str, str_int_map, table_value};

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

fn wearable_from_row(name: &str, row: &Json) -> Wearable {
    Wearable {
        name: name.to_string(),
        wearpos: num(row, "wearpos", -1),
        wearpos2: num(row, "wearpos2", -1),
        wearpos3: num(row, "wearpos3", -1),
        category: opt_str(row, "category"),
        attack: str_int_map(row, "attack"),
        defence: str_int_map(row, "defence"),
        strength: num(row, "strength", 0),
        prayer: num(row, "prayer", 0),
        requirements: str_int_map(row, "requirements"),
    }
}

/// Every wearable, keyed by debugname in the dump.
pub fn all_wearables() -> Vec<Wearable> {
    match table_value("equipment") {
        Some(Json::Obj(fields)) => fields.iter().map(|(name, row)| wearable_from_row(name, row)).collect(),
        _ => Vec::new(),
    }
}

/// Wearable by debugname, or None.
pub fn by_name(name: &str) -> Option<Wearable> {
    match table_value("equipment") {
        Some(Json::Obj(fields)) => {
            fields.iter().find(|(entry, _)| *entry == name).map(|(entry, row)| wearable_from_row(entry, row))
        }
        _ => None,
    }
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
