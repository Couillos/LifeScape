//! Web view over the orchestrator: dashboard and HTTP/WS API.
//!
//! Same routes, same JSON shapes: GET / /spectator.js /items /items.js
//! /market /market.js /chart.js /rates /rates.js /table.js /life /life.js /economy /economy.js
//! /errors /errors.js /api/errors/summary|list|dossier/<file> (web_errors)
//! /map/<dir>/<file>.png /map/<dir>/manifest.json /mapscene/<sprite>.png
//! /api/map /api/status /api/state/<account> /api/items /api/items/history
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
use std::sync::Arc;
use std::time::Duration;

use lifescape_json::{self as json, Value, object, str_value};
use crate::paths;

pub const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
pub const WS_INTERVAL: f64 = 1.0;

pub type Dispatch = Arc<dyn Fn(Value) -> Value + Send + Sync>;
pub use crate::web_lives::{economy_response, economy_response_in, life_response, life_response_in, lives_response, lives_response_in};
pub use crate::web_market::snap_range;
pub use crate::web_ws::state_payload;


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

pub(crate) fn parse_query(query: &str) -> HashMap<String, String> {
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

fn not_found() -> Response {
    json_reply(object(vec![("ok", Value::Bool(false)), ("error", str_value("not found"))]), 404)
}

pub(crate) fn handle_get(path: &str, query: &str, dispatch: &Dispatch) -> Response {
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
                .and_then(|sprite| crate::web_ws::shared_maps().cache().mapscene_png(sprite));
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
        if let Some(reply) = crate::web_errors::route(&parts, query) {
            return Ok(reply);
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
            shaped.insert("state".to_string(), crate::web_ws::annotate_mapscene(state));
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
        if parts == ["chart.js"] {
            return Ok(Response {
                status: 200,
                content_type: "application/javascript",
                cache: "no-store",
                body: asset("chart.js").map_err(|error| error.to_string())?,
            });
        }
        if parts == ["table.js"] {
            return Ok(Response {
                status: 200,
                content_type: "application/javascript",
                cache: "no-store",
                body: asset("table.js").map_err(|error| error.to_string())?,
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
            return Ok(ok_json(crate::web_lives::lives_response()));
        }
        if parts.len() >= 2 && parts[..2] == ["api", "life"] {
            let name = if parts.len() > 2 { parts[2] } else { "" };
            return Ok(ok_json(crate::web_lives::life_response(name)));
        }
        if parts == ["api", "economy"] {
            return Ok(ok_json(crate::web_lives::economy_response(dispatch)));
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
            return Ok(crate::web_market::market_api(parts[2], query, dispatch));
        }
        if parts == ["api", "items", "history"] {
            return Ok(crate::web_items::items_history_api(query, &crate::paths::run_dir()));
        }
        if parts == ["api", "items"] {
            return Ok(crate::web_items::items_list_api(dispatch));
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
