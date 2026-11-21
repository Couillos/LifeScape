//! Plans asked while a leg still runs server-side (split out of
//! `travel_here.rs`).
//!
//! Live 2026-11-19 (~6 refusals/h: frank_shaw-50111, tim_ford-58733,
//! alice_knight-33213, lily_brook-22228, brook_daisy-29735,
//! Zezima-29342): the market pause starts its walk inside the pump of a
//! leg in flight. The plan is asked from the tile of that moment, the
//! server keeps walking the old leg during the round trip (0.2-0.5 s),
//! and the plan comes back starting 2-10 tiles from the bot: the first
//! MOVE aims at a far waypoint, the engine's straight step hits the
//! obstacle the plan went around, stall, dossier. frank_shaw: asked at
//! (3207,3384), plan back with the bot at (3209,3380), stuck at
//! (3208,3380).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lifescape_nav::plan::{Segment, Tile};
use lifescape_nav::service::{test_socket, PlanClient, Service, Snapshot};

use crate::client::travel::test_nav_context;
use crate::client::Bot;
use crate::isaac::Isaac;

/// Fake game server: `steps` east steps (one per 20 ms tick) after
/// `idle` idle ticks, then idle ticks until the client hangs up.
fn walking_server(idle: usize, steps: usize) -> (u16, std::thread::JoinHandle<()>, [i32; 4], [i32; 4]) {
    let (client_seed, server_seed) = ([3, 1, 4, 1], [5, 9, 2, 6]);
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback bind");
    let port = listener.local_addr().expect("local addr").port();
    let handle = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("accept");
        sock.set_nonblocking(true).expect("nonblocking");
        let mut cipher = Isaac::new(server_seed);
        let mut sink = [0u8; 256];
        for tick in 0..400 {
            let _ = sock.read(&mut sink);
            // update=1 walk=1 dir=4 (east), or an idle tick.
            let body: [u8; 2] = if tick >= idle && tick < idle + steps { [0xA0 | (4 << 2), 0x00] } else { [0x00, 0x00] };
            let frame = [167u8.wrapping_add(cipher.next_int() as u8), 0, 2, body[0], body[1]];
            if sock.write_all(&frame).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    (port, handle, client_seed, server_seed)
}

/// Proxy in front of the real plan service: every `plan` request waits
/// `delay` (a loaded service) and its `from` tile is recorded.
fn slow_proxy(service: std::path::PathBuf, delay: Duration) -> (std::path::PathBuf, Arc<Mutex<Vec<(i64, i64)>>>) {
    let socket = test_socket("sdk-drift-proxy");
    let listener = UnixListener::bind(&socket).expect("proxy bind");
    let froms = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&froms);
    std::thread::spawn(move || {
        for incoming in listener.incoming() {
            let Ok(client) = incoming else { return };
            let mut line = String::new();
            if BufReader::new(&client).read_line(&mut line).is_err() {
                continue;
            }
            if let Ok(request) = lifescape_json::parse(&line) {
                if request.get("op").as_str() == Some("plan") {
                    let from = request.get("from");
                    seen.lock().expect("froms").push((from.get("x").as_i64().unwrap_or(0), from.get("z").as_i64().unwrap_or(0)));
                    std::thread::sleep(delay);
                }
            }
            let Ok(upstream) = UnixStream::connect(&service) else { continue };
            if (&upstream).write_all(line.as_bytes()).is_err() {
                continue;
            }
            let mut reply = String::new();
            if BufReader::new(&upstream).read_line(&mut reply).is_err() {
                continue;
            }
            let _ = (&client).write_all(reply.as_bytes());
        }
    });
    (socket, froms)
}

/// Real dossier (frank_shaw-50111, 2026-11-19): a step of the previous
/// walk still rolls when the trip is planned. The bot is waited for until
/// it stands still (5 tiles walked), then the plan is asked once, from
/// the tile it stopped on: no replan, a plan starting beside the bot.
#[test]
fn plan_waits_for_the_bot_to_stand_still_then_plans_from_there() {
    let (world, tables) = test_nav_context();
    let version = "drift";
    let service = Service::start(Snapshot::new(world, tables, version.to_string()), &test_socket("sdk-drift"), 1)
        .expect("service starts");
    let (proxy, froms) = slow_proxy(service.socket_path().to_path_buf(), Duration::from_millis(400));
    let (port, game, client_seed, server_seed) = walking_server(0, 5);
    let mut bot = bot_on(port, &proxy, version, client_seed, server_seed);
    let outcome = bot.plan_travel(Tile { x: 3186, z: 3436, level: 0 });
    let here = (bot.world.x, bot.world.z);
    drop(bot);
    let _ = game.join();
    let _ = std::fs::remove_file(&proxy);
    let (_, plan, _) = outcome.expect("plans");
    assert_eq!(here, (3210, 3400), "the bot walked until it stopped");
    let asked = froms.lock().expect("froms").clone();
    assert_eq!(asked, vec![(3210, 3400)], "asked once, from the tile of the stop");
    let first = match plan.segments.first() {
        Some(Segment::Walk { tiles }) => tiles[0],
        other => panic!("a walk first, got {other:?}"),
    };
    assert!((first.x - here.0).abs() <= 1 && (first.z - here.1).abs() <= 1, "plan starts beside the bot: {first:?} vs {here:?}");
}

/// A bot that sets off while the plan is computed (still when asked):
/// waited for again after the drift, then planned again once from the
/// tile of the stop.
#[test]
fn drift_during_the_round_trip_waits_for_stillness_before_the_replan() {
    let (world, tables) = test_nav_context();
    let version = "drift-late";
    let service = Service::start(Snapshot::new(world, tables, version.to_string()), &test_socket("sdk-drift-late"), 1)
        .expect("service starts");
    let (proxy, froms) = slow_proxy(service.socket_path().to_path_buf(), Duration::from_millis(400));
    let (port, game, client_seed, server_seed) = walking_server(4, 5);
    let mut bot = bot_on(port, &proxy, version, client_seed, server_seed);
    let outcome = bot.plan_travel(Tile { x: 3186, z: 3436, level: 0 });
    let here = (bot.world.x, bot.world.z);
    drop(bot);
    let _ = game.join();
    let _ = std::fs::remove_file(&proxy);
    let (_, plan, _) = outcome.expect("plans");
    assert_eq!(here, (3210, 3400));
    let asked = froms.lock().expect("froms").clone();
    assert_eq!(asked, vec![(3205, 3400), (3210, 3400)], "one replan, from the tile of the stop");
    let first = match plan.segments.first() {
        Some(Segment::Walk { tiles }) => tiles[0],
        other => panic!("a walk first, got {other:?}"),
    };
    assert!((first.x - here.0).abs() <= 1 && (first.z - here.1).abs() <= 1, "plan starts beside the bot: {first:?}");
}

fn bot_on(port: u16, proxy: &std::path::Path, version: &str, client_seed: [i32; 4], server_seed: [i32; 4]) -> Bot {
    let client = TcpStream::connect(("127.0.0.1", port)).expect("loopback dial");
    client.set_read_timeout(Some(Duration::from_millis(500))).expect("client read timeout");
    let mut bot = Bot::new("127.0.0.1", port, ".", ".");
    bot.conn.attach_for_test(client, client_seed, server_seed);
    bot.last_keepalive = Some(Instant::now());
    bot.last_send_tick = bot.world.ticks;
    (bot.world.x, bot.world.z, bot.world.level) = (3205, 3400, 0);
    bot.plan_client = Some(PlanClient::new(proxy, version));
    bot.plan_data_version = Some(version.to_string());
    bot.wants_cache = Some((version.to_string(), Vec::new()));
    bot
}

/// A bot that never stops is not planned from: after the tick bound the
/// wait is a typed `Timeout`, and no plan was asked.
#[test]
fn a_bot_that_never_stops_is_a_typed_timeout_without_any_plan() {
    let (world, tables) = test_nav_context();
    let version = "drift-never";
    let service = Service::start(Snapshot::new(world, tables, version.to_string()), &test_socket("sdk-drift-never"), 1)
        .expect("service starts");
    let (proxy, froms) = slow_proxy(service.socket_path().to_path_buf(), Duration::from_millis(10));
    let (port, game, client_seed, server_seed) = walking_server(0, 100);
    let mut bot = bot_on(port, &proxy, version, client_seed, server_seed);
    let outcome = bot.plan_travel(Tile { x: 3186, z: 3436, level: 0 });
    drop(bot);
    let _ = game.join();
    let _ = std::fs::remove_file(&proxy);
    assert!(matches!(outcome, Err(crate::error::Error::Timeout(ref detail)) if detail.contains("never stood still")), "{outcome:?}");
    assert!(froms.lock().expect("froms").is_empty(), "no plan asked from a moving bot");
}
