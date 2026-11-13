//! Unit tests of `client::actions` (split out to keep the module under
//! the 1000-line rule).

use super::*;

/// Refusal is scoped to fresh messages: earlier traffic never fails
/// a new click, unrelated chatter never matches.
#[test]
fn bank_click_refused_only_matches_fresh_refusal() {
    let messages = vec!["Welcome.".to_string(), "I can't reach that!".to_string()];
    assert!(!Bot::bank_click_refused(&messages, 2), "nothing fresh");
    assert!(Bot::bank_click_refused(&messages, 1), "fresh refusal fails fast");
    let other = vec!["A bank booth.".to_string()];
    assert!(!Bot::bank_click_refused(&other, 0), "unrelated text passes");
}

/// The stands come from the plan service: without it open_bank fails
/// loud `nav-down` before any click, never Ok(false) and never a blind
/// click from wherever the bot stands.
#[test]
fn open_bank_without_nav_service_fails_loud() {
    let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
    bot.world.level = 0;
    bot.world.x = 3091;
    bot.world.z = 3246;
    let err = bot.open_bank(3091, 3245, 2213, 50).expect_err("no service fails loud");
    assert!(format!("{err}").contains("nav-down"), "typed nav-down, got: {err}");
}

/// Stand walk budget: 60-tick floor plus 10 per tile, so a handover
/// from across the map is not timed out by the old 60-tick cap.
#[test]
fn bank_stand_walk_budget_scales_with_distance() {
    assert_eq!(Bot::bank_stand_walk_ticks((3251, 3420), (3252, 3420)), 70);
    assert_eq!(Bot::bank_stand_walk_ticks((3199, 3333), (3185, 3436)), 1230);
}

/// Socket dead mid-walk degrades into Ok(false) under the old
/// `.is_err()` count: swallowed Io errors read as refusals and
/// `bank_trip` strands instead of reconnecting. Only true refusals
/// count, so transport/operator stops propagate. The stand loop is
/// unreachable offline (the service read fails before it), so this
/// locks the classification every stand walk error goes through.
#[test]
fn bank_stand_walk_propagates_dead_socket_instead_of_refusing() {
    for err in [
        Error::Io("not logged in".to_string()),
        Error::Login("stale session".to_string()),
        Error::Interrupted("run stopped".to_string()),
        Error::State("bank_main:inv missing".to_string()),
        Error::Protocol("bad frame".to_string()),
    ] {
        assert!(!Bot::bank_stand_walk_refused(&err), "propagates, never a refusal: {err}");
    }
}

/// Unreachable (wall, teller side) and Timeout (stalled leg) are the
/// only stand-walk refusals.
#[test]
fn bank_stand_walk_counts_only_true_refusals() {
    assert!(Bot::bank_stand_walk_refused(&Error::Unreachable("counter".to_string())), "wall counts");
    assert!(Bot::bank_stand_walk_refused(&Error::Timeout("stalled leg".to_string())), "stall counts");
    assert!(Bot::bank_stand_walk_refused(&Error::StepRefused("shut".to_string())), "raw refused step counts");
    for kind in [TravelErrorKind::NoRoute, TravelErrorKind::StepRefused, TravelErrorKind::Timeout] {
        assert!(
            Bot::bank_stand_walk_refused(&travel_failure(kind)),
            "walk_to dossier {:?} counts",
            kind
        );
    }
    for kind in
        [TravelErrorKind::DoorDidNotOpen, TravelErrorKind::RideDidNotMove, TravelErrorKind::Io, TravelErrorKind::Interrupted]
    {
        assert!(!Bot::bank_stand_walk_refused(&travel_failure(kind)), "walk_to dossier {:?} propagates", kind);
    }
}

fn travel_failure(kind: TravelErrorKind) -> Error {
    Error::TravelFailure(crate::error::TravelError {
        kind,
        segment_index: None,
        position: (0, 0, 0),
        tick: 0,
        expected: String::new(),
        observed: String::new(),
        dossier: None,
    })
}

/// Spell-cast payloads lock the server decoder order (LostCity
/// OpObjT/OpHeldT/OpLocTDecoder: g2 fields, big-endian): a swapped
/// field ships a silent bad packet, so the bytes pin by hand.
#[test]
fn spell_cast_payloads_match_the_server_decoders() {
    // x=2930 (0x0B72), z=3515 (0x0DBB), obj=0x0010, spell=0x00A0.
    assert_eq!(super::opobjt_bytes(2930, 3515, 0x10, 0xA0), vec![0x0B, 0x72, 0x0D, 0xBB, 0x00, 0x10, 0x00, 0xA0]);
    // obj=0x0200, slot=3, com=0x0D12, spell=0x00A1.
    assert_eq!(super::opheldt_bytes(0x0200, 3, 0x0D12, 0xA1), vec![0x02, 0x00, 0x00, 0x03, 0x0D, 0x12, 0x00, 0xA1]);
    // x=100 (0x0064), z=200 (0x00C8), loc=0x0600, spell=0x00A2.
    assert_eq!(super::oploct_bytes(100, 200, 0x0600, 0xA2), vec![0x00, 0x64, 0x00, 0xC8, 0x06, 0x00, 0x00, 0xA2]);
}

/// Player spell/use payloads lock the server decoder order
/// (`OpPlayerTDecoder`: slot, spell com; `OpPlayerUDecoder`: slot,
/// obj com, use slot, use com — the same shape as `op_npc_t/u`).
#[test]
fn player_spell_and_use_payloads_match_the_server_decoders() {
    assert_eq!(super::opplayert_bytes(0x0007, 0x00A0), vec![0x00, 0x07, 0x00, 0xA0]);
    assert_eq!(super::opplayeru_bytes(0x0007, 0x0D12, 0x0003, 0x0A1B), vec![0x00, 0x07, 0x0D, 0x12, 0x00, 0x03, 0x0A, 0x1B]);
}

/// The wrappers resolve their op names (a bad name fails `Protocol`,
/// never reaches the socket): dead socket reads `Login`, proving the
/// send path is wired, framing included.
#[test]
fn player_spell_and_use_wrappers_reach_the_send_path() {
    let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
    for outcome in [bot.op_player_t(0x0007, 0x00A0), bot.op_player_u(0x0007, 0x0D12, 0x0003, 0x0A1B)] {
        assert!(matches!(outcome, Err(Error::Login(_))), "dead socket fails at login, never at the op name: {outcome:?}");
    }
}

/// The wrappers resolve their op names (a bad name fails `Protocol`,
/// never reaches the socket): dead socket reads `Login`, proving the
/// send path is wired, framing included (8-byte payloads fit the
/// table sizes or `frame_packet` would reject them live).
#[test]
fn spell_cast_wrappers_reach_the_send_path() {
    let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
    for outcome in [
        bot.op_obj_t(2930, 3515, 0x10, 0xA0),
        bot.op_held_t(0x0200, 3, 0x0D12, 0xA1),
        bot.op_loc_t(100, 200, 0x0600, 0xA2),
    ] {
        assert!(
            matches!(outcome, Err(Error::Login(_))),
            "dead socket fails at login, never at the op name: {outcome:?}"
        );
    }
}

/// An item already in view is clicked at once: live, every take paid a
/// blind 3-tick pump before its click (6291 of 7333 adjacent takes
/// clicked exactly 3 ticks after the call). Offline the default tables
/// know no Take op, so reaching that check proves no pump ran first (a
/// pump on the dead socket fails at login instead).
#[test]
fn take_clicks_an_item_in_view_without_pumping_first() {
    let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
    bot.world.level = 0;
    bot.world.x = 3250;
    bot.world.z = 3290;
    bot.world.objs.insert(((3251, 3290, 0), 1739), Some(1));
    let err = bot.take(Some(1739), None, 30).expect_err("offline tables know no Take op");
    assert!(format!("{err}").contains("has no Take op"), "decided on the visible item at once, got: {err}");
}

/// A ground object the server refused this session is no longer offered
/// to `take` (live: an egg past a pen fence was re-clicked after every
/// kill); other objects and other tiles stay takeable.
#[test]
fn refused_ground_objects_are_skipped_for_the_session() {
    let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
    bot.world.level = 0;
    bot.world.x = 3017;
    bot.world.z = 3294;
    bot.world.static_objs.insert(((3017, 3295, 0), 1944), 1);
    bot.world.static_objs.insert(((3016, 3293, 0), 1944), 1);
    assert_eq!(bot.reachable_ground_items(Some(1944)).len(), 2);
    bot.unreachable_objs.insert((1944, 3017, 3295, 0));
    let left = bot.reachable_ground_items(Some(1944));
    assert_eq!(left, vec![((3016, 3293, 0), 1944)], "only the refused tile drops out");
}

/// Live 2026-11-13 14:10:31 (bh99tw2f, deals 1063/1064): `bank_withdraw
/// Feather x100` clicked twice, then waited 300 ticks for nothing while
/// the bank held 2969 feathers. The server streams one withdrawal as
/// three partials in listener order (`inventory:inv` +10, `bank_side:inv`
/// +10, `bank_main:inv` -10, NetworkPlayer.updateInvs); a pump returning
/// between the second and the third left a pack+side+bank total 10 too
/// high, the next click's `before` was taken from it, and the settled
/// total never exceeded it. Progress is read on the pack alone.
#[test]
fn withdraw_progress_never_reads_a_transient_overcount() {
    const PACK: u16 = 3214;
    const SIDE: u16 = 2006;
    const BANK: u16 = 5382;
    const FEATHER: u16 = 314;
    let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
    bot.tables.com_id = |name| match name {
        "inventory:inv" => Some(PACK),
        "bank_side:inv" => Some(SIDE),
        "bank_main:inv" => Some(BANK),
        _ => None,
    };
    // UPDATE_INV_FULL: com, slot count, then (obj + 1, count) per slot.
    let empty = |com: u16, slots: u8| {
        let mut body = vec![(com >> 8) as u8, com as u8, slots];
        for _ in 0..slots {
            body.extend([0, 0, 0]);
        }
        body
    };
    // UPDATE_INV_PARTIAL: com, then (slot, obj + 1, count; 255 + u32 past 254).
    let partial = |com: u16, slot: u8, count: u32| {
        let mut body = vec![(com >> 8) as u8, com as u8, slot, ((FEATHER + 1) >> 8) as u8, (FEATHER + 1) as u8];
        if count < 255 {
            body.push(count as u8);
        } else {
            body.push(255);
            body.extend(count.to_be_bytes());
        }
        body
    };
    bot.world.feed("UPDATE_INV_FULL", &empty(PACK, 28)).expect("pack");
    bot.world.feed("UPDATE_INV_FULL", &empty(SIDE, 28)).expect("side");
    bot.world.feed("UPDATE_INV_FULL", &empty(BANK, 9)).expect("bank");
    bot.world.feed("UPDATE_INV_PARTIAL", &partial(BANK, 8, 2969)).expect("bank stack");
    assert_eq!(bot.pack_count(FEATHER), 0, "the bank stack is not carried");
    // Click 1: the pump returns after two of the three partials.
    bot.world.feed("UPDATE_INV_PARTIAL", &partial(PACK, 0, 10)).expect("pack +10");
    bot.world.feed("UPDATE_INV_PARTIAL", &partial(SIDE, 0, 10)).expect("side +10");
    let before_second_click = bot.pack_count(FEATHER);
    // The rest of click 1, then click 2 in full.
    bot.world.feed("UPDATE_INV_PARTIAL", &partial(BANK, 8, 2959)).expect("bank -10");
    bot.world.feed("UPDATE_INV_PARTIAL", &partial(PACK, 0, 20)).expect("pack +10");
    bot.world.feed("UPDATE_INV_PARTIAL", &partial(SIDE, 0, 20)).expect("side +10");
    bot.world.feed("UPDATE_INV_PARTIAL", &partial(BANK, 8, 2949)).expect("bank -10");
    assert!(
        bot.pack_count(FEATHER) > before_second_click,
        "the second click landed: {} after vs {before_second_click} before",
        bot.pack_count(FEATHER)
    );
    assert_eq!(bot.pack_count(FEATHER), 20, "the pack alone");
}

/// Live 2026-11-13 14:11:28 (bmzvbp5u, deal 1066, 16 coins owed, the
/// whole purse carried): two `inv_button3` one tick apart, the second
/// decided before the first click's partials arrived — 20 coins on the
/// window, both sides waited 3000 ticks for an exact match, `mismatch`.
/// Fake server: 25 coins in the pack, each offer click answered 8 ticks
/// later (pack, trade pack, offer partials). Every click waits for its
/// own answer: the window ends at exactly 16.
#[test]
fn trade_offer_never_clicks_past_the_owed_count() {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::{Duration, Instant};

    use crate::isaac::Isaac;
    use crate::protocol::client_op;

    const PACK: u16 = 3214;
    const COINS: u16 = 995;
    let (client_seed, server_seed) = ([3, 1, 4, 1], [5, 9, 2, 6]);
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback bind");
    let port = listener.local_addr().expect("local addr").port();
    let server = std::thread::spawn(move || -> u32 {
        let (mut sock, _) = listener.accept().expect("accept");
        sock.set_read_timeout(Some(Duration::from_millis(2))).expect("server read timeout");
        let mut outgoing = Isaac::new(server_seed);
        let mut incoming = Isaac::new(client_seed);
        let mut frame = |op: u8, body: &[u8]| -> Vec<u8> {
            let mut bytes = vec![op.wrapping_add(outgoing.next_int() as u8), (body.len() >> 8) as u8, body.len() as u8];
            bytes.extend_from_slice(body);
            bytes
        };
        let partial = |com: u16, count: u32| -> Vec<u8> {
            let obj = if count == 0 { 0 } else { COINS + 1 };
            vec![(com >> 8) as u8, com as u8, 0, (obj >> 8) as u8, obj as u8, count as u8]
        };
        let sizes: Vec<(u8, usize, u32)> = [("NO_TIMEOUT", 0), ("INV_BUTTON1", 1), ("INV_BUTTON2", 5), ("INV_BUTTON3", 10), ("INV_BUTTON4", u32::MAX)]
            .iter()
            .map(|(name, amount)| {
                let (op, size) = client_op(name).expect("known op");
                (op, size as usize, *amount)
            })
            .collect();
        let (mut carried, mut offered) = (25u32, 0u32);
        let mut pending: Vec<(u32, u32)> = Vec::new();
        let mut inbox: Vec<u8> = Vec::new();
        let started = Instant::now();
        let mut tick = 0u32;
        while started.elapsed() < Duration::from_secs(8) {
            let mut chunk = [0u8; 256];
            match sock.read(&mut chunk) {
                Ok(0) => break,
                Ok(got) => inbox.extend_from_slice(&chunk[..got]),
                Err(_) => {}
            }
            // Decode whole client packets: opcode then a fixed payload.
            while let Some(&first) = inbox.first() {
                let op = first.wrapping_sub(incoming.clone().next_int() as u8);
                let Some(&(_, size, amount)) = sizes.iter().find(|(known, _, _)| *known == op) else {
                    panic!("unexpected client op {op}");
                };
                if inbox.len() < 1 + size {
                    break;
                }
                incoming.next_int();
                inbox.drain(..1 + size);
                if size > 0 {
                    pending.push((tick + 8, amount));
                }
            }
            std::thread::sleep(Duration::from_millis(15));
            tick += 1;
            let mut out = frame(167, &[0x00, 0x00]);
            for (due, amount) in pending.iter().filter(|(due, _)| *due == tick) {
                let _ = due;
                let moved = (*amount).min(carried);
                carried -= moved;
                offered += moved;
                out.extend(frame(172, &partial(PACK, carried)));
                out.extend(frame(172, &partial(Bot::TRADE_PACK, carried)));
                out.extend(frame(172, &partial(Bot::TRADE_OFFER, offered)));
            }
            pending.retain(|(due, _)| *due > tick);
            if sock.write_all(&out).is_err() {
                break;
            }
        }
        offered
    });
    let client = TcpStream::connect(("127.0.0.1", port)).expect("loopback dial");
    client.set_read_timeout(Some(Duration::from_millis(500))).expect("client read timeout");
    let mut bot = Bot::new("127.0.0.1", port, ".", ".");
    bot.conn.attach_for_test(client, client_seed, server_seed);
    bot.last_keepalive = Some(Instant::now());
    bot.last_send_tick = bot.world.ticks;
    bot.tables.com_id = |name| match name {
        "inventory:inv" => Some(3214),
        _ => None,
    };
    bot.world.feed("UPDATE_INV_FULL", &[(PACK >> 8) as u8, PACK as u8, 1, ((COINS + 1) >> 8) as u8, (COINS + 1) as u8, 25]).expect("pack");
    bot.world.modal = Some(crate::world::Modal { layer: "main_side".to_string(), interface_id: 3321 });
    let offered_ok = bot.trade_offer(COINS, 16, 300).expect("socket alive");
    let window: u32 = bot.world.inv.get(&Bot::TRADE_OFFER).map(|slots| slots.values().map(|(_, count)| *count).sum()).unwrap_or(0);
    drop(bot);
    let server_offered = server.join().expect("server thread");
    assert_eq!(server_offered, 16, "the server holds exactly the owed coins on the window");
    assert_eq!(window, 16);
    assert!(offered_ok);
}
