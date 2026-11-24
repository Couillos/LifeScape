//! Round trip on recorded frame shapes (lot 5.2): the lines below are the
//! shapes the worker's encoders write (`session_push`, `heartbeat_flows`)
//! as they travel on `push.sock`, with the lot 4.2 envelope.

use lifescape_json::{self as json, Value};

use super::{Frame, FrameError};
use crate::job_state::{Phase, Status};

const FULL: &str = r#"{"type":"snapshot","account":"amy_gray","phase":"running","status":"running","sent":1795477612.5,"snapshot":{"account":"amy_gray","updated":1795477612.123456,"position":[0,3228,3298],"zone":"Lumbridge","speed":1.0,"ticks":4410,"inv":[{"name":"coins","count":5000000}],"bank":[],"skills":[{"id":14,"xp":83000}],"npcs_at":[[837,3231,3301]],"players_at":[]}}"#;
const FULL_OLD_WORKER: &str = r#"{"type":"snapshot","account":"amy_gray","snapshot":{"account":"amy_gray","ticks":12}}"#;
const LIGHT: &str = r#"{"type":"snapshot_light","account":"amy_gray","script":"mining","status":"running","phase":"running","position":[0,3228,3298],"ticks":4411,"zone":"Lumbridge","intention":"fight","updated":1795477613.5,"npcs_at":[[837,3231,3301]],"players_at":[["will_field",3229,3301]]}"#;
const LIGHT_OLD_WORKER: &str = r#"{"type":"snapshot_light","account":"amy_gray","script":"","status":"idle","position":[0,3222,3218],"ticks":0,"zone":"","intention":"","updated":1795477600.25,"npcs_at":[],"players_at":[]}"#;
const REPORT: &str = r#"{"type":"task_report","account":"amy_gray","report":{"stop":"goal_reached","activity":"mining","ticks":1141,"error":null,"yields":[{"item":"iron_ore","count":28}]}}"#;
const EVENT: &str = r#"{"type":"event","account":"amy_gray","kind":"random_event","event":"maze","resolved":false,"tick":77,"script":"rescue_maze","args":["a","b"],"message":"","at":[0,2900,4560]}"#;
const SCRIPT_ERROR: &str = r#"{"type":"event","account":"amy_gray","kind":"script_error","event":"unknown","resolved":true,"tick":9,"script":"fishing","args":[],"message":"naïve \"quoted\"","at":[0,3200,3200]}"#;

fn parse(line: &str) -> Value {
    json::parse(line).expect("a recorded line")
}

#[test]
fn every_recorded_frame_survives_decode_then_encode() {
    for line in [FULL, FULL_OLD_WORKER, LIGHT, LIGHT_OLD_WORKER, REPORT, EVENT, SCRIPT_ERROR] {
        let frame = Frame::decode(parse(line)).unwrap_or_else(|error| panic!("{error:?}: {line}"));
        let back = frame.encode();
        let original = parse(line);
        // A light frame keeps its null-free keys; the others every key.
        assert_eq!(back, original, "{line}");
        // And decoding what was encoded gives the same typed frame.
        assert_eq!(Frame::decode(back).expect("decodes"), frame);
    }
}

#[test]
fn the_envelope_facts_are_typed() {
    let Frame::Snapshot(full) = Frame::decode(parse(FULL)).expect("full") else { panic!("a snapshot") };
    assert_eq!((full.account.as_str(), full.phase.clone(), full.status.clone(), full.sent), ("amy_gray", Some(Phase::Running), Some(Status::Running), Some(1795477612.5)));
    assert_eq!(full.snapshot.get("ticks").as_i64(), Some(4410), "the tree is the snapshot, the envelope is not in it");
    let facts = Frame::decode(parse(LIGHT)).expect("light").session_facts().expect("a phase");
    assert_eq!((facts.phase, facts.status, facts.sent), (Phase::Running, Some(Status::Running), Some(1795477613.5)), "a light frame's send time is its `updated`");
    assert_eq!(Frame::decode(parse(LIGHT_OLD_WORKER)).expect("old light").session_facts(), None, "an older worker's frame has no phase");
    assert_eq!(Frame::decode(parse(REPORT)).expect("report").session_facts(), None);
}

#[test]
fn a_light_frame_merges_only_its_keys_like_merge_light() {
    let cached = parse(r#"{"account":"amy_gray","inv":[1],"bank":[2],"script":"old","position":[0,1,1],"npcs_at":[]}"#);
    for line in [LIGHT, LIGHT_OLD_WORKER] {
        let (mut typed, mut legacy) = (cached.clone(), cached.clone());
        let Frame::Light(light) = Frame::decode(parse(line)).expect("light") else { panic!("a light frame") };
        light.merge_into(&mut typed);
        crate::jobs::merge_light(&mut legacy, &parse(line));
        assert_eq!(typed, legacy, "{line}");
        assert_eq!(typed.get("inv"), cached.get("inv"));
    }
}

#[test]
fn decoding_reads_the_defaults_the_daemon_applied() {
    let Frame::Event(event) = Frame::decode(parse(r#"{"type":"event","account":"a"}"#)).expect("event") else { panic!("an event") };
    assert_eq!((event.kind.as_str(), event.event.as_str(), event.resolved, event.tick, event.message.as_str()), ("", "unknown", false, 0, ""));
}

#[test]
fn what_the_daemon_cannot_take_is_a_typed_error() {
    assert_eq!(Frame::decode(Value::Null), Err(FrameError::NotAnObject));
    let unknown = Frame::decode(parse(r#"{"type":"mystery","account":"a"}"#)).expect_err("unknown");
    assert_eq!(unknown, FrameError::UnknownType { account: "a".to_string(), kind: "mystery".to_string() });
    assert_eq!(unknown.account(), "a");
    assert_eq!(Frame::decode(parse(r#"{"account":"a"}"#)), Err(FrameError::UnknownType { account: "a".to_string(), kind: String::new() }), "no type is an unknown type");
}
