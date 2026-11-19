//! Walk legs the server cancels (`UNSET_MAP_FLAG`), split out of
//! `movement.rs` (1000-line rule).
//!
//! Live 2026-11-19 (deploy 17:41 UTC, ~45 of the 54 non-pause step
//! refusals of the first hour): the server dropped or stopped the walk
//! and said so with `UNSET_MAP_FLAG`, the SDK ignored it, waited 13
//! ticks and wrote a refusal dossier.
//! - MOVE while the player is delayed (`MoveClickHandler`: `delayed` ->
//!   `UnsetMapFlag`, the click is dropped): the smithing session
//!   (`smithing_anvil`, delayed the whole session) under a market pause,
//!   the last `p_delay` of a casting/mining/cooking loop (nina_87-58003,
//!   wolfe_jack-18993, harry_page-8194);
//! - `p_stopaction` from content (`wilderness_warning.rs2`, first
//!   crossing of z 3512..3515 per session): mid-leg stop, 26 dossiers at
//!   (3219,3513)/(3217,3514)/(3054,3513)/(2979,3513)...
//!
//! The replan after the stall always walked the same tiles fine.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use crate::client::Bot;
use crate::error::Error;
use crate::isaac::Isaac;

const PLAYER_INFO: u8 = 167;
const UNSET_MAP_FLAG: u8 = 115;
const MOVE_GAMECLICK: u8 = 207;

/// One scripted server event after a MOVE lands.
#[derive(Clone, Copy)]
enum Event {
    /// Idle tick (own player not updated).
    Idle,
    /// Tick where the bot steps one tile (engine direction: 4 east).
    Walk(u8),
    /// `UNSET_MAP_FLAG` (no tick).
    Unset,
    /// Wait (bounded) for the client's next MOVE; without one the rest
    /// of the script is idle ticks (the server never walks a dropped
    /// click).
    AwaitMove,
}

/// MOVE_GAMECLICK seen by the fake server: start tile and step offsets.
#[derive(Debug, Clone, PartialEq)]
struct SeenMove {
    start: (i32, i32),
    steps: Vec<(i32, i32)>,
}

/// Reads one client packet (opcode ISAAC-shifted, var-byte size) and
/// decodes it as a MOVE; anything else is `None`.
fn read_move(sock: &mut TcpStream, cipher: &mut Isaac) -> Option<SeenMove> {
    let mut head = [0u8; 2];
    sock.read_exact(&mut head).ok()?;
    let opcode = head[0].wrapping_sub(cipher.next_int() as u8);
    let mut body = vec![0u8; usize::from(head[1])];
    sock.read_exact(&mut body).ok()?;
    if opcode != MOVE_GAMECLICK || body.len() < 5 {
        return None;
    }
    let start = (i32::from(u16::from_be_bytes([body[1], body[2]])), i32::from(u16::from_be_bytes([body[3], body[4]])));
    let steps = body[5..].chunks(2).map(|pair| (i32::from(pair[0] as i8), i32::from(pair[1] as i8))).collect();
    Some(SeenMove { start, steps })
}

/// Loopback game server for one leg from (3200,3200,0): answers the
/// first MOVE with `script`. Returns the outcome, the end tile and every
/// MOVE the server read.
fn scripted_leg(leg: &[(i32, i32)], script: Vec<Event>) -> (crate::error::Result<()>, (i32, i32), Vec<SeenMove>) {
    let (client_seed, server_seed) = ([11, 22, 33, 44], [55, 66, 77, 88]);
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback bind");
    let port = listener.local_addr().expect("local addr").port();
    let server = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("accept");
        sock.set_read_timeout(Some(Duration::from_secs(5))).expect("server read timeout");
        let mut cipher = Isaac::new(server_seed);
        let mut client_cipher = Isaac::new(client_seed);
        let mut moves = Vec::new();
        let idle = [0x00u8, 0x00];
        let walk = |dir: u8| [0xA0u8 | (dir << 2), 0x00];
        let tick = |sock: &mut TcpStream, cipher: &mut Isaac, body: &[u8]| -> bool {
            let mut bytes = vec![PLAYER_INFO.wrapping_add(cipher.next_int() as u8), 0, body.len() as u8];
            bytes.extend_from_slice(body);
            std::thread::sleep(Duration::from_millis(2));
            sock.write_all(&bytes).is_ok()
        };
        match read_move(&mut sock, &mut client_cipher) {
            Some(seen) => moves.push(seen),
            None => return moves,
        }
        let mut dropped = false;
        for event in script {
            let alive = match event {
                Event::Idle => tick(&mut sock, &mut cipher, &idle),
                Event::Walk(dir) if !dropped => tick(&mut sock, &mut cipher, &walk(dir)),
                Event::Walk(_) => tick(&mut sock, &mut cipher, &idle),
                Event::Unset => sock.write_all(&[UNSET_MAP_FLAG.wrapping_add(cipher.next_int() as u8)]).is_ok(),
                Event::AwaitMove => {
                    // Keep ticking idle while waiting: the client's
                    // pump needs ticks to notice the cancel.
                    sock.set_read_timeout(Some(Duration::from_millis(40))).expect("short read timeout");
                    let deadline = Instant::now() + Duration::from_secs(3);
                    let mut seen = None;
                    while seen.is_none() && Instant::now() < deadline {
                        let mut peek = [0u8; 1];
                        match sock.peek(&mut peek) {
                            Ok(1) => {
                                sock.set_read_timeout(Some(Duration::from_secs(5))).expect("server read timeout");
                                seen = read_move(&mut sock, &mut client_cipher);
                                sock.set_read_timeout(Some(Duration::from_millis(40))).expect("short read timeout");
                            }
                            _ => {
                                if !tick(&mut sock, &mut cipher, &idle) {
                                    break;
                                }
                            }
                        }
                    }
                    sock.set_read_timeout(Some(Duration::from_secs(5))).expect("server read timeout");
                    match seen {
                        Some(found) => moves.push(found),
                        None => dropped = true,
                    }
                    true
                }
            };
            if !alive {
                return moves;
            }
        }
        // Idle until the client is done (stall watch or arrival).
        for _ in 0..40 {
            if !tick(&mut sock, &mut cipher, &idle) {
                break;
            }
        }
        moves
    });
    let client = TcpStream::connect(("127.0.0.1", port)).expect("loopback dial");
    client.set_read_timeout(Some(Duration::from_millis(500))).expect("client read timeout");
    let mut bot = Bot::new("127.0.0.1", port, ".", ".");
    bot.conn.attach_for_test(client, client_seed, server_seed);
    bot.last_keepalive = Some(Instant::now());
    bot.last_send_tick = bot.world.ticks;
    bot.world.x = 3200;
    bot.world.z = 3200;
    bot.world.level = 0;
    let deadline = bot.world.ticks + 100;
    let outcome = bot.send_and_await(leg, false, deadline, false);
    let end = (bot.world.x, bot.world.z);
    drop(bot);
    let moves = server.join().expect("server thread");
    (outcome, end, moves)
}

/// Delayed player (smithing session, last `p_delay` of a loop): the
/// server drops the MOVE with `UNSET_MAP_FLAG` and never walks it. The
/// leg is sent again from where the bot stands, never read as a refusal.
#[test]
fn dropped_move_is_sent_again() {
    let leg = [(3201, 3200), (3202, 3200)];
    let script = vec![Event::Unset, Event::Idle, Event::Idle, Event::AwaitMove, Event::Walk(4), Event::Walk(4)];
    let (outcome, end, moves) = scripted_leg(&leg, script);
    assert!(outcome.is_ok(), "dropped MOVE resent, leg arrives, got {outcome:?}");
    assert_eq!(end, (3202, 3200), "leg end reached");
    assert_eq!(moves.len(), 2, "one resend, got {moves:?}");
    assert_eq!(moves[1], SeenMove { start: (3200, 3200), steps: vec![(1, 0), (2, 0)] }, "whole leg again from the start");
}

/// Content `p_stopaction` mid-leg (Wilderness warning): the walk stops
/// after one step with `UNSET_MAP_FLAG`; the rest of the leg is sent
/// again from the tile reached.
#[test]
fn stopped_walk_resends_the_rest_of_the_leg() {
    let leg = [(3201, 3200), (3202, 3200), (3203, 3200)];
    let script = vec![Event::Walk(4), Event::Unset, Event::Idle, Event::Idle, Event::AwaitMove, Event::Walk(4), Event::Walk(4)];
    let (outcome, end, moves) = scripted_leg(&leg, script);
    assert!(outcome.is_ok(), "stopped walk resumed, got {outcome:?}");
    assert_eq!(end, (3203, 3200), "leg end reached");
    assert_eq!(moves.len(), 2, "one resend, got {moves:?}");
    assert_eq!(moves[1], SeenMove { start: (3201, 3200), steps: vec![(1, 0), (2, 0)] }, "rest of the leg from the tile reached");
}

/// A step the server refuses without any cancel (a wall, a blocking
/// NPC: the engine keeps the waypoint) is still a stall: no flag, no
/// resend, the typed stall stands.
#[test]
fn refusal_without_cancel_is_never_resent() {
    let leg = [(3201, 3200), (3202, 3200)];
    let (outcome, end, moves) = scripted_leg(&leg, vec![Event::Idle, Event::Idle, Event::AwaitMove]);
    assert!(matches!(&outcome, Err(Error::Timeout(text)) if text.contains("stalled")), "real stall, got {outcome:?}");
    assert_eq!(end, (3200, 3200), "never moved");
    assert_eq!(moves.len(), 1, "no resend without a cancel, got {moves:?}");
}

/// The flag that closes a walk the bot keeps making (a late frame) is
/// no cancel: progress continues, nothing is resent.
#[test]
fn flag_while_walking_is_no_cancel() {
    let leg = [(3201, 3200), (3202, 3200), (3203, 3200)];
    let script = vec![Event::Walk(4), Event::Unset, Event::Walk(4), Event::Walk(4)];
    let (outcome, end, moves) = scripted_leg(&leg, script);
    assert!(outcome.is_ok(), "leg arrives, got {outcome:?}");
    assert_eq!(end, (3203, 3200), "leg end reached");
    assert_eq!(moves.len(), 1, "no resend while moving, got {moves:?}");
}
