#![forbid(unsafe_code)]
//! Off-acceptor serving (CPU 2026-11-14): the `plan-accept` thread only
//! accepts and hands each connection to the bounded `plan-query` pool,
//! which reads the request line and answers it (direct reads,
//! `walk_costs`, submit/poll, the `plan` fast-lane handoff).
//!
//! Before: every non-`plan` op (and its line read, 5 s timeout) ran
//! inline on the single acceptor — a `walk_costs` search or a slow
//! client stalled every other worker's request behind it. Now the
//! acceptor never reads a byte. A full queue blocks the acceptor (back
//! pressure, the kernel backlog holds new connections), never drops one.

use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};

use crate::service::Service;

/// Query threads: 2 lanes (CPU budget first: parallelism is for
/// latency, never more total work).
pub const QUERY_THREADS: usize = 2;
/// Accepted connections waiting for a lane before the acceptor blocks.
pub const QUERY_QUEUE: usize = 128;

/// Start the bounded query pool; returns the hand-off sender.
pub(crate) fn start_query_pool(service: Arc<Service>, threads: usize) -> SyncSender<UnixStream> {
    let (sender, receiver) = mpsc::sync_channel::<UnixStream>(QUERY_QUEUE);
    let shared: Arc<Mutex<Receiver<UnixStream>>> = Arc::new(Mutex::new(receiver));
    for index in 0..threads.max(1) {
        let worker = service.clone();
        let queue = shared.clone();
        std::thread::Builder::new()
            .name(format!("plan-query-{index}"))
            .spawn(move || loop {
                let stream = {
                    let guard = queue.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    guard.recv()
                };
                match stream {
                    Ok(stream) => worker.serve_pooled(stream),
                    Err(_) => return,
                }
            })
            .expect("plan query pool spawns");
    }
    sender
}

/// Acceptor loop: accept, hand off, nothing else.
pub(crate) fn accept_loop(listener: UnixListener, lanes: SyncSender<UnixStream>) {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if lanes.send(stream).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    use crate::service::{test_socket, Service, Snapshot};

    fn bare_service(prefix: &str) -> std::sync::Arc<Service> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
        let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
        let gametypes = std::fs::read_to_string(root.join("LifeScape/gamedata/data/gametypes.json")).expect("gametypes");
        let defs = crate::loctypes::parse_loctypes(&gametypes, "{}").expect("loctypes");
        let world = crate::static_world::StaticWorld::open(&zip, defs).expect("world");
        let tables = crate::tables::Tables::load(&root.join("LifeScape/gamedata/data")).expect("tables");
        Service::start(Snapshot::new(world, std::sync::Arc::new(tables), "acc".to_string()), &test_socket(prefix), 1)
            .expect("service starts")
    }

    fn stats(service: &Service) -> lifescape_json::Value {
        let stream = UnixStream::connect(service.socket_path()).expect("connect");
        stream.set_read_timeout(Some(Duration::from_secs(10))).expect("timeout");
        (&stream).write_all(b"{\"op\":\"stats\"}\n").expect("write");
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).expect("read");
        lifescape_json::parse(&line).expect("json")
    }

    /// A client that connects and never sends its line used to hold the
    /// acceptor for the whole 5 s read timeout, stalling every worker.
    /// Now it only holds one query lane: the next request answers at once.
    #[test]
    fn silent_client_never_stalls_the_next_request() {
        let service = bare_service("acc-silent");
        let silent = UnixStream::connect(service.socket_path()).expect("silent connects");
        std::thread::sleep(Duration::from_millis(50));
        let started = Instant::now();
        let reply = stats(&service);
        let waited = started.elapsed();
        assert_eq!(reply.get("ok").as_bool(), Some(true));
        assert!(waited < Duration::from_secs(2), "stats waited {waited:?} behind a silent client");
        drop(silent);
    }

    /// Every connection is served by the pool, counted in `pooled`.
    #[test]
    fn every_request_is_served_by_the_pool() {
        let service = bare_service("acc-pooled");
        for _ in 0..3 {
            stats(&service);
        }
        let reply = stats(&service);
        assert_eq!(reply.get("pooled").as_i64(), Some(4), "four connections, four pooled serves");
    }
}
