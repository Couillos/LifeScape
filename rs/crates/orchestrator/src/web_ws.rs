#![forbid(unsafe_code)]
//! WebSocket layer of the dashboard (split from web.rs, same behavior):
//! per-connection state push (1 Hz full/diff frames), the market live
//! tail, the log tail and ping/pong framing. The router stays in web.rs.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lifescape_json::{self as json, Value, object, str_value};
use crate::minimap::Minimap;
use crate::paths;
use crate::web::{Dispatch, WS_INTERVAL};


/// Loc columns on the wire. The screen only reads `name` (via `loc_names`),
/// `cosmetic` (scenery panel) and `id` (the names key): x/z/shape/angle and
/// the mapscene origin ride no pixel, the map is the static PNG.
const LOC_FIELDS: &[&str] = &["id", "cosmetic"];

/// Job columns on the wire: exactly what the spectator bot table renders
/// (status/name/level/hp/script/zone/position). Phase/ticks/errors ride the
/// followed state, not the list. Goal/task come from the life file
/// (`job_lives`), not from the job.
const JOB_LIST_FIELDS: &[&str] = &[
    "account",
    "account_name",
    "combat",
    "position",
    "zone",
    "status",
    "paused",
    "script",
    "hitpoints",
];

/// Pack-backed ground and sprite renderer, shared by every connection.
pub(crate) fn shared_maps() -> &'static Minimap {
    static MAPS: OnceLock<Minimap> = OnceLock::new();
    MAPS.get_or_init(Minimap::from_pack_dir)
}

pub(crate) fn annotate_mapscene(mut state: Value) -> Value {
    if let Value::Obj(ref mut map) = state {
        if let Some(locs) = map.get_mut("locs").and_then(|locs| locs.as_arr().map(|items| items.to_vec())) {
            let maps = shared_maps();
            let mut out = Vec::with_capacity(locs.len());
            for mut loc in locs {
                // Loc id to mapscene sprite and its own origin, the way
                // the browser draws buildings instead of floor colours.
                let sprite = maps.cache().mapscene_for(loc.get("id").as_i64().unwrap_or(-1) as i32);
                let (origin_x, origin_y) =
                    if sprite >= 0 { maps.cache().mapscene_origin(sprite) } else { (0, 0) };
                if let Value::Obj(entry) = &mut loc {
                    entry.insert("mapscene".to_string(), Value::Int(sprite as i64));
                    entry.insert("mapsceneX".to_string(), Value::Int(origin_x as i64));
                    entry.insert("mapsceneY".to_string(), Value::Int(origin_y as i64));
                }
                out.push(loc);
            }
            map.insert("locs".to_string(), Value::Arr(out));
        }
    }
    state
}

/// Full-weight fields the spectator screen never reads (no `last.<name>`
/// in `spectator.js`): dropped from the 1 Hz push only. `/api/state` and the
/// CLI keep the full state.
const WS_DROP: &[&str] = &[
    "trail",
    "bank",
    "messages",
    "message_ticks",
    "texts",
    "flora",
    "models",
];
/// Encode locs as arrays plus one id -> name table (same wire shape).
fn compact_locs(state: &mut Value) {
    let locs = match state.as_obj().and_then(|map| map.get("locs")) {
        Some(Value::Arr(locs)) if !locs.is_empty() => locs.clone(),
        _ => return,
    };
    let mut names = std::collections::BTreeMap::new();
    let mut table = Vec::with_capacity(locs.len());
    for loc in &locs {
        let id = loc.get("id").as_i64().unwrap_or(-1).to_string();
        names
            .entry(id)
            .or_insert_with(|| loc.get("name").as_str().unwrap_or("").to_string());
        let mut row = Vec::with_capacity(LOC_FIELDS.len());
        for field in LOC_FIELDS {
            let cell = loc.get(field);
            row.push(match cell {
                Value::Bool(flag) => Value::Int(i64::from(*flag)),
                other => other.clone(),
            });
        }
        table.push(Value::Arr(row));
    }
    if let Value::Obj(map) = state {
        let mut table_names = std::collections::BTreeMap::new();
        for (id, name) in names {
            table_names.insert(id, Value::Str(name));
        }
        let fields = LOC_FIELDS.iter().map(|name| str_value(name)).collect();
        map.insert("locs".to_string(), Value::Arr(table));
        map.insert("loc_names".to_string(), Value::Obj(table_names));
        map.insert("loc_fields".to_string(), Value::Arr(fields));
    }
}

/// Full state plus its per-connection diff: `previous` is the last full
/// state this connection got (`None` on connect or account switch). Same
/// shape as `/api/state` minus `WS_DROP`, locs compacted; unchanged keys
/// are omitted and the client merges (`Object.assign`) over its copy.
/// Returns the wire payload and the full state to remember for next time.
pub fn state_payload(
    dispatch: &Dispatch,
    account: &str,
    previous: Option<&Value>,
) -> (Value, Option<Value>) {
    let jobs = dispatch(object(vec![("cmd", str_value("status"))]));
    let mut listed = Vec::new();
    for job in jobs.get("jobs").as_arr().map(|items| items.to_vec()).unwrap_or_default() {
        let mut slim = std::collections::BTreeMap::new();
        for field in JOB_LIST_FIELDS {
            slim.insert(field.to_string(), job.get(field).clone());
        }
        if let Some(life) = job.get("account").as_str().and_then(crate::web_job_lives::summary_for) {
            slim.insert("goal".to_string(), str_value(&life.goal));
            slim.insert("task".to_string(), str_value(&life.task));
            slim.insert("task_title".to_string(), str_value(&life.task_title));
        }
        listed.push(Value::Obj(slim));
    }
    let mut payload = object(vec![("jobs", Value::Arr(listed))]);
    let mut remembered = None;
    if !account.is_empty() {
        let mut state = dispatch(object(vec![
            ("cmd", str_value("live")),
            ("account", str_value(account)),
        ]))
        .get("state")
        .clone();
        if state.as_obj().is_none() {
            state = object(vec![]);
        }
        if let Value::Obj(map) = &mut state {
            for dead in WS_DROP {
                map.remove(*dead);
            }
        }
        compact_locs(&mut state);
        remembered = Some(state.clone());
        let frame = match previous {
            Some(prev) => {
                let mut changed = std::collections::BTreeMap::new();
                if let Value::Obj(map) = &state {
                    for (key, value) in map {
                        if *prev.get(key) != *value {
                            changed.insert(key.clone(), value.clone());
                        }
                    }
                }
                let mut frame = Value::Obj(changed);
                if let Value::Obj(map) = &mut frame {
                    // `account` marks a live frame (its absence reads as
                    // offline), and compact rows need their table to decode.
                    map.insert("account".to_string(), state.get("account").clone());
                    if map.contains_key("locs") {
                        map.insert("loc_names".to_string(), state.get("loc_names").clone());
                        map.insert("loc_fields".to_string(), state.get("loc_fields").clone());
                    }
                }
                frame
            }
            None => state,
        };
        if let Value::Obj(map) = &mut payload {
            map.insert("state".to_string(), frame);
        }
    }
    (payload, remembered)
}

// -- websocket framing (RFC 6455 server frames) ----------------------------

fn ws_frame(payload: &[u8], opcode: u8) -> Vec<u8> {
    let mut head = vec![0x80 | opcode];
    let len = payload.len();
    if len < 126 {
        head.push(len as u8);
    } else if len < 65536 {
        head.push(126);
        head.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        head.push(127);
        head.extend_from_slice(&(len as u64).to_be_bytes());
    }
    head.extend_from_slice(payload);
    head
}

fn ws_read(sock: &TcpStream, len: usize) -> std::io::Result<Vec<u8>> {
    let mut data = vec![0u8; len];
    let mut filled = 0;
    while filled < len {
        let count = (&*sock).read(&mut data[filled..])?;
        if count == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "websocket closed",
            ));
        }
        filled += count;
    }
    Ok(data)
}

fn ws_recv(sock: &TcpStream) -> std::io::Result<(u8, Vec<u8>)> {
    let head = ws_read(sock, 2)?;
    let opcode = head[0] & 0x0F;
    let masked = head[1] & 0x80 != 0;
    let mut len = (head[1] & 0x7F) as u64;
    if len == 126 {
        len = u16::from_be_bytes(ws_read(sock, 2)?[..2].try_into().unwrap()) as u64;
    } else if len == 127 {
        len = u64::from_be_bytes(ws_read(sock, 8)?[..8].try_into().unwrap());
    }
    let mask = if masked { ws_read(sock, 4)? } else { vec![0, 0, 0, 0] };
    let mut data = if len > 0 { ws_read(sock, len as usize)? } else { Vec::new() };
    if masked {
        for (index, byte) in data.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }
    Ok((opcode, data))
}

pub(crate) fn sha1(message: &[u8]) -> [u8; 20] {
    let mut words = [
        0x67452301u32,
        0xEFCDAB89,
        0x98BADCFE,
        0x10325476,
        0xC3D2E1F0,
    ];
    let bit_len = (message.len() as u64).wrapping_mul(8);
    let mut padded = message.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for block in padded.chunks(64) {
        let mut schedule = [0u32; 80];
        for (index, word) in schedule.iter_mut().enumerate().take(16) {
            let at = index * 4;
            *word = u32::from_be_bytes(block[at..at + 4].try_into().unwrap());
        }
        for index in 16..80 {
            schedule[index] = (schedule[index - 3]
                ^ schedule[index - 8]
                ^ schedule[index - 14]
                ^ schedule[index - 16])
                .rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) =
            (words[0], words[1], words[2], words[3], words[4]);
        for (index, word) in schedule.iter().enumerate() {
            let (f, k) = match index {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        words[0] = words[0].wrapping_add(a);
        words[1] = words[1].wrapping_add(b);
        words[2] = words[2].wrapping_add(c);
        words[3] = words[3].wrapping_add(d);
        words[4] = words[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (index, word) in words.iter().enumerate() {
        out[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

pub(crate) fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let mut block = [0u8; 3];
        block[..chunk.len()].copy_from_slice(chunk);
        let triple = ((block[0] as u32) << 16) | ((block[1] as u32) << 8) | block[2] as u32;
        let chars = [
            ALPHABET[((triple >> 18) & 63) as usize] as char,
            ALPHABET[((triple >> 12) & 63) as usize] as char,
            if chunk.len() > 1 {
                ALPHABET[((triple >> 6) & 63) as usize] as char
            } else {
                '='
            },
            if chunk.len() > 2 {
                ALPHABET[(triple & 63) as usize] as char
            } else {
                '='
            },
        ];
        out.extend(chars);
    }
    out
}

/// Incremental log tail for the WS: the 120-line window slides as the file
/// grows, so the new tail is never a prefix of the old one. Find how many
/// leading lines were already sent (overlap) and send only the new tail as
/// `log_append` (possibly empty, the client no-ops). Full `logs` resend
/// only when nothing overlaps (rotated file) or on first contact. The
/// client appends to its copy.
fn log_delta(previous: &[String], fresh: &[String]) -> (String, Value) {
    let max = fresh.len().min(previous.len());
    let mut overlap = 0;
    for len in (1..=max).rev() {
        if fresh[..len] == previous[previous.len() - len..] {
            overlap = len;
            break;
        }
    }
    if overlap == 0 && !previous.is_empty() && !fresh.is_empty() {
        return (
            "logs".to_string(),
            Value::Arr(fresh.iter().map(|line| str_value(line)).collect()),
        );
    }
    (
        "log_append".to_string(),
        Value::Arr(fresh[overlap..].iter().map(|line| str_value(line)).collect()),
    )
}

/// Push jobs + the followed account's state until the client leaves.
pub(crate) fn ws_session(mut sock: TcpStream, dispatch: &Dispatch) {
    let _ = sock.set_read_timeout(Some(Duration::from_millis(200)));
    let mut account = String::new();
    let mut last_send = 0.0f64;
    let mut log_mark: Option<(u128, u64)> = None;
    let mut last_logs: Vec<String> = Vec::new();
    let mut last_state: Option<Value> = None;
    let mut last_market: i64 = 0;
    loop {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|span| span.as_secs_f64())
            .unwrap_or(0.0);
        if now - last_send >= WS_INTERVAL {
            let (mut payload, full) = state_payload(dispatch, &account, last_state.as_ref());
            last_state = full.or(last_state);
            // Market live tail on the same 1 Hz frame: the last prints
            // per item let the market graph stretch its candle live.
            let tick = dispatch(object(vec![
                ("cmd", str_value("market")),
                ("op", str_value("tick")),
                ("since", Value::Int(last_market)),
            ]));
            for trade in tick.get("trades").as_arr().map(|items| items.to_vec()).unwrap_or_default() {
                if let Some(at) = trade.get("t").as_i64() {
                    if at > last_market {
                        last_market = at;
                    }
                }
            }
            if let Value::Obj(map) = &mut payload {
                map.insert("market".to_string(), tick);
            }
            if !account.is_empty() {
                let (_merged, _partial, log_path) = paths::user_paths(&account);
                let mark = std::fs::metadata(&log_path).ok().and_then(|meta| {
                    meta.modified()
                        .ok()?
                        .duration_since(UNIX_EPOCH)
                        .ok()
                        .map(|span| (span.as_nanos(), meta.len()))
                });
                if mark != log_mark {
                    let lines = dispatch(object(vec![
                        ("cmd", str_value("logs")),
                        ("account", str_value(&account)),
                        ("lines", Value::Int(120)),
                    ]))
                    .get("lines")
                    .clone();
                    if !lines.is_null() {
                        let fresh: Vec<String> = lines
                            .as_arr()
                            .map(|items| {
                                items
                                    .iter()
                                    .filter_map(|line| line.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default();
                        if let Value::Obj(map) = &mut payload {
                            let (key, value) = log_delta(&last_logs, &fresh);
                            map.insert(key, value);
                        }
                        log_mark = mark;
                        last_logs = fresh;
                    }
                }
            }
            if sock
                .write_all(&ws_frame(json::stringify(&payload).as_bytes(), 0x1))
                .is_err()
            {
                return;
            }
            last_send = now;
        }
        match ws_recv(&sock) {
            Err(error)
                if error.kind() == std::io::ErrorKind::TimedOut
                    || error.kind() == std::io::ErrorKind::WouldBlock =>
            {
                continue;
            }
            Err(_) => return,
            Ok((opcode, data)) => {
                if opcode == 0x8 {
                    return;
                } else if opcode == 0x9 {
                    if sock.write_all(&ws_frame(&data, 0xA)).is_err() {
                        return;
                    }
                } else if opcode == 0x1 {
                    let text = String::from_utf8_lossy(&data);
                    if let Ok(request) = json::parse(&text) {
                        let next = request.get("account").as_str().unwrap_or("").to_string();
                        // New base: the client drops its copy (`last = null`)
                        // and expects a full state, not a diff of another bot.
                        if next != account {
                            account = next;
                            last_state = None;
                            last_logs = Vec::new();
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn websocket_accept_vector() {
        // RFC 6455 example.
        let accept = base64(&sha1(b"dGhlIHNhbXBsZSBub25jZQ==258EAFA5-E914-47DA-95CA-C5AB0DC85B11"));
        assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn ws_payload_keeps_displayed_drops_dead_weight() {
        fn live_dispatch(ticks: i64, loc_id: i64) -> Dispatch {
            Arc::new(move |request: Value| match request.get("cmd").as_str().unwrap_or("") {
            "status" => object(vec![("jobs", Value::Arr(Vec::new()))]),
            _ => object(vec![("state", object(vec![
                ("account", str_value("bob")),
                ("position", Value::Arr(vec![Value::Int(0), Value::Int(1), Value::Int(2)])),
                ("zone", str_value("Lumbridge")),
                ("ticks", Value::Int(ticks)),
                ("inv", Value::Arr(Vec::new())),
                ("skills", Value::Arr(Vec::new())),
                ("locs", Value::Arr(vec![object(vec![
                    ("id", Value::Int(loc_id)),
                    ("name", str_value("Bank booth")),
                    ("cosmetic", Value::Bool(false)),
                ])])),
                ("trail", Value::Arr(vec![Value::Int(1)])),
                ("bank", Value::Arr(vec![Value::Int(2)])),
                ("messages", Value::Arr(vec![Value::Int(3)])),
                ("message_ticks", Value::Arr(vec![Value::Int(4)])),
                ("texts", Value::Obj(Default::default())),
                ("flora", Value::Arr(Vec::new())),
                ("models", Value::Obj(Default::default())),
            ]))])})
        }
        let (payload, full) = state_payload(&live_dispatch(7, 3), "bob", None);
        let state = payload.get("state");
        for dead in ["trail", "bank", "messages", "message_ticks", "texts", "flora", "models"] {
            assert!(state.get(dead).is_null(), "{dead} rides the WS but the screen never reads it");
        }
        assert!(!state.get("inv").is_null());
        assert!(!state.get("skills").is_null());
        // locs stay compact: [id, cosmetic] rows + the names table.
        let locs = state.get("locs").as_arr().unwrap();
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].as_arr().unwrap().len(), 2);
        assert_eq!(
            state.get("loc_names").get("3").as_str(),
            Some("Bank booth")
        );
        // Same snapshot again: only the `account` marker rides, the client
        // keeps its copy.
        let full = full.expect("full state to remember");
        let (repeat, _) = state_payload(&live_dispatch(7, 3), "bob", Some(&full));
        let frame = repeat.get("state");
        assert_eq!(frame.as_obj().map(|map| map.len()), Some(1));
        assert_eq!(frame.get("account").as_str(), Some("bob"));
        // One tick moved: only that key rides, plus `account` (a frame
        // without it reads as offline on the client).
        let (delta, _) = state_payload(&live_dispatch(8, 3), "bob", Some(&full));
        let frame = delta.get("state");
        assert_eq!(frame.as_obj().map(|map| map.len()), Some(2));
        assert_eq!(frame.get("ticks").as_i64(), Some(8));
        assert_eq!(frame.get("account").as_str(), Some("bob"));
        // Locs moved: the rows ride with their decode table.
        let (moved, _) = state_payload(&live_dispatch(8, 4), "bob", Some(&full));
        let frame = moved.get("state");
        assert!(!frame.get("locs").as_arr().unwrap().is_empty());
        assert_eq!(frame.get("loc_names").get("4").as_str(), Some("Bank booth"));
        assert!(!frame.get("loc_fields").as_arr().unwrap().is_empty());
    }

    #[test]
    fn log_tail_goes_incremental() {
        let old = vec!["a".to_string(), "b".to_string()];
        let (key, value) = log_delta(&old, &["a".to_string(), "b".to_string(), "c".to_string()]);
        assert_eq!(key, "log_append");
        assert_eq!(value.as_arr().unwrap().len(), 1);
        let (key, value) = log_delta(&old, &old.clone());
        assert_eq!(key, "log_append");
        assert!(value.as_arr().unwrap().is_empty());
        // Truncated or rotated file: full resend, the client replaces.
        let (key, value) = log_delta(&old, &["x".to_string()]);
        assert_eq!(key, "logs");
        assert_eq!(value.as_arr().unwrap().len(), 1);
        // Sliding window: only the new tail rides.
        let (key, value) = log_delta(
            &["a".to_string(), "b".to_string(), "c".to_string()],
            &["b".to_string(), "c".to_string(), "d".to_string()],
        );
        assert_eq!(key, "log_append");
        let tail = value.as_arr().unwrap();
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].as_str(), Some("d"));
    }

    #[test]
    fn compact_locs_shape() {
        let mut state = object(vec![(
            "locs",
            Value::Arr(vec![object(vec![
                ("x", Value::Int(1)),
                ("z", Value::Int(2)),
                ("id", Value::Int(3)),
                ("name", str_value("Bank booth")),
                ("act", Value::Bool(true)),
            ])]),
        )]);
        compact_locs(&mut state);
        let locs = state.get("locs").as_arr().unwrap();
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].as_arr().unwrap().len(), LOC_FIELDS.len());
        assert_eq!(
            state.get("loc_names").get("3").as_str(),
            Some("Bank booth")
        );
        assert!(!state.get("loc_fields").as_arr().unwrap().is_empty());
    }
}
