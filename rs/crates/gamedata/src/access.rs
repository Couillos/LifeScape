//! Access gates of F2P sites (audit 2026-11-13 item A5): what a bot needs
//! before it may work a site the danger model calls safe. The planner's
//! site pick reads `gates_at` and either passes the gate (level, uniform,
//! key held), files its dependency (`access:<guild>`, the key's obtain),
//! or refuses the site (boat without a nav edge, underground).
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
//!   sails back for 30. The navigation rides both (`nav_overrides.json`
//!   `custom_edges` "npc_fare" + the disembark gangplanks; offline
//!   `orchestrator route 3029 3217 0 2915 3150 0 --paid` sails), so
//!   `BOAT_ROUTED` is true: a Karamja site (tuna, lobster, swordfish,
//!   Karamja gold) is a paid site, its round trip budgeted like the Al
//!   Kharid toll (planner `combat::pick_site_gated`, launch flag `boat`);
//! - the Wilderness is open (user 2026-11-16): its hunters count in the
//!   danger model like any other, the zone is never a gate;
//! - underground (level 0, z ≥ 6400) outside the Mining Guild: dungeons
//!   reached by trapdoors, ladders and ropes. A gather never goes down
//!   (`Gate::Underground`, impassable). A FIGHT does when the navigation
//!   reaches the camp (user 2026-11-21: the Karamja volcano demons behind
//!   the boat and the rope, the Edgeville dungeon by its trapdoor, the
//!   Asgarnian ice dungeon by the Mudskipper Point ladder):
//!   `fight_gates_at` lifts the generic gate on a camp position
//!   (`combat_spots::routed_underground`: `nav/tests/fight_sites.rs`
//!   proves a plan from the surface and back for each, `no_route`
//!   otherwise, e.g. Elvarg's lair behind its one-way secret door); the
//!   site's other gates stay (the boat, a key door), its danger and bank
//!   read at the spawn (`danger::fight_site_requirement`, `site_banks`
//!   priced by route through the ladders).
//! The Al Kharid toll is positional (which side the bot stands on), see
//! `toll::crossing`.

/// The nav can follow the Port Sarim ↔ Karamja sailors (see module doc).
pub const BOAT_ROUTED: bool = true;

/// Port Sarim dock tile the Karamja boat lands on coming back (the
/// disembark gangplank's telejump): a Karamja site's mainland anchor.
pub const KARAMJA_DOCK: (i64, i64, i64) = (3029, 3217, 0);

/// True when the site sits behind the Port Sarim ↔ Karamja boat.
pub fn behind_boat(x: i64, z: i64, level: i64) -> bool {
    gates_at(x, z, level).iter().any(|gate| matches!(gate, Gate::Boat { area: "karamja", .. }))
}

/// Where a site's banking and mainland distances read: the Port Sarim
/// dock for a Karamja site (no F2P bank on the island, every trip sails
/// back there), the site itself elsewhere.
pub fn mainland_anchor(x: i64, z: i64, level: i64) -> (i64, i64, i64) {
    if behind_boat(x, z, level) {
        KARAMJA_DOCK
    } else {
        (x, z, level)
    }
}

/// One gate a site sits behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// Guild door: the skill level and the uniform worn at the door.
    Guild { area: &'static str, skill: &'static str, level: i64, uniform: Option<&'static str> },
    /// Door a carried key opens.
    Key { area: &'static str, item: &'static str },
    /// Island reached by the Port Sarim boat (fare each way).
    Boat { area: &'static str, fare: i64 },
    /// Door the content script opens only above a quest-point total
    /// (Champions' Guild, `champions_guild.rs2`: 32). The planner's
    /// `Pass` opens it with the quest points of the finished quests
    /// (user 2026-11-16, Q7: Scavvo's and Valaine's stock), the nav with
    /// the `qp:32` tag the live varp satisfies.
    QuestPoints { area: &'static str, points: i64 },
    /// Door the content script opens only for a quest's own state (an
    /// item used on it, a quest stage): never opened by the planner.
    Quest { area: &'static str, quest: &'static str },
    /// Tiles no player ever reaches (a museum display behind barriers):
    /// the spawns there are scenery.
    Sealed { area: &'static str },
    /// Below ground with no gate the planner models.
    Underground,
}

impl Gate {
    /// Planner key of the gate (`access:<area>` blocks, logs).
    pub fn area(&self) -> &'static str {
        match self {
            Gate::Guild { area, .. } | Gate::Key { area, .. } | Gate::Boat { area, .. } | Gate::QuestPoints { area, .. } | Gate::Quest { area, .. } | Gate::Sealed { area } => area,
            Gate::Underground => "underground",
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
        // The guild's silver yard east of the door (walls x 2939..2943,
        // z 3286..3291, open only to the guild interior): live admin
        // 2026-11-15, the silver rock (2943,3290) read public and the
        // stand outside its wall answered "I can't reach that!".
        Area {
            x: (2939, 2943),
            z: (3285, 3291),
            levels: (0, 0),
            gate: Gate::Guild { area: "crafting_guild", skill: "crafting", level: 40, uniform: Some("brown_apron") },
            door: Some((2933, 3290, 0)),
        },
        // The guild's wheel room east of the door (x 2935..2938, z
        // 3289..3292: the four pottery wheels, the jug on its table; south
        // walls (2936..2937,3293) in m45_51), open only from the guild
        // interior: offline `route 2933 3295 0 2936 3290 0` crosses door
        // 2647; live 2026-11-19, pottery sent there below crafting 40
        // ("shaping consumed nothing" at the door, 65 failures).
        Area {
            x: (2935, 2938),
            z: (3289, 3292),
            levels: (0, 0),
            gate: Gate::Guild { area: "crafting_guild", skill: "crafting", level: 40, uniform: Some("brown_apron") },
            door: Some((2933, 3290, 0)),
        },
        // The guild's north room west of the door (walls x 2927..2932,
        // z up to 3292: pottery oven, amulet mould on the table), open
        // only from the guild interior: live admin2 2026-11-16, the mould
        // read public and the bot stood outside the west wall.
        Area {
            x: (2928, 2931),
            z: (3289, 3292),
            levels: (0, 0),
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
        // Champions' Guild (door 1805 at (3191,3363), 32 quest points):
        // nav reads every tile here NeedsRequirement (live 2026-11-14,
        // ryan_park sent to chickens at (3198,3355) through the door).
        // Its south row holds Scavvo upstairs at (3191,3351,1) (live
        // admin2 2026-11-16: the rune store read public, "no reachable
        // keeper" on every buy).
        Area {
            x: (3188, 3198),
            z: (3351, 3362),
            levels: (0, 1),
            gate: Gate::QuestPoints { area: "champions_guild", points: 32 },
            door: Some((3191, 3364, 0)),
        },
        // Draynor Manor closet (door 131 at (3107,3367), nav blocked):
        // `quest_haunted.rs2` opens it on the closet key used on it
        // only; the rubber tube spawns inside (live admin 2026-11-15).
        Area { x: (3108, 3112), z: (3366, 3368), levels: (0, 0), gate: Gate::Quest { area: "draynor_manor_closet", quest: "haunted" }, door: None },
        // Wydin's back room (door 2069 at (3012,3204), nav blocked):
        // `food_store.rs2` lets in Pirate's Treasure employees wearing
        // the white apron only; the white apron spawns inside.
        Area { x: (3009, 3011), z: (3205, 3210), levels: (0, 0), gate: Gate::Quest { area: "wydin_back_room", quest: "hunt" }, door: None },
        // Varrock museum armour display (marble 747 at (3254,3451..3453),
        // statues west, museum barriers east): the iron sword, platelegs
        // and platebody spawned there are never picked up (live 2026-11-16:
        // admin "nothing to pick", oscar_cole four budget-exhausted runs).
        Area { x: (3254, 3254), z: (3451, 3453), levels: (0, 0), gate: Gate::Sealed { area: "varrock_museum_display" }, door: None },
        // Karamja (F2P north half), south of the Rimmington coast.
        Area { x: (2740, 2975), z: (2880, 3186), levels: (0, 1), gate: Gate::Boat { area: "karamja", fare: crate::services::BOAT_FARE_GP }, door: None },
        // The volcano dungeon under Karamja (entrance (2856,3168) ->
        // (2856,9568); Elvarg's lair starts past the secret door at z
        // 9600): same boat, same fare back (live 2026-11-18, three bots
        // stranded 8 hours at (2836,9599) with 20 coins).
        Area { x: (2820, 2880), z: (9530, 9599), levels: (0, 0), gate: Gate::Boat { area: "karamja", fare: crate::services::BOAT_FARE_GP }, door: None },
        // Crandor: Ned's ship during Dragon Slayer only.
        Area { x: (2813, 2869), z: (3222, 3310), levels: (0, 0), gate: Gate::Boat { area: "crandor", fare: 0 }, door: None },
    ]
}

/// Ground tiles outside every gated area's door (`surface_of`): sites of
/// the bank table (`site_banks::catalogue`).
pub fn area_doors() -> Vec<(i64, i64, i64)> {
    areas().into_iter().filter_map(|area| area.door).collect()
}

/// Gates of the site (x, z, level): the most specific area first, then
/// the generic underground gate. Empty = open. The Wilderness is no gate
/// (user 2026-11-16: accessible to every bot, always).
pub fn gates_at(x: i64, z: i64, level: i64) -> Vec<Gate> {
    let mut out: Vec<Gate> = areas()
        .into_iter()
        .filter(|area| inside(area, x, z, level))
        .map(|area| area.gate)
        .collect();
    let modelled = out.iter().any(|gate| matches!(gate, Gate::Guild { .. }));
    if z >= 6400 && !modelled {
        out.push(Gate::Underground);
    }
    out
}

/// Gates of a FIGHT site (a camp's spawn, a drop farm's foe): `gates_at`
/// without the generic underground gate on a camp position the
/// navigation reaches from the surface and back
/// (`combat_spots::routed_underground`, user 2026-11-21); every other
/// gate stays (the volcano's Karamja boat, a key door, a guild). Any
/// other underground tile keeps `Gate::Underground`; gathers and
/// stations read `gates_at`.
pub fn fight_gates_at(x: i64, z: i64, level: i64) -> Vec<Gate> {
    let mut gates = gates_at(x, z, level);
    if crate::combat_spots::routed_underground(x, z, level) {
        gates.retain(|gate| *gate != Gate::Underground);
    }
    gates
}

/// Ground tile a guild site is reached from (outside its door, the
/// Mining Guild ladder top): its bank and danger read there (an upstairs
/// range or an underground rock has no bank on its own level). The site
/// itself elsewhere; an upstairs station with no bank on its own floor
/// (the Lumbridge mill hopper, level 2) reads at the ground tile below.
pub fn surface_of(x: i64, z: i64, level: i64) -> (i64, i64, i64) {
    if let Some(door) = areas().into_iter().find(|area| inside(area, x, z, level)).and_then(|area| area.door) {
        return door;
    }
    // No F2P booth stands on an upper floor (rev 274): an upstairs site
    // reads at the ground tile below, whatever bank its route reaches.
    if level > 0 && !crate::banks::all_banks().iter().any(|bank| !bank.members && bank.level == level) {
        return (x, z, 0);
    }
    (x, z, level)
}

/// Underground shop floors the nav routes: the Dwarven Mine below
/// Falador's trapdoor (`trapdoor_open_357` (3018, 3450) → (3018, 9850),
/// offline `orchestrator route 2965 3380 0 2998 9844 0`: 3 segments,
/// cost 257). Its keepers: Nurmof's pickaxes (2998, 9844), the only F2P
/// seller of iron to rune pickaxes; Drogo (3036, 9846); the dwarven
/// general store (2993, 9828). Only a keeper's purchase walks there
/// (`shopbuy` follows the nav route); gathers and drop farms still
/// refuse the underground (`Gate::Underground` stays on these tiles).
const ROUTED_SHOP_FLOORS: [((i64, i64), (i64, i64)); 1] = [((2985, 3045), (9820, 9855))];

/// True when an underground shop keeper at (x, z, level) is reachable
/// by the nav (`ROUTED_SHOP_FLOORS`).
pub fn shop_floor_routed(x: i64, z: i64, level: i64) -> bool {
    level == 0 && ROUTED_SHOP_FLOORS.iter().any(|((west, east), (south, north))| (*west..=*east).contains(&x) && (*south..=*north).contains(&z))
}

/// True when every bot passes the gate with coins alone: the Karamja
/// boat, part of the map like a road (user 2026-11-16), its round trip
/// carried in the pack.
pub fn fare_only(gate: &Gate) -> bool {
    matches!(gate, Gate::Boat { area: "karamja", .. }) && !impassable(gate)
}

/// True when no bot can ever pass the gate today (no nav edge, no model).
pub fn impassable(gate: &Gate) -> bool {
    match gate {
        // Only the Port Sarim boat has nav edges; Crandor (Ned's ship,
        // Dragon Slayer) stays out.
        Gate::Boat { area: "karamja", .. } => !BOAT_ROUTED,
        Gate::Boat { .. } => true,
        Gate::Underground | Gate::Sealed { .. } => true,
        Gate::Guild { .. } | Gate::Key { .. } | Gate::QuestPoints { .. } | Gate::Quest { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    /// The Dwarven Mine keepers are routed shop floors; the Mining Guild,
    /// other dungeons and the surface are not.
    #[test]
    fn dwarven_mine_keepers_are_routed() {
        use super::shop_floor_routed;
        for (x, z) in [(2998, 9844), (3036, 9846), (2993, 9828)] {
            assert!(shop_floor_routed(x, z, 0), "({x},{z})");
        }
        assert!(!shop_floor_routed(3040, 9740, 0), "the Mining Guild keeps its gate");
        assert!(!shop_floor_routed(3100, 9900, 0), "another dungeon");
        assert!(!shop_floor_routed(2998, 3444, 0), "surface");
    }

    /// User 2026-11-21: a fight goes underground where the navigation
    /// reaches the camp. The volcano demons keep only the boat (offline
    /// `route 3092 3245 0 2837 9565 0 --paid`: sail, the rope, 37 tiles);
    /// the Edgeville dungeon giants (trapdoor) and the ice dungeon
    /// (Mudskipper Point ladder) open; Elvarg's lair (one-way secret
    /// door) and the moss giants' hall (web) stay shut; a rock below
    /// ground keeps the gate (gathers never go down).
    #[test]
    fn routed_camps_lift_the_underground_gate_for_fights_only() {
        use super::fight_gates_at;
        let volcano = fight_gates_at(2837, 9565, 0);
        assert!(matches!(volcano.as_slice(), [Gate::Boat { area: "karamja", fare: 30 }]), "{volcano:?}");
        assert!(gates_at(2837, 9565, 0).contains(&Gate::Underground), "the plain gates keep it");
        assert!(fight_gates_at(3110, 9845, 0).is_empty(), "a hill giant: {:?}", fight_gates_at(3110, 9845, 0));
        assert!(fight_gates_at(3053, 9576, 0).is_empty(), "an ice warrior");
        assert!(fight_gates_at(2838, 9610, 0).contains(&Gate::Underground), "Elvarg's lair");
        assert!(fight_gates_at(3157, 9905, 0).contains(&Gate::Underground), "the moss giants behind the web");
        for (x, z, level) in crate::danger::sites("mining", "coal").into_iter().chain(crate::danger::sites("mining", "mithril_ore")) {
            let gates = gates_at(x, z, level);
            if z >= 6400 && !gates.iter().any(|gate| matches!(gate, Gate::Guild { .. })) {
                assert!(gates.iter().any(impassable), "underground rock ({x},{z}) stays shut: {gates:?}");
            }
        }
    }

    /// The guild's silver yard and the two quest-locked rooms are gated;
    /// the path outside the yard's east wall is open.
    #[test]
    fn walled_rooms_are_gated() {
        assert!(super::gates_at(2943, 3290, 0).iter().any(|gate| gate.area() == "crafting_guild"));
        assert!(super::gates_at(2944, 3290, 0).is_empty());
        assert!(super::gates_at(2928, 3290, 0).iter().any(|gate| gate.area() == "crafting_guild"), "the mould room");
        assert!(super::gates_at(2926, 3290, 0).is_empty(), "outside its west wall");
        assert!(super::gates_at(3254, 3452, 0).iter().any(super::impassable), "the museum display");
        assert!(super::gates_at(3111, 3367, 0).iter().any(|gate| gate.area() == "draynor_manor_closet"));
        assert!(super::gates_at(3010, 3208, 0).iter().any(|gate| gate.area() == "wydin_back_room"));
        assert!(super::gates_at(3191, 3351, 1).iter().any(|gate| gate.area() == "champions_guild"), "Scavvo's floor");
    }

    #[test]
    fn champions_guild_is_quest_point_gated() {
        let gates = super::gates_at(3197, 3355, 0);
        assert!(gates.iter().any(|gate| matches!(gate, super::Gate::QuestPoints { area: "champions_guild", points: 32 })));
        assert!(super::gates_at(3199, 3355, 0).is_empty(), "the field east of the guild is open");
    }

    use super::{behind_boat, gates_at, impassable, mainland_anchor, surface_of, Gate, KARAMJA_DOCK};

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
        assert_eq!(surface_of(3166, 3307, 2), (3166, 3307, 0), "the Lumbridge mill hopper reads downstairs");
        assert!(gates_at(2975, 3234, 0).is_empty(), "Rimmington gold is open");
        assert!(gates_at(2955, 3192, 0).is_empty(), "the Rimmington coast is mainland");
        let karamja = gates_at(2924, 3178, 0);
        assert!(matches!(karamja.as_slice(), [Gate::Boat { area: "karamja", fare: 30 }]), "{karamja:?}");
        assert!(!impassable(&karamja[0]), "the sailors have nav edges");
        assert!(impassable(&Gate::Boat { area: "crandor", fare: 0 }), "Ned's ship stays out");
        assert!(behind_boat(2924, 3178, 0) && !behind_boat(2955, 3192, 0));
        assert_eq!(mainland_anchor(2924, 3178, 0), KARAMJA_DOCK, "Karamja reads at the Port Sarim dock");
        assert_eq!(mainland_anchor(2975, 3234, 0), (2975, 3234, 0));
        assert!(gates_at(3050, 3704, 0).is_empty(), "the Wilderness is open");
        assert_eq!(gates_at(3100, 9900, 0), vec![Gate::Underground]);
        assert_eq!(gates_at(3232, 3203, 0), Vec::new(), "Lumbridge is open");
    }

    /// Every F2P lobster, tuna and swordfish spot is on Karamja: each is
    /// a boat site (paid, budgeted).
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

    /// Live 2026-11-18: alicefield, ryan_park and kevin_ellis looped
    /// NoRoute for 8 hours at (2836,9599), the volcano dungeon under
    /// Karamja, 20 coins carried for a 30-coin fare back: the dungeon was
    /// no boat area, so the run's Luthas job (`kit::stranded`) never
    /// started. The dungeon is behind the boat; Elvarg's lair under
    /// Crandor (past the secret door at z 9600) is not Karamja.
    #[test]
    fn the_volcano_dungeon_is_behind_the_boat() {
        for (x, z) in [(2836, 9599), (2856, 9568), (2845, 9580)] {
            assert!(behind_boat(x, z, 0), "({x},{z}) under Karamja: {:?}", gates_at(x, z, 0));
        }
        assert!(!behind_boat(2833, 9657, 0), "Elvarg's lair is Crandor's");
        assert!(!behind_boat(3222, 9618, 0), "another dungeon");
    }

    /// Live 2026-11-19 (frank_shaw crafting 27, pia_brown, 65 failures
    /// "shaping consumed nothing" at (2933,3289) and (3011,3355)): the
    /// four guild wheels (2935..2938, 3289..3291) read public, the brain
    /// sent pottery there, the walk died on the guild door (crafting 40 +
    /// worn brown apron, `crafting_guild.rs2`). Offline `orchestrator
    /// route 2933 3295 0 2936 3290 0`: 3 segments through door 2647. The
    /// corridor west (x 2934) and the row north (z 3292) stay open.
    #[test]
    fn guild_wheels_are_gated() {
        for station in crate::stations::stations_of("potterywheel") {
            let gates = gates_at(station.x, station.z, station.level);
            let guild = (2935..=2938).contains(&station.x) && (3289..=3291).contains(&station.z);
            assert_eq!(gates.iter().any(|gate| gate.area() == "crafting_guild"), guild, "wheel ({},{}) gates {gates:?}", station.x, station.z);
        }
        assert_eq!(surface_of(2936, 3290, 0), (2933, 3290, 0), "an inside wheel reads at the door");
        // The room reaches z 3292 (walls (2936,3293), (2937,3293), corners
        // (2935,3292), (2938,3292) in m45_51): the jug (2936,3292) lies on
        // its table, inside. The corridor west (x 2934) stays open.
        assert!(gates_at(2936, 3292, 0).iter().any(|gate| gate.area() == "crafting_guild"), "the jug is inside the room");
        for (x, z) in [(2934, 3291), (2934, 3289), (2934, 3292)] {
            assert!(gates_at(x, z, 0).is_empty(), "({x},{z}) is outside the wheel room");
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
