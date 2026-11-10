//! Web view over the orchestrator: dashboard and HTTP/WS API.
//!
//! Same routes, same JSON shapes: GET / /spectator.js /items /items.js
//! /market /market.js /rates /rates.js /life /life.js /economy /economy.js
//! /map/<dir>/<file>.png /map/<dir>/manifest.json /mapscene/<sprite>.png
//! /api/map /api/status /api/state/<account> /api/items/history
//! /api/rates/table /api/lives /api/life/<account> /api/economy
//! /api/market/<op>?… /api/scripts /api/accounts /api/logs/<account> +
//! POST /api/open|run|unload|pause|resume|close + /ws pushing every 1 s.
//! Static files are re-read from orchestration/static/ on every request.
//!
//! The spectator map is precomputed once at daemon startup (mapbuild: one
//! PNG per painted cluster per level, 4 px per tile) and served immutable;
//! the client crops around the player, so the dashboard issues zero map
//! image requests. Ground, walls and mapscene sprites come from the packed
//! game data via mapcache/minimap (same passes as the client).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lifescape_json::{self as json, Value, object, str_value};
use crate::minimap::Minimap;
use crate::paths;

pub const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
pub const WS_INTERVAL: f64 = 1.0;

pub type Dispatch = Arc<dyn Fn(Value) -> Value + Send + Sync>;

/// Loc columns on the wire. The screen only reads `name` (via `loc_names`),
/// `cosmetic` (scenery panel) and `id` (the names key): x/z/shape/angle and
/// the mapscene origin ride no pixel, the map is the static PNG.
const LOC_FIELDS: &[&str] = &["id", "cosmetic"];

/// Job columns on the wire: exactly what the spectator list renders
/// (name/level/position/window/state line). Phase/zone/ticks/errors ride the
/// followed state, not the list.
const JOB_LIST_FIELDS: &[&str] = &[
    "account",
    "account_name",
    "combat",
    "position",
    "modal",
    "intention",
    "status",
    "paused",
    "script",
    "hitpoints",
];

/// Pack-backed ground and sprite renderer, shared by every connection.
fn shared_maps() -> &'static Minimap {
    static MAPS: OnceLock<Minimap> = OnceLock::new();
    MAPS.get_or_init(Minimap::from_pack_dir)
}

fn annotate_mapscene(mut state: Value) -> Value {
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

fn sha1(message: &[u8]) -> [u8; 20] {
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

fn base64(data: &[u8]) -> String {
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
fn ws_session(mut sock: TcpStream, dispatch: &Dispatch) {
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

// -- HTTP ------------------------------------------------------------------

fn asset(name: &str) -> std::io::Result<Vec<u8>> {
    std::fs::read(paths::static_dir().join(name))
}

/// One precomputed map file: (dir, file) when the path is exactly
/// `map-<16 hex>/<name>.png|manifest.json`, nothing otherwise (no
/// traversal: parts are already split on '/', ".." never matches).
fn map_asset(parts: &[&str]) -> Option<(String, String)> {
    if parts.len() != 2 {
        return None;
    }
    let (dir, file) = (parts[0], parts[1]);
    if dir.len() != 20 || !dir.starts_with("map-") || !dir[4..].chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let ok = file == "manifest.json"
        || (file.ends_with(".png")
            && file.len() <= 64
            && file.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'));
    if !ok {
        return None;
    }
    Some((dir.to_string(), file.to_string()))
}

fn parse_query(query: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = match pair.find('=') {
            Some(pos) => (&pair[..pos], &pair[pos + 1..]),
            None => (pair, ""),
        };
        params.insert(key.to_string(), value.to_string());
    }
    params
}

struct Response {
    status: u16,
    content_type: &'static str,
    cache: &'static str,
    body: Vec<u8>,
}

fn json_reply(data: Value, status: u16) -> Response {
    Response {
        status,
        content_type: "application/json",
        cache: "no-store",
        body: json::stringify(&data).into_bytes(),
    }
}

fn ok_json(data: Value) -> Response {
    json_reply(data, 200)
}

fn err_json(message: String) -> Value {
    object(vec![("ok", Value::Bool(false)), ("error", str_value(&message))])
}

// -- life + economy (L10 front) ------------------------------------------

/// Candle width of a history resolution: `1d` days, anything else hours
/// (same mapping as the market `history` op).
fn res_width(res: &str) -> i64 {
    if res == "1d" { 86400 } else { 3600 }
}

/// Snap a requested history window to candle boundaries: `from` floors to
/// the resolution, `to` ceils so the live candle stays inside the
/// half-open `[from, to)` the store reads. At least one width apart.
pub fn snap_range(from: i64, to: i64, res: &str) -> (i64, i64) {
    let width = res_width(res);
    let snapped_from = (from.max(0) / width) * width;
    let snapped_to = ((to.max(1) + width - 1) / width) * width;
    (snapped_from, snapped_to.max(snapped_from + width))
}

fn lives_dir(run_dir: &std::path::Path) -> std::path::PathBuf {
    run_dir.join("lives")
}

/// One life file verbatim: goal, nodes (status, method, alternatives with
/// costs), engagement, history. Manual bots have no file: loud error.
pub fn life_response(account: &str) -> Value {
    life_response_in(&crate::paths::run_dir(), account)
}

/// One life file verbatim (see `life_response`); `run_dir` is explicit so
/// tests serve a scratch root without touching the process environment.
pub fn life_response_in(run_dir: &std::path::Path, account: &str) -> Value {
    if account.is_empty() || account == "." || account == ".." || account.contains('/') {
        return err_json(format!("bad account '{account}'"));
    }
    let text = match std::fs::read_to_string(lives_dir(run_dir).join(format!("{account}.json"))) {
        Ok(text) => text,
        Err(_) => return err_json(format!("no life for {account}")),
    };
    match json::parse(&text) {
        Ok(life) => object(vec![("ok", Value::Bool(true)), ("life", life)]),
        Err(error) => err_json(format!("life {account} corrupt: {error}")),
    }
}

/// Every account with a life file: goal plus node counts by status.
pub fn lives_response() -> Value {
    lives_response_in(&crate::paths::run_dir())
}

/// Life index (see `lives_response`) over an explicit root.
pub fn lives_response_in(run_dir: &std::path::Path) -> Value {
    let mut rows = Vec::new();
    if let Ok(entries) = std::fs::read_dir(lives_dir(run_dir)) {
        let mut names: Vec<String> = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.ends_with(".json"))
            .collect();
        names.sort();
        for name in names {
            let account = name[..name.len() - 5].to_string();
            let Ok(text) = std::fs::read_to_string(lives_dir(run_dir).join(&name)) else {
                continue;
            };
            let Ok(life) = json::parse(&text) else {
                continue;
            };
            let mut counts = std::collections::BTreeMap::new();
            for node in life.get("nodes").as_arr().cloned().unwrap_or_default() {
                let status = node.get("status").as_str().unwrap_or("pending").to_string();
                *counts.entry(status).or_insert(0i64) += 1;
            }
            let mut summary = std::collections::BTreeMap::new();
            for (status, count) in counts {
                summary.insert(status, Value::Int(count));
            }
            rows.push(object(vec![
                ("account", str_value(&account)),
                ("goal", life.get("goal").clone()),
                ("active", life.get("active").clone()),
                ("ironman", life.get("profile").get("ironman").clone()),
                ("nodes", Value::Obj(summary)),
            ]));
        }
    }
    object(vec![("ok", Value::Bool(true)), ("lives", Value::Arr(rows))])
}

/// Open sell/buy exposure of one merchant account from its open orders.
fn merchant_exposure(dispatch: &Dispatch, account: &str) -> (i64, i64, i64, usize, Vec<i64>) {
    let orders = dispatch(object(vec![
        ("cmd", str_value("market")),
        ("op", str_value("my_orders")),
        ("account", str_value(account)),
    ]));
    let mut buy_value = 0i64;
    let mut sell_qty = 0i64;
    let mut sell_value = 0i64;
    let mut open = 0usize;
    let mut items = Vec::new();
    for row in orders.get("orders").as_arr().cloned().unwrap_or_default() {
        if row.get("status").as_str() != Some("open") {
            continue;
        }
        open += 1;
        let left = row.get("left").as_i64().unwrap_or(0).max(0);
        let price = row.get("price").as_i64().unwrap_or(0).max(0);
        let item = row.get("item").as_i64().unwrap_or(-1);
        if row.get("side").as_str() == Some("buy") {
            buy_value = buy_value.saturating_add(left.saturating_mul(price));
        } else {
            sell_qty = sell_qty.saturating_add(left);
            sell_value = sell_value.saturating_add(left.saturating_mul(price));
        }
        if item >= 0 && !items.contains(&item) {
            items.push(item);
        }
    }
    (buy_value, sell_qty, sell_value, open, items)
}

/// Depth quantity of one book side: `[[price, qty], …]`.
fn book_side_qty(book: &Value, side: &str) -> i64 {
    book.get(side)
        .as_arr()
        .map(|levels| {
            levels
                .iter()
                .map(|level| level.as_arr().and_then(|cells| cells.get(1)).and_then(|qty| qty.as_i64()).unwrap_or(0))
                .sum()
        })
        .unwrap_or(0)
}

/// Largest share of resting depth held by open orders on one side: the
/// merchant's weight in the book on the items they quote.
fn merchant_book_share(dispatch: &Dispatch, account: &str, items: &[i64]) -> f64 {
    let orders = dispatch(object(vec![
        ("cmd", str_value("market")),
        ("op", str_value("my_orders")),
        ("account", str_value(account)),
    ]));
    let mut open: std::collections::BTreeMap<(i64, String), i64> = std::collections::BTreeMap::new();
    for row in orders.get("orders").as_arr().cloned().unwrap_or_default() {
        if row.get("status").as_str() != Some("open") {
            continue;
        }
        let key = (row.get("item").as_i64().unwrap_or(-1), row.get("side").as_str().unwrap_or("").to_string());
        *open.entry(key).or_insert(0) += row.get("left").as_i64().unwrap_or(0).max(0);
    }
    let mut top = 0.0f64;
    for item in items.iter().take(12) {
        let book = dispatch(object(vec![
            ("cmd", str_value("market")),
            ("op", str_value("book")),
            ("item", Value::Int(*item)),
            ("depth", Value::Int(10)),
        ]));
        for (side, key) in [("buys", "buy"), ("sells", "sell")] {
            let depth = book_side_qty(&book, side);
            let held = open.get(&(*item, key.to_string())).cloned().unwrap_or(0) as f64;
            if held > 0.0 {
                let share = if depth > 0 { held / depth as f64 } else { 1.0 };
                if share > top {
                    top = share;
                }
            }
        }
    }
    top
}

/// Economy payload from existing sources only: holdings via the `items`
/// command, market stats via the `market` command, deals/trends/merchants
/// via the store, top activities via the rates table. One JSON fetch for
/// the economy page; bots never read it.
pub fn economy_response(dispatch: &Dispatch) -> Value {
    economy_response_in(dispatch, &crate::paths::run_dir())
}

/// Economy payload (see `economy_response`) over an explicit root.
pub fn economy_response_in(dispatch: &Dispatch, run_dir: &std::path::Path) -> Value {
    let store = match lifescape_store::Store::open(&run_dir.join("lifescape.db")) {
        Ok(store) => store,
        Err(error) => return err_json(format!("economy store: {error}")),
    };
    let holdings = dispatch(object(vec![("cmd", str_value("items"))])).get("data").clone();
    let market = dispatch(object(vec![
        ("cmd", str_value("market")),
        ("op", str_value("stats")),
    ]));
    let accounts: Vec<String> = dispatch(object(vec![("cmd", str_value("accounts"))]))
        .get("accounts")
        .as_arr()
        .map(|rows| rows.iter().filter_map(|row| row.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let mut settled = 0i64;
    let mut failed = 0i64;
    for account in &accounts {
        if let Ok(Some(row)) = store.get_account(account) {
            settled += row.deals_settled;
            failed += row.deals_failed;
        }
    }
    let deals_total = settled + failed;
    let mut trends = Vec::new();
    if let Ok(catalogue) = store.list_items() {
        for item in &catalogue {
            for trend in store.trends_for_item(item.item_id).unwrap_or_default() {
                if trend.period == "24h" && trend.score != 0.0 {
                    trends.push((item.item_id, item.name.clone(), trend.shortage, trend.momentum, trend.score));
                }
            }
        }
    }
    trends.sort_by(|left, right| right.4.abs().total_cmp(&left.4.abs()));
    trends.truncate(15);
    let rates = crate::rates_service::rates_api(&**dispatch);
    let mut activities: Vec<(String, i64, f64)> = Vec::new();
    let mut flip: Option<(String, f64, bool)> = None;
    for row in rates.get("profit").as_arr().cloned().unwrap_or_default() {
        let method = row.get("method").as_str().unwrap_or("").to_string();
        let profit = row.get("profit").as_f64().unwrap_or(0.0);
        if method == "market:flip" || (flip.is_none() && method == "market") {
            flip = Some((method.clone(), profit, row.get("price_prior").as_bool().unwrap_or(true)));
        }
        activities.push((method, row.get("band").as_i64().unwrap_or(0), profit));
    }
    activities.sort_by(|left, right| right.2.total_cmp(&left.2));
    activities.truncate(10);
    let mut merchants = Vec::new();
    if let Ok(entries) = std::fs::read_dir(lives_dir(run_dir)) {
        let mut names: Vec<String> = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.ends_with(".json"))
            .collect();
        names.sort();
        for name in names {
            let account = name[..name.len() - 5].to_string();
            let text = std::fs::read_to_string(lives_dir(run_dir).join(&name)).unwrap_or_default();
            let life = json::parse(&text).unwrap_or(Value::Null);
            if life.get("profile").get("archetype").as_str() != Some("merchant") {
                continue;
            }
            let (buy_value, sell_qty, sell_value, open_orders, items) = merchant_exposure(dispatch, &account);
            let open_deals = dispatch(object(vec![
                ("cmd", str_value("market")),
                ("op", str_value("my_deals")),
                ("account", str_value(&account)),
            ]))
            .get("deals")
            .as_arr()
            .map(|rows| {
                rows.iter()
                    .filter(|row| !matches!(row.get("status").as_str(), Some("settled") | Some("failed")))
                    .count()
            })
            .unwrap_or(0);
            let (deal_settled, deal_failed) = store
                .get_account(&account)
                .ok()
                .flatten()
                .map(|row| (row.deals_settled, row.deals_failed))
                .unwrap_or((0, 0));
            merchants.push(object(vec![
                ("account", str_value(&account)),
                ("buy_value", Value::Int(buy_value)),
                ("sell_qty", Value::Int(sell_qty)),
                ("sell_value", Value::Int(sell_value)),
                ("settled", Value::Int(deal_settled)),
                ("failed", Value::Int(deal_failed)),
                ("open_orders", Value::Int(open_orders as i64)),
                ("open_deals", Value::Int(open_deals as i64)),
                ("book_share", Value::Float(merchant_book_share(dispatch, &account, &items))),
            ]));
        }
    }
    let clock = SystemTime::now().duration_since(UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0);
    object(vec![
        ("ok", Value::Bool(true)),
        ("clock", Value::Int(clock)),
        ("holdings", holdings),
        (
            "market",
            object(vec![
                ("items", market.get("items").clone()),
                ("books", market.get("books").clone()),
                ("orders", market.get("orders").clone()),
                ("placed", market.get("placed").clone()),
                ("matched", market.get("matched").clone()),
                ("deals", market.get("deals").clone()),
            ]),
        ),
        (
            "deals",
            object(vec![
                ("total", Value::Int(deals_total)),
                ("settled", Value::Int(settled)),
                ("failed", Value::Int(failed)),
                (
                    "fail_rate",
                    Value::Float(if deals_total > 0 { failed as f64 / deals_total as f64 } else { 0.0 }),
                ),
            ]),
        ),
        (
            "activities",
            Value::Arr(
                activities
                    .iter()
                    .map(|(method, band, profit)| {
                        object(vec![
                            ("method", str_value(method)),
                            ("band", Value::Int(*band)),
                            ("profit", Value::Float(*profit)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "flip",
            flip.map(|(method, profit, prior)| {
                object(vec![
                    ("method", str_value(&method)),
                    ("profit", Value::Float(profit)),
                    ("price_prior", Value::Bool(prior)),
                ])
            })
            .unwrap_or(Value::Null),
        ),
        (
            "trends",
            Value::Arr(
                trends
                    .iter()
                    .map(|(item, name, shortage, momentum, score)| {
                        object(vec![
                            ("item", Value::Int(*item)),
                            ("name", str_value(name)),
                            ("shortage", Value::Float(*shortage)),
                            ("momentum", Value::Float(*momentum)),
                            ("score", Value::Float(*score)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("merchants", Value::Arr(merchants)),
    ])
}

fn not_found() -> Response {
    json_reply(object(vec![("ok", Value::Bool(false)), ("error", str_value("not found"))]), 404)
}

fn handle_get(path: &str, query: &str, dispatch: &Dispatch) -> Response {
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let result: Result<Response, String> = (|| {
        if parts.is_empty() {
            return Ok(Response {
                status: 200,
                content_type: "text/html; charset=utf-8",
                cache: "no-store",
                body: asset("index.html").map_err(|error| error.to_string())?,
            });
        }
        if parts == ["spectator.js"] {
            return Ok(Response {
                status: 200,
                content_type: "application/javascript",
                cache: "no-store",
                body: asset("spectator.js").map_err(|error| error.to_string())?,
            });
        }
        if parts == ["items"] {
            return Ok(Response {
                status: 200,
                content_type: "text/html; charset=utf-8",
                cache: "no-store",
                body: asset("items.html").map_err(|error| error.to_string())?,
            });
        }
        if parts == ["items.js"] {
            return Ok(Response {
                status: 200,
                content_type: "application/javascript",
                cache: "no-store",
                body: asset("items.js").map_err(|error| error.to_string())?,
            });
        }
        if let Some((dir, file)) = map_asset(&parts) {
            let body = std::fs::read(paths::static_dir().join(&dir).join(&file)).map_err(|error| error.to_string())?;
            return Ok(Response {
                status: 200,
                content_type: if file.ends_with(".json") { "application/json" } else { "image/png" },
                cache: "public, max-age=31536000, immutable",
                body,
            });
        }
        if parts.len() == 2 && parts[0] == "mapscene" && parts[1].ends_with(".png") {
            let png = parts[1][..parts[1].len() - 4]
                .parse::<i32>()
                .ok()
                .and_then(|sprite| shared_maps().cache().mapscene_png(sprite));
            return match png {
                Some(body) => Ok(Response {
                    status: 200,
                    content_type: "image/png",
                    cache: "public, max-age=86400",
                    body,
                }),
                None => Ok(not_found()),
            };
        }
        if parts == ["api", "status"] {
            return Ok(ok_json(dispatch(object(vec![("cmd", str_value("status"))]))));
        }
        if parts == ["api", "map"] {
            let pointer = std::fs::read_to_string(paths::static_dir().join("map-current.json")).map_err(|error| error.to_string())?;
            let parsed = json::parse(&pointer).map_err(|error| error.to_string())?;
            return Ok(ok_json(object(vec![
                ("ok", Value::Bool(true)),
                ("dir", parsed.get("dir").clone()),
                ("hash", parsed.get("hash").clone()),
            ])));
        }
        if parts.len() >= 2 && parts[..2] == ["api", "state"] {
            let name = if parts.len() > 2 { parts[2] } else { "" };
            let reply = dispatch(object(vec![
                ("cmd", str_value("show")),
                ("account", str_value(name)),
            ]));
            let state = reply.get("state").clone();
            let mut shaped = std::collections::BTreeMap::new();
            shaped.insert("ok".to_string(), reply.get("ok").clone());
            shaped.insert("state".to_string(), annotate_mapscene(state));
            return Ok(ok_json(Value::Obj(shaped)));
        }
        if parts == ["market"] {
            return Ok(Response {
                status: 200,
                content_type: "text/html; charset=utf-8",
                cache: "no-store",
                body: asset("market.html").map_err(|error| error.to_string())?,
            });
        }
        if parts == ["market.js"] {
            return Ok(Response {
                status: 200,
                content_type: "application/javascript",
                cache: "no-store",
                body: asset("market.js").map_err(|error| error.to_string())?,
            });
        }
        if parts == ["rates"] {
            return Ok(Response {
                status: 200,
                content_type: "text/html; charset=utf-8",
                cache: "no-store",
                body: asset("rates.html").map_err(|error| error.to_string())?,
            });
        }
        if parts == ["rates.js"] {
            return Ok(Response {
                status: 200,
                content_type: "application/javascript",
                cache: "no-store",
                body: asset("rates.js").map_err(|error| error.to_string())?,
            });
        }
        if parts == ["api", "rates", "table"] {
            return Ok(ok_json(crate::rates_service::rates_api(&**dispatch)));
        }
        if parts == ["api", "lives"] {
            return Ok(ok_json(lives_response()));
        }
        if parts.len() >= 2 && parts[..2] == ["api", "life"] {
            let name = if parts.len() > 2 { parts[2] } else { "" };
            return Ok(ok_json(life_response(name)));
        }
        if parts == ["api", "economy"] {
            return Ok(ok_json(economy_response(dispatch)));
        }
        if parts == ["life"] {
            return Ok(Response {
                status: 200,
                content_type: "text/html; charset=utf-8",
                cache: "no-store",
                body: asset("life.html").map_err(|error| error.to_string())?,
            });
        }
        if parts == ["life.js"] {
            return Ok(Response {
                status: 200,
                content_type: "application/javascript",
                cache: "no-store",
                body: asset("life.js").map_err(|error| error.to_string())?,
            });
        }
        if parts == ["economy"] {
            return Ok(Response {
                status: 200,
                content_type: "text/html; charset=utf-8",
                cache: "no-store",
                body: asset("economy.html").map_err(|error| error.to_string())?,
            });
        }
        if parts == ["economy.js"] {
            return Ok(Response {
                status: 200,
                content_type: "application/javascript",
                cache: "no-store",
                body: asset("economy.js").map_err(|error| error.to_string())?,
            });
        }
        if parts.len() >= 3 && parts[..2] == ["api", "market"] {
            // Market JSON API: the op is the third segment, the rest of
            // the query string becomes the request fields.
            let op = parts[2].to_string();
            let params = parse_query(query);
            let num = |key: &str| params.get(key).and_then(|raw| raw.parse::<i64>().ok());
            let mut fields: Vec<(String, Value)> = vec![("cmd".to_string(), str_value("market")), ("op".to_string(), str_value(&op))];
            for key in ["item", "depth", "from", "to", "limit", "since"] {
                if let Some(value) = num(key) {
                    fields.push((key.to_string(), Value::Int(value)));
                }
            }
            for key in ["q", "res", "account", "side"] {
                if let Some(value) = params.get(key) {
                    // Search names carry spaces (`Air rune`): decode the
                    // two escapes browsers send, nothing more.
                    let decoded = value.replace('+', " ").replace("%20", " ");
                    fields.push((key.to_string(), str_value(&decoded)));
                }
            }
            // History reads half-open `[from, to)` candle windows: snap
            // both bounds to the resolution so the live candle stays
            // inside, whatever the client clock sent.
            if op == "history" {
                if let (Some(from), Some(to)) = (num("from"), num("to")) {
                    let res = params.get("res").map(String::as_str).unwrap_or("1h");
                    let (snapped_from, snapped_to) = snap_range(from, to, res);
                    fields.retain(|(key, _)| key != "from" && key != "to");
                    fields.push(("from".to_string(), Value::Int(snapped_from)));
                    fields.push(("to".to_string(), Value::Int(snapped_to)));
                }
            }
            let mut map = std::collections::BTreeMap::new();
            for (key, value) in fields {
                map.insert(key, value);
            }
            return Ok(ok_json(dispatch(Value::Obj(map))));
        }
        if parts == ["api", "items", "history"] {
            let reply = dispatch(object(vec![("cmd", str_value("items"))]));
            let data = reply.get("data").clone();
            return Ok(ok_json(if data.is_null() { reply } else { data }));
        }
        if parts == ["api", "scripts"] {
            return Ok(ok_json(dispatch(object(vec![("cmd", str_value("scripts"))]))));
        }
        if parts == ["api", "accounts"] {
            return Ok(ok_json(dispatch(object(vec![("cmd", str_value("accounts"))]))));
        }
        if parts.len() >= 2 && parts[..2] == ["api", "logs"] {
            let name = if parts.len() > 2 { parts[2] } else { "" };
            let lines = parse_query(query)
                .get("lines")
                .and_then(|raw| raw.parse::<i64>().ok())
                .unwrap_or(60);
            return Ok(ok_json(dispatch(object(vec![
                ("cmd", str_value("logs")),
                ("account", str_value(name)),
                ("lines", Value::Int(lines)),
            ]))));
        }
        Ok(not_found())
    })();
    match result {
        Ok(reply) => reply,
        Err(error) => json_reply(
            object(vec![("ok", Value::Bool(false)), ("error", str_value(&error))]),
            500,
        ),
    }
}

fn handle_post(path: &str, body: &[u8], dispatch: &Dispatch) -> Response {
    let text = String::from_utf8_lossy(body);
    let payload = match json::parse(&text) {
        Ok(Value::Obj(map)) => Value::Obj(map),
        _ => {
            return json_reply(
                object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value("invalid JSON")),
                ]),
                400,
            );
        }
    };
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let account = payload.get("account").as_str().unwrap_or("").to_string();
    let result: Result<Response, String> = (|| {
        if parts == ["api", "open"] {
            return Ok(ok_json(dispatch(object(vec![
                ("cmd", str_value("open")),
                ("account", str_value(&account)),
            ]))));
        }
        if parts == ["api", "run"] {
            let script = payload.get("script").as_str().unwrap_or("").to_string();
            let args = payload.get("args").clone();
            return Ok(ok_json(dispatch(object(vec![
                ("cmd", str_value("run")),
                ("account", str_value(&account)),
                ("script", str_value(&script)),
                (
                    "args",
                    match args {
                        Value::Arr(_) => args,
                        _ => Value::Arr(Vec::new()),
                    },
                ),
            ]))));
        }
        for action in ["unload", "pause", "resume", "close"] {
            if parts == ["api", action] {
                return Ok(ok_json(dispatch(object(vec![
                    ("cmd", str_value(action)),
                    ("account", str_value(&account)),
                ]))));
            }
        }
        Ok(not_found())
    })();
    match result {
        Ok(reply) => reply,
        Err(error) => json_reply(
            object(vec![("ok", Value::Bool(false)), ("error", str_value(&error))]),
            500,
        ),
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        101 => "Switching Protocols",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

fn serve_conn(stream: TcpStream, dispatch: &Dispatch) {
    let mut sock = stream;
    let _ = sock.set_read_timeout(Some(Duration::from_secs(30)));
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match (&sock).read(&mut buf) {
            Ok(0) => return,
            Ok(count) => {
                raw.extend_from_slice(&buf[..count]);
                if raw.len() > 65536 || raw.windows(4).any(|win| win == b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => return,
        }
    }
    let head_end = match raw.windows(4).position(|win| win == b"\r\n\r\n") {
        Some(pos) => pos + 4,
        None => return,
    };
    let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or("");
    let mut words = request_line.split_whitespace();
    let method = words.next().unwrap_or("");
    let target = words.next().unwrap_or("/");
    let mut headers = HashMap::new();
    for line in lines {
        if let Some(pos) = line.find(':') {
            headers.insert(line[..pos].trim().to_lowercase(), line[pos + 1..].trim().to_string());
        }
    }
    let (path, query) = match target.find('?') {
        Some(pos) => (&target[..pos], &target[pos + 1..]),
        None => (target, ""),
    };
    if method == "GET"
        && path == "/ws"
        && headers
            .get("upgrade")
            .is_some_and(|value| value.to_lowercase().contains("websocket"))
    {
        let key = headers.get("sec-websocket-key").cloned().unwrap_or_default();
        if key.is_empty() {
            let reply = json_reply(
                object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value("websocket key required")),
                ]),
                400,
            );
            write_response(&sock, &reply);
            return;
        }
        let accept = base64(&sha1(format!("{key}{WS_GUID}").as_bytes()));
        let handshake = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
        );
        if sock.write_all(handshake.as_bytes()).is_err() {
            return;
        }
        ws_session(sock, dispatch);
        return;
    }
    let reply = if method == "GET" {
        handle_get(path, query, dispatch)
    } else if method == "POST" {
        let len: usize = headers
            .get("content-length")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let mut body = raw[head_end..].to_vec();
        while body.len() < len {
            match (&sock).read(&mut buf) {
                Ok(0) => break,
                Ok(count) => body.extend_from_slice(&buf[..count]),
                Err(_) => break,
            }
            if body.len() > 4_194_304 {
                break;
            }
        }
        body.truncate(len);
        handle_post(path, &body, dispatch)
    } else {
        not_found()
    };
    write_response(&sock, &reply);
}

fn write_response(mut sock: &TcpStream, reply: &Response) {
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nCache-Control: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reply.status,
        reason(reply.status),
        reply.content_type,
        reply.cache,
        reply.body.len()
    );
    let _ = sock.write_all(head.as_bytes());
    let _ = sock.write_all(&reply.body);
}

/// Bind the dashboard (called before sessions are adopted: a bind failure
/// exits with every session untouched).
pub fn start_web(dispatch: Dispatch, port: u16) {
    let server = match TcpListener::bind(("0.0.0.0", port)) {
        Ok(server) => server,
        Err(error) => {
            eprintln!("webapp bind failed on {port}: {error}");
            std::process::exit(1);
        }
    };
    println!("webapp listening on http://0.0.0.0:{port}/");
    std::thread::Builder::new()
        .name("web-accept".to_string())
        .spawn(move || {
            for stream in server.incoming() {
                match stream {
                    Ok(stream) => {
                        let dispatch = dispatch.clone();
                        std::thread::Builder::new()
                            .name("web-conn".to_string())
                            .spawn(move || serve_conn(stream, &dispatch))
                            .expect("web connection thread spawns");
                    }
                    Err(_) => break,
                }
            }
        })
        .expect("web acceptor spawns");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websocket_accept_vector() {
        // RFC 6455 example.
        let accept = base64(&sha1(b"dGhlIHNhbXBsZSBub25jZQ==258EAFA5-E914-47DA-95CA-C5AB0DC85B11"));
        assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn map_asset_accepts_only_hashed_files() {
        assert_eq!(
            map_asset(&["map-0123456789abcdef", "c44-l0.png"]),
            Some(("map-0123456789abcdef".to_string(), "c44-l0.png".to_string()))
        );
        assert_eq!(
            map_asset(&["map-0123456789abcdef", "manifest.json"]),
            Some(("map-0123456789abcdef".to_string(), "manifest.json".to_string()))
        );
        assert_eq!(map_asset(&["map-xyz", "c44-l0.png"]), None);
        assert_eq!(map_asset(&["map-0123456789abcdef", "x.png", "y"]), None);
        assert_eq!(map_asset(&["map-0123456789abcdef", ".."]), None);
        assert_eq!(map_asset(&["map-0123456789abcdef", "c44-l0.jpg"]), None);
        assert_eq!(map_asset(&["map-0123456789ABCDEF", "c44-l0.png"]), Some(("map-0123456789ABCDEF".to_string(), "c44-l0.png".to_string())));
        assert_eq!(map_asset(&["map-0123456789abcde", "c44-l0.png"]), None);
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

    #[test]
    fn history_window_snaps_to_candle_edges() {
        // Day resolution: from floors, to ceils so the live candle stays in.
        assert_eq!(snap_range(100001, 200001, "1d"), (86400, 259200));
        // Hour resolution, aligned bounds pass through.
        assert_eq!(snap_range(3600, 7200, "1h"), (3600, 7200));
        assert_eq!(snap_range(3601, 7200, "1h"), (3600, 7200));
        assert_eq!(snap_range(3600, 7201, "1h"), (3600, 10800));
        // Unknown resolutions read hourly, like the market op.
        assert_eq!(snap_range(3601, 7201, "bogus"), (3600, 10800));
        // Degenerate spans keep at least one candle.
        assert_eq!(snap_range(5000, 5000, "1h"), (3600, 7200));
    }

    #[test]
    fn market_history_forwards_the_snapped_window() {
        fn echo_dispatch(request: Value) -> Value {
            object(vec![
                ("ok", Value::Bool(true)),
                ("op", request.get("op").clone()),
                ("from", request.get("from").clone()),
                ("to", request.get("to").clone()),
                ("res", request.get("res").clone()),
            ])
        }
        let dispatch: Dispatch = Arc::new(echo_dispatch);
        let reply = handle_get("/api/market/history", "item=7&res=1d&from=100001&to=200001", &dispatch);
        assert_eq!(reply.status, 200);
        let body = String::from_utf8(reply.body).expect("json body");
        let payload = json::parse(&body).expect("json parses");
        assert_eq!(payload.get("from").as_i64(), Some(86400));
        assert_eq!(payload.get("to").as_i64(), Some(259200));
        assert_eq!(payload.get("res").as_str(), Some("1d"));
        // Other ops pass their bounds through untouched.
        let reply = handle_get("/api/market/trades", "item=7&limit=5", &dispatch);
        let payload = json::parse(&String::from_utf8(reply.body).expect("json body")).expect("json parses");
        assert_eq!(payload.get("op").as_str(), Some("trades"));
        assert!(payload.get("from").is_null());
    }

    /// Life + economy shapes against a scratch project root: one life
    /// file, an empty store, canned daemon replies. Single test so the
    /// `LIFESCAPE_ROOT` override never races another test.
    #[test]
    fn life_and_economy_shapes() {
        let root = std::env::temp_dir().join(format!("lifescape-l10-{}", std::process::id()));
        let run = root.join("run");
        std::fs::create_dir_all(run.join("lives")).expect("scratch lives dir");
        std::fs::write(
            run.join("lives").join("m1.json"),
            r#"{"schema":1,"account":"m1","created_at":1,"seed":9,
                "profile":{"ironman":false,"market_affinity":0.5,"price_patience":0.5,"archetype":"merchant"},
                "goal":{"kind":"get_rich","merchant":true},
                "nodes":[{"id":"n1","kind":"make_money","params":{},"deps":[],"status":"active",
                "method":"market:flip","alternatives":[{"method":"market:flip","est_ticks":6000,"est_coins":0}],
                "progress":{"current":0,"target":0},"spent_ticks":100}],
                "engagement":{"node":"n1","min_ticks":6000,"max_ticks":12000,"chosen":9000},
                "active":"n1","history":[]}"#,
        )
        .expect("scratch life file");
        // Life file verbatim: goal, statuses, method, alternatives, engagement.
        let life = life_response_in(&run, "m1");
        assert_eq!(life.get("ok").as_bool(), Some(true));
        assert_eq!(life.get("life").get("account").as_str(), Some("m1"));
        assert_eq!(life.get("life").get("goal").get("kind").as_str(), Some("get_rich"));
        let nodes = life.get("life").get("nodes").as_arr().expect("nodes array");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].get("status").as_str(), Some("active"));
        assert_eq!(nodes[0].get("method").as_str(), Some("market:flip"));
        assert_eq!(nodes[0].get("alternatives").as_arr().map(|rows| rows.len()), Some(1));
        assert_eq!(life.get("life").get("engagement").get("node").as_str(), Some("n1"));
        // Unknown account and traversal read loud, never the filesystem.
        assert_eq!(life_response_in(&run, "nobody").get("ok").as_bool(), Some(false));
        assert_eq!(life_response_in(&run, "..").get("ok").as_bool(), Some(false));

        // Index: goal plus counts by status.
        let lives = lives_response_in(&run);
        let rows = lives.get("lives").as_arr().expect("lives array");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get("account").as_str(), Some("m1"));
        assert_eq!(rows[0].get("nodes").get("active").as_i64(), Some(1));

        // Economy: one stub dispatch for every daemon command used.
        fn stub(request: Value) -> Value {
            match request.get("cmd").as_str().unwrap_or("") {
                "status" => object(vec![("jobs", Value::Arr(Vec::new()))]),
                "items" => object(vec![(
                    "data",
                    object(vec![("liveTotal", Value::Int(42)), ("items", Value::Arr(Vec::new()))]),
                )]),
                "accounts" => object(vec![(
                    "accounts",
                    Value::Arr(vec![str_value("m1")]),
                )]),
                _ => match request.get("op").as_str().unwrap_or("") {
                    "stats" => object(vec![
                        ("ok", Value::Bool(true)),
                        ("items", Value::Int(3)),
                        ("books", Value::Int(1)),
                        ("orders", Value::Int(2)),
                        ("placed", Value::Int(2)),
                        ("matched", Value::Int(0)),
                        ("deals", Value::Int(0)),
                    ]),
                    "my_orders" => object(vec![(
                        "orders",
                        Value::Arr(vec![
                            object(vec![
                                ("side", str_value("buy")),
                                ("item", Value::Int(5)),
                                ("left", Value::Int(10)),
                                ("price", Value::Int(4)),
                                ("status", str_value("open")),
                            ]),
                            object(vec![
                                ("side", str_value("sell")),
                                ("item", Value::Int(5)),
                                ("left", Value::Int(6)),
                                ("price", Value::Int(7)),
                                ("status", str_value("open")),
                            ]),
                        ]),
                    )]),
                    "my_deals" => object(vec![("deals", Value::Arr(Vec::new()))]),
                    "book" => object(vec![
                        ("buys", Value::Arr(vec![Value::Arr(vec![Value::Int(4), Value::Int(20)])])),
                        ("sells", Value::Arr(vec![Value::Arr(vec![Value::Int(7), Value::Int(12)])])),
                    ]),
                    _ => object(vec![("ok", Value::Bool(false))]),
                },
            }
        }
        let dispatch: Dispatch = Arc::new(stub);
        let economy = economy_response_in(&dispatch, &run);
        assert_eq!(economy.get("ok").as_bool(), Some(true), "empty store still serves a shape");
        assert_eq!(economy.get("holdings").get("liveTotal").as_i64(), Some(42));
        assert_eq!(economy.get("deals").get("total").as_i64(), Some(0));
        assert_eq!(economy.get("market").get("items").as_i64(), Some(3));
        assert!(economy.get("activities").as_arr().is_some());
        assert!(economy.get("trends").as_arr().is_some());
        let merchants = economy.get("merchants").as_arr().expect("merchants array");
        assert_eq!(merchants.len(), 1, "the merchant life file drives the card");
        assert_eq!(merchants[0].get("account").as_str(), Some("m1"));
        assert_eq!(merchants[0].get("buy_value").as_i64(), Some(40));
        assert_eq!(merchants[0].get("sell_qty").as_i64(), Some(6));
        assert_eq!(merchants[0].get("sell_value").as_i64(), Some(42));
        assert_eq!(merchants[0].get("open_orders").as_i64(), Some(2));
        // 10 of 20 resting bids, 6 of 12 asks: share 0.5.
        assert!((merchants[0].get("book_share").as_f64().unwrap_or(-1.0) - 0.5).abs() < 1e-9);

        let _ = std::fs::remove_dir_all(&root);
    }
}
