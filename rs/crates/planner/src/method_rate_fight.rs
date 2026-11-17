//! Fight actions of the rates prior (`method_rate`, split for rule 4):
//! one attempt = one kill of the spot's first rated NPC.
//! - melee: `gamedata::melee_math::estimate` (hit odds against the NPC's
//!   defence roll, max hit, kill overhead), every melee stat at the key's
//!   level, hitpoints at least 10, the key's worn weapon;
//! - ranged and magic: the server's rolls (`player_combat_stat.rs2`,
//!   `npc_defence_roll_specific` in `npc_combat.rs2`): a hit lands when
//!   `randominc(attack) > randominc(defence)` (`melee_math::hit_chance`),
//!   attack = effective level × (gear bonus + 64) (ranged: level + 8 + 3
//!   on the accurate style; magic: level + 8 + 1, its fixed style
//!   bonus), defence = (NPC defence + 9) × (`rangedefence` + 64), or
//!   (NPC magic + 9) × (`magicdefence` + 64) (dump `npc_combat.json`);
//!   a landed hit deals `randominc(maxhit)`, half the max hit on average
//!   (ranged: `combat_maxhit(effective × (arrow strength + 64))`, bronze
//!   arrows unless the key's gear names others). A shot every 4 ticks
//!   (shortbow, the server's default `attackrate`), a cast every 5
//!   (`player_magic.rs2`); an arrow per shot (the scripts never pick
//!   them back up), the spell's runes per cast.

use lifescape_gamedata::melee_math::{estimate, hit_chance, max_hit, Fighter, KILL_OVERHEAD_TICKS, PLAYER_ATTACK_TICKS};
use lifescape_gamedata::npc_combat::NpcCombat;

use crate::method_rate::{Action, PriorError, PriorQuery};

/// Ticks between two shots of a shortbow (default `attackrate`).
pub const RANGED_ATTACK_TICKS: f64 = 4.0;
/// Ticks between two combat casts.
pub const MAGIC_ATTACK_TICKS: f64 = 5.0;
/// Ranged style bonus of the accurate style (`combat_get_damagestyle_
/// bonuses`: `^style_ranged_accurate` = +3 ranged).
pub const RANGED_ACCURATE_BONUS: i64 = 3;
/// Magic's fixed style bonus (`player_combat_stat.rs2`: always +1).
pub const MAGIC_STYLE_BONUS: i64 = 1;
/// Ranged strength of a bronze arrow (`rangebonus`), the ammo the prior
/// burns when the key's gear names none.
pub const BRONZE_ARROW_STRENGTH: i64 = 7;

fn spot_target(method: &str) -> Result<(String, NpcCombat), PriorError> {
    let key = crate::method_way::fight_spot(method).ok_or_else(|| PriorError::NoWay(method.to_string()))?;
    let spot = lifescape_gamedata::combat_spots::by_key(key).ok_or_else(|| PriorError::NoWay(method.to_string()))?;
    spot.npc_names
        .iter()
        .find_map(|name| lifescape_gamedata::npc_combat::by_name(name).map(|block| (name.to_string(), block)))
        .ok_or_else(|| PriorError::NoTarget(key.to_string()))
}

fn melee_fighter(level: u8, tool: &str) -> Fighter {
    let level = i64::from(level.max(1));
    let gear: Vec<lifescape_gamedata::equipment::Wearable> = tool.split('+').filter_map(lifescape_gamedata::equipment::by_name).collect();
    Fighter::naked(level, level, level, level.max(10), level).wearing(&gear)
}

/// Kill ticks of a ranged or magic attacker landing `odds` of its
/// attacks for half of `most` on average, one every `interval` ticks.
fn kill_ticks(npc: &NpcCombat, odds: f64, most: f64, interval: f64) -> Option<f64> {
    let per_tick = odds * most / 2.0 / interval;
    (per_tick > 0.0).then(|| npc.hitpoints.max(1) as f64 / per_tick)
}

/// Gear a key's tool names (`shortbow+bronze_arrow`).
fn worn(tool: &str) -> Vec<lifescape_gamedata::equipment::Wearable> {
    tool.split('+').filter_map(lifescape_gamedata::equipment::by_name).collect()
}

fn attack_bonus(gear: &[lifescape_gamedata::equipment::Wearable], kind: &str) -> i64 {
    gear.iter().flat_map(|item| item.attack.iter()).filter(|(name, _)| name == kind).map(|(_, bonus)| *bonus).sum()
}

/// Odds a ranged shot lands on `npc` and its max hit, at `level` with
/// the gear `tool` names (module doc).
pub fn ranged_roll(npc: &NpcCombat, level: u8, tool: &str) -> (f64, i64) {
    let gear = worn(tool);
    let effective = i64::from(level.max(1)) + 8 + RANGED_ACCURATE_BONUS;
    let attack = effective * (attack_bonus(&gear, "range") + 64);
    let defence = (npc.defence + 9) * (npc.ranged_defence + 64);
    let arrows = gear.iter().filter(|item| item.rangebonus >= 0).map(|item| item.rangebonus).max().unwrap_or(BRONZE_ARROW_STRENGTH);
    (hit_chance(attack, defence), max_hit(effective, arrows))
}

/// Odds a combat cast lands on `npc` at `level` with the gear `tool`
/// names (the max hit is the spell's).
pub fn magic_odds(npc: &NpcCombat, level: u8, tool: &str) -> f64 {
    let effective = i64::from(level.max(1)) + 8 + MAGIC_STYLE_BONUS;
    let attack = effective * (attack_bonus(&worn(tool), "magic") + 64);
    let defence = (npc.magic + 9) * (npc.magic_defence + 64);
    hit_chance(attack, defence)
}

fn kill_units(npc_name: &str, hitpoints: i64) -> Vec<(&'static str, String, f64)> {
    vec![("kill", npc_name.to_string(), 1.0), ("damage", npc_name.to_string(), hitpoints.max(1) as f64)]
}

/// Expected drops of one kill of a `fight:<style>/...` method's NPC (the
/// spot's first rated NPC, its F2P drop table: rate × mean quantity),
/// debug names. Empty when the spot or its table is unknown.
pub fn fight_drops_per_kill(method: &str) -> Vec<(String, f64)> {
    // Static drop tables, read once per method (the guides ask it per
    // level: 0.5 ms a read).
    static MEMO: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Vec<(String, f64)>>>> = std::sync::OnceLock::new();
    crate::guide_methods::memo(&MEMO, method.to_string(), || compute_fight_drops_per_kill(method))
}

fn compute_fight_drops_per_kill(method: &str) -> Vec<(String, f64)> {
    let Ok((npc_name, _)) = spot_target(method) else { return Vec::new() };
    let Some(npc) = lifescape_gamedata::npcs_by_name(&npc_name).into_iter().next() else { return Vec::new() };
    let mut out: Vec<(String, f64)> = Vec::new();
    for drop in lifescape_gamedata::drops::drops_for_npc(npc.npc_id) {
        let per_kill = drop.expected_per_kill();
        if per_kill <= 0.0 || drop.obj.is_empty() {
            continue;
        }
        match out.iter_mut().find(|(item, _)| *item == drop.obj) {
            Some(known) => known.1 += per_kill,
            None => out.push((drop.obj.clone(), per_kill)),
        }
    }
    out
}

/// One kill of a `fight:<style>/...` method.
pub fn fight_action(query: &PriorQuery) -> Result<Action, PriorError> {
    let method = query.method.as_str();
    let (npc_name, npc) = spot_target(method)?;
    let sites = crate::method_way::fight_spot(method).map(lifescape_gamedata::combat_spots::positions).unwrap_or_default();
    let mut units = kill_units(&npc_name, npc.hitpoints);
    let mut flows: Vec<(String, &'static str, f64)> = Vec::new();
    let ticks = if method.starts_with("fight:ranged/") {
        let (odds, most) = ranged_roll(&npc, query.level, &query.tool);
        let killing = kill_ticks(&npc, odds, most.max(1) as f64, RANGED_ATTACK_TICKS).ok_or_else(|| PriorError::NoTarget(npc_name.clone()))?;
        flows.push(("bronze_arrow".to_string(), "ammo", -(killing / RANGED_ATTACK_TICKS)));
        killing + KILL_OVERHEAD_TICKS
    } else if let Some(spell) = crate::method_way::magic_spell(method) {
        let row = lifescape_gamedata::spells::all_spells().into_iter().find(|row| row.spell == spell).ok_or_else(|| PriorError::NoOdds(spell.clone()))?;
        let killing = kill_ticks(&npc, magic_odds(&npc, query.level, &query.tool), row.maxhit.unwrap_or(0) as f64, MAGIC_ATTACK_TICKS).ok_or_else(|| PriorError::NoOdds(format!("{spell} max hit")))?;
        let casts = killing / MAGIC_ATTACK_TICKS;
        flows.extend(row.runes.iter().map(|(rune, count)| (rune.to_string(), "rune", -(*count as f64) * casts)));
        units.push(("cast", spell, casts));
        killing + KILL_OVERHEAD_TICKS
    } else {
        estimate(&melee_fighter(query.level, &query.tool), &npc, 0.0).ok_or_else(|| PriorError::NoTarget(npc_name.clone()))?.cycle_ticks
    };
    Ok(Action { ticks: ticks.max(PLAYER_ATTACK_TICKS), success: 1.0, flows, units, slots: 0.0, sites, banks: false, depletion_ticks: 0.0, trip: None })
}

/// One kill of the melee drop farm `farm:<item>`: the first NPC dropping
/// it with a combat block, its drop rate times the mean quantity.
pub fn drop_action(method: &str, query: &PriorQuery) -> Result<Action, PriorError> {
    let item = method.strip_prefix("farm:").ok_or_else(|| PriorError::NoWay(method.to_string()))?;
    let way = lifescape_gamedata::obtain::ways(item)
        .into_iter()
        .filter(|way| way.kind == lifescape_gamedata::obtain::WayKind::Drop && way.rate.is_some_and(|rate| rate > 0.0))
        .find(|way| lifescape_gamedata::npc_combat::by_name(&way.detail).is_some())
        .ok_or_else(|| PriorError::NoWay(method.to_string()))?;
    let npc = lifescape_gamedata::npc_combat::by_name(&way.detail).ok_or_else(|| PriorError::NoTarget(way.detail.clone()))?;
    let found = estimate(&melee_fighter(query.level, &query.tool), &npc, 0.0).ok_or_else(|| PriorError::NoTarget(way.detail.clone()))?;
    let per_kill = way.rate.unwrap_or(0.0) * way.qty.map(|(low, high)| (low + high) as f64 / 2.0).unwrap_or(1.0);
    Ok(Action {
        ticks: found.cycle_ticks.max(PLAYER_ATTACK_TICKS),
        success: 1.0,
        flows: vec![(item.to_string(), "loot", per_kill)],
        units: kill_units(&way.detail, npc.hitpoints),
        slots: per_kill,
        sites: way.sites,
        banks: false,
        depletion_ticks: 0.0,
        trip: None,
    })
}

