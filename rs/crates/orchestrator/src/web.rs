//! Web view over the orchestrator, mirroring orchestration/web.py.
//!
//! Same routes, same JSON shapes: GET / /spectator.js /items /items.js
//! /minimap.png /mapscene/<sprite>.png /api/status /api/state/<account>
//! /api/items/history /api/scripts /api/accounts /api/logs/<account> +
//! POST /api/open|run|unload|pause|resume|close + /ws pushing every 0.2 s.
//! Static files are re-read from orchestration/static/ on every request.
//!
//! Ground, walls and mapscene sprites come from the packed game data via
//! mapcache/minimap (same passes as the client), so /minimap.png renders
//! the real window and /mapscene/<sprite>.png serves the real sprite.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::json::{self, Value, object, str_value};
use crate::minimap::Minimap;
use crate::paths;

pub const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
pub const WS_INTERVAL: f64 = 0.2;

pub type Dispatch = Arc<dyn Fn(Value) -> Value + Send + Sync>;

const LOC_FIELDS: &[&str] = &[
    "x", "z", "id", "shape", "angle", "act", "active", "block", "width", "length", "mapscene",
    "mapsceneX", "mapsceneY", "cosmetic",
];

const JOB_LIST_FIELDS: &[&str] = &[
    "account",
    "account_name",
    "combat",
    "position",
    "modal",
    "intention",
    "status",
    "paused",
    "phase",
    "script",
    "hitpoints",
    "zone",
    "ticks",
    "last_error",
    "last_event",
    "updated",
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

fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in text.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Cheap signature of the fields that only change when the bot moves.
fn heavy_signature(state: &Value) -> Value {
    let locs = state.get("locs").as_arr().map(|items| items.to_vec()).unwrap_or_default();
    let mut hash: u64 = 0xcbf29ce484222325;
    for loc in &locs {
        let row = format!(
            "{:?}{:?}{:?}{:?}{:?}",
            loc.get("x"),
            loc.get("z"),
            loc.get("id"),
            loc.get("shape"),
            loc.get("angle")
        );
        hash ^= fnv1a(&row);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Value::Arr(vec![
        state.get("position").clone(),
        state.get("zone").clone(),
        Value::Int(locs.len() as i64),
        Value::Int(hash as i64),
    ])
}

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

pub fn state_payload(dispatch: &Dispatch, account: &str, previous: Option<&Value>) -> Value {
    let jobs = dispatch(object(vec![("cmd", str_value("status"))]));
    let mut listed = Vec::new();
    for job in jobs.get("jobs").as_arr().map(|items| items.to_vec()).unwrap_or_default() {
        let mut slim = std::collections::BTreeMap::new();
        for field in JOB_LIST_FIELDS {
            slim.insert(field.to_string(), job.get(field).clone());
        }
        listed.push(Value::Obj(slim));
    }
    let mut payload =
        object(vec![("jobs", Value::Arr(listed)), ("signature", Value::Null)]);
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
        state = annotate_mapscene(state);
        let signature = heavy_signature(&state);
        if previous == Some(&signature) {
            if let Value::Obj(map) = &mut state {
                map.remove("locs");
                map.insert("partial".to_string(), Value::Bool(true));
            }
        } else {
            compact_locs(&mut state);
        }
        if let Value::Obj(map) = &mut payload {
            map.insert("state".to_string(), state);
            map.insert("signature".to_string(), signature);
        }
    }
    payload
}

// -- websocket framing (same wire format as web.py) ------------------------

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

/// Push jobs + the followed account's state until the client leaves.
fn ws_session(mut sock: TcpStream, dispatch: &Dispatch) {
    let _ = sock.set_read_timeout(Some(Duration::from_millis(200)));
    let mut account = String::new();
    let mut last_send = 0.0f64;
    let mut log_mark: Option<(u128, u64)> = None;
    let mut signature: Option<Value> = None;
    loop {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|span| span.as_secs_f64())
            .unwrap_or(0.0);
        if now - last_send >= WS_INTERVAL {
            let mut payload = state_payload(dispatch, &account, signature.as_ref());
            if let Value::Obj(map) = &mut payload {
                signature = map.remove("signature");
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
                        if let Value::Obj(map) = &mut payload {
                            map.insert("logs".to_string(), lines);
                        }
                        log_mark = mark;
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
                        account = request.get("account").as_str().unwrap_or("").to_string();
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
        if parts == ["minimap.png"] {
            let params = parse_query(query);
            let level = params.get("level").and_then(|raw| raw.parse::<i32>().ok()).unwrap_or(0).clamp(0, 3);
            let x = params.get("x").and_then(|raw| raw.parse::<i32>().ok()).unwrap_or(0);
            let z = params.get("z").and_then(|raw| raw.parse::<i32>().ok()).unwrap_or(0);
            let size = params.get("size").and_then(|raw| raw.parse::<usize>().ok()).unwrap_or(33).clamp(1, 65);
            return Ok(Response {
                status: 200,
                content_type: "image/png",
                cache: "no-store",
                body: shared_maps().png(level, x, z, size),
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
/// exits with every session untouched, like web.py).
pub fn start_web(dispatch: Dispatch, port: u16) {
    let server = match TcpListener::bind(("0.0.0.0", port)) {
        Ok(server) => server,
        Err(error) => {
            eprintln!("webapp bind failed on {port}: {error}");
            std::process::exit(1);
        }
    };
    println!("webapp listening on http://0.0.0.0:{port}/");
    std::thread::spawn(move || {
        for stream in server.incoming() {
            match stream {
                Ok(stream) => {
                    let dispatch = dispatch.clone();
                    std::thread::spawn(move || serve_conn(stream, &dispatch));
                }
                Err(_) => break,
            }
        }
    });
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
