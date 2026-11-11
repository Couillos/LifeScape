//! `verify` asserts: the six observable checks, all against a pre-run baseline.
//!
//! Position, inventory (pack plus bank: cooked loads land in the bank),
//! inventory delta, skill XP, new game messages (matched by tick, the
//! snapshots keep only the last 25) and visible NPCs. Never the exit code:
//! a `give` answers Ok on a full pack, so only the visible state proves it.

#![forbid(unsafe_code)]

use lifescape_json::Value;

use crate::verify_scenario::Assert;

// -- observable state --------------------------------------------------------

fn normalize_item(name: &str) -> String {
    name.to_ascii_lowercase().replace(' ', "_")
}

fn item_matches(entry: &str, want: &str) -> bool {
    let have = normalize_item(entry);
    let need = normalize_item(want);
    have == need || have.contains(&need) || need.contains(&have)
}

/// Pack plus bank: cooked loads land in the bank, the pack alone would
/// read an empty delta after a banked run.
pub fn item_count(state: &Value, item: &str) -> i64 {
    let mut total = 0;
    for key in ["inv", "bank"] {
        for entry in state.get(key).as_arr().cloned().unwrap_or_default() {
            if entry.get("name").as_str().is_some_and(|name| item_matches(name, item)) {
                total += entry.get("count").as_i64().unwrap_or(0);
            }
        }
    }
    total
}

fn skill_xp(state: &Value, skill: i64) -> Option<i64> {
    state.get("skills").as_arr().cloned().unwrap_or_default().iter().find_map(|entry| {
        if entry.get("id").as_i64() == Some(skill) { entry.get("xp").as_i64() } else { None }
    })
}

pub fn position_of(state: &Value) -> Option<(i64, i64, i64)> {
    let pos = state.get("position").as_arr().cloned().unwrap_or_default();
    match (pos.first().and_then(|v| v.as_i64()), pos.get(1).and_then(|v| v.as_i64()), pos.get(2).and_then(|v| v.as_i64())) {
        (Some(level), Some(x), Some(z)) => Some((level, x, z)),
        _ => None,
    }
}

pub fn check_assert(assert: &Assert, before: &Value, after: &Value) -> Result<(), String> {
    match assert {
        Assert::Position { level, x, z, radius } => {
            let Some((found_level, found_x, found_z)) = position_of(after) else {
                return Err("position: no position in show".to_string());
            };
            let dist = (found_x - x).abs() + (found_z - z).abs();
            if found_level == *level && dist <= *radius {
                Ok(())
            } else {
                Err(format!("position: want ({level},{x},{z})±{radius}, at ({found_level},{found_x},{found_z})"))
            }
        }
        Assert::Inventory { item, min } => {
            let count = item_count(after, item);
            if count >= *min { Ok(()) } else { Err(format!("inventory: want {item}≥{min}, have {count}")) }
        }
        Assert::InventoryDelta { item, min_delta } => {
            let delta = item_count(after, item) - item_count(before, item);
            if delta >= *min_delta {
                Ok(())
            } else {
                Err(format!("inventory_delta: want {item}Δ≥{min_delta}, got {delta}"))
            }
        }
        Assert::Xp { skill, name, min_gain } => {
            match (skill_xp(before, *skill), skill_xp(after, *skill)) {
                (Some(was), Some(now)) if now - was >= *min_gain => Ok(()),
                (Some(was), Some(now)) => Err(format!("xp: want {name}Δ≥{min_gain}, got {} ({}→{})", now - was, was, now)),
                _ => Err(format!("xp: no {name} skill in show")),
            }
        }
        Assert::Message { contains } => {
            // Both snapshots keep only the last 25 messages: skip by
            // index breaks when the window is full, so compare ticks.
            let since = before
                .get("message_ticks")
                .as_arr()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|tick| tick.as_i64())
                .max()
                .unwrap_or(-1);
            let messages = after.get("messages").as_arr().cloned().unwrap_or_default();
            let ticks = after.get("message_ticks").as_arr().cloned().unwrap_or_default();
            let found = messages.iter().zip(ticks.iter()).any(|(line, tick)| {
                tick.as_i64().is_some_and(|tick| tick > since)
                    && line.as_str().is_some_and(|text| text.to_ascii_lowercase().contains(&contains.to_ascii_lowercase()))
            });
            if found { Ok(()) } else { Err(format!("message: no new message contains {contains:?}")) }
        }
        Assert::Npc { name, max_dist } => {
            let Some((_, here_x, here_z)) = position_of(after) else {
                return Err("npc: no position in show".to_string());
            };
            let seen = after.get("npcs").as_arr().cloned().unwrap_or_default().iter().any(|npc| {
                let near = npc.get("x").as_i64().is_some_and(|x| (x - here_x).abs() <= *max_dist)
                    && npc.get("z").as_i64().is_some_and(|z| (z - here_z).abs() <= *max_dist);
                near && npc.get("name").as_str().is_some_and(|label| label.to_ascii_lowercase().contains(&name.to_ascii_lowercase()))
            });
            if seen { Ok(()) } else { Err(format!("npc: no {name:?} within {max_dist}")) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lifescape_json as json;

    fn state_fixture() -> Value {
        json::parse(
            r#"{
                "account": "t40",
                "position": [0, 3223, 3217],
                "ticks": 120,
                "inv": [{"id": 1, "name": "Wool", "count": 3}],
                "bank": [{"id": 2, "name": "Shrimp", "count": 5}],
                "skills": [{"id": 7, "level": 3, "xp": 130}, {"id": 10, "level": 5, "xp": 600}],
                "messages": ["Welcome to RuneScape.", "You cook the shrimp."],
                "message_ticks": [0, 118],
                "npcs": [{"name": "Sheep", "x": 3222, "z": 3216}]
            }"#,
        )
        .expect("fixture parses")
    }

    fn empty_state() -> Value {
        json::parse(
            r#"{"account": "t40", "position": [0, 3220, 3217], "ticks": 100, "inv": [], "bank": [],
                "skills": [{"id": 7, "level": 3, "xp": 100}], "messages": ["Welcome to RuneScape."], "message_ticks": [0], "npcs": []}"#,
        )
        .expect("fixture parses")
    }

    /// All six assert kinds check the observable state, deltas against the
    /// pre-run baseline, never the exit code.
    #[test]
    fn asserts_read_observable_state() {
        let before = empty_state();
        let after = state_fixture();
        assert!(check_assert(&Assert::Position { level: 0, x: 3223, z: 3217, radius: 3 }, &before, &after).is_ok());
        assert!(check_assert(&Assert::Position { level: 0, x: 3000, z: 3000, radius: 3 }, &before, &after).is_err());
        assert!(check_assert(&Assert::Inventory { item: "wool".to_string(), min: 3 }, &before, &after).is_ok());
        assert!(check_assert(&Assert::Inventory { item: "wool".to_string(), min: 4 }, &before, &after).is_err());
        assert!(check_assert(&Assert::InventoryDelta { item: "wool".to_string(), min_delta: 3 }, &before, &after).is_ok());
        // Banked loads count: pack plus bank, not the pack alone.
        assert!(check_assert(&Assert::InventoryDelta { item: "shrimp".to_string(), min_delta: 5 }, &before, &after).is_ok());
        assert!(check_assert(&Assert::Xp { skill: 7, name: "cooking".to_string(), min_gain: 30 }, &before, &after).is_ok());
        assert!(check_assert(&Assert::Xp { skill: 7, name: "cooking".to_string(), min_gain: 31 }, &before, &after).is_err());
        assert!(check_assert(&Assert::Message { contains: "cook the shrimp".to_string() }, &before, &after).is_ok());
        assert!(check_assert(&Assert::Message { contains: "Welcome".to_string() }, &before, &after).is_err(), "baseline message is not new");
        assert!(check_assert(&Assert::Npc { name: "sheep".to_string(), max_dist: 12 }, &before, &after).is_ok());
        assert!(check_assert(&Assert::Npc { name: "dragon".to_string(), max_dist: 12 }, &before, &after).is_err());
        assert!(crate::verify_scenario::skill_id("cooking") == Some(7) && crate::verify_scenario::skill_id("FISHING") == Some(10) && crate::verify_scenario::skill_id("nope").is_none());
    }
}
