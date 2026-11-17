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

/// Choices from the guides kept (`orchestrator economy`, section « choix
/// des bots »). The oldest go first.
pub const PICKS_CAP: usize = 40;

/// One choice a bot made from a guide (plan rendements R7): the line,
/// its score, how many lines it was drawn among, whether the exploration
/// slice chose it (D14).
#[derive(Debug, Clone, PartialEq)]
pub struct PickRecord {
    /// Unix seconds.
    pub at: i64,
    pub node: String,
    /// `money` or `training`.
    pub kind: String,
    /// The trained skill (training), empty for money.
    pub skill: String,
    pub method: String,
    pub score: f64,
    /// The class drawn in (`guide_pick::class_order`).
    pub class: u8,
    /// Lines in the 15 % band (D10), lines of the class offered.
    pub band: usize,
    pub candidates: usize,
    pub explored: bool,
    /// The node's previous pick taken again (no draw).
    pub sticky: bool,
}

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
    /// The last `trade_craft::LOTS_CAP` lots of 100 % market (plan
    /// rendements R5), oldest first: predicted profit, purchases, products
    /// made (the economy report adds the realized coins).
    pub trades: Vec<crate::trade_craft::TradeLot>,
    /// The last `PICKS_CAP` choices from the guides (R7), oldest first.
    pub picks: Vec<PickRecord>,
    /// Draws this life made (the counter of `guide_pick::seed_for`: the
    /// next draw of the same node is another one).
    pub draws: u64,
}

impl LifeMemory {
    /// Record one choice from a guide (R7), keeping the last `PICKS_CAP`;
    /// the draw counter moves on.
    pub fn note_pick(&mut self, record: PickRecord) {
        self.draws = self.draws.saturating_add(1);
        self.picks.push(record);
        let excess = self.picks.len().saturating_sub(PICKS_CAP);
        if excess > 0 {
            self.picks.drain(..excess);
        }
    }

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

fn pick_to_value(record: &PickRecord) -> Value {
    object(vec![
        ("at", Value::Int(record.at)),
        ("node", str_value(&record.node)),
        ("kind", str_value(&record.kind)),
        ("skill", str_value(&record.skill)),
        ("method", str_value(&record.method)),
        ("score", Value::Float(record.score)),
        ("class", Value::Int(i64::from(record.class))),
        ("band", Value::Int(record.band as i64)),
        ("candidates", Value::Int(record.candidates as i64)),
        ("explored", Value::Bool(record.explored)),
        ("sticky", Value::Bool(record.sticky)),
    ])
}

/// One recorded choice from its value (`None`: no method).
pub fn pick_from_value(value: &Value) -> Option<PickRecord> {
    let count = |key: &str| value.get(key).as_i64().unwrap_or(0).max(0);
    Some(PickRecord {
        at: value.get("at").as_i64().unwrap_or(0),
        node: value.get("node").as_str().unwrap_or("").to_string(),
        kind: value.get("kind").as_str().unwrap_or("").to_string(),
        skill: value.get("skill").as_str().unwrap_or("").to_string(),
        method: value.get("method").as_str()?.to_string(),
        score: value.get("score").as_f64().unwrap_or(0.0),
        class: count("class").min(255) as u8,
        band: count("band") as usize,
        candidates: count("candidates") as usize,
        explored: value.get("explored").as_bool().unwrap_or(false),
        sticky: value.get("sticky").as_bool().unwrap_or(false),
    })
}

/// The recorded choices of a life file's `memory` value.
pub fn picks_from_value(value: &Value) -> Vec<PickRecord> {
    value.get("picks").as_arr().map(|rows| rows.iter().filter_map(pick_from_value).collect()).unwrap_or_default()
}

pub fn memory_to_value(memory: &LifeMemory) -> Value {
    object(vec![
        ("last_number", Value::Int(memory.last_number as i64)),
        ("floor", memory.floor.map(|floor| Value::Int(floor as i64)).unwrap_or(Value::Null)),
        ("quests_done", Value::Arr(memory.quests_done.iter().map(|quest| str_value(quest)).collect())),
        ("done_total", Value::Int(memory.done_total as i64)),
        ("abandoned_total", Value::Int(memory.abandoned_total as i64)),
        ("recent", Value::Arr(memory.recent.iter().map(retired_to_value).collect())),
        ("trades", Value::Arr(memory.trades.iter().map(crate::trade_craft::lot_to_value).collect())),
        ("picks", Value::Arr(memory.picks.iter().map(pick_to_value).collect())),
        ("draws", Value::Int(memory.draws.min(i64::MAX as u64) as i64)),
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
        trades: crate::trade_craft::lots_from_value(&value.get("trades")),
        picks: picks_from_value(value),
        draws: count("draws") as u64,
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
        memory.note_pick(super::PickRecord { at: 5, node: "n9".to_string(), kind: "money".to_string(), skill: String::new(), method: "fight:melee/cows".to_string(), score: 812.5, class: 0, band: 3, candidates: 7, explored: true, sticky: false });
        assert_eq!(memory.draws, 1);
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
