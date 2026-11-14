//! Compact, bounded memory of a life (user request 2026-11-14: done and
//! abandoned tasks leave the graph so it never grows forever). What the
//! brain still needs from finished nodes lives here, never in the graph:
//! the id high-water mark (ids are never reused), the current goal's
//! first node number (older abandons are retired), the quests the graph
//! proved, lifetime counters and the last `RECENT_CAP` finished nodes.
//! File key `memory` (absent in older files: inferred at the first
//! retirement, `lifecycle_retire`).

use lifescape_json::{object, str_value, Value};

use crate::model::NodeKind;

/// Finished nodes kept as compact records (dashboard, janitor streak,
/// goal root proof). The oldest go first.
pub const RECENT_CAP: usize = 40;
/// Longest `last_error` kept in a record.
pub const RECORD_ERROR_MAX_CHARS: usize = 200;

/// One finished node, retired from the graph.
#[derive(Debug, Clone, PartialEq)]
pub struct Retired {
    pub id: String,
    pub kind: NodeKind,
    /// `done` or `abandoned`.
    pub outcome: String,
    pub last_error: String,
}

/// Bounded memory carried by `Life::memory`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LifeMemory {
    /// Highest node number ever issued (`n<k>`): a retired id is never
    /// handed out again, so a late report for it stays unambiguous.
    pub last_number: usize,
    /// First node number of the current goal (`append_goal`). Abandoned
    /// nodes below it served a closed goal and are retired. `None` in
    /// files written before the memory: inferred once.
    pub floor: Option<usize>,
    /// Quests the graph proved complete (`quests_done_in_graph`), the
    /// tutorial included; bounded by the quest list.
    pub quests_done: Vec<String>,
    /// Lifetime counts of retired nodes (dashboard).
    pub done_total: u64,
    pub abandoned_total: u64,
    /// The last `RECENT_CAP` retired nodes, oldest first.
    pub recent: Vec<Retired>,
}

impl LifeMemory {
    /// Record retired nodes, keeping only the last `RECENT_CAP`.
    pub fn remember(&mut self, records: Vec<Retired>) {
        for record in &records {
            if record.outcome == "done" {
                self.done_total += 1;
            } else {
                self.abandoned_total += 1;
            }
        }
        self.recent.extend(records);
        let excess = self.recent.len().saturating_sub(RECENT_CAP);
        if excess > 0 {
            self.recent.drain(..excess);
        }
    }
}

fn retired_to_value(record: &Retired) -> Value {
    let (kind, params) = crate::model::kind_to_parts(&record.kind);
    object(vec![
        ("id", str_value(&record.id)),
        ("kind", str_value(&kind)),
        ("params", params),
        ("outcome", str_value(&record.outcome)),
        ("last_error", str_value(&record.last_error)),
    ])
}

fn retired_from_value(value: &Value) -> Option<Retired> {
    Some(Retired {
        id: value.get("id").as_str()?.to_string(),
        kind: crate::model::kind_from_parts(value.get("kind").as_str().unwrap_or(""), &value.get("params"))?,
        outcome: value.get("outcome").as_str().unwrap_or("done").to_string(),
        last_error: value.get("last_error").as_str().unwrap_or("").to_string(),
    })
}

pub fn memory_to_value(memory: &LifeMemory) -> Value {
    object(vec![
        ("last_number", Value::Int(memory.last_number as i64)),
        ("floor", memory.floor.map(|floor| Value::Int(floor as i64)).unwrap_or(Value::Null)),
        ("quests_done", Value::Arr(memory.quests_done.iter().map(|quest| str_value(quest)).collect())),
        ("done_total", Value::Int(memory.done_total as i64)),
        ("abandoned_total", Value::Int(memory.abandoned_total as i64)),
        ("recent", Value::Arr(memory.recent.iter().map(retired_to_value).collect())),
    ])
}

/// Absent or null (older files) reads as the default memory.
pub fn memory_from_value(value: &Value) -> LifeMemory {
    let count = |key: &str| value.get(key).as_i64().unwrap_or(0).max(0);
    LifeMemory {
        last_number: count("last_number") as usize,
        floor: value.get("floor").as_i64().map(|floor| floor.max(0) as usize),
        quests_done: value
            .get("quests_done")
            .as_arr()
            .map(|rows| rows.iter().filter_map(|row| row.as_str().map(str::to_string)).collect())
            .unwrap_or_default(),
        done_total: count("done_total") as u64,
        abandoned_total: count("abandoned_total") as u64,
        recent: value.get("recent").as_arr().map(|rows| rows.iter().filter_map(retired_from_value).collect()).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::{memory_from_value, memory_to_value, LifeMemory, Retired, RECENT_CAP};
    use crate::model::NodeKind;

    fn record(number: usize, outcome: &str) -> Retired {
        Retired { id: format!("n{number}"), kind: NodeKind::SpaceCleanup, outcome: outcome.to_string(), last_error: String::new() }
    }

    #[test]
    fn memory_roundtrips_and_older_files_read_as_default() {
        let mut memory = LifeMemory { last_number: 205, floor: Some(180), quests_done: vec!["sheep".to_string()], ..LifeMemory::default() };
        memory.remember(vec![record(1, "done"), Retired { kind: NodeKind::ObtainItems { item: "logs".to_string(), count: 28 }, ..record(2, "abandoned") }]);
        assert_eq!(memory_from_value(&memory_to_value(&memory)), memory, "exact round trip");
        assert_eq!(memory_from_value(&lifescape_json::Value::Null), LifeMemory::default(), "no memory key: default");
    }

    #[test]
    fn recent_records_stay_capped_and_counters_keep_counting() {
        let mut memory = LifeMemory::default();
        for number in 0..(RECENT_CAP * 3) {
            memory.remember(vec![record(number, if number % 2 == 0 { "done" } else { "abandoned" })]);
        }
        assert_eq!(memory.recent.len(), RECENT_CAP);
        assert_eq!(memory.recent.last().map(|row| row.id.as_str()), Some(format!("n{}", RECENT_CAP * 3 - 1).as_str()), "newest last");
        assert_eq!(memory.done_total + memory.abandoned_total, (RECENT_CAP * 3) as u64);
    }
}
