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
/// Farthest a gather site may sit from its bank, in ROUTE cost (the
/// planner's tiles, doors and rides: `site_banks`), shared by the
/// planner's requirement and the scripts' pickers. Tour 3 point 2: was
/// 150 Manhattan; 250 keeps every site the old trek reached and a route
/// serves (the swamp mine: Draynor at 170).
pub const BANK_TREK_LIMIT: i64 = 250;
/// Requirement of a place no combat level makes safe (no bank in reach).
/// The Wilderness is never one (user 2026-11-16: "le wilderness doit être
/// accessible à tous, toujours") : its hunters count like any other.
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

/// Wilderness, the engine's own test (`Player.isInWilderness`, every
/// level): x 2944..3392, the surface from z 3520 below the underground
/// band, and the dungeons under it from z 9920 (Edgeville dungeon north
/// of its wild gate). Hunters ignore the passive rule there. The rest
/// of the underground band (Edgeville dungeon giants, Varrock sewers)
/// is not Wilderness: its hunters turn passive like the surface ones.
pub fn in_wilderness(x: i64, z: i64, _level: i64) -> bool {
    (2944..3392).contains(&x) && ((3520..6400).contains(&z) || (9920..12800).contains(&z))
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

/// Combat level from which this hunter is no lethal threat: past twice
/// its level it turns passive on the surface; in the Wilderness it still
/// attacks (`hunts`), and the same level beats it — the zone itself is
/// never refused (user 2026-11-16).
pub fn passive_from(hunter: &Hunter) -> u8 {
    (hunter.vislevel * 2 + 1).clamp(3, i64::from(NEVER) - 1) as u8
}

fn covers(hunter: &Hunter, x: i64, z: i64, level: i64) -> bool {
    hunter.level == level && (x - hunter.x).abs().max((z - hunter.z).abs()) <= hunter.radius
}

/// Lowest combat level at which no lethal hunter covers this tile (3 when
/// none does).
pub fn required_combat_at(x: i64, z: i64, level: i64) -> u8 {
    hunters().iter().filter(|hunter| covers(hunter, x, z, level)).map(passive_from).max().unwrap_or(3)
}

/// The bank serving a site: the cheapest ROUTE of the offline table
/// (`site_banks`, tour 3 point 2: never the nearest booth as the crow
/// flies), within the trek limit. A Karamja site reads from the Port
/// Sarim dock its boat lands on (`access::mainland_anchor`).
pub fn site_bank(x: i64, z: i64, level: i64) -> Option<crate::banks::Bank> {
    // A level with no F2P booth (an upper floor) has no bank of its own:
    // its callers read it at the ground tile below (`access::surface_of`).
    let (_, _, anchored) = crate::access::mainland_anchor(x, z, level);
    static LEVELS: OnceLock<Vec<i64>> = OnceLock::new();
    if !LEVELS.get_or_init(|| crate::banks::all_banks().into_iter().filter(|bank| !bank.members).map(|bank| bank.level).collect()).contains(&anchored) {
        return None;
    }
    crate::site_banks::serving(x, z, level).filter(|served| served.cost <= BANK_TREK_LIMIT).map(|served| served.bank)
}

/// Combat a bot needs to work this site and bank its output: the site
/// and its bank must both sit outside every hunter it would meet.
/// `NEVER` only without a reachable bank (the Wilderness is open).
pub fn site_requirement(x: i64, z: i64, level: i64) -> u8 {
    let Some(bank) = site_bank(x, z, level) else {
        return NEVER;
    };
    required_combat_at(x, z, level).max(required_combat_at(bank.x, bank.z, bank.level))
}

/// Combat a bot needs to FIGHT at this site (user 2026-11-20): with a
/// bank in reach, the site and its bank (`site_requirement`); without
/// one, the site alone: a fight is one trip that lasts until its food
/// runs out, then the walk to the nearest bank (the deep Wilderness
/// greater demons, 400 tiles from Edgeville). The danger stays a combat
/// requirement, never a refused zone; a gather site still needs its bank
/// (`site_requirement`: every load is banked).
pub fn fight_site_requirement(x: i64, z: i64, level: i64) -> u8 {
    let site = required_combat_at(x, z, level);
    match site_bank(x, z, level) {
        Some(bank) => site.max(required_combat_at(bank.x, bank.z, bank.level)),
        None => site,
    }
}

/// `fight_site_requirement` with the camp's own foes left out (`foes`,
/// NPC debugnames; the melee ladder's `cluster_requirement`: we came to
/// fight them). For a risk camp only, whose floor
/// (`combat_spots::risk_floor`, foes' level less two) is the price of
/// fighting them: their own aggression bound (twice their level, 165 for
/// the volcano's lesser demons) is a level no bot ever reaches. Other
/// hunters at the site and its bank still count.
pub fn camp_site_requirement(foes: &[&str], x: i64, z: i64, level: i64) -> u8 {
    let required_at = |x: i64, z: i64, level: i64| {
        hunters().iter().filter(|hunter| !foes.contains(&hunter.name) && covers(hunter, x, z, level)).map(passive_from).max().unwrap_or(3)
    };
    let site = required_at(x, z, level);
    match site_bank(x, z, level) {
        Some(bank) => site.max(required_at(bank.x, bank.z, bank.level)),
        None => site,
    }
}

/// Requirement of a site worked by a method of `kind`: a fight reads
/// `fight_site_requirement`, every other kind `site_requirement`.
pub fn kind_site_requirement(kind: &str, x: i64, z: i64, level: i64) -> u8 {
    if kind == "fight" {
        fight_site_requirement(x, z, level)
    } else {
        site_requirement(x, z, level)
    }
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
                    // A spawn no stand reaches (fence, wall, sealed
                    // pocket) is no work site (plan P6, `site_reach`).
                    out.extend(
                        loc.spawns
                            .iter()
                            .filter(|spawn| spawn.members != Some(true) && free(spawn.x, spawn.z) && crate::site_reach::unreachable(spawn.x, spawn.z, spawn.level).is_none())
                            .map(|spawn| (spawn.x, spawn.z, spawn.level)),
                    );
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
    sites(kind, resource).into_iter().map(|(x, z, level)| kind_site_requirement(kind, x, z, level)).min()
}

#[cfg(test)]
mod tests {
    use super::{hunters, in_wilderness, method_requirement, passive_from, required_combat_at, site_requirement, NEVER};

    /// Server line (`Player.isInWilderness`): the Edgeville dungeon
    /// under z 9920 is no Wilderness, so its skeletons turn passive past
    /// twice their level like surface hunters (before: every underground
    /// hunter read wild, never passive, and no dungeon camp next to one
    /// was ever safe).
    /// Decision D3 (2026-11-17): the aggressive NPCs of a gathering site
    /// count in its combat requirement. The Wilderness iron rocks
    /// (3104,3570) sit among level-22 skeletons: a bot works them from
    /// combat 45, the Varrock east iron (no hunter) from 3.
    #[test]
    fn hunters_of_a_gathering_site_raise_its_requirement() {
        let wild_iron = super::site_requirement(3104, 3570, 0);
        assert!(wild_iron >= 45 && wild_iron < NEVER, "wilderness iron: {wild_iron}");
        assert!(super::sites("mining", "iron_ore").iter().any(|site| *site == (3104, 3570, 0) || (site.0 - 3104).abs() + (site.1 - 3570).abs() <= 4), "the wild iron is a site");
        assert_eq!(super::site_requirement(3285, 3365, 0), 3, "Varrock east iron: no hunter");
    }

    #[test]
    fn underground_is_wild_only_past_the_server_line() {
        assert!(!in_wilderness(3111, 9837, 0), "Edgeville dungeon hill giants");
        assert!(!in_wilderness(3160, 9890, 0), "moss giants hall");
        assert!(in_wilderness(3105, 9940, 0), "chaos druids past the wild gate");
        assert!(in_wilderness(3050, 3704, 0), "surface Wilderness");
        assert!(!in_wilderness(3392, 3600, 0), "x 3392 is east of the line");
        assert!(!in_wilderness(3094, 3491, 0), "Edgeville bank");
        let skeleton = hunters().iter().find(|hunter| hunter.name == "skeleton_armed" && (hunter.x, hunter.z) == (3121, 9908)).expect("dungeon skeleton");
        assert!(!skeleton.wilderness);
        assert_eq!(passive_from(skeleton), 45, "vislevel 22: passive from combat 45");
        let druid = hunters().iter().find(|hunter| hunter.name == "chaos_druid" && (hunter.x, hunter.z) == (3105, 9936)).expect("wild druid");
        assert_eq!(passive_from(druid), (druid.vislevel * 2 + 1) as u8, "beaten like any hunter, never a closed zone");
    }

    /// Computed, not written: the only toll-free shrimp water with a bank
    /// in reach is Draynor's, under level-7 wizards (passive past 14).
    #[test]
    fn shrimp_needs_combat_fifteen_from_the_data() {
        assert_eq!(required_combat_at(3085, 3230, 0), 15, "north Draynor spot under the wizards");
        let shrimp = method_requirement("fishing", "raw_shrimp").expect("shrimp sites exist");
        assert!(shrimp <= 15, "some shrimp water is safe from 15 on, got {shrimp}");
        assert!(in_wilderness(3090, 3540, 0));
        assert!(site_requirement(3090, 3540, 0) < NEVER, "the Wilderness is open to every bot (user 2026-11-16)");
        // Deep north: the bank trek (`BANK_TREK_LIMIT`, a ROUTE cost since
        // tour 3), the same rule as anywhere else, not the zone: 222 tiles
        // from Edgeville is in reach, 281 is not, no route is never.
        assert!(site_requirement(3050, 3704, 0) < NEVER);
        assert_eq!(site_requirement(3079, 3767, 0), NEVER);
        assert_eq!(site_requirement(3054, 3852, 0), NEVER);
    }

    /// User 2026-11-20 (item 2): a fight site with no bank in reach is one
    /// trip, its requirement the hunters covering it; a gather site there
    /// still is `NEVER` (every load banked).
    #[test]
    fn a_fight_far_from_any_bank_is_one_trip() {
        assert_eq!(site_requirement(3054, 3852, 0), NEVER, "no bank for a gather");
        let fight = super::fight_site_requirement(3054, 3852, 0);
        assert!(fight < NEVER, "a fight there is one trip: {fight}");
        assert_eq!(fight, required_combat_at(3054, 3852, 0));
        let greater = method_requirement("fight", "greater_demons").expect("greater demon sites");
        assert!(greater < NEVER, "the Wilderness greater demons: {greater}");
        // A site with a bank in reach reads the same either way.
        assert_eq!(super::fight_site_requirement(3285, 3365, 0), site_requirement(3285, 3365, 0));
    }

    #[test]
    fn chickens_and_copper_are_open_to_fresh_bots() {
        assert_eq!(method_requirement("fight", "chickens"), Some(3));
        assert_eq!(method_requirement("mining", "copper_ore"), Some(3));
        assert_eq!(method_requirement("woodcut", "logs"), Some(3));
        assert_eq!(method_requirement("fishing", "no_such_fish"), None);
    }

    /// User 2026-11-21: the volcano's lesser demons hunt every bot (level
    /// 82, passive only from 165): the camp read 165 at its own spawns,
    /// no bot ever fought there. Left out (we came to fight them), the
    /// site reads its other hunters only; its floor (80) is the
    /// planner's.
    #[test]
    fn a_camp_leaves_its_own_foes_out() {
        let volcano = super::fight_site_requirement(2837, 9565, 0);
        assert!(volcano > 99, "the demons themselves: {volcano}");
        let camp = super::camp_site_requirement(&["lesser_demon"], 2837, 9565, 0);
        assert!(camp < 80, "the demons left out: {camp}");
    }
}
