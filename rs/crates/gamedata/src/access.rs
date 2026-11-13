//! Access gates of F2P sites (audit 2026-11-13 item A5): what a bot needs
//! before it may work a site the danger model calls safe. The planner's
//! site pick reads `gates_at` and either passes the gate (level, uniform,
//! key held), files its dependency (`access:<guild>`, the key's obtain),
//! or refuses the site (boat without a nav edge, underground, Wilderness).
//!
//! Server truth (rev 274, doors located in `content/maps`):
//! - Crafting Guild door `crafting_guild_door` (2933, 3289):
//!   `crafting_guild.rs2` asks crafting 40 and a WORN brown apron;
//! - Cooks' Guild door `chefdoor` (3143, 3444): `cooking_guild.rs2` asks
//!   cooking 32 and a WORN chef's hat;
//! - Mining Guild ladder (3019, 3339) → (3019, 9739) and door:
//!   `mining_guild.rs2` asks mining 60, no item;
//! - Edgeville dungeon `brasskeydoor` (3115, 3450): the carried brass key
//!   (`edgevilledungeonkey`, nav requirement `item:983`);
//! - Karamja: `sailors.rs2` (Port Sarim) takes 30 coins from the pack and
//!   sails (`~set_sail`, a teleport onto the ship); the customs officer
//!   sails back for 30. The navigation has NO edge for that teleport
//!   (`nav_overrides.json` `custom_edges` empty; offline 2026-11-13:
//!   `orchestrator route 3029 3217 0 2915 3150 0` → `BudgetExhausted
//!   closest=(2955,3192,0)`, the Rimmington coast), so `BOAT_ROUTED` is
//!   false: Karamja sites (tuna, lobster, swordfish, Karamja gold) stay
//!   unplannable until the SDK/nav sails;
//! - Wilderness (`danger::in_wilderness`): no combat level makes it safe;
//! - underground (level 0, z ≥ 6400) outside the Mining Guild: dungeons
//!   reached by trapdoors and ladders the scripts never take for a gather
//!   or a drop farm (`kit::hunt::run_spot_with` refuses a bare key
//!   underground).
//! The Al Kharid toll is positional (which side the bot stands on), see
//! `toll::crossing`.

/// The nav can follow the Port Sarim ↔ Karamja sailors (see module doc).
pub const BOAT_ROUTED: bool = false;

/// One gate a site sits behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// Guild door: the skill level and the uniform worn at the door.
    Guild { area: &'static str, skill: &'static str, level: i64, uniform: Option<&'static str> },
    /// Door a carried key opens.
    Key { area: &'static str, item: &'static str },
    /// Island reached by the Port Sarim boat (fare each way).
    Boat { area: &'static str, fare: i64 },
    /// Below ground with no gate the planner models.
    Underground,
    /// Wilderness surface.
    Wilderness,
}

impl Gate {
    /// Planner key of the gate (`access:<area>` blocks, logs).
    pub fn area(&self) -> &'static str {
        match self {
            Gate::Guild { area, .. } | Gate::Key { area, .. } | Gate::Boat { area, .. } => area,
            Gate::Underground => "underground",
            Gate::Wilderness => "wilderness",
        }
    }
}

/// A gated rectangle: x and z inclusive, the levels it spans.
struct Area {
    x: (i64, i64),
    z: (i64, i64),
    levels: (i64, i64),
    gate: Gate,
    /// Ground tile outside the door (or the ladder top): where the bank
    /// and the danger of an inside site read.
    door: Option<(i64, i64, i64)>,
}

fn inside(area: &Area, x: i64, z: i64, level: i64) -> bool {
    (area.x.0..=area.x.1).contains(&x) && (area.z.0..=area.z.1).contains(&z) && (area.levels.0..=area.levels.1).contains(&level)
}

fn areas() -> Vec<Area> {
    vec![
        // Ground floor and loft south of the north door (2933, 3289).
        Area {
            x: (2929, 2943),
            z: (3276, 3288),
            levels: (0, 1),
            gate: Gate::Guild { area: "crafting_guild", skill: "crafting", level: 40, uniform: Some("brown_apron") },
            door: Some((2933, 3290, 0)),
        },
        // Three floors north of the south door (3143, 3444): ranges on
        // the first floor (3143..3145, 3452), the windmill hopper on top.
        Area {
            x: (3138, 3148),
            z: (3445, 3453),
            levels: (0, 2),
            gate: Gate::Guild { area: "cooks_guild", skill: "cooking", level: 32, uniform: Some("chefs_hat") },
            door: Some((3143, 3443, 0)),
        },
        // Below Falador: the ladder lands at (3019, 9739).
        Area {
            x: (3014, 3058),
            z: (9730, 9758),
            levels: (0, 0),
            gate: Gate::Guild { area: "mining_guild", skill: "mining", level: 60, uniform: None },
            door: Some((3019, 3339, 0)),
        },
        // The brass-key house west of Varrock and the dungeon room below.
        Area { x: (3112, 3118), z: (3447, 3453), levels: (0, 0), gate: Gate::Key { area: "edgeville_dungeon", item: "edgevilledungeonkey" }, door: None },
        // Karamja (F2P north half), south of the Rimmington coast.
        Area { x: (2740, 2975), z: (2880, 3186), levels: (0, 1), gate: Gate::Boat { area: "karamja", fare: crate::services::BOAT_FARE_GP }, door: None },
        // Crandor: Ned's ship during Dragon Slayer only.
        Area { x: (2813, 2869), z: (3222, 3310), levels: (0, 0), gate: Gate::Boat { area: "crandor", fare: 0 }, door: None },
    ]
}

/// Gates of the site (x, z, level): the most specific area first, then
/// the generic Wilderness and underground gates. Empty = open.
pub fn gates_at(x: i64, z: i64, level: i64) -> Vec<Gate> {
    let mut out: Vec<Gate> = areas()
        .into_iter()
        .filter(|area| inside(area, x, z, level))
        .map(|area| area.gate)
        .collect();
    // The danger test reads level 0 only: underground rows (z ≥ 6400)
    // are dungeons, never the Wilderness surface.
    if z < 6400 && crate::danger::in_wilderness(x, z, level) {
        out.push(Gate::Wilderness);
    }
    let modelled = out.iter().any(|gate| matches!(gate, Gate::Guild { .. }));
    if z >= 6400 && !modelled {
        out.push(Gate::Underground);
    }
    out
}

/// Ground tile a guild site is reached from (outside its door, the
/// Mining Guild ladder top): its bank and danger read there (an upstairs
/// range or an underground rock has no bank on its own level). The site
/// itself elsewhere.
pub fn surface_of(x: i64, z: i64, level: i64) -> (i64, i64, i64) {
    areas().into_iter().find(|area| inside(area, x, z, level)).and_then(|area| area.door).unwrap_or((x, z, level))
}

/// True when no bot can ever pass the gate today (no nav edge, no model).
pub fn impassable(gate: &Gate) -> bool {
    match gate {
        Gate::Boat { .. } => !BOAT_ROUTED,
        Gate::Underground | Gate::Wilderness => true,
        Gate::Guild { .. } | Gate::Key { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{gates_at, impassable, surface_of, Gate};

    /// Doors from the server map: the guild gates cover their interiors,
    /// the rest of the world stays open.
    #[test]
    fn guilds_and_islands_are_gated() {
        let crafting = gates_at(2938, 3282, 0);
        assert!(matches!(crafting.as_slice(), [Gate::Guild { area: "crafting_guild", level: 40, uniform: Some("brown_apron"), .. }]), "{crafting:?}");
        assert!(matches!(gates_at(3143, 3447, 0).as_slice(), [Gate::Guild { area: "cooks_guild", level: 32, .. }]));
        assert!(matches!(gates_at(3040, 9740, 0).as_slice(), [Gate::Guild { area: "mining_guild", level: 60, uniform: None, .. }]));
        assert_eq!(surface_of(3040, 9740, 0), (3019, 3339, 0), "the guild banks at Falador");
        assert_eq!(surface_of(3143, 3452, 1), (3143, 3443, 0), "the guild range reads at its door");
        assert!(gates_at(2975, 3234, 0).is_empty(), "Rimmington gold is open");
        assert!(gates_at(2955, 3192, 0).is_empty(), "the Rimmington coast is mainland");
        let karamja = gates_at(2924, 3178, 0);
        assert!(matches!(karamja.as_slice(), [Gate::Boat { area: "karamja", fare: 30 }]), "{karamja:?}");
        assert!(impassable(&karamja[0]), "no nav edge for the sailors yet");
        assert_eq!(gates_at(3050, 3704, 0), vec![Gate::Wilderness]);
        assert_eq!(gates_at(3100, 9900, 0), vec![Gate::Underground]);
        assert_eq!(gates_at(3232, 3203, 0), Vec::new(), "Lumbridge is open");
    }

    /// Every F2P lobster, tuna and swordfish spot is on Karamja: none is
    /// plannable while the boat has no nav edge.
    #[test]
    fn karamja_water_is_behind_the_boat() {
        for fish in ["raw_lobster", "raw_tuna", "raw_swordfish"] {
            let sites = crate::danger::sites("fishing", fish);
            assert!(!sites.is_empty(), "{fish} has F2P water");
            for (x, z, level) in sites {
                let gates = gates_at(x, z, level);
                assert!(gates.iter().any(|gate| matches!(gate, Gate::Boat { area: "karamja", .. })), "{fish} at ({x},{z}) not behind the boat: {gates:?}");
            }
        }
    }

    /// The crafting guild's gold and silver rocks are inside its door.
    #[test]
    fn guild_rocks_are_gated() {
        let gold = crate::danger::sites("mining", "gold_ore");
        assert!(gold.iter().any(|(x, z, level)| matches!(gates_at(*x, *z, *level).as_slice(), [Gate::Guild { area: "crafting_guild", .. }])), "{gold:?}");
        assert!(gold.iter().any(|(x, z, level)| gates_at(*x, *z, *level).is_empty()), "open gold exists too: {gold:?}");
    }
}
