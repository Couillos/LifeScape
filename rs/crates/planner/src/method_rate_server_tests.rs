//! Priors pinned to the server (rev 274 scripts) family by family, and
//! the generic guards: no prior above the physical ceiling, none more
//! than ×2 away from a reference measure (steady-state runs of the live
//! fleet, contract 2, n ≥ 3, or a bot log's cadence).

use super::{action_of, method_prior, units_ceiling_per_tick, PriorQuery, RatePrior, INSTANT_USE_TICKS, RATE_WINDOW_TICKS};

fn query(method: &str, level: u8) -> PriorQuery {
    PriorQuery { method: method.to_string(), level, tool: String::new(), site: None }
}

fn at(method: &str, level: u8, tool: &str, site: Option<(i64, i64, i64)>) -> PriorQuery {
    PriorQuery { method: method.to_string(), level, tool: tool.to_string(), site }
}

fn gained(prior: &RatePrior, item: &str) -> f64 {
    prior.flows.iter().filter(|(name, cause, qty)| name == item && *qty > 0.0 && (*cause == "product" || *cause == "ground_spawn")).map(|(_, _, qty)| qty).sum()
}

/// Gem cutting (`uncut_gem.rs2`): `[opheldu,chisel]` cuts one stone at
/// once, no `p_delay`, no make-X: one client op a tick. The load is 27
/// (28 slots beside the chisel), its bank visit open + deposit + close
/// and one withdraw-X (button + count): 5 ops, never 3.
#[test]
fn gem_cutting_is_one_instant_use_per_stone() {
    let prior = method_prior(&query("cut:sapphire", 20)).expect("sapphire");
    assert_eq!(prior.ticks_per_action, INSTANT_USE_TICKS);
    assert!((prior.bank_ticks_per_action - 5.0 / 27.0).abs() < 1e-9, "{prior:?}");
    assert!(prior.actions_per_6000 < RATE_WINDOW_TICKS);
}

/// Smithing (`smithing.rs2` `p_delay(1)` + `p_delay(2)`) 5 ticks an
/// item; cooking 3 (`p_delay(1)`); filling 2 (`p_delay(0)`); low
/// alchemy 3, high alchemy 5 (`alchemy.rs2`).
#[test]
fn one_shot_actions_follow_their_p_delay() {
    assert_eq!(action_of(&query("smith:bronze_dagger", 1)).expect("smith").ticks, 5.0);
    assert_eq!(action_of(&query("cook/raw_shrimp", 1)).expect("cook").ticks, 3.0);
    assert_eq!(action_of(&query("fill:bucket_empty", 1)).expect("fill").ticks, 2.0);
    assert_eq!(action_of(&query("alch:iron_dagger", 21)).expect("low").ticks, 3.0);
    assert_eq!(action_of(&query("alch:iron_dagger", 55)).expect("high").ticks, 5.0);
}

/// Mining (`mining.rs2`): every success empties the rock for its
/// `rock_respawnrate`; the rocks near the worked one share the load.
/// Silver (200 ticks) at Rimmington's six rocks: an ore at most every
/// 200 / 6 ticks, whatever the pickaxe.
#[test]
fn mining_waits_the_respawn_of_its_rocks() {
    let site = Some((2940, 3286, 0));
    let prior = method_prior(&at("gather:mining/silver_ore", 99, "rune_pickaxe", site)).expect("silver");
    let cluster = crate::method_rate_trip::cluster("silver_ore", &lifescape_gamedata::obtain::ways("silver_ore")[0].sites, (2940, 3286, 0));
    let per_ore = prior.ticks_per_action / prior.success;
    assert!(per_ore + 1e-9 >= 200.0 / cluster.count, "{per_ore} ticks an ore for {cluster:?}");
    // Essence never depletes: a roll every rate + 1, nothing more.
    let essence = method_prior(&at("essence:blankrune", 30, "mithril_pickaxe", None)).expect("essence");
    assert_eq!(essence.ticks_per_action, 5.0);
}

/// Woodcutting (`woodcutting.rs2`): a normal tree falls with every log
/// (120 + random(80) ticks), a yew with one in 8 (200 ticks): the
/// normal tree's log costs the walk to the next tree, the yew's barely.
#[test]
fn trees_fall_as_the_server_fells_them() {
    let logs = method_prior(&at("gather:woodcut/logs", 99, "rune_axe", None)).expect("logs");
    let yew = method_prior(&at("gather:woodcut/yew_logs", 99, "rune_axe", None)).expect("yew");
    assert!(logs.ticks_per_action > super::CHOP_TICKS, "a felled tree sends to the next: {logs:?}");
    assert!(yew.ticks_per_action / yew.success < logs.ticks_per_action / logs.success + 200.0 / 8.0);
}

/// Bank walks: the Mining Guild's coal climbs its ladder (not 6400 tiles
/// across the underground offset); the essence mine rides Aubury's
/// teleport (6 ticks) and the portal (3).
#[test]
fn bank_trips_follow_the_way_in_and_out() {
    let guild = method_prior(&at("gather:mining/coal", 70, "adamant_pickaxe", Some((3044, 9753, 0)))).expect("guild");
    assert!(guild.bank_ticks_per_action * 27.0 < 200.0, "{guild:?}");
    let essence = method_prior(&at("essence:blankrune", 60, "adamant_pickaxe", None)).expect("essence");
    let trip = essence.bank_ticks_per_action * 27.0;
    assert!(trip > 9.0 && trip < 200.0, "{trip}");
}

/// Ground spawns and sheep: a taken spawn respawns (obj `respawnrate`), a
/// shorn sheep regrows 100 ticks (`shear_sheep.rs2`): the spawns near
/// the worked one share them, never the world's thousands of spawns.
#[test]
fn spawns_share_the_cluster_not_the_world() {
    let logs = method_prior(&query("ground:logs", 1)).expect("ground logs");
    let respawn = lifescape_gamedata::objs_by_name("logs")[0].respawn_ticks as f64;
    let spawns = crate::method_way::method_way("ground:logs").map(|(_, way)| way.sites.len()).unwrap_or(1) as f64;
    assert!(spawns > 10.0, "logs spawn all over the world: {spawns}");
    assert!(logs.ticks_per_action > respawn / spawns, "{logs:?}");
    let wool = method_prior(&at("gather:sheep/wool", 1, "shears", None)).expect("wool");
    assert!(wool.ticks_per_action / wool.success > 3.0, "{wool:?}");
}

/// Runecraft (`runecraft.rs2`): the pack crafts at once (`p_delay(3)`),
/// with the ruins' `p_delay(1)` and the exit portal per load.
#[test]
fn runecraft_load_counts_ruins_and_portal() {
    let action = action_of(&query("runecraft:airrune", 1)).expect("air");
    assert!((action.ticks - (4.0 + super::RUINS_ENTRY_TICKS + super::ALTAR_EXIT_TICKS) / 27.0).abs() < 1e-9);
}

/// Generic guard: across the catalogue at several levels, no prior
/// yields above the physical ceiling (server action ticks, certain
/// success), no action is shorter than a tick but a pack's share, and
/// none exceeds one action a tick.
#[test]
fn no_prior_beats_the_physical_ceiling() {
    let mut checked = 0;
    for entry in crate::guide_methods::catalogue() {
        let gate = crate::guide_methods::gate_level(&entry.method);
        for level in [1u8, 30, 60, 99] {
            let level = level.max(gate);
            let tool = crate::guide_methods::tool_at(&entry.method, &entry.style, level);
            let ask = PriorQuery { method: entry.method.clone(), level, tool, site: None };
            let (Ok(prior), Ok(ceiling)) = (method_prior(&ask), units_ceiling_per_tick(&ask)) else { continue };
            checked += 1;
            assert!(prior.actions_per_6000 <= RATE_WINDOW_TICKS + 1e-9, "{}: {prior:?}", entry.method);
            if !entry.method.starts_with("runecraft:") {
                assert!(prior.ticks_per_action + 1e-9 >= INSTANT_USE_TICKS, "{}: {prior:?}", entry.method);
            }
            for (kind, subject, per_6000) in &prior.units {
                let limit: f64 = ceiling.iter().filter(|(unit, name, _)| unit == kind && name == subject).map(|(_, _, per_tick)| per_tick * RATE_WINDOW_TICKS).sum();
                assert!(*per_6000 <= limit * (1.0 + 1e-9), "{} L{level}: {kind}:{subject} {per_6000} above {limit}", entry.method);
            }
        }
    }
    assert!(checked > 500, "{checked}");
}

/// Generic guard: each prior lies within ×2 of its reference measure:
/// steady-state contract-2 runs of the fleet (n ≥ 3, at the key's
/// level, tool and site), or a bot log's cadence: wool at Lumbridge's
/// field, a wool every 20 ticks plus the 98-tile bank walk; gem cutting,
/// 26 stones in 34 ticks plus a 15-tick bank visit; low alchemy, a cast
/// every 3 ticks, 27 a load plus the bank visit.
#[test]
fn priors_stay_within_twice_their_reference_measure() {
    let references: [(PriorQuery, &str, f64); 9] = [
        (at("essence:blankrune", 63, "adamant_pickaxe", None), "blankrune", 532.0),
        (at("gather:mining/coal", 73, "adamant_pickaxe", Some((3044, 9753, 0))), "coal", 128.0),
        (at("gather:mining/gold_ore", 51, "mithril_pickaxe", Some((3294, 3288, 0))), "gold_ore", 34.0),
        (at("gather:mining/mithril_ore", 70, "adamant_pickaxe", Some((3053, 9737, 0))), "mithril_ore", 33.0),
        (at("gather:woodcut/yew_logs", 88, "mithril_axe", Some((3248, 3472, 0))), "yew_logs", 66.0),
        (at("gather:woodcut/logs", 88, "mithril_axe", Some((3245, 3400, 0))), "logs", 104.0),
        (at("gather:sheep/wool", 1, "shears", Some((3195, 3265, 0))), "wool", 27.0 / (27.0 * 20.0 + 2.0 * 98.0) * RATE_WINDOW_TICKS),
        (at("cut:sapphire", 44, "chisel", None), "sapphire", 26.0 / (34.0 + 15.0) * RATE_WINDOW_TICKS),
        (at("alch:steel_sq_shield", 31, "", None), "steel_sq_shield", 27.0 / (27.0 * 3.0 + 15.0) * RATE_WINDOW_TICKS),
    ];
    for (ask, item, reference) in references {
        let prior = method_prior(&ask).unwrap_or_else(|error| panic!("{}: {error}", ask.method));
        let made = if ask.method.starts_with("alch:") { -prior.flows.iter().filter(|(name, _, _)| name == item).map(|(_, _, qty)| qty).sum::<f64>() } else { gained(&prior, item) };
        let ratio = made / reference;
        assert!((0.5..=2.0).contains(&ratio), "{} L{}: prior {made:.1} vs reference {reference:.1} (x{ratio:.2})", ask.method, ask.level);
    }
}
