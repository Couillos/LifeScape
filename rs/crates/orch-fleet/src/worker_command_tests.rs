use lifescape_json::{self as json, Value};

use super::WorkerCommand;

/// Requests as the daemon wrote them before the type existed (key for key).
const WIRE: &[(&str, &str)] = &[
    ("ping", r#"{"cmd":"ping"}"#),
    ("status", r#"{"cmd":"status"}"#),
    ("run", r#"{"cmd":"run","script":"mining","args":["--node","n1"]}"#),
    ("unload", r#"{"cmd":"unload"}"#),
    ("pause", r#"{"cmd":"pause"}"#),
    ("resume", r#"{"cmd":"resume"}"#),
    ("hold", r#"{"cmd":"hold"}"#),
    ("close", r#"{"cmd":"close"}"#),
    ("settle", r#"{"cmd":"settle"}"#),
    ("call", r#"{"cmd":"call","method":"bank.open","args":["a",3],"admin":true}"#),
    ("methods", r#"{"cmd":"methods","admin":false}"#),
    ("pose_market", r#"{"cmd":"pose_market","script":"market","args":["deliver","o1"]}"#),
];

#[test]
fn every_command_encodes_to_the_line_the_daemon_always_sent() {
    for (word, line) in WIRE {
        let wire = json::parse(line).expect("a recorded request");
        let command = WorkerCommand::decode(&wire).unwrap_or_else(|| panic!("{word} decodes"));
        assert_eq!(command.word(), *word);
        assert_eq!(command.encode(), wire, "{word}");
        assert_eq!(json::stringify(&command.encode()), json::stringify(&wire), "{word}: same bytes");
    }
}

#[test]
fn a_word_this_build_never_sends_is_not_a_command() {
    assert_eq!(WorkerCommand::decode(&json::parse(r#"{"cmd":"set_locks"}"#).unwrap()), None);
    assert_eq!(WorkerCommand::decode(&Value::Null), None);
}
