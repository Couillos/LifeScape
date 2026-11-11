#![forbid(unsafe_code)]
//! Session heartbeat: the atomic `run/users/<account>.bot.json` the
//! daemon merges into the live state (`Job::describe` reads it back).
//! Same keys the heartbeat writer publishes, so the dashboard and
//! `show`/`see` keep working unchanged.

use lifescape_sdk::Bot;
use lifescape_scripts::tables_live::LIVETABLES;

use lifescape_json::{Value, object, str_value};
use crate::session::now_secs;
use crate::zones;

pub(crate) fn heartbeat_value(bot: &Bot, account: &str, script: &str, trail: &[(i32, i32, u64)]) -> Value {
    let (level, pos_x, pos_z) = bot.position();
    let hitpoints = bot.world.stats.get(&3).map(|(level, _)| Value::Int(i64::from(*level))).unwrap_or(Value::Null);
    let combat = bot.combat_level().map(|level| Value::Int(i64::from(level))).unwrap_or(Value::Null);
    let modal = match &bot.world.modal {
        Some(open) => Value::Arr(vec![str_value(&open.layer), Value::Int(i64::from(open.interface_id))]),
        None => Value::Null,
    };
    let hint = match &bot.world.hint {
        Some(lifescape_sdk::world::Hint::Npc(slot)) => Value::Arr(vec![str_value("npc"), Value::Int(i64::from(*slot))]),
        Some(lifescape_sdk::world::Hint::Coord(hint_x, hint_z, kind)) => Value::Arr(vec![
            str_value("coord"),
            Value::Int(i64::from(*hint_x)),
            Value::Int(i64::from(*hint_z)),
            Value::Int(i64::from(*kind)),
        ]),
        Some(lifescape_sdk::world::Hint::Player(slot)) => Value::Arr(vec![str_value("player"), Value::Int(i64::from(*slot))]),
        None => Value::Null,
    };
    let route: Vec<Value> = bot.route.iter().map(|(step_x, step_z)| Value::Arr(vec![Value::Int(i64::from(*step_x)), Value::Int(i64::from(*step_z))])).collect();
    let route_target = match bot.route_target {
        Some((target_x, target_z)) => Value::Arr(vec![Value::Int(i64::from(target_x)), Value::Int(i64::from(target_z))]),
        None => Value::Null,
    };
    let mut skills: Vec<Value> = bot
        .world
        .stats
        .iter()
        .map(|(index, (level, xp))| {
            object(vec![
                ("id", Value::Int(i64::from(*index))),
                ("level", Value::Int(i64::from(*level))),
                ("xp", Value::Int(i64::from(*xp))),
            ])
        })
        .collect();
    skills.sort_by(|left, right| {
        left.get("id").as_i64().unwrap_or(0).cmp(&right.get("id").as_i64().unwrap_or(0))
    });
    let mut npcs: Vec<(i64, Value)> = bot
        .world
        .npcs
        .iter()
        .map(|(slot, npc)| {
            let dist = (npc.x - bot.world.x).abs() + (npc.z - bot.world.z).abs();
            let name = (LIVETABLES.npc_name)(npc.npc_type).unwrap_or_default();
            (
                dist as i64,
                object(vec![
                    ("nid", Value::Int(i64::from(*slot))),
                    ("type", Value::Int(i64::from(npc.npc_type))),
                    ("name", str_value(&name)),
                    ("x", Value::Int(i64::from(npc.x))),
                    ("z", Value::Int(i64::from(npc.z))),
                ]),
            )
        })
        .filter(|(dist, _)| *dist <= 12)
        .collect();
    npcs.sort_by_key(|(dist, _)| *dist);
    let npcs: Vec<Value> = npcs.into_iter().take(25).map(|(_, entry)| entry).collect();
    let mut locs: Vec<Value> = bot
        .world
        .locs
        .iter()
        .filter_map(|((loc_x, loc_z, loc_level), slot)| {
            if *loc_level != level {
                return None;
            }
            if (loc_x - bot.world.x).abs().max((loc_z - bot.world.z).abs()) > 16 {
                return None;
            }
            let (loc_id, _shape, _angle) = (*slot)?;
            let name = (LIVETABLES.loc_name)(loc_id).unwrap_or_default();
            if name.is_empty() || name.starts_with("null") {
                return None;
            }
            Some(object(vec![
                ("name", str_value(&name)),
                ("x", Value::Int(i64::from(*loc_x))),
                ("z", Value::Int(i64::from(*loc_z))),
                ("id", Value::Int(i64::from(loc_id))),
            ]))
        })
        .collect();
    locs.truncate(1200);
    let mut objs: Vec<Value> = bot
        .world
        .objs
        .iter()
        .filter_map(|(((obj_x, obj_z, obj_level), obj_id), count)| {
            let count = (*count)?;
            if *obj_level != level || (obj_x - bot.world.x).abs() + (obj_z - bot.world.z).abs() > 8 {
                return None;
            }
            let name = (LIVETABLES.obj_name)(*obj_id).unwrap_or_default();
            Some(object(vec![
                ("name", str_value(&name)),
                ("x", Value::Int(i64::from(*obj_x))),
                ("z", Value::Int(i64::from(*obj_z))),
                ("id", Value::Int(i64::from(*obj_id))),
                ("count", Value::Int(i64::from(count))),
            ]))
        })
        .collect();
    objs.truncate(120);
    let players: Vec<Value> = bot
        .world
        .players
        .values()
        .filter(|player| player.name.is_some())
        .take(20)
        .map(|player| {
            object(vec![
                ("name", str_value(player.name.as_deref().unwrap_or("?"))),
                ("x", Value::Int(i64::from(player.x))),
                ("z", Value::Int(i64::from(player.z))),
            ])
        })
        .collect();
    let bank_com = (LIVETABLES.com_id)("bank_main:inv");
    let worn_com = (LIVETABLES.com_id)("wornitems:wear");
    let pack_com = (LIVETABLES.com_id)("inventory:inv");
    let slot_items = |com: Option<u16>| -> Vec<Value> {
        let Some(com) = com else { return Vec::new() };
        let Some(slots) = bot.world.inv.get(&com) else { return Vec::new() };
        let mut items: Vec<(u8, Value)> = slots
            .iter()
            .map(|(slot, (held, count))| {
                let name = (LIVETABLES.obj_name)(*held).unwrap_or_default();
                (
                    *slot,
                    object(vec![
                        ("id", Value::Int(i64::from(*held))),
                        ("name", str_value(&name)),
                        ("count", Value::Int(i64::from(*count))),
                    ]),
                )
            })
            .collect();
        items.sort_by_key(|(slot, _)| *slot);
        items.into_iter().map(|(_, entry)| entry).collect()
    };
    let paired: Vec<(&String, &u64)> = bot.world.messages.iter().zip(bot.world.message_ticks.iter()).collect();
    let start = paired.len().saturating_sub(25);
    let chat_msgs: Vec<Value> = bot.world.chat.iter().rev().take(25).map(|line| str_value(&line.text)).collect();
    let chat: Vec<Value> = chat_msgs.into_iter().rev().collect();
    // Last tells, oldest first (matches the chat shape): reception
    // becomes visible in `see` and the dashboard, not just in calls.
    let tell_msgs: Vec<Value> = bot
        .world
        .private
        .iter()
        .rev()
        .take(10)
        .map(|line| object(vec![("who", str_value(&line.who)), ("text", str_value(&line.text))]))
        .collect();
    let tells: Vec<Value> = tell_msgs.into_iter().rev().collect();
    let messages: Vec<Value> = paired[start..].iter().map(|(line, _)| str_value(line)).collect();
    let message_ticks: Vec<Value> = paired[start..].iter().map(|(_, tick)| Value::Int(**tick as i64)).collect();
    let mut texts: Vec<(u16, &String)> = bot.world.texts.iter().map(|(com, text)| (*com, text)).collect();
    texts.sort();
    let mut text_map = std::collections::BTreeMap::new();
    for (com, text) in texts.iter().rev().take(12) {
        text_map.insert(com.to_string(), str_value(text));
    }
    let (zone, zone_dist) = match zones::zone_name(pos_x, pos_z) {
        Some((name, Some(dist))) => (str_value(&name), Value::Int(dist.round() as i64)),
        Some((name, None)) => (str_value(&name), Value::Null),
        None => (Value::Null, Value::Null),
    };
    object(vec![
        ("account", str_value(account)),
        ("script", str_value(script)),
        ("pid", Value::Int(std::process::id() as i64)),
        ("intention", str_value(&bot.intent)),
        (
            "position",
            Value::Arr(vec![Value::Int(i64::from(level)), Value::Int(i64::from(pos_x)), Value::Int(i64::from(pos_z))]),
        ),
        ("zone", zone),
        ("zone_dist", zone_dist),
        ("hitpoints", hitpoints),
        ("ticks", Value::Int(bot.world.ticks as i64)),
        ("updated", Value::Float(now_secs())),
        ("modal", modal),
        ("combat", combat),
        ("energy", Value::Int(i64::from(bot.world.energy))),
        ("hint", hint),
        ("hint_type", Value::Null),
        ("flash", bot.world.side_flash.map(|tab| Value::Int(i64::from(tab))).unwrap_or(Value::Null)),
        ("route", Value::Arr(route)),
        ("route_target", route_target),
        ("route_action", Value::Null),
        (
            "trail",
            Value::Arr(
                trail
                    .iter()
                    .map(|(trail_x, trail_z, trail_tick)| {
                        Value::Arr(vec![
                            Value::Int(i64::from(*trail_x)),
                            Value::Int(i64::from(*trail_z)),
                            Value::Int(*trail_tick as i64),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("skills", Value::Arr(skills)),
        ("npcs", Value::Arr(npcs)),
        ("locs", Value::Arr(locs)),
        ("objs", Value::Arr(objs)),
        ("players", Value::Arr(players)),
        ("inv", Value::Arr(slot_items(pack_com))),
        ("worn", Value::Arr(slot_items(worn_com))),
        ("models", Value::Obj(Default::default())),
        ("bank", Value::Arr(slot_items(bank_com))),
        ("chat", Value::Arr(chat)),
        ("tells", Value::Arr(tells)),
        ("messages", Value::Arr(messages)),
        ("message_ticks", Value::Arr(message_ticks)),
        ("texts", Value::Obj(text_map)),
    ])
}

#[cfg(test)]
mod tests {
    use super::heartbeat_value;
    use lifescape_json::Value;

    /// Received tells surface in the heartbeat JSON (who + text, oldest
    /// first), so `see` and the dashboard show reception live.
    #[test]
    fn heartbeat_carries_received_tells() {
        let mut bot = lifescape_sdk::Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.private.push(lifescape_sdk::world::ChatLine {
            who: "t02".to_string(),
            text: "meet at varrock".to_string(),
            color: 0,
            effect: 0,
        });
        let value = heartbeat_value(&bot, "t01", "testing", &[]);
        let Value::Arr(tells) = value.get("tells") else {
            panic!("tells is an array");
        };
        assert_eq!(tells.len(), 1, "one tell received");
        assert_eq!(tells[0].get("who").as_str(), Some("t02"));
        assert_eq!(tells[0].get("text").as_str(), Some("meet at varrock"));
    }
}
