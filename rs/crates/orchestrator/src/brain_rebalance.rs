#![forbid(unsafe_code)]
//! The one-time rebalance of the fleet's ambitions (plan progression A4
//! (2), user 2026-11-21): the ONLY exception to A2 (an ambition never
//! changes before it is achieved). The first launch pass that meets no
//! marker computes the plan for the whole fleet
//! (`lifescape_planner::rebalance`: the value of the unserved demand, the
//! climb included, shared by the bots already on each kind), writes it
//! with its marker to `run/ambition_rebalance_a4b.json` and logs it bot by
//! bot with its figures; every bot then applies ITS row in its own launch
//! pass (its life is saved by its own thread, never by another's). With
//! the marker present the pass never runs again, whatever the restarts;
//! after it A2 holds (an ambition changes only once achieved). The marker
//! is named `a4b` since 2026-11-21: the value now discounts a chain step
//! nobody covers (`planner::chain_coverage`) and the pass runs once more
//! with it (the first marker, `a4`, stays on disk, unread).
//!
//! Never planned, never changed: `lifescape_planner::PROTECTED_ACCOUNTS`
//! (Zezima, N0valyfe, Yogosun) and every operator ambition (pins, market
//! makers, alcher, overall): their kinds count as holders, nothing else.
//! The plan waits until every active life (saved in the last
//! `ACTIVE_LIFE_SECS`) has its job (`covers`), so a start that registers
//! the bots one by one never plans a partial fleet.

use std::sync::Mutex;

use lifescape_json::{object, parse, str_value, stringify_pretty, Value};
use lifescape_planner::{Life, Reassignment};

use crate::daemon::Orchestrator;

/// A life saved within this many seconds belongs to a running bot.
pub const ACTIVE_LIFE_SECS: u64 = 3600;

/// One bot's row of the plan.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanRow {
    pub account: String,
    pub from: String,
    pub kind: String,
    pub why: String,
    pub numbers: String,
}

/// The persisted plan: when it was computed, the rows, who applied theirs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Marker {
    pub computed_secs: i64,
    pub plan: Vec<PlanRow>,
    pub applied: Vec<String>,
}

pub fn marker_path() -> std::path::PathBuf {
    crate::paths::run_dir().join("ambition_rebalance_a4b.json")
}

/// One process at a time reads-modifies-writes the marker.
fn marker_guard() -> &'static Mutex<()> {
    static GUARD: Mutex<()> = Mutex::new(());
    &GUARD
}

impl Marker {
    pub fn to_value(&self) -> Value {
        let rows = self
            .plan
            .iter()
            .map(|row| {
                object(vec![
                    ("account", str_value(&row.account)),
                    ("from", str_value(&row.from)),
                    ("kind", str_value(&row.kind)),
                    ("why", str_value(&row.why)),
                    ("numbers", str_value(&row.numbers)),
                ])
            })
            .collect();
        object(vec![
            ("version", Value::Int(1)),
            ("computed_secs", Value::Int(self.computed_secs)),
            ("plan", Value::Arr(rows)),
            ("applied", Value::Arr(self.applied.iter().map(|name| str_value(name)).collect())),
        ])
    }

    pub fn from_value(value: &Value) -> Option<Marker> {
        let text = |row: &Value, key: &str| row.get(key).as_str().map(str::to_string);
        let plan = value
            .get("plan")
            .as_arr()?
            .iter()
            .map(|row| Some(PlanRow { account: text(row, "account")?, from: text(row, "from")?, kind: text(row, "kind")?, why: text(row, "why")?, numbers: text(row, "numbers")? }))
            .collect::<Option<Vec<_>>>()?;
        let applied = value.get("applied").as_arr()?.iter().filter_map(|name| name.as_str().map(str::to_string)).collect();
        Some(Marker { computed_secs: value.get("computed_secs").as_i64()?, plan, applied })
    }

    /// The row `account` still has to apply (case-insensitive).
    pub fn pending_for(&self, account: &str) -> Option<&PlanRow> {
        if self.applied.iter().any(|name| name.eq_ignore_ascii_case(account)) {
            return None;
        }
        self.plan.iter().find(|row| row.account.eq_ignore_ascii_case(account))
    }
}

/// Plan rows of a planner rebalance (the protected and operators never
/// appear: the planner leaves them out).
pub fn rows_of(moved: &[Reassignment]) -> Vec<PlanRow> {
    moved
        .iter()
        .map(|row| PlanRow { account: row.account.clone(), from: row.from.clone().unwrap_or_default(), kind: row.pick.kind.clone(), why: row.pick.why(), numbers: row.pick.numbers() })
        .collect()
}

/// True when every active life has its job: the plan covers the whole
/// fleet, never the few bots a start had registered so far.
pub fn covers(fleet: &std::collections::BTreeSet<String>, active_lives: &[String]) -> bool {
    !fleet.is_empty() && active_lives.iter().all(|name| fleet.contains(name))
}

fn read_marker() -> Option<Marker> {
    let text = std::fs::read_to_string(marker_path()).ok()?;
    match parse(&text).ok().and_then(|value| Marker::from_value(&value)) {
        Some(marker) => Some(marker),
        None => {
            crate::logln!("brain rebalance: {} unreadable, left as is (never planned again over it)", marker_path().display());
            Some(Marker::default())
        }
    }
}

/// The marker could not be written: nothing is applied (typed, no fallback).
#[derive(Debug, Clone, PartialEq)]
pub enum MarkerError {
    Write { path: String, cause: String },
}

impl std::fmt::Display for MarkerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MarkerError::Write { path, cause } => write!(formatter, "rebalance marker {path} not written: {cause}"),
        }
    }
}

/// Writes the marker next to its final name, then renames it.
pub fn write_marker_at(path: &std::path::Path, marker: &Marker) -> Result<(), MarkerError> {
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, stringify_pretty(&marker.to_value()))
        .and_then(|()| std::fs::rename(&temporary, path))
        .map_err(|error| MarkerError::Write { path: path.display().to_string(), cause: error.to_string() })
}

fn write_marker(marker: &Marker) -> Result<(), MarkerError> {
    write_marker_at(&marker_path(), marker)
}

/// Set once the marker cannot be written: the pass stops for this process
/// (the plan is never recomputed at every step).
fn blocked() -> &'static std::sync::atomic::AtomicBool {
    static BLOCKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    &BLOCKED
}

/// Active lives on disk (saved within `ACTIVE_LIFE_SECS`), their accounts.
fn active_lives() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(crate::brain::lives_dir()) else { return Vec::new() };
    let excluded = crate::economy_guard::excluded_accounts();
    entries
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        .filter(|entry| entry.metadata().and_then(|meta| meta.modified()).ok().and_then(|at| at.elapsed().ok()).is_some_and(|age| age.as_secs() <= ACTIVE_LIFE_SECS))
        .filter_map(|entry| entry.path().file_stem().and_then(|stem| stem.to_str()).map(str::to_string))
        .filter(|name| !excluded.contains(&name.to_lowercase()))
        .collect()
}

/// What one bot's pass does with the marker: the marker to persist (when
/// it changed) and the row this bot still has to apply.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub persist: Option<Marker>,
    pub row: Option<PlanRow>,
}

/// Pure core of `pass`: with a marker, never planned again (a marker, even
/// an empty one, is the « done once » stamp); without, `plan` runs (`None`:
/// the fleet is not complete, nothing persisted). The bot's pending row,
/// if any, is returned and marked applied in the marker to persist.
pub fn step(existing: Option<Marker>, plan: &dyn Fn() -> Option<Marker>, account: &str) -> Step {
    let mut marker = match existing {
        Some(marker) => marker,
        None => match plan() {
            Some(marker) => marker,
            None => return Step { persist: None, row: None },
        },
    };
    let row = marker.pending_for(account).cloned();
    if row.is_some() {
        marker.applied.push(account.to_string());
    }
    Step { persist: Some(marker), row }
}

/// The launch pass of one bot: plans the fleet when no marker exists,
/// then applies this bot's row. True when its life changed (save it).
pub(crate) fn pass(orch: &Orchestrator, life: &mut Life, live: &Value) -> bool {
    if life.ambition.is_none() {
        return false;
    }
    if blocked().load(std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    let _guard = marker_guard().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let existing = read_marker();
    let fresh = existing.is_none();
    let outcome = step(existing, &|| plan_fleet(orch, life, live), &life.account);
    if let Some(marker) = &outcome.persist {
        if fresh || outcome.row.is_some() {
            if let Err(error) = write_marker(marker) {
                // Nothing applies without its marker: the pass stops here.
                crate::logln!("brain rebalance: {error}; nothing applied, the pass stops for this run");
                blocked().store(true, std::sync::atomic::Ordering::Relaxed);
                return false;
            }
        }
    }
    let Some(row) = outcome.row else { return false };
    apply_row(life, live, &row)
}

/// Computes the plan over every fleet life (`brain_ambition::fleet_lives`),
/// logging each bot with its figures. `None`: the fleet is not complete yet.
fn plan_fleet(orch: &Orchestrator, life: &Life, live: &Value) -> Option<Marker> {
    let fleet = crate::brain_ambition::fleet_names();
    let active = active_lives();
    if !covers(&fleet, &active) {
        crate::logln!("brain rebalance: waiting for the fleet ({} jobs, {} active lives)", fleet.len(), active.len());
        return None;
    }
    let (bots, _) = crate::brain_ambition::fleet_bots(orch, life, live);
    let demand = crate::market_demand::kind_demand();
    let moved = crate::brain_value::with_context(&demand.value, &demand.surplus, |context| lifescape_planner::rebalance(&bots, context));
    let rows = rows_of(&moved);
    for bot in &bots {
        if lifescape_planner::protected(&bot.account) {
            crate::logln!("brain rebalance {}: kept {} (protected account, never touched)", bot.account, bot.kind.clone().unwrap_or_default());
        } else if bot.operator || bot.market_maker {
            crate::logln!("brain rebalance {}: kept {} (operator ambition, never touched)", bot.account, bot.kind.clone().unwrap_or_default());
        }
    }
    for row in &rows {
        if row.from == row.kind {
            crate::logln!("brain rebalance {}: kept {} ({})", row.account, row.kind, row.numbers);
        } else {
            crate::logln!("brain rebalance {}: {} -> {} ({})", row.account, if row.from.is_empty() { "none" } else { &row.from }, row.kind, row.numbers);
        }
    }
    let changing = rows.iter().filter(|row| row.from != row.kind).count();
    crate::logln!("brain rebalance: plan of {} bots, {changing} change ambition, marker {}", rows.len(), marker_path().display());
    let computed_secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0);
    Some(Marker { computed_secs, plan: rows, applied: Vec::new() })
}

/// Applies one row to the bot's own life.
fn apply_row(life: &mut Life, live: &Value, row: &PlanRow) -> bool {
    let account = life.account.clone();
    let Some(held) = life.ambition.as_ref() else { return false };
    if lifescape_planner::pinned(held) || lifescape_planner::maker_role(life).is_some() || lifescape_planner::protected(&account) {
        crate::logln!("brain rebalance {account}: kept {} (operator or protected, never touched)", held.kind);
        return false;
    }
    // The row was planned for the ambition the bot held then: when A4 or
    // anything else changed it since, the row is stale and applies nothing.
    if held.kind != row.from {
        crate::logln!("brain rebalance {account}: row stale (planned from {:?}, the bot holds {}), nothing applied", row.from, held.kind);
        return false;
    }
    if held.kind == row.kind {
        crate::logln!("brain rebalance {account}: kept {} ({})", held.kind, row.numbers);
        return false;
    }
    crate::brain_ambition::assign_rebalanced(life, live, &row.kind, &row.why, &row.numbers);
    life.ambition.as_ref().is_some_and(|now| now.kind == row.kind)
}

#[cfg(test)]
#[path = "brain_rebalance_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "brain_rebalance_sim_tests.rs"]
mod sim_tests;
