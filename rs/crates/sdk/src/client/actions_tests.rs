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

/// Two consecutive refusals still precede a success live (booth 3243
/// opened on its 3rd fan side): only the 3rd consecutive refusal aborts.
#[test]
fn bank_fan_aborts_on_third_consecutive_refusal() {
    assert!(!Bot::bank_fan_refused(0), "no refusal keeps trying");
    assert!(!Bot::bank_fan_refused(1), "first refusal keeps trying");
    assert!(!Bot::bank_fan_refused(2), "second refusal keeps trying");
    assert!(Bot::bank_fan_refused(3), "third consecutive aborts");
    assert!(Bot::bank_fan_refused(8), "late refusal aborts");
}

/// A dead socket still propagates typed instead of degrading into
/// Ok(false): adjacent to the booth the approach short-circuits, the
/// click send fails loud, never masked as a refusal.
#[test]
fn open_bank_offline_propagates_send_error() {let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
    bot.world.level = 0;
    bot.world.x = 3091;
    bot.world.z = 3246;
    let err = bot.open_bank(3091, 3245, 2213, 50).expect_err("dead socket fails loud");
    assert!(format!("{err}").contains("not logged in"), "typed conn error, got: {err}");
}

/// Socket dead mid-fan degrades into Ok(false) under the old
/// `.is_err()` count: 3 swallowed Io errors read as 3 refusals and
/// `bank_trip` strands instead of reconnecting. Only true refusals
/// count now, so transport/operator stops propagate. The fan loop
/// itself is unreachable offline (the click send fails before it),
/// so this locks the classification every fan error goes through.
#[test]
fn bank_fan_propagates_dead_socket_instead_of_refusing() {
    for err in [
        Error::Io("not logged in".to_string()),
        Error::Login("stale session".to_string()),
        Error::Interrupted("run stopped".to_string()),
        Error::State("bank_main:inv missing".to_string()),
        Error::Protocol("bad frame".to_string()),
    ] {
        assert!(!Bot::bank_fan_counts_as_refusal(&err), "propagates, never a refusal: {err}");
    }
}

/// 3 true refusals still abort the booth as before: Unreachable (wall,
/// counter) and Timeout (stalled leg) are the only fan refusals.
#[test]
fn bank_fan_counts_only_true_refusals() {
    assert!(Bot::bank_fan_counts_as_refusal(&Error::Unreachable("counter".to_string())), "wall counts");
    assert!(Bot::bank_fan_counts_as_refusal(&Error::Timeout("stalled leg".to_string())), "stall counts");
    assert!(Bot::bank_fan_counts_as_refusal(&Error::StepRefused("shut".to_string())), "raw refused step counts");
    for kind in [TravelErrorKind::NoRoute, TravelErrorKind::StepRefused, TravelErrorKind::Timeout] {
        assert!(
            Bot::bank_fan_counts_as_refusal(&travel_failure(kind)),
            "walk_to dossier {:?} counts",
            kind
        );
    }
    for kind in
        [TravelErrorKind::DoorDidNotOpen, TravelErrorKind::RideDidNotMove, TravelErrorKind::Io, TravelErrorKind::Interrupted]
    {
        assert!(!Bot::bank_fan_counts_as_refusal(&travel_failure(kind)), "walk_to dossier {:?} propagates", kind);
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
