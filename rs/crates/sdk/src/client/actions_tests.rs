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
