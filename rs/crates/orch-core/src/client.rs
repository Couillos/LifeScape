#![forbid(unsafe_code)]
//! Client half of the daemon socket: one JSON line out, one reply in.
//! Hoisted out of `cli` and `daemon` so the lower layers (brain notes,
//! daemon-side commands) can talk to the daemon without depending on the
//! CLI or the daemon module (layering step A1, 2026-11-23).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use lifescape_json::{self as json, Value, object};

use crate::paths;

/// Seconds a worker RPC may take before the daemon gives up on it.
pub const WORKER_TIMEOUT: u64 = 40;

pub fn send_command(request: &Value, timeout: Duration) -> Result<Value, String> {
    let stream = UnixStream::connect(paths::socket_path())
        .map_err(|error| format!("session unreachable: {error}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| error.to_string())?;
    let mut buf = json::stringify(request);
    buf.push('\n');
    (&stream)
        .write_all(buf.as_bytes())
        .map_err(|error| format!("session unreachable: {error}"))?;
    let mut line = String::new();
    BufReader::new(&stream)
        .read_line(&mut line)
        .map_err(|error| format!("session unreachable: {error}"))?;
    if line.trim().is_empty() {
        return Ok(object(vec![]));
    }
    json::parse(&line).map_err(|error| format!("bad daemon reply: {error}"))
}
