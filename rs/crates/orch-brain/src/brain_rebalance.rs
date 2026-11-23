#![forbid(unsafe_code)]
//! The one-time rebalance of the fleet's ambitions (plan progression A4
//! (2), user 2026-11-21): the ONLY exception to A2 (an ambition never
//! changes before it is achieved). It ran once: the plan of the whole fleet
//! (`lifescape_planner::rebalance`: the value of the unserved demand, the
//! climb included, shared by the bots already on each kind) was written
//! with its marker to `run/ambition_rebalance_a4c.json` (named `a4c`
//! since 2026-11-21: the value discounts a chain step nobody covers, the
//! earlier markers `a4`, `a4b` stay on disk, unread) and every bot applied
//! ITS row in its own launch pass.
//!
//! Since lot 5 of the launch refactor (2026-11-23) the rule no longer runs
//! in the launch pass: every row of the marker was applied (71 of 71, no
//! life left to move), and the plan, its fleet reads and the per-pass
//! marker read are gone. What stays: the marker's shape and its pure
//! decisions (`step`, `covers`, `rows_of`, `write_marker_at`), read by the
//! dashboard (`web_brain`) and by `orchestrator migrate-lives`
//! (`migrate_cmd`), which reports the rows a marker would still hold; a row
//! gives a new ambition drawn from the live fleet, so it cannot be applied
//! offline.
//!
//! Never planned, never changed: `lifescape_planner::PROTECTED_ACCOUNTS`
//! (Zezima, N0valyfe, Yogosun) and every operator ambition (pins, market
//! makers, alcher, overall).

use lifescape_json::{object, str_value, stringify_pretty, Value};
use lifescape_planner::Reassignment;
#[cfg(test)]
use lifescape_planner::upkeep_ambition::RebalanceRow;
#[cfg(test)]
use lifescape_planner::Life;

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
    crate::paths::run_dir().join("ambition_rebalance_a4c.json")
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

/// Applies one row to the bot's own life (the unit tests of the rule as the
/// loop ran it until lot 5: verdict, then the draw over the live snapshot).
#[cfg(test)]
fn apply_row(life: &mut Life, live: &Value, row: &PlanRow) -> bool {
    use lifescape_planner::upkeep_ambition::{row_applied, row_verdict, RowVerdict};
    let row = RebalanceRow { from: row.from.clone(), kind: row.kind.clone(), why: row.why.clone(), numbers: row.numbers.clone() };
    match row_verdict(life, &row) {
        RowVerdict::Nothing | RowVerdict::Kept(_) => false,
        RowVerdict::Give => {
            crate::brain_ambition::assign_rebalanced(life, live, &row.kind, &row.why, &row.numbers);
            row_applied(life, &row)
        }
    }
}

#[cfg(test)]
#[path = "brain_rebalance_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "brain_rebalance_sim_tests.rs"]
mod sim_tests;
