//! Tranche sizing (audit 2026-11-13 item A9): how many actions a reach
//! tranche takes and what those actions burn — recipe inputs, bait,
//! feathers, arrows, runes — from the XP the tranche must earn and the XP
//! one action pays. Pure arithmetic over the server tables.
//!
//! Server truth (rev 274):
//! - XP table in tenths (`gamedata::xp`), action XP in tenths in the dumps;
//! - ranged pays 4 XP per damage point (`combat.rs2` `stat_advance(ranged,
//!   $base)`, base = damage × 40 tenths); an arrow breaks one shot in five
//!   (`^dropammo_chance = 5`) and the rest falls under the target: the
//!   scripts never pick arrows back up, so every shot costs an arrow;
//! - magic pays the spell's base XP plus 2 XP per damage point;
//! - failed actions (iron smelts) burn their inputs without XP
//!   (`Way::success`).
//!
//! Fight damage is a model, not a table: a shot hits `HIT_ODDS` of the
//! time for half its max hit on average (ranged max hit from the bronze-
//! arrow formula, magic from the spell table).

use crate::method_way::{action_xp, magic_spell, method_way, spell_runes};

/// Share of shots or casts that land on the low-defence F2P spots.
pub const HIT_ODDS: f64 = 0.6;

/// Display XP between two levels (server table in tenths).
pub fn xp_between(from: u8, to: u8) -> f64 {
    let at = |level: u8| lifescape_gamedata::xp::xp_for_level(u32::from(level.clamp(1, 99))).unwrap_or(0);
    ((at(to) - at(from)).max(0)) as f64 / 10.0
}

/// Ranged max hit with bronze arrows (ranged strength 7) at `level`.
fn ranged_max_hit(level: u8) -> f64 {
    (0.5 + (f64::from(level) + 8.0) * (7.0 + 64.0) / 640.0).floor().max(1.0)
}

/// XP one action of `method` pays at `level` (fights: the damage model).
pub fn xp_per_action(method: &str, level: u8) -> Option<f64> {
    if method.starts_with("fight:ranged/") {
        return Some(4.0 * HIT_ODDS * ranged_max_hit(level) / 2.0);
    }
    if let Some(spell) = magic_spell(method) {
        let row = lifescape_gamedata::spells::all_spells().into_iter().find(|row| row.spell == spell)?;
        let damage = HIT_ODDS * row.maxhit.unwrap_or(0) as f64 / 2.0;
        return Some(row.xp as f64 / 10.0 + 2.0 * damage);
    }
    action_xp(method)
}

/// Actions a tranche from `from` to `to` takes with `method` (`None`: no
/// XP data for the method, e.g. melee fights, which burn nothing).
pub fn tranche_actions(method: &str, from: u8, to: u8) -> Option<i64> {
    let per = xp_per_action(method, from.max(1))?;
    (per > 0.0).then(|| (xp_between(from, to) / per).ceil() as i64)
}

/// What a tranche burns, (object, units): the way's consumed inputs (raw
/// fish, ores, logs, essence, bait) per action divided by the success
/// odds, a fight's arrows or the spell's runes per cast. Tools are not
/// consumption (`tools::required_tool`).
pub fn tranche_consumption(method: &str, from: u8, to: u8) -> Vec<(String, i64)> {
    let Some(actions) = tranche_actions(method, from, to) else { return Vec::new() };
    if actions <= 0 {
        return Vec::new();
    }
    if method.starts_with("fight:ranged/") {
        return vec![("bronze_arrow".to_string(), actions)];
    }
    if let Some(spell) = magic_spell(method) {
        return spell_runes(&spell).into_iter().map(|(rune, per_cast)| (rune, per_cast * actions)).collect();
    }
    let Some((_, way)) = method_way(method) else { return Vec::new() };
    let success = if way.success > 0.0 { way.success } else { 1.0 };
    let mut out: Vec<(String, i64)> = Vec::new();
    for (input, per_action) in &way.inputs {
        let units = (actions as f64 * (*per_action).max(1) as f64 / success).ceil() as i64;
        out.push((input.clone(), units));
    }
    // Thread wears out one per five sewings (`leather.rs2`).
    if way.tools.iter().any(|tool| tool == "thread") {
        out.push(("thread".to_string(), (actions + 4) / 5));
    }
    out
}

/// Units of `need` a tranche of `method` from `from` to `to` burns
/// (`None` when the tranche does not consume it).
pub fn tranche_need(method: &str, from: u8, to: u8, need: &str) -> Option<i64> {
    tranche_consumption(method, from, to).into_iter().find(|(item, _)| item == need).map(|(_, units)| units)
}

#[cfg(test)]
mod tests {
    use super::{tranche_actions, tranche_consumption, tranche_need, xp_between};

    #[test]
    fn xp_between_reads_the_server_table() {
        assert_eq!(xp_between(1, 2), 83.0);
        assert_eq!(xp_between(1, 10), 1154.0);
        assert_eq!(xp_between(10, 10), 0.0);
    }

    /// Fishing 20→30 on trout (50 XP a catch): 8 965 XP, 180 catches, one
    /// feather each (audit 2026-11-13: consumables were sized at 1 unit).
    #[test]
    fn bait_scales_with_the_tranche() {
        let actions = tranche_actions("gather:fishing/raw_trout", 20, 30).expect("trout xp");
        assert_eq!(actions, ((133_630.0 - 44_700.0) / 10.0 / 50.0_f64).ceil() as i64);
        assert_eq!(tranche_need("gather:fishing/raw_trout", 20, 30, "feather"), Some(actions));
        assert_eq!(tranche_need("gather:fishing/raw_shrimp", 1, 5, "feather"), None, "nets burn nothing");
    }

    /// Recipe inputs follow the XP: cooking shrimps 1→10 (30 XP each) eats
    /// 39 raw shrimps; iron smelting fails half the time, so every bar
    /// costs two ores; runecraft burns one essence per rune.
    #[test]
    fn recipe_inputs_follow_the_xp() {
        assert_eq!(tranche_need("cook/raw_shrimp", 1, 10, "raw_shrimp"), Some(39));
        let iron_bars = tranche_actions("smelt:iron_bar", 15, 20).expect("iron xp");
        assert_eq!(tranche_need("smelt:iron_bar", 15, 20, "iron_ore"), Some(iron_bars * 2));
        let airs = tranche_actions("runecraft:airrune", 1, 2).expect("air xp");
        assert_eq!(airs, 17, "83 XP at 5 XP a rune");
        assert_eq!(tranche_need("runecraft:airrune", 1, 2, "blankrune"), Some(17));
        assert_eq!(tranche_need("runecraft:airrune", 1, 2, "air_talisman"), None, "the talisman is a tool");
        assert!(tranche_need("jewelry:gold_ring", 5, 8, "gold_bar").is_some_and(|bars| bars > 10));
        assert_eq!(tranche_need("jewelry:gold_ring", 5, 8, "ring_mould"), None, "the mould is kept");
    }

    /// Fights burn ammunition: every shot an arrow, every cast the spell's
    /// runes (fire strike: 3 fire, 2 air, 1 mind).
    #[test]
    fn fights_burn_arrows_and_runes() {
        let arrows = tranche_need("fight:ranged/cows", 1, 10, "bronze_arrow").expect("arrows");
        assert!(arrows > 500, "1 154 XP at ~1.2 XP a shot, got {arrows}");
        let runes = tranche_consumption("fight:magic/fire_strike/cows", 13, 20);
        let casts = tranche_actions("fight:magic/fire_strike/cows", 13, 20).expect("casts");
        assert!(runes.contains(&("firerune".to_string(), casts * 3)), "{runes:?}");
        assert!(runes.contains(&("airrune".to_string(), casts * 2)), "{runes:?}");
        assert!(tranche_consumption("fight:melee/cows", 1, 10).is_empty(), "melee burns nothing");
    }
}
