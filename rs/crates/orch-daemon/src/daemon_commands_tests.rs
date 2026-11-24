use lifescape_json::{self as json, Value};

use super::OperatorCommand;
use crate::daemon::Orchestrator;

fn request(line: &str) -> Value {
    json::parse(line).expect("a request")
}

/// Every word the old `dispatch` match knew, with the keys it read.
#[test]
fn every_command_word_parses_to_its_variant() {
    let cases: Vec<(&str, OperatorCommand)> = vec![
        (r#"{"cmd":"open","account":"a"}"#, OperatorCommand::Open),
        (r#"{"cmd":"run","account":"a","script":"mining","args":["--x"]}"#, OperatorCommand::Run { script: "mining".to_string(), args: vec![Value::Str("--x".to_string())] }),
        (r#"{"cmd":"unload"}"#, OperatorCommand::Unload),
        (r#"{"cmd":"pause"}"#, OperatorCommand::Pause),
        (r#"{"cmd":"resume"}"#, OperatorCommand::Resume),
        (r#"{"cmd":"hold"}"#, OperatorCommand::Hold),
        (r#"{"cmd":"close"}"#, OperatorCommand::Close),
        (r#"{"cmd":"call","method":"bank.open","args":["a",3]}"#, OperatorCommand::Call { method: "bank.open".to_string(), args: vec![Value::Str("a".to_string()), Value::Int(3)] }),
        (r#"{"cmd":"methods"}"#, OperatorCommand::Methods),
        (r#"{"cmd":"ambition","kind":"smith"}"#, OperatorCommand::Ambition { kind: "smith".to_string() }),
        (r#"{"cmd":"rares"}"#, OperatorCommand::Rares),
        (r#"{"cmd":"fund_makers"}"#, OperatorCommand::FundMakers { dry_run: true, force: false }),
        (r#"{"cmd":"fund_makers","dry_run":false,"force":true}"#, OperatorCommand::FundMakers { dry_run: false, force: true }),
        (r#"{"cmd":"bid_why","item":"logs"}"#, OperatorCommand::BidWhy { item: "logs".to_string() }),
        (r#"{"cmd":"status"}"#, OperatorCommand::Status),
        (r#"{"cmd":"job_list"}"#, OperatorCommand::JobList),
        (r#"{"cmd":"items"}"#, OperatorCommand::Items),
        (r#"{"cmd":"scripts"}"#, OperatorCommand::Scripts),
        (r#"{"cmd":"accounts"}"#, OperatorCommand::Accounts),
        (r#"{"cmd":"show"}"#, OperatorCommand::Show),
        (r#"{"cmd":"live"}"#, OperatorCommand::Live),
        (r#"{"cmd":"logs"}"#, OperatorCommand::Logs { lines: 40 }),
        (r#"{"cmd":"logs","lines":7}"#, OperatorCommand::Logs { lines: 7 }),
        (r#"{"cmd":"shutdown"}"#, OperatorCommand::Shutdown),
        (r#"{"cmd":"reload-nav"}"#, OperatorCommand::ReloadNav),
        (r#"{"cmd":"nav-stats"}"#, OperatorCommand::NavStats),
        (r#"{"cmd":"res-stats"}"#, OperatorCommand::ResStats),
        (r#"{"cmd":"brain_wishes"}"#, OperatorCommand::BrainWishes),
        (r#"{"cmd":"market","op":"book"}"#, OperatorCommand::Market),
        (r#"{"cmd":"nope"}"#, OperatorCommand::Unknown("nope".to_string())),
        (r#"{}"#, OperatorCommand::Unknown(String::new())),
    ];
    for (line, expected) in cases {
        assert_eq!(OperatorCommand::parse(&request(line)), expected, "{line}");
    }
}

#[test]
fn the_cli_strings_of_a_call_reach_the_worker_as_strings() {
    let call = OperatorCommand::parse(&request(r#"{"cmd":"call","method":"m","args":["5",5,{"k":1}]}"#));
    let OperatorCommand::Call { args, .. } = call else { panic!("a call") };
    assert_eq!(args, vec![Value::Str("5".to_string()), Value::Int(5), request(r#"{"k":1}"#)]);
}

#[test]
fn the_replies_that_need_no_worker_are_what_they_were() {
    let orch = Orchestrator::new();
    let unknown = orch.dispatch(&request(r#"{"cmd":"frobnicate"}"#));
    assert_eq!((unknown.get("ok").as_bool(), unknown.get("error").as_str()), (Some(false), Some("unknown command 'frobnicate'")));
    let open = orch.dispatch(&request(r#"{"cmd":"open","account":"a"}"#));
    assert_eq!(open.get("error").as_str(), Some("open needs daemon context"));
    let scripts = orch.dispatch(&request(r#"{"cmd":"scripts"}"#));
    assert_eq!(scripts.get("ok").as_bool(), Some(true));
    assert!(scripts.get("scripts").as_arr().is_some_and(|names| !names.is_empty()));
    let status = orch.dispatch(&request(r#"{"cmd":"status"}"#));
    assert_eq!(status.get("jobs").as_arr().map(|jobs| jobs.len()), Some(0), "no job opened");
    let accounts = orch.dispatch(&request(r#"{"cmd":"accounts"}"#));
    assert_eq!(accounts.get("live").as_arr().map(|live| live.len()), Some(0));
    // A command on an account with no session says so, as before.
    let unload = orch.dispatch(&request(r#"{"cmd":"unload","account":"nobody_here"}"#));
    assert_eq!(unload.get("error").as_str(), Some("no session for nobody_here"));
    let call = orch.dispatch(&request(r#"{"cmd":"call","account":"nobody_here","method":"m"}"#));
    assert_eq!(call.get("error").as_str(), Some("no session, open nobody_here first"));
}
