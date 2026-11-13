//! Nav-service round trips without starving the game session.
//!
//! The plan service answers over a Unix socket, synchronously. While the
//! bot thread waits on it, nothing reaches the game server, and the engine
//! force-logs-out a client silent for 100 ticks (`World.TIMEOUT_NO_RESPONSE`,
//! 6 s at the dev 60 ms tick). Live 2026-11-13 14:10:26 (bmzvbp5u): the
//! booth stands of `open_bank` waited ~5 s on a loaded service, the session
//! was reaped ("server closed the socket", deal 1065 lost, 1068 late).
//!
//! `serve_fed` runs the round trip on a helper thread while the bot
//! thread drains the frames the server already sent (`catch_up`, no
//! hooks) and sends the keepalive when due. A round trip that times out
//! (EAGAIN, live 14:14:25 lara_lane deal 1071) is a stall, not a dead
//! service: callers retry it inside the service guard, then fail typed.

#![forbid(unsafe_code)]

use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use crate::client::Bot;
use crate::error::{Error, Result};

/// Feed rhythm while a service call is pending: IPC pacing, not a game
/// wait. The keepalive itself stays tick/wall-driven (`keepalive_if_due`).
const FEED_SLICE: Duration = Duration::from_millis(100);

/// True when a service transport error means "no answer in time" (read
/// or write timeout on the plan socket: EAGAIN/EWOULDBLOCK, ETIMEDOUT),
/// as opposed to a refused connect (service down) or a bad frame.
pub(crate) fn service_stalled(detail: &str) -> bool {
    let lower = detail.to_lowercase();
    lower.contains("os error 11") || lower.contains("resource temporarily unavailable") || lower.contains("would block") || lower.contains("timed out")
}

impl Bot {
    /// One blocking service call with the game session kept alive: the
    /// call runs on a helper thread, the bot thread drains frames and
    /// sends the keepalive every slice. Without a game socket (offline
    /// tools, unit tests) the call runs inline. `Err` only for the game
    /// session (Io), never for the service: its outcome is the value.
    pub(crate) fn serve_fed<T: Send + 'static>(&mut self, call: impl FnOnce() -> T + Send + 'static) -> Result<T> {
        if !self.conn.is_open() {
            return Ok(call());
        }
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("nav-call".to_string())
            .spawn(move || {
                let _ = sender.send(call());
            })
            .map_err(|error| Error::Travel(format!("nav-down: cannot start the service call: {error}")))?;
        loop {
            match receiver.recv_timeout(FEED_SLICE) {
                Ok(value) => return Ok(value),
                Err(RecvTimeoutError::Timeout) => {
                    self.catch_up()?;
                    self.keepalive_if_due()?;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Error::Travel("nav-down: service call thread died".to_string()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use lifescape_nav::service::{test_socket, PlanClient};

    use super::service_stalled;
    use crate::client::Bot;
    use crate::isaac::Isaac;

    /// Timeouts are stalls; a refused connect or a bad frame is not.
    #[test]
    fn only_timeouts_are_stalls() {
        assert!(service_stalled("Resource temporarily unavailable (os error 11)"));
        assert!(service_stalled("connection timed out (os error 110)"));
        assert!(!service_stalled("pathfinding unreachable on /run/pathfinding.sock"));
        assert!(!service_stalled("expected value at line 1 column 1"));
    }

    /// Fake game server: one idle PLAYER_INFO tick every 20 ms, records
    /// when client bytes arrive (offset from `start`) until the client
    /// closes or `stop` is raised.
    fn game_server(stop: Arc<AtomicBool>) -> (u16, std::thread::JoinHandle<Vec<Duration>>, [i32; 4], [i32; 4]) {
        let (client_seed, server_seed) = ([7, 7, 7, 7], [8, 8, 8, 8]);
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback bind");
        let port = listener.local_addr().expect("local addr").port();
        let handle = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().expect("accept");
            sock.set_read_timeout(Some(Duration::from_millis(2))).expect("server read timeout");
            let mut cipher = Isaac::new(server_seed);
            let start = Instant::now();
            let mut arrivals = Vec::new();
            while !stop.load(Ordering::Relaxed) && start.elapsed() < Duration::from_secs(20) {
                let mut chunk = [0u8; 64];
                match sock.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(_) => arrivals.push(start.elapsed()),
                    Err(_) => {}
                }
                std::thread::sleep(Duration::from_millis(20));
                let tick = [167u8.wrapping_add(cipher.next_int() as u8), 0, 2, 0x00, 0x00];
                if sock.write_all(&tick).is_err() {
                    break;
                }
            }
            arrivals
        });
        (port, handle, client_seed, server_seed)
    }

    fn served_bot(port: u16, client_seed: [i32; 4], server_seed: [i32; 4], socket: &std::path::Path) -> Bot {
        let client = TcpStream::connect(("127.0.0.1", port)).expect("loopback dial");
        client.set_read_timeout(Some(Duration::from_millis(500))).expect("client read timeout");
        let mut bot = Bot::new("127.0.0.1", port, ".", ".");
        bot.conn.attach_for_test(client, client_seed, server_seed);
        bot.last_keepalive = Some(Instant::now());
        bot.last_send_tick = bot.world.ticks;
        bot.plan_client = Some(PlanClient::new(socket, "fed"));
        bot.plan_data_version = Some("v1".to_string());
        bot
    }

    const REPLY: &str = r#"{"ok":true,"version":"v1","tiles":[{"tile":{"x":3200,"z":3200,"level":0},"flags":0}]}"#;

    /// A service answering after 4 s: the game server still hears from
    /// the bot inside the wait (a keepalive is due after 3 s), so the
    /// engine's 100-tick silence reap never fires.
    #[test]
    fn slow_service_reply_keeps_the_game_session_fed() {
        let socket = test_socket("sdk-fed");
        let service = UnixListener::bind(&socket).expect("service bind");
        let answering = std::thread::spawn(move || {
            let (stream, _) = service.accept().expect("service accept");
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).expect("request line");
            std::thread::sleep(Duration::from_millis(4000));
            (&stream).write_all(format!("{REPLY}\n").as_bytes()).expect("reply");
        });
        let stop = Arc::new(AtomicBool::new(false));
        let (port, game, client_seed, server_seed) = game_server(stop.clone());
        let mut bot = served_bot(port, client_seed, server_seed, &socket);
        let called = Instant::now();
        let reads = bot.tiles_flags(&[(3200, 3200, 0)]).expect("slow answer still served");
        let waited = called.elapsed();
        stop.store(true, Ordering::Relaxed);
        drop(bot);
        let arrivals = game.join().expect("game thread");
        answering.join().expect("service thread");
        let _ = std::fs::remove_file(&socket);
        assert_eq!(reads.len(), 1);
        assert!(waited >= Duration::from_millis(3900), "the service really stalled: {waited:?}");
        assert!(
            arrivals.iter().any(|at| *at > Duration::from_millis(500) && *at < waited),
            "the game server heard nothing during a {waited:?} service wait: {arrivals:?}"
        );
    }

    /// Live 2026-11-13 14:14:25 (lara_lane, deal 1071): one plan socket
    /// read timed out (EAGAIN, os error 11) and the delivery aborted
    /// `nav-down`. A stalled round trip is retried inside the service
    /// guard; the second connection answers.
    #[test]
    fn stalled_service_reply_is_retried() {
        let socket = test_socket("sdk-stall");
        let service = UnixListener::bind(&socket).expect("service bind");
        let answering = std::thread::spawn(move || -> usize {
            let mut connections = 0usize;
            // First connection: read the request, never answer.
            let (silent, _) = service.accept().expect("first accept");
            connections += 1;
            let mut line = String::new();
            BufReader::new(&silent).read_line(&mut line).expect("first request");
            // Second connection: answer at once.
            let (stream, _) = service.accept().expect("second accept");
            connections += 1;
            let mut again = String::new();
            BufReader::new(&stream).read_line(&mut again).expect("second request");
            (&stream).write_all(format!("{REPLY}\n").as_bytes()).expect("reply");
            drop(silent);
            connections
        });
        let stop = Arc::new(AtomicBool::new(false));
        let (port, game, client_seed, server_seed) = game_server(stop.clone());
        let mut bot = served_bot(port, client_seed, server_seed, &socket);
        let outcome = bot.tiles_flags(&[(3200, 3200, 0)]);
        stop.store(true, Ordering::Relaxed);
        drop(bot);
        let _ = game.join();
        let _ = std::fs::remove_file(&socket);
        let reads = outcome.expect("the stalled read is retried, not nav-down");
        assert_eq!(reads.len(), 1);
        assert_eq!(answering.join().expect("service thread"), 2, "one retry after the stall");
    }

    /// Same stall on the plan op (5.5 s socket bound): the plan retries
    /// inside the 30 s guard instead of failing `nav-down` on EAGAIN. The
    /// second answer (a refusal) proves the retry reached the service.
    #[test]
    fn stalled_plan_round_trip_is_retried() {
        let socket = test_socket("sdk-plan-stall");
        let service = UnixListener::bind(&socket).expect("service bind");
        let answering = std::thread::spawn(move || -> usize {
            let (silent, _) = service.accept().expect("first accept");
            let mut line = String::new();
            BufReader::new(&silent).read_line(&mut line).expect("first request");
            let (stream, _) = service.accept().expect("second accept");
            let mut again = String::new();
            BufReader::new(&stream).read_line(&mut again).expect("second request");
            (&stream).write_all(b"{\"ok\":false,\"error\":\"refused:probe\",\"version\":\"v1\"}\n").expect("reply");
            drop(silent);
            2
        });
        let stop = Arc::new(AtomicBool::new(false));
        let (port, game, client_seed, server_seed) = game_server(stop.clone());
        let mut bot = served_bot(port, client_seed, server_seed, &socket);
        bot.wants_cache = Some(("v1".to_string(), Vec::new()));
        bot.world.x = 3200;
        bot.world.z = 3200;
        let outcome = bot.plan_travel(lifescape_nav::plan::Tile { x: 3210, z: 3200, level: 0 });
        stop.store(true, Ordering::Relaxed);
        drop(bot);
        let arrivals = game.join().expect("game thread");
        let _ = std::fs::remove_file(&socket);
        let error = outcome.expect_err("the probe refuses");
        assert!(format!("{error}").contains("refused:probe"), "the retry reached the service, got: {error}");
        assert_eq!(answering.join().expect("service thread"), 2);
        assert!(arrivals.iter().any(|at| *at > Duration::from_secs(1)), "session fed during the stall: {arrivals:?}");
    }
}
