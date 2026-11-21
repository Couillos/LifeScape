//! Life-file form of an ambition (`ambition_to_value` / `ambition_from_value`),
//! split from `ambition.rs` by role; the paths `ambition::ambition_*` stay.

use lifescape_json::{object, str_value, Value};

use super::{Ambition, Milestone, MilestoneStatus, Pin, Revision, Target};

pub fn ambition_to_value(ambition: &Ambition) -> Value {
    let milestones = ambition
        .milestones
        .iter()
        .map(|stone| {
            object(vec![
                ("id", str_value(&stone.id)),
                ("label", str_value(&stone.label)),
                ("kind", str_value(stone.target.kind())),
                ("target", stone.target.to_value()),
                ("status", str_value(stone.status.as_str())),
                ("progress", Value::Arr(vec![Value::Int(stone.progress.0), Value::Int(stone.progress.1)])),
                ("blocked_count", Value::Int(i64::from(stone.blocked_count))),
                ("origin", str_value(&stone.origin)),
            ])
        })
        .collect();
    let revisions = ambition
        .revisions
        .iter()
        .map(|revision| {
            object(vec![
                ("tick", Value::Int(revision.tick as i64)),
                ("at_secs", Value::Int(revision.at_secs)),
                ("from", str_value(&revision.from)),
                ("to", str_value(&revision.to)),
                ("why", str_value(&revision.why)),
            ])
        })
        .collect();
    object(vec![
        ("kind", str_value(&ambition.kind)),
        ("label", str_value(&ambition.label)),
        ("groups", Value::Arr(ambition.groups.iter().map(|group| str_value(group)).collect())),
        ("why", str_value(&ambition.why)),
        ("milestones", Value::Arr(milestones)),
        ("drawn_at_tick", Value::Int(ambition.drawn_at_tick as i64)),
        ("drawn_at", Value::Int(ambition.drawn_at)),
        ("rung", Value::Int(ambition.rung as i64)),
        ("revisions", Value::Arr(revisions)),
        (
            "pin",
            match &ambition.pin {
                Some(pin) => object(vec![("skill", str_value(&pin.skill)), ("level", Value::Int(i64::from(pin.level)))]),
                None => Value::Null,
            },
        ),
        ("draw_rule", str_value(&ambition.draw_rule)),
    ])
}

/// `None` for a missing section (old files) or an unreadable one.
pub fn ambition_from_value(value: &Value) -> Option<Ambition> {
    let kind = value.get("kind").as_str()?.to_string();
    let text = |row: &Value, key: &str| row.get(key).as_str().unwrap_or("").to_string();
    let milestones = value
        .get("milestones")
        .as_arr()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let target = Target::from_value(row.get("kind").as_str().unwrap_or(""), &row.get("target"))?;
                    let progress = row.get("progress");
                    let cell = |slot: usize| progress.as_arr().and_then(|cells| cells.get(slot).and_then(|cell| cell.as_i64())).unwrap_or(0);
                    Some(Milestone {
                        id: text(row, "id"),
                        label: text(row, "label"),
                        target,
                        status: MilestoneStatus::parse(row.get("status").as_str().unwrap_or("pending")),
                        progress: (cell(0), cell(1)),
                        blocked_count: row.get("blocked_count").as_i64().unwrap_or(0).clamp(0, i64::from(u32::MAX)) as u32,
                        origin: row.get("origin").as_str().unwrap_or("ladder").to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let revisions = value
        .get("revisions")
        .as_arr()
        .map(|rows| {
            rows.iter()
                .map(|row| Revision {
                    tick: row.get("tick").as_i64().unwrap_or(0).max(0) as u64,
                    at_secs: row.get("at_secs").as_i64().or_else(|| row.get("at").as_i64()).unwrap_or(0),
                    from: text(row, "from"),
                    to: text(row, "to"),
                    why: text(row, "why"),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Ambition {
        kind,
        label: text(value, "label"),
        groups: value.get("groups").as_arr().map(|rows| rows.iter().filter_map(|row| row.as_str().map(str::to_string)).collect()).unwrap_or_default(),
        why: text(value, "why"),
        milestones,
        drawn_at_tick: value.get("drawn_at_tick").as_i64().unwrap_or(0).max(0) as u64,
        drawn_at: value.get("drawn_at").as_i64().unwrap_or(0),
        rung: value.get("rung").as_i64().unwrap_or(0).max(0) as usize,
        revisions,
        // Absent before pins (2026-11-14): a drawn ambition.
        pin: value.get("pin").get("skill").as_str().map(|skill| Pin {
            skill: skill.to_string(),
            level: value.get("pin").get("level").as_i64().unwrap_or(99).clamp(2, 99) as u8,
        }),
        // Absent before the D3 mark (2026-11-19): drawn by the old rule.
        draw_rule: text(value, "draw_rule"),
    })
}
