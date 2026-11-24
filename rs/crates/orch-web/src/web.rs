//! Web view over the orchestrator: dashboard and HTTP/WS API.
//!
//! Same routes, same JSON shapes: GET / /spectator.js /items /items.js
//! /market /market.js /chart.js /rates /rates.js /table.js /life /life.js /brain.js /economy /economy.js
//! /guides-money /guides-training /guides.js /api/guides/money|training?skill&level&family&quests (web_guides)
//! /errors /errors.js /api/errors/summary|list|dossier/<file> (web_errors)
//! /hiscores /hiscores.js /api/hiscores (cached snapshot, hiscores_service)
//! /xp /xp.js /api/xp (cached snapshot, xp_service)
//! /map/<dir>/<file>.png /map/<dir>/manifest.json /mapscene/<sprite>.png
//! /api/map /api/status /api/state/<account> /api/items /api/items/history|flows
//! /api/rates/table /api/rates/activities /api/rates/detail /api/rates/trading /api/lives /api/life/<account> /api/brain/<account> /api/economy
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
use std::time::Duration;

use lifescape_json::{self as json, Value, object, str_value};

pub const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
pub const WS_INTERVAL: f64 = 1.0;

pub use lifescape_orch_fleet::autopilot::Dispatch;
pub use crate::web_lives::{economy_response, economy_response_in, life_response, life_response_in, lives_response, lives_response_in};
pub use crate::web_market::snap_range;
pub use crate::web_ws::state_payload;


// -- HTTP ------------------------------------------------------------------

pub fn parse_query(query: &str) -> HashMap<String, String> {
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

pub(crate) struct Response {
    pub(crate) status: u16,
    pub(crate) content_type: &'static str,
    pub(crate) cache: &'static str,
    pub(crate) body: Vec<u8>,
}

fn json_reply(data: Value, status: u16) -> Response {
    Response {
        status,
        content_type: "application/json",
        cache: "no-store",
        body: json::stringify(&data).into_bytes(),
    }
}

pub(crate) fn ok_json(data: Value) -> Response {
    json_reply(data, 200)
}

pub(crate) fn err_json(message: String) -> Value {
    object(vec![("ok", Value::Bool(false)), ("error", str_value(&message))])
}

pub(crate) fn not_found() -> Response {
    json_reply(object(vec![("ok", Value::Bool(false)), ("error", str_value("not found"))]), 404)
}

pub(crate) fn handle_get(path: &str, query: &str, dispatch: &Dispatch) -> Response {
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    match crate::web_routes::route(&parts, query, dispatch) {
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
        let accept = crate::web_ws::base64(&crate::web_ws::sha1(format!("{key}{WS_GUID}").as_bytes()));
        let handshake = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
        );
        if sock.write_all(handshake.as_bytes()).is_err() {
            return;
        }
        crate::web_ws::ws_session(sock, dispatch);
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
    crate::logln!("webapp listening on http://0.0.0.0:{port}/");
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
