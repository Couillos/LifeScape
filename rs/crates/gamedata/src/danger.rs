//! One danger model for the whole stack, computed from game data only
//! (no zone written by hand): aggressive NPC spawns, their hunt and wander
//! ranges (server configs) and the server's passive rule. The planner
//! derives a method's combat requirement from it, the scripts pick sites
//! and banks with it, and the routes avoid its squares.
//!
//! Server rule (engine `Npc.ts` hunt check): a hunter attacks a player
//! unless `player.combatLevel > vislevel * 2`, and always in the
//! Wilderness. A hunter is a danger when it can also kill: level 5 and up
//! (level-2 goblins aggro fresh bots but never kill them).

#![forbid(unsafe_code)]

use std::sync::OnceLock;

/// Hunters below this level never count (they aggro but cannot kill).
pub const LETHAL_LEVEL: i64 = 5;
/// Slack past `huntrange + wanderrange` (the hunter strays `wanderrange`
/// off its spawn, then hunts `huntrange` around itself).
pub const DANGER_MARGIN: i64 = 1;
/// Farthest a gather site may sit from its bank (Manhattan), shared by
/// the planner's requirement and the scripts' pickers.
pub const BANK_TREK_LIMIT: i64 = 150;
/// Requirement of a place no combat level makes safe (Wilderness).
pub const NEVER: u8 = u8::MAX;

/// One aggressive spawn and the square where it hunts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hunter {
    pub x: i64,
    pub z: i64,
    pub level: i64,
    pub radius: i64,
    pub vislevel: i64,
    pub wilderness: bool,
    pub name: &'static str,
}

/// Wilderness surface (level 0, z 3520 and up): hunters ignore the
/// passive rule there.
pub fn in_wilderness(x: i64, z: i64, level: i64) -> bool {
    level == 0 && (2944..=3392).contains(&x) && z >= 3520
}

/// Every lethal aggressive spawn of the dump (members spawns excluded).
pub fn hunters() -> &'static [Hunter] {
    static ALL: OnceLock<Vec<Hunter>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut out = Vec::new();
        for entry in crate::all_npcs() {
            if !entry.hunts_players || entry.vislevel < LETHAL_LEVEL {
                continue;
            }
            let name = entry.name.unwrap_or("?");
            let radius = entry.huntrange + crate::npc_ranges::wander_range(name) + DANGER_MARGIN;
            for spawn in entry.spawns {
                if spawn.members {
                    continue;
                }
                out.push(Hunter {
                    x: spawn.x,
                    z: spawn.z,
                    level: spawn.level,
                    radius,
                    vislevel: entry.vislevel,
                    wilderness: in_wilderness(spawn.x, spawn.z, spawn.level),
                    name,
                });
            }
        }
        out
    })
}

/// True when this hunter attacks (and can kill) a bot of this combat
/// level (`None` = unknown level: cautious).
pub fn hunts(hunter: &Hunter, combat: Option<u8>) -> bool {
    hunter.wilderness || combat.is_none_or(|ours| i64::from(ours) <= hunter.vislevel * 2)
}

/// Combat level this hunter stops hunting at (`NEVER` in the Wilderness).
pub fn passive_from(hunter: &Hunter) -> u8 {
    if hunter.wilderness {
        NEVER
    } else {
        (hunter.vislevel * 2 + 1).clamp(3, i64::from(NEVER) - 1) as u8
    }
}

fn covers(hunter: &Hunter, x: i64, z: i64, level: i64) -> bool {
    hunter.level == level && (x - hunter.x).abs().max((z - hunter.z).abs()) <= hunter.radius
}

/// Lowest combat level at which no lethal hunter covers this tile (3 when
/// none does, `NEVER` when a Wilderness hunter does).
pub fn required_combat_at(x: i64, z: i64, level: i64) -> u8 {
    hunters().iter().filter(|hunter| covers(hunter, x, z, level)).map(passive_from).max().unwrap_or(3)
}

/// Nearest non-members bank on the same level, if within the trek limit.
/// A Karamja site (no F2P bank on the island) reads from the Port Sarim
/// dock its boat lands on (`access::mainland_anchor`).
pub fn site_bank(x: i64, z: i64, level: i64) -> Option<crate::banks::Bank> {
    let (x, z, level) = crate::access::mainland_anchor(x, z, level);
    static BANKS: OnceLock<Vec<crate::banks::Bank>> = OnceLock::new();
    BANKS
        .get_or_init(crate::banks::all_banks)
        .iter()
        .cloned()
        .filter(|bank| !bank.members && bank.level == level)
        .map(|bank| ((bank.x - x).abs() + (bank.z - z).abs(), bank))
        .filter(|(trek, _)| *trek <= BANK_TREK_LIMIT)
        .min_by_key(|(trek, _)| *trek)
        .map(|(_, bank)| bank)
}

/// Combat a bot needs to work this site and bank its output: the site
/// and its bank must both sit outside every hunter it would meet.
/// `NEVER` without a reachable bank or inside the Wilderness.
pub fn site_requirement(x: i64, z: i64, level: i64) -> u8 {
    if in_wilderness(x, z, level) {
        return NEVER;
    }
    let Some(bank) = site_bank(x, z, level) else {
        return NEVER;
    };
    required_combat_at(x, z, level).max(required_combat_at(bank.x, bank.z, bank.level))
}

/// F2P sites (x, z, level) where one resource is worked: fishing spots of
/// the tiers that yield it, rocks, trees, or a combat spot's NPCs.
pub fn sites(kind: &str, resource: &str) -> Vec<(i64, i64, i64)> {
    let mut out: Vec<(i64, i64, i64)> = Vec::new();
    let free = |x: i64, z: i64| crate::free_zones::is_free(x, z);
    match kind {
        "fishing" => {
            for tier in crate::fishing::all_fish_tiers().iter().filter(|tier| tier.products.contains(&resource)) {
                for npc_id in crate::fishing::spot_npc_ids(tier.category) {
                    if let Some(npc) = crate::npc_by_id(npc_id) {
                        out.extend(npc.spawns.iter().filter(|spawn| !spawn.members && free(spawn.x, spawn.z)).map(|spawn| (spawn.x, spawn.z, spawn.level)));
                    }
                }
            }
        }
        "mining" | "woodcut" => {
            let names: Vec<String> = if kind == "mining" {
                crate::rocks::all_rock_tiers().into_iter().filter(|tier| tier.product == resource).flat_map(|tier| tier.rocks).collect()
            } else {
                crate::trees::all_tiers().into_iter().filter(|tier| tier.product == resource).flat_map(|tier| tier.trees).collect()
            };
            for name in names {
                for loc in crate::locs_by_name(&name) {
                    out.extend(loc.spawns.iter().filter(|spawn| spawn.members != Some(true) && free(spawn.x, spawn.z)).map(|spawn| (spawn.x, spawn.z, spawn.level)));
                }
            }
        }
        "fight" => out.extend(crate::combat_spots::positions(resource)),
        _ => {}
    }
    out.sort();
    out.dedup();
    out
}

/// Combat requirement of a method: its least demanding site. `None` when
/// the data names no site (unknown resource): the caller decides.
pub fn method_requirement(kind: &str, resource: &str) -> Option<u8> {
    sites(kind, resource).into_iter().map(|(x, z, level)| site_requirement(x, z, level)).min()
}

#[cfg(test)]
mod tests {
    use super::{method_requirement, required_combat_at, site_requirement, NEVER};

    /// Computed, not written: the only toll-free shrimp water with a bank
    /// in reach is Draynor's, under level-7 wizards (passive past 14).
    #[test]
    fn shrimp_needs_combat_fifteen_from_the_data() {
        assert_eq!(required_combat_at(3085, 3230, 0), 15, "north Draynor spot under the wizards");
        let shrimp = method_requirement("fishing", "raw_shrimp").expect("shrimp sites exist");
        assert!(shrimp <= 15, "some shrimp water is safe from 15 on, got {shrimp}");
        assert_eq!(site_requirement(3050, 3704, 0), NEVER, "Wilderness water is never safe");
    }

    #[test]
    fn chickens_and_copper_are_open_to_fresh_bots() {
        assert_eq!(method_requirement("fight", "chickens"), Some(3));
        assert_eq!(method_requirement("mining", "copper_ore"), Some(3));
        assert_eq!(method_requirement("woodcut", "logs"), Some(3));
        assert_eq!(method_requirement("fishing", "no_such_fish"), None);
    }
}
