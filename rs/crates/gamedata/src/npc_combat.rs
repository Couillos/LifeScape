//! NPC combat blocks (`gamedata/data/npc_combat.json`, written by
//! `orchestrator dump-npc-ranges` from the server's `.npc` configs; the
//! game dump carries none of it). Absent keys take the engine defaults
//! (`NpcType`: stats 1, bonuses 0, `attackrate` 4 from `combat.param`).

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::OnceLock;

/// Engine default attack delay in ticks (`combat.param [attackrate]`).
pub const DEFAULT_ATTACK_RATE: i64 = 4;

const RAW: &str = include_str!("../../../../gamedata/data/npc_combat.json");

/// Melee damage type of a hit (`^stab_style`, `^slash_style`,
/// `^crush_style`); anything else (ranged, magic) is `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageType {
    Stab,
    Slash,
    Crush,
    Other,
}

impl DamageType {
    /// Index into `[stab, slash, crush]` bonus triples (None for Other).
    pub fn melee_index(self) -> Option<usize> {
        match self {
            DamageType::Stab => Some(0),
            DamageType::Slash => Some(1),
            DamageType::Crush => Some(2),
            DamageType::Other => None,
        }
    }

    fn parse(raw: &str) -> DamageType {
        match raw {
            "stab" => DamageType::Stab,
            "slash" => DamageType::Slash,
            "crush" => DamageType::Crush,
            _ => DamageType::Other,
        }
    }
}

/// One NPC's combat block, engine defaults filled in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NpcCombat {
    pub hitpoints: i64,
    pub attack: i64,
    pub strength: i64,
    pub defence: i64,
    pub attack_bonus: i64,
    pub strength_bonus: i64,
    /// `[stab, slash, crush]` defence bonuses.
    pub defence_bonus: [i64; 3],
    pub attack_rate: i64,
    pub damage_type: DamageType,
}

fn table() -> &'static HashMap<String, NpcCombat> {
    static TABLE: OnceLock<HashMap<String, NpcCombat>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut out = HashMap::new();
        let Ok(doc) = lifescape_json::parse(RAW) else { return out };
        if let Some(rows) = doc.get("npcs").as_obj() {
            for (name, row) in rows {
                let int = |key: &str, default: i64| row.get(key).as_i64().unwrap_or(default);
                out.insert(
                    name.clone(),
                    NpcCombat {
                        hitpoints: int("hitpoints", 1),
                        attack: int("attack", 1),
                        strength: int("strength", 1),
                        defence: int("defence", 1),
                        attack_bonus: int("attackbonus", 0),
                        strength_bonus: int("strengthbonus", 0),
                        defence_bonus: [int("stabdefence", 0), int("slashdefence", 0), int("crushdefence", 0)],
                        attack_rate: int("attackrate", DEFAULT_ATTACK_RATE).max(1),
                        damage_type: DamageType::parse(row.get("damagetype").as_str().unwrap_or("")),
                    },
                );
            }
        }
        out
    })
}

/// Combat block of one NPC debugname, or None when the configs give it
/// no hitpoints (not attackable).
pub fn by_name(name: &str) -> Option<NpcCombat> {
    table().get(name).copied()
}

#[cfg(test)]
mod tests {
    use super::{by_name, DamageType, DEFAULT_ATTACK_RATE};

    /// Values straight from the server configs (`_unpack/225/all.npc`).
    #[test]
    fn blocks_come_from_the_server_configs() {
        let cow = by_name("cow").expect("cow block");
        assert_eq!((cow.hitpoints, cow.attack, cow.defence), (8, 1, 1), "stats default to 1");
        assert_eq!(cow.attack_rate, DEFAULT_ATTACK_RATE);
        assert_eq!(cow.damage_type, DamageType::Crush);
        let guard = by_name("guard1").expect("guard block");
        assert_eq!((guard.hitpoints, guard.attack, guard.strength, guard.defence), (22, 19, 18, 14));
        assert_eq!(guard.defence_bonus, [18, 25, 19]);
        let giant = by_name("giant").expect("hill giant block");
        assert_eq!((giant.hitpoints, giant.attack_rate, giant.strength_bonus), (35, 6, 16));
        assert!(by_name("hans").is_none(), "no hitpoints line, no block");
    }
}
