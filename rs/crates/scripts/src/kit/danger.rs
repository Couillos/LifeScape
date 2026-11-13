//! Static danger map for gather spots: aggressive NPCs that would attack
//! this bot (hunts players, vislevel high enough) cover a square around
//! each spawn. A spot is dangerous when a zone covers it or the straight
//! approach from its bank: bots fishing Draynor died ~100 times to the
//! young dark wizards (spawn (3084,3236), magic reach 5) because the old
//! filter used `huntrange + 2` only and was not wired for fishing.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

/// Slack past `huntrange + wanderrange` (the hunter strays `wanderrange`
/// off its spawn, read from the server configs: jail guards 11, young
/// dark wizards 3, engine default 5). The square is where a bot gets
/// aggroed; routes go around it (`NavOptions::danger`), so the chase and
/// the 5-tile magic reach never start.
pub const DANGER_MARGIN: i64 = 1;

/// One threat square: spawn and Chebyshev radius.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Threat {
    pub x: i64,
    pub z: i64,
    pub radius: i64,
    pub name: &'static str,
}

/// Server rule (engine `Npc.ts` hunt check): a hunter turns passive only
/// when `player.combatLevel > vislevel * 2`, and never in the Wilderness.
fn threatens(our_combat: Option<u8>, vislevel: i64) -> bool {
    our_combat.is_none_or(|ours| i64::from(ours) <= vislevel * 2)
}

/// Wilderness surface (level 0, z 3520 and up in the F2P world): hunters
/// there attack every combat level.
pub(crate) fn in_wilderness(spawn_x: i64, spawn_z: i64, level: i64) -> bool {
    level == 0 && (2944..=3392).contains(&spawn_x) && spawn_z >= 3520
}

/// Hunters at or above this level can kill a low-level bot whatever its
/// own level (a level-7 dark wizard's magic killed a combat-12 bot).
const LETHAL_LEVEL: i64 = 5;

/// Dangerous = it attacks us (server rule above) and can hurt us: not
/// weaker than us, or strong enough to kill anyway. Level-2 goblins and
/// giant spiders aggro a combat-3 bot but never count.
fn dangerous(our_combat: Option<u8>, vislevel: i64) -> bool {
    threatens(our_combat, vislevel) && (vislevel >= LETHAL_LEVEL || our_combat.is_none_or(|ours| vislevel >= i64::from(ours)))
}

/// Threats on the bot's level for its current combat level (empty with
/// `wild`: the operator opted into danger).
pub fn threats(bot: &Bot) -> Vec<Threat> {
    if bot.allow_wild {
        return Vec::new();
    }
    threats_for(bot.combat_level(), i64::from(bot.world.level))
}

/// Pure form of `threats` (tests, offline checks).
pub fn threats_for(our_combat: Option<u8>, level: i64) -> Vec<Threat> {
    let mut out = Vec::new();
    for entry in knowledge::all_npcs() {
        if !entry.hunts_players {
            continue;
        }
        let name = entry.name.unwrap_or("?");
        let radius = entry.huntrange + knowledge::npc_ranges::wander_range(name) + DANGER_MARGIN;
        for spawn in entry.spawns {
            let aggressive = if in_wilderness(spawn.x, spawn.z, spawn.level) { entry.vislevel >= LETHAL_LEVEL } else { dangerous(our_combat, entry.vislevel) };
            if spawn.level == level && aggressive {
                out.push(Threat { x: spawn.x, z: spawn.z, radius, name });
            }
        }
    }
    out
}

/// Threats farther than this from the bot are not sent with plans: a
/// route rarely leaves this box, and every zone costs search time.
const ARM_RADIUS: i64 = 250;

/// Load the danger squares around the bot into `bot.nav_danger`, so every
/// route the SDK plans (bank trips, spot walks, fights) goes around the
/// hunters that would attack this bot. Called by `check_events`, i.e.
/// at each loop step: cheap, and it follows the combat level and moves.
pub fn arm(bot: &mut Bot) {
    let here = (i64::from(bot.world.x), i64::from(bot.world.z));
    bot.nav_danger = threats(bot)
        .into_iter()
        .filter(|threat| (threat.x - here.0).abs().max((threat.z - here.1).abs()) <= ARM_RADIUS)
        .map(|threat| lifescape_nav::plan::DangerZone {
            x: threat.x as i32,
            z: threat.z as i32,
            level: i32::from(bot.world.level),
            radius: threat.radius as i32,
        })
        .collect();
}

/// First threat covering the tile (Chebyshev, like NPC aggression).
pub fn covering(threats: &[Threat], tile_x: i32, tile_z: i32) -> Option<Threat> {
    threats
        .iter()
        .find(|threat| (i64::from(tile_x) - threat.x).abs().max((i64::from(tile_z) - threat.z).abs()) <= threat.radius)
        .copied()
}

/// First threat on the spot or on the straight approach from `from`
/// (sampled every tile; the real path stays close to it on open ground).
pub fn on_approach(threats: &[Threat], from: (i32, i32), spot: (i32, i32)) -> Option<Threat> {
    let steps = (spot.0 - from.0).abs().max((spot.1 - from.1).abs()).max(1);
    (0..=steps).find_map(|step| {
        let tile_x = from.0 + (spot.0 - from.0) * step / steps;
        let tile_z = from.1 + (spot.1 - from.1) * step / steps;
        covering(threats, tile_x, tile_z)
    })
}

#[cfg(test)]
mod tests {
    use super::{covering, on_approach, threatens, threats_for};

    /// Threat rule (server skips players above twice the hunter level):
    /// at or under the line is a threat, unknown level stays cautious.
    #[test]
    fn hunter_threat_follows_server_rule() {
        assert!(threatens(Some(4), 2), "spider hunts combat 4");
        assert!(!threatens(Some(5), 2), "combat 5 too strong for spiders");
        assert!(threatens(Some(4), 26), "jailguard hunts everyone here");
        assert!(threatens(None, 1), "unknown level stays cautious");
    }

    /// Draynor shrimp: the north spot (3085,3230) sits in the wizards'
    /// aggro square, the south one (3086,3227) does not (the route around
    /// the square is the planner's job); the straight bank walk would
    /// cross it. Mudskipper Point stays open.
    #[test]
    fn draynor_north_spot_is_dangerous_south_spot_is_not() {
        let threats = threats_for(Some(3), 0);
        let wizard = covering(&threats, 3085, 3230).expect("north Draynor water is covered");
        assert_eq!(wizard.name, "young_dark_wizard");
        assert!(covering(&threats, 3086, 3227).is_none(), "south Draynor water is out of aggro reach");
        assert!(on_approach(&threats, (3092, 3243), (3086, 3227)).is_some(), "a straight bank walk would cross the square");
        assert!(covering(&threats, 2996, 3158).is_none(), "Mudskipper Point is safe");
    }

    /// Jail guards wander 11 tiles (draynor.npc): the road at (3137,3255),
    /// where bots died 10 tiles off the spawn, is inside their square. A
    /// level-2 giant spider never counts for a combat-3 bot (cannot kill).
    #[test]
    fn jail_guards_cover_the_road_and_weak_hunters_never_count() {
        let threats = threats_for(Some(3), 0);
        assert!(covering(&threats, 3137, 3255).is_some_and(|threat| threat.name == "jailguard"), "the death tile is covered");
        assert!(threats.iter().all(|threat| threat.name != "giantspider1"), "a weaker hunter is not a danger");
    }

    /// A combat-12 bot is still aggroed by the level-7 wizards (12 <= 14)
    /// and their magic kills it: still a danger.
    #[test]
    fn wizards_stay_dangerous_until_they_stop_aggroing() {
        assert!(covering(&threats_for(Some(12), 0), 3085, 3230).is_some(), "combat 12 is still a target");
        assert!(covering(&threats_for(Some(15), 0), 3085, 3230).is_none(), "combat 15 is ignored (15 > 2 x 7)");
    }

    /// In the Wilderness the passive rule does not apply: a combat-30 bot
    /// is still a target for the level-7 dark wizards up there.
    #[test]
    fn wilderness_hunters_ignore_the_two_times_rule() {
        let strong = threats_for(Some(30), 0);
        assert!(strong.iter().any(|threat| threat.z >= 3520), "wilderness hunters stay dangerous for combat 30");
    }

    /// A high-level bot is not a target: vislevel 7 wizards ignore
    /// combat 30.
    #[test]
    fn strong_bots_are_not_threatened() {
        let threats = threats_for(Some(30), 0);
        assert!(covering(&threats, 3086, 3227).is_none());
    }
}
