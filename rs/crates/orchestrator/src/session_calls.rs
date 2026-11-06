#![forbid(unsafe_code)]
//! Session calls: the `call`/`methods` whitelist and its execution.
//!
//! Reads come from the last heartbeat snapshot (never from the live bot
//! off its thread); writes run on the idle pump thread at the safe-point.
//! `cheat` answers only to admin accounts; anything else unknown is an
//! error, never a silent fallback.

use lifescape_sdk::client::Snapshot;
use lifescape_sdk::Bot;
use lifescape_scripts::tables_live::LIVETABLES;

use crate::json::{self, Value, object, str_value};

/// Snapshot reads served from the heartbeat, never from the live bot.
const READ_METHODS: &[&str] = &[
    "inventory",
    "bank_items",
    "backpack",
    "invs",
    "chat_texts",
    "chat_options",
    "combat_level",
    "position",
];

/// Writes the worker executes when idle. `cheat` needs an admin account.
const WRITE_METHODS: &[&str] = &[
    "approach",
    "approach_npc",
    "bank_deposit_all",
    "bank_withdraw",
    "chat_choose",
    "chat_continue",
    "cheat",
    "close_modal",
    "debug_walk",
    "drop",
    "find_npc",
    "find_player",
    "friend_add",
    "friend_del",
    "idle",
    "if_button",
    "intend",
    "inv_button",
    "inv_button_d",
    "open_bank",
    "op_held",
    "op_held_u",
    "op_loc",
    "op_loc_u",
    "op_npc",
    "op_npc_t",
    "op_npc_u",
    "op_obj",
    "op_player",
    "resume_pcount",
    "save_design",
    "say",
    "step_to",
    "take",
    "talk_to",
    "trade_accept",
    "trade_close",
    "trade_modal",
    "trade_offer",
    "trade_partner_offer",
    "trade_request",
    "trade_status_text",
    "tut_clickside",
    "walk_steps",
    "walk_to",
];

// -- reads ---------------------------------------------------------------

/// CLI strings to values: JSON when it parses, else the raw string.
fn coerce_arg(raw: &Value) -> Value {
    match raw {
        Value::Str(text) => json::parse(text).unwrap_or_else(|_| str_value(text)),
        other => other.clone(),
    }
}

fn num_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Int(num) => Some(*num),
        Value::Float(num) => Some(*num as i64),
        Value::Str(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn need_i64(args: &[Value], slot: usize, method: &str) -> Result<i64, String> {
    args.get(slot).and_then(num_i64).ok_or_else(|| format!("{method}: arg {slot} must be a number"))
}

fn need_u64(args: &[Value], slot: usize, method: &str, default: u64) -> Result<u64, String> {
    let Some(raw) = args.get(slot) else { return Ok(default.min(CALL_TICK_BOUND)) };
    num_i64(raw).and_then(|num| u64::try_from(num).ok()).ok_or_else(|| format!("{method}: arg {slot} must be a number"))
}

/// Tick bound for every blocking session call: a call must always answer
/// inside the daemon's worker timeout (40s), or the CLI surfaces a raw
/// EAGAIN ("Resource temporarily unavailable") and the worker stays busy
/// behind it. Long treks belong in supervised scripts, not in calls.
pub(crate) const CALL_TICK_BOUND: u64 = 500;

/// Timeout arg clamped to the call bound (see `CALL_TICK_BOUND`).
fn call_timeout(args: &[Value], slot: usize, method: &str, default: u64) -> Result<u64, String> {
    need_u64(args, slot, method, default).map(|timeout| timeout.min(CALL_TICK_BOUND))
}

fn need_str<'args>(args: &'args [Value], slot: usize, method: &str) -> Result<&'args str, String> {
    match args.get(slot) {
        Some(Value::Str(text)) => Ok(text),
        _ => Err(format!("{method}: arg {slot} must be a string")),
    }
}

pub(crate) fn read_from_snapshot(snapshot: &Snapshot, method: &str) -> Result<Value, String> {
    match method {
        "position" => {
            let (level, pos_x, pos_z) = snapshot.position;
            Ok(Value::Arr(vec![Value::Int(i64::from(level)), Value::Int(i64::from(pos_x)), Value::Int(i64::from(pos_z))]))
        }
        "inventory" => {
            let mut items = std::collections::BTreeMap::new();
            for (held, count) in lifescape_sdk::snapshot::aggregate(&snapshot.inv) {
                items.insert(held.to_string(), Value::Int(i64::from(count)));
            }
            Ok(Value::Obj(items))
        }
        "bank_items" => {
            let bank_com = (LIVETABLES.com_id)("bank_main:inv");
            let mut items = std::collections::BTreeMap::new();
            for (held, count) in lifescape_sdk::snapshot::bank_items(&snapshot.inv, bank_com) {
                items.insert(held.to_string(), Value::Int(i64::from(count)));
            }
            Ok(Value::Obj(items))
        }
        "backpack" => {
            let (com, slots) = lifescape_sdk::snapshot::backpack_view(&snapshot.inv, &snapshot.inv_cap, snapshot.backpack_com);
            let items: Vec<Value> = slots
                .into_iter()
                .map(|(slot, held, count)| {
                    object(vec![
                        ("slot", Value::Int(i64::from(slot))),
                        ("id", Value::Int(i64::from(held))),
                        ("count", Value::Int(i64::from(count))),
                    ])
                })
                .collect();
            Ok(object(vec![
                ("com", com.map(|com| Value::Int(i64::from(com))).unwrap_or(Value::Null)),
                ("items", Value::Arr(items)),
            ]))
        }
        "invs" => {
            // Every streamed container: com, capacity, item count. The one
            // place to see which com the server actually uses per panel.
            let mut rows: Vec<(u16, u8, usize)> = snapshot
                .inv
                .iter()
                .map(|(com, slots)| (*com, snapshot.inv_cap.get(com).copied().unwrap_or(0), slots.len()))
                .collect();
            rows.sort();
            Ok(Value::Arr(
                rows.into_iter()
                    .map(|(com, cap, count)| {
                        object(vec![
                            ("com", Value::Int(i64::from(com))),
                            ("cap", Value::Int(i64::from(cap))),
                            ("items", Value::Int(count as i64)),
                        ])
                    })
                    .collect(),
            ))
        }
        "chat_texts" => {
            let mut texts: Vec<(u16, &String)> = snapshot.texts.iter().map(|(com, text)| (*com, text)).collect();
            texts.sort();
            Ok(Value::Arr(
                texts.into_iter().map(|(com, text)| Value::Arr(vec![Value::Int(i64::from(com)), str_value(text)])).collect(),
            ))
        }
        "chat_options" => Ok(Value::Arr(
            lifescape_sdk::snapshot::chat_options(&snapshot.texts)
                .into_iter()
                .map(|(com, text)| Value::Arr(vec![Value::Int(i64::from(com)), str_value(&text)]))
                .collect(),
        )),
        "combat_level" => Ok(lifescape_sdk::snapshot::combat_level(&snapshot.stats)
            .map(|level| Value::Int(i64::from(level)))
            .unwrap_or(Value::Null)),
        _ => Err(format!("unknown method {method:?}")),
    }
}

// -- writes ---------------------------------------------------------------

pub(crate) fn method_lists(admin: bool) -> (Vec<String>, Vec<String>) {
    let mut reads: Vec<String> = READ_METHODS.iter().map(|name| name.to_string()).collect();
    reads.sort();
    let mut writes: Vec<String> = WRITE_METHODS
        .iter()
        .filter(|name| admin || **name != "cheat")
        .map(|name| name.to_string())
        .collect();
    writes.sort();
    (reads, writes)
}

fn parse_id_or_name(args: &[Value], slot: usize, method: &str) -> Result<(Option<u16>, Option<String>), String> {
    let Some(raw) = args.get(slot) else { return Err(format!("{method}: arg {slot} is required")) };
    if let Some(num) = num_i64(raw) {
        if !(0..=u16::MAX as i64).contains(&num) {
            return Err(format!("{method}: arg {slot} must be an object id or name"));
        }
        return Ok((Some(num as u16), None));
    }
    match raw {
        Value::Str(text) if !text.is_empty() => Ok((None, Some(text.clone()))),
        _ => Err(format!("{method}: arg {slot} must be an object id or name")),
    }
}

pub(crate) fn execute_write(bot: &mut Bot, method: &str, raw_args: &[Value]) -> Result<Value, String> {
    let args: Vec<Value> = raw_args.iter().map(coerce_arg).collect();
    match method {
        "intend" => {
            bot.intend(need_str(&args, 0, method)?);
            Ok(Value::Null)
        }
        "say" => {
            bot.say(need_str(&args, 0, method)?).map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "idle" => {
            bot.idle().map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "walk_to" => {
            let target_x = need_i64(&args, 0, method)? as i32;
            let target_z = need_i64(&args, 1, method)? as i32;
            let level = args.get(2).and_then(num_i64).map(|num| num as i32).unwrap_or_else(|| bot.world.level);
            let timeout = call_timeout(&args, 3, method, 9000)?;
            bot.walk_to(target_x, target_z, level, timeout).map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "approach" => {
            let target_x = need_i64(&args, 0, method)? as i32;
            let target_z = need_i64(&args, 1, method)? as i32;
            let level = args.get(2).and_then(num_i64).map(|num| num as i32).unwrap_or_else(|| bot.world.level);
            let radius = args.get(3).and_then(num_i64).map(|num| num as i32).unwrap_or(1);
            let timeout = call_timeout(&args, 4, method, 9000)?;
            bot.approach(target_x, target_z, level, radius, timeout).map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "approach_npc" => {
            let slot = need_i64(&args, 0, method)? as u16;
            let timeout = call_timeout(&args, 1, method, 9000)?;
            bot.approach_npc(slot, timeout).map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "step_to" => {
            let target_x = need_i64(&args, 0, method)? as i32;
            let target_z = need_i64(&args, 1, method)? as i32;
            let timeout = call_timeout(&args, 2, method, 30)?;
            bot.step_to(target_x, target_z, timeout).map_err(|error| error.to_string()).map(Value::Bool)
        }
        "walk_steps" => {
            let timeout = args.last().and_then(num_i64).map(|num| (num.max(0) as u64).min(CALL_TICK_BOUND)).unwrap_or(CALL_TICK_BOUND);
            let steps: Vec<(i32, i32)> = match args.first() {
                Some(Value::Str(text)) => text
                    .split(';')
                    .filter(|pair| !pair.trim().is_empty())
                    .map(|pair| {
                        let mut parts = pair.split(',').map(str::trim);
                        let (Some(first), Some(second)) = (parts.next(), parts.next()) else {
                            return Err(format!("{method}: steps must look like \"x,z;x,z\""));
                        };
                        Ok((first.parse::<i32>().map_err(|_| format!("{method}: bad step {pair:?}"))?, second.parse::<i32>().map_err(|_| format!("{method}: bad step {pair:?}"))?))
                    })
                    .collect::<Result<Vec<(i32, i32)>, String>>()?,
                Some(Value::Arr(pairs)) => pairs
                    .iter()
                    .map(|pair| {
                        let coords = pair.as_arr().ok_or_else(|| format!("{method}: steps must be [x, z] pairs"))?;
                        Ok((coords.first().and_then(num_i64).ok_or_else(|| format!("{method}: bad step"))? as i32, coords.get(1).and_then(num_i64).ok_or_else(|| format!("{method}: bad step"))? as i32))
                    })
                    .collect::<Result<Vec<(i32, i32)>, String>>()?,
                _ => return Err(format!("{method}: arg 0 must be [x, z] pairs")),
            };
            bot.walk_steps(&steps, timeout).map_err(|error| error.to_string()).map(Value::Bool)
        }
        "debug_walk" => {
            let target_x = need_i64(&args, 0, method)? as i32;
            let target_z = need_i64(&args, 1, method)? as i32;
            let level = args.get(2).and_then(num_i64).map(|num| num as i32).unwrap_or_else(|| bot.world.level);
            let report = bot.debug_walk(target_x, target_z, level);
            Ok(Value::Arr(report.lines.into_iter().map(|line| str_value(&line)).collect()))
        }
        "op_loc" => {
            bot.op_loc(need_i64(&args, 0, method)? as u8, need_i64(&args, 1, method)? as i32, need_i64(&args, 2, method)? as i32, need_i64(&args, 3, method)? as u16)
                .map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "op_npc" => {
            bot.op_npc(need_i64(&args, 0, method)? as u8, need_i64(&args, 1, method)? as u16).map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "op_npc_u" => {
            bot.op_npc_u(need_i64(&args, 0, method)? as u16, need_i64(&args, 1, method)? as u16, need_i64(&args, 2, method)? as u16, need_i64(&args, 3, method)? as u16)
                .map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "op_obj" => {
            bot.op_obj(need_i64(&args, 0, method)? as u8, need_i64(&args, 1, method)? as i32, need_i64(&args, 2, method)? as i32, need_i64(&args, 3, method)? as u16)
                .map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "op_loc_u" => {
            bot.op_loc_u(
                need_i64(&args, 0, method)? as i32,
                need_i64(&args, 1, method)? as i32,
                need_i64(&args, 2, method)? as u16,
                need_i64(&args, 3, method)? as u16,
                need_i64(&args, 4, method)? as u16,
                need_i64(&args, 5, method)? as u16,
            )
            .map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "op_npc_t" => {
            bot.op_npc_t(need_i64(&args, 0, method)? as u16, need_i64(&args, 1, method)? as u16).map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "op_player" => {
            bot.op_player(need_i64(&args, 0, method)? as u8, need_i64(&args, 1, method)? as u16).map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "op_held" => {
            bot.op_held(need_i64(&args, 0, method)? as u8, need_i64(&args, 1, method)? as u16, need_i64(&args, 2, method)? as u8, need_i64(&args, 3, method)? as u16)
                .map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "op_held_u" => {
            bot.op_held_u(
                need_i64(&args, 0, method)? as u16,
                need_i64(&args, 1, method)? as u8,
                need_i64(&args, 2, method)? as u16,
                need_i64(&args, 3, method)? as u16,
                need_i64(&args, 4, method)? as u8,
                need_i64(&args, 5, method)? as u16,
            )
            .map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "inv_button" => {
            bot.inv_button(need_i64(&args, 0, method)? as u8, need_i64(&args, 1, method)? as u16, need_i64(&args, 2, method)? as u8, need_i64(&args, 3, method)? as u16)
                .map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "inv_button_d" => {
            bot.inv_button_d(need_i64(&args, 0, method)? as u16, need_i64(&args, 1, method)? as u8, need_i64(&args, 2, method)? as u16, need_i64(&args, 3, method)? as u8)
                .map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "if_button" => {
            bot.if_button(need_i64(&args, 0, method)? as u16).map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "close_modal" => {
            bot.close_modal().map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "talk_to" => {
            bot.talk_to(need_i64(&args, 0, method)? as u16).map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "chat_continue" => {
            bot.chat_continue().map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "chat_choose" => bot.chat_choose(need_str(&args, 0, method)?).map_err(|error| error.to_string()).map(|com| Value::Int(i64::from(com))),
        "take" => {
            // One arg: id or name. Three args: id, name, timeout.
            if args.len() >= 3 {
                let held = args.first().and_then(num_i64).and_then(|num| u16::try_from(num).ok());
                let timeout = call_timeout(&args, 2, method, 9000)?;
                match args.get(1) {
                    Some(Value::Str(text)) if !text.is_empty() => bot.take(held, Some(text.as_str()), timeout),
                    _ => bot.take(held, None, timeout),
                }
                .map_err(|error| error.to_string())
                .map(Value::Bool)
            } else {
                let (held, name) = parse_id_or_name(&args, 0, method)?;
                let timeout = call_timeout(&args, 1, method, 9000)?;
                bot.take(held, name.as_deref(), timeout).map_err(|error| error.to_string()).map(Value::Bool)
            }
        }
        "drop" => {
            let timeout = call_timeout(&args, 1, method, 9000)?;
            bot.drop(need_i64(&args, 0, method)? as u16, timeout).map_err(|error| error.to_string()).map(Value::Bool)
        }
        "open_bank" => {
            let timeout = call_timeout(&args, 3, method, 9000)?;
            bot.open_bank(need_i64(&args, 0, method)? as i32, need_i64(&args, 1, method)? as i32, need_i64(&args, 2, method)? as u16, timeout)
                .map_err(|error| error.to_string())
                .map(Value::Bool)
        }
        "bank_withdraw" => {
            let timeout = call_timeout(&args, 2, method, 9000)?;
            bot.bank_withdraw(need_i64(&args, 0, method)? as u16, need_i64(&args, 1, method)? as u32, timeout)
                .map_err(|error| error.to_string())
                .map(|count| Value::Int(i64::from(count)))
        }
        "bank_deposit_all" => {
            let keep: Vec<u16> = match args.get(1) {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Arr(items)) => items.iter().filter_map(|item| num_i64(item)).filter(|num| (0..=u16::MAX as i64).contains(num)).map(|num| num as u16).collect(),
                Some(Value::Str(text)) if text.trim().is_empty() => Vec::new(),
                Some(Value::Str(text)) => text
                    .split(',')
                    .map(|part| part.trim().parse::<u16>().map_err(|_| format!("{method}: keep ids must look like \"1,2\"")))
                    .collect::<Result<Vec<u16>, String>>()?,
                Some(_) => return Err(format!("{method}: keep ids must be an array or \"1,2\"")),
            };
            let timeout = call_timeout(&args, 2, method, 9000)?;
            bot.bank_deposit_all(need_i64(&args, 0, method)? as u16, &keep, timeout).map_err(|error| error.to_string()).map(Value::Bool)
        }
        "trade_request" => {
            let timeout = call_timeout(&args, 1, method, 9000)?;
            bot.trade_request(need_str(&args, 0, method)?, timeout).map_err(|error| error.to_string()).map(Value::Bool)
        }
        "trade_offer" => {
            let timeout = call_timeout(&args, 2, method, 9000)?;
            bot.trade_offer(need_i64(&args, 0, method)? as u16, need_i64(&args, 1, method)? as u32, timeout)
                .map_err(|error| error.to_string())
                .map(Value::Bool)
        }
        "trade_accept" => {
            let timeout = call_timeout(&args, 0, method, 9000)?;
            bot.trade_accept(timeout).map_err(|error| error.to_string()).map(Value::Bool)
        }
        "trade_close" => {
            bot.trade_close().map_err(|error| error.to_string()).map(Value::Bool)
        }
        "find_npc" => Ok(bot.find_npc(need_str(&args, 0, method)?).map(|slot| Value::Int(i64::from(slot))).unwrap_or(Value::Null)),
        "find_player" => Ok(bot.find_player(need_str(&args, 0, method)?).map(|slot| Value::Int(i64::from(slot))).unwrap_or(Value::Null)),
        "trade_modal" => Ok(bot.trade_modal().map(str_value).unwrap_or(Value::Null)),
        "trade_status_text" => Ok(str_value(&bot.trade_status_text())),
        "trade_partner_offer" => {
            let mut items = std::collections::BTreeMap::new();
            for (held, count) in bot.trade_partner_offer() {
                items.insert(held.to_string(), Value::Int(i64::from(count)));
            }
            Ok(Value::Obj(items))
        }
        "resume_pcount" => {
            bot.resume_pcount(need_i64(&args, 0, method)? as i32).map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "tut_clickside" => {
            bot.tut_clickside(need_i64(&args, 0, method)? as u8).map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "save_design" => {
            let kits: Vec<i8> = match args.get(1) {
                Some(Value::Arr(items)) => items
                    .iter()
                    .map(|item| num_i64(item).and_then(|num| i8::try_from(num).ok()).ok_or_else(|| format!("{method}: kits must fit in i8")))
                    .collect::<Result<Vec<i8>, String>>()?,
                Some(Value::Str(text)) => text
                    .split(',')
                    .map(|part| part.trim().parse::<i8>().map_err(|_| format!("{method}: kits must look like \"1,2\"")))
                    .collect::<Result<Vec<i8>, String>>()?,
                _ => return Err(format!("{method}: arg 1 must be kits")),
            };
            let colors: Vec<u8> = match args.get(2) {
                Some(Value::Arr(items)) => items
                    .iter()
                    .map(|item| num_i64(item).and_then(|num| u8::try_from(num).ok()).ok_or_else(|| format!("{method}: colors must fit in u8")))
                    .collect::<Result<Vec<u8>, String>>()?,
                Some(Value::Str(text)) => text
                    .split(',')
                    .map(|part| part.trim().parse::<u8>().map_err(|_| format!("{method}: colors must look like \"1,2\"")))
                    .collect::<Result<Vec<u8>, String>>()?,
                _ => return Err(format!("{method}: arg 2 must be colors")),
            };
            bot.save_design(need_i64(&args, 0, method)? as u8, &kits, &colors).map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "friend_add" => {
            bot.friend_add(need_str(&args, 0, method)?).map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "friend_del" => {
            bot.friend_del(need_str(&args, 0, method)?).map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        "cheat" => {
            bot.cheat(need_str(&args, 0, method)?).map_err(|error| error.to_string())?;
            Ok(Value::Null)
        }
        _ => Err(format!("unknown method {method:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::{call_timeout, coerce_arg, execute_write, method_lists, read_from_snapshot, CALL_TICK_BOUND};
    use crate::json::{self, Value, str_value};
    use lifescape_sdk::client::Snapshot;

    fn snapshot_fixture() -> Snapshot {
        Snapshot {
            position: (0, 3200, 3227),
            ticks: 42,
            modal: None,
            stats: [(0, (40, 100000)), (3, (45, 120000))].into_iter().collect(),
            inv: [(99u16, [(0u8, (5u16, 10u32))].into_iter().collect())].into_iter().collect(),
            inv_cap: [(99u16, 28u8)].into_iter().collect(),
            backpack_com: Some(99),
            texts: [(100u16, "Select an Option".to_string()), (101u16, "Yes".to_string())].into_iter().collect(),
            energy: 100,
            intent: "testing".to_string(),
        }
    }

    #[test]
    fn reads_match_worker_shapes() {
        let snapshot = snapshot_fixture();
        assert_eq!(json::stringify(&read_from_snapshot(&snapshot, "position").unwrap()), "[0, 3200, 3227]");
        let inventory = read_from_snapshot(&snapshot, "inventory").unwrap();
        assert_eq!(inventory.get("5").as_i64(), Some(10));
        let backpack = read_from_snapshot(&snapshot, "backpack").unwrap();
        assert_eq!(backpack.get("com").as_i64(), Some(99));
        assert_eq!(backpack.get("items").as_arr().map(Vec::len), Some(1));
        let texts = read_from_snapshot(&snapshot, "chat_texts").unwrap();
        assert_eq!(texts.as_arr().map(Vec::len), Some(2));
        let options = read_from_snapshot(&snapshot, "chat_options").unwrap();
        assert_eq!(options.as_arr().map(Vec::len), Some(1));
        assert!(read_from_snapshot(&snapshot, "nope").is_err());
    }

    #[test]
    fn methods_gate_cheat() {
        let (_, writes) = method_lists(false);
        assert!(!writes.contains(&"cheat".to_string()));
        let (_, admin_writes) = method_lists(true);
        assert!(admin_writes.contains(&"cheat".to_string()));
        let (reads, _) = method_lists(false);
        assert!(reads.contains(&"invs".to_string()), "container diagnostic is a read");
        assert!(reads.len() >= 7);
    }

    #[test]
    fn cli_strings_coerce_like_the_worker() {
        assert_eq!(coerce_arg(&str_value("12")), Value::Int(12));
        assert_eq!(coerce_arg(&str_value("walk")), str_value("walk"));
    }

    #[test]
    fn call_timeouts_stay_inside_the_worker_timeout() {
        // A 9000-tick call outlives the daemon's 40s worker timeout and
        // wedges the session behind a raw EAGAIN: clamp to the bound.
        let huge = vec![Value::Int(9000)];
        assert_eq!(call_timeout(&huge, 0, "walk_to", 9000).expect("number"), CALL_TICK_BOUND);
        assert_eq!(call_timeout(&[], 0, "walk_to", 9000).expect("default"), CALL_TICK_BOUND);
        assert_eq!(call_timeout(&[Value::Int(100)], 0, "step_to", 30).expect("small"), 100);
        assert!(call_timeout(&[Value::Str("x".to_string())], 0, "walk_to", 9000).is_err());
    }

    #[test]
    fn unknown_write_is_an_error_not_a_fallback() {
        let engine = "missing-engine".to_string();
        let mut bot = lifescape_sdk::Bot::new("127.0.0.1", 1, &engine, &engine);
        assert!(execute_write(&mut bot, "nope", &[]).is_err());
        assert!(execute_write(&mut bot, "walk_to", &[str_value("x")]).is_err());
    }
}
