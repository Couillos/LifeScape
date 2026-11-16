#![forbid(unsafe_code)]
//! Gear plan hysteresis (plan marché §3.5, C2). Live 2026-11-16: 77 % of
//! the true bid cancellations came from the gear plan: its budget was
//! 50 % of the wallet left free at the moment of each 60 s pass, so
//! every gain, spend or non-gear bid moved the dearest slot one tier up
//! or down, cancelled the bid it no longer wanted and re-placed the
//! other one (~5 300 "not wanted by the gear plan" a day).
//!
//! Now a plan is HELD per account:
//! - the budget is smoothed (`BUDGET_SMOOTHING` of each new reading);
//! - a held slot keeps its piece while its ceiling, scaled by the
//!   smoothed budget against the budget it was planned with, stays at or
//!   above `HOLD_CEILING_PCT` % of its initial ceiling; under that the
//!   slot (only it) takes the fresh plan's piece;
//! - a slot the plan left empty is added when the held ceilings leave
//!   room for it in the smoothed budget (adding never cancels);
//! - the whole plan is redone at most once per `GEAR_REPLAN_TICKS`, or at
//!   once on a tier gain (combat levels crossing a gear tier, a piece
//!   acquired or lost: the plan's signature changes).
//!
//! Pure: the account's memory comes in and goes out; `held_wants` keeps
//! the per-account table for the brain.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use lifescape_planner::gear_plan::GearWant;

/// Ticks between two full replans of one account (§3.5: 36 000 ticks,
/// 36 min at 60 ms, 6 h at 600 ms).
pub const GEAR_REPLAN_TICKS: u64 = 36_000;
/// A held slot keeps its piece down to this share of its initial ceiling.
pub const HOLD_CEILING_PCT: i64 = 70;
/// Weight of a new budget reading in the smoothed budget.
pub const BUDGET_SMOOTHING: f64 = 0.25;
/// Level thresholds of the F2P gear tiers (bronze 1 … rune 40, the
/// ranged and magic tiers on the same steps): crossing one is a tier gain.
pub const GEAR_TIER_LEVELS: [i64; 8] = [1, 5, 10, 20, 30, 40, 50, 60];

/// One slot of a held plan.
#[derive(Debug, Clone, PartialEq)]
pub struct HeldWant {
    pub want: GearWant,
    pub initial_ceiling: i64,
}

/// What the gear plan of one account remembers between passes.
#[derive(Debug, Clone, PartialEq)]
pub struct GearMemory {
    pub wants: Vec<HeldWant>,
    pub planned_tick: u64,
    pub budget_at_plan: i64,
    pub smoothed_budget: f64,
    pub signature: String,
}

/// Tier signature of a plan's inputs: the style, the gear tier of each
/// combat level, the wearables owned. A change replans at once.
pub fn gear_signature(style: &str, levels: &[(&str, i64)], owned_wearables: &[String]) -> String {
    let mut tiers: Vec<String> = levels
        .iter()
        .filter(|(skill, _)| matches!(*skill, "attack" | "strength" | "defence" | "ranged" | "magic" | "prayer"))
        .map(|(skill, level)| format!("{skill}:{}", GEAR_TIER_LEVELS.iter().filter(|tier| level >= tier).count()))
        .collect();
    tiers.sort();
    let mut owned = owned_wearables.to_vec();
    owned.sort();
    owned.dedup();
    format!("{style}|{}|{}", tiers.join(","), owned.join(","))
}

fn fresh_memory(plan: &dyn Fn(i64) -> Vec<GearWant>, budget: i64, smoothed: f64, now_tick: u64, signature: &str) -> GearMemory {
    let wants = plan(budget).into_iter().map(|want| HeldWant { initial_ceiling: want.ceiling, want }).collect();
    GearMemory { wants, planned_tick: now_tick, budget_at_plan: budget, smoothed_budget: smoothed, signature: signature.to_string() }
}

/// One pass: the wants to bid this pass and the memory to keep.
/// `plan(budget)` is the pure gear plan at a budget (`gear_plan::plan_gear`).
pub fn hold_plan(memory: Option<&GearMemory>, budget: i64, now_tick: u64, signature: &str, plan: &dyn Fn(i64) -> Vec<GearWant>) -> (Vec<GearWant>, GearMemory) {
    let smoothed = match memory {
        Some(known) => known.smoothed_budget + BUDGET_SMOOTHING * (budget as f64 - known.smoothed_budget),
        None => budget as f64,
    };
    let smoothed_coins = smoothed.round().max(0.0) as i64;
    let replan = match memory {
        None => true,
        Some(known) => known.signature != signature || now_tick.saturating_sub(known.planned_tick) >= GEAR_REPLAN_TICKS,
    };
    let next = match memory {
        Some(known) if !replan => held_memory(known, smoothed, smoothed_coins, plan),
        _ => fresh_memory(plan, smoothed_coins, smoothed, now_tick, signature),
    };
    (next.wants.iter().map(|held| held.want.clone()).collect(), next)
}

fn held_memory(known: &GearMemory, smoothed: f64, smoothed_coins: i64, plan: &dyn Fn(i64) -> Vec<GearWant>) -> GearMemory {
    let scale = if known.budget_at_plan > 0 { smoothed / known.budget_at_plan as f64 } else { 1.0 };
    let mut fresh: Option<Vec<GearWant>> = None;
    let mut wants: Vec<HeldWant> = Vec::new();
    for held in &known.wants {
        let ceiling = (held.initial_ceiling as f64 * scale).round() as i64;
        if ceiling.saturating_mul(100) >= held.initial_ceiling.saturating_mul(HOLD_CEILING_PCT) {
            let mut want = held.want.clone();
            want.ceiling = ceiling.max(1);
            wants.push(HeldWant { want, initial_ceiling: held.initial_ceiling });
            continue;
        }
        // A real budget drop: this slot (only it) takes the fresh piece.
        let fresh_wants = fresh.get_or_insert_with(|| plan(smoothed_coins));
        if let Some(want) = fresh_wants.iter().find(|want| want.wearpos == held.want.wearpos) {
            wants.push(HeldWant { initial_ceiling: want.ceiling, want: want.clone() });
        }
    }
    // Empty slots join when the held ceilings leave room (never a cancel).
    let mut committed: i64 = wants.iter().map(|held| held.want.ceiling).sum();
    let fresh_wants = fresh.get_or_insert_with(|| plan(smoothed_coins));
    for want in fresh_wants.iter() {
        if wants.iter().any(|held| held.want.wearpos == want.wearpos) || committed + want.ceiling > smoothed_coins {
            continue;
        }
        committed += want.ceiling;
        wants.push(HeldWant { initial_ceiling: want.ceiling, want: want.clone() });
    }
    GearMemory { wants, planned_tick: known.planned_tick, budget_at_plan: known.budget_at_plan, smoothed_budget: smoothed, signature: known.signature.clone() }
}

fn table() -> &'static Mutex<HashMap<String, GearMemory>> {
    static TABLE: OnceLock<Mutex<HashMap<String, GearMemory>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The account's held wants this pass (`hold_plan` over its memory).
pub fn held_wants(account: &str, budget: i64, now_tick: u64, signature: &str, plan: &dyn Fn(i64) -> Vec<GearWant>) -> Vec<GearWant> {
    let mut memories = table().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (wants, next) = hold_plan(memories.get(account), budget, now_tick, signature, plan);
    memories.insert(account.to_string(), next);
    wants
}

#[cfg(test)]
mod tests {
    use super::*;
    use lifescape_gamedata::gear::Style;

    fn price(item: &str) -> i64 {
        lifescape_gamedata::objs_by_name(item).into_iter().next().map(|obj| obj.alch_high).unwrap_or(1).max(1)
    }

    fn plan_at(budget: i64) -> Vec<GearWant> {
        let levels = [("attack", 40), ("strength", 40), ("defence", 40), ("hitpoints", 40)];
        let owned = vec!["bronze_sword".to_string()];
        lifescape_planner::gear_plan::plan_gear(Style::Melee, &levels, &[], &owned, budget, &price)
    }

    /// Wants of one pass that the next pass no longer wants: each one is
    /// a cancelled bid (`brain_gear::cancel_stale_bids`).
    fn cancels(passes: &[Vec<GearWant>]) -> usize {
        passes.windows(2).map(|pair| pair[0].iter().filter(|want| !pair[1].iter().any(|next| next.item == want.item)).count()).sum()
    }

    /// Plan marché C2/M2: a wallet that swings ±20 % around its level
    /// for 10 passes (60 s each) cancels no gear bid; the unheld plan
    /// on the same readings does.
    #[test]
    fn a_wallet_swinging_twenty_percent_cancels_nothing() {
        let base = (1..200)
            .map(|step| step * 500)
            .find(|budget| {
                let readings: Vec<Vec<GearWant>> = (0..10).map(|pass| plan_at(if pass % 2 == 0 { budget * 6 / 5 } else { budget * 4 / 5 })).collect();
                cancels(&readings) > 0
            })
            .expect("some budget where the raw plan churns");
        let readings: Vec<i64> = (0..10).map(|pass| if pass % 2 == 0 { base * 6 / 5 } else { base * 4 / 5 }).collect();
        let raw: Vec<Vec<GearWant>> = readings.iter().map(|budget| plan_at(*budget)).collect();
        assert!(cancels(&raw) > 0, "the old plan churns at {base}");
        let mut memory: Option<GearMemory> = None;
        let mut held: Vec<Vec<GearWant>> = Vec::new();
        for (pass, budget) in readings.iter().enumerate() {
            let (wants, next) = hold_plan(memory.as_ref(), *budget, pass as u64 * 1000, "melee|40", &plan_at);
            held.push(wants);
            memory = Some(next);
        }
        assert!(!held[0].is_empty());
        assert_eq!(cancels(&held), 0, "held plan at {base}: {held:?}");
    }

    /// A tier gain or the replan horizon redoes the plan; a real budget
    /// drop (under 70 % of the initial ceiling) moves the slot.
    #[test]
    fn replans_on_tier_gain_horizon_or_a_real_drop() {
        let first = GearWant { wearpos: 3, item: "rune_scimitar".to_string(), gain: 9, expected: 100, ceiling: 100 };
        let cheaper = GearWant { wearpos: 3, item: "adamant_scimitar".to_string(), gain: 5, expected: 40, ceiling: 40 };
        let plan = |budget: i64| if budget >= 100 { vec![first.clone()] } else { vec![cheaper.clone()] };
        let (_, memory) = hold_plan(None, 100, 0, "a", &plan);
        let (kept, memory) = hold_plan(Some(&memory), 85, 10, "a", &plan);
        assert_eq!(kept[0].item, "rune_scimitar", "smoothed 96: held");
        let (moved, _) = hold_plan(Some(&memory), 85, 10, "b", &plan);
        assert_eq!(moved[0].item, "adamant_scimitar", "tier gain: replanned");
        let (late, _) = hold_plan(Some(&memory), 85, GEAR_REPLAN_TICKS + 10, "a", &plan);
        assert_eq!(late[0].item, "adamant_scimitar", "horizon: replanned");
        let mut low = Some(memory);
        for pass in 0..8 {
            let (wants, next) = hold_plan(low.as_ref(), 10, 20 + pass, "a", &plan);
            low = Some(next);
            if wants[0].item == "adamant_scimitar" {
                return;
            }
        }
        panic!("a lasting drop to 10 % never moved the slot");
    }

    #[test]
    fn the_signature_follows_tiers_not_every_level() {
        let owned = vec!["bronze_sword".to_string()];
        let at = |attack: i64| gear_signature("melee", &[("attack", attack), ("woodcutting", attack)], &owned);
        assert_eq!(at(31), at(39), "inside the 30-39 tier");
        assert_ne!(at(39), at(40), "rune opens at 40");
    }
}
