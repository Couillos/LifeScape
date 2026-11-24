use super::{CallError, Refusal};

#[test]
fn a_refusal_is_read_by_what_it_proves() {
    for text in ["busy: market is running", "busy: unload in progress", "busy: session is reconnecting", "busy: script did not yield within 500 ticks"] {
        assert_eq!(Refusal::parse(text), Refusal::Busy(text.to_string()));
        assert!(Refusal::parse(text).never_ran(), "{text}");
    }
    for text in ["no session, open lucy_rhodes first", "worker for amy died, open amy first"] {
        assert!(matches!(Refusal::parse(text), Refusal::NoSession(_)), "{text}");
        assert!(Refusal::parse(text).never_ran(), "{text}");
    }
    for text in ["io: timed out", "unknown script 'x'", "?", ""] {
        assert!(matches!(Refusal::parse(text), Refusal::Other(_)), "{text}");
        assert!(!Refusal::parse(text).never_ran(), "{text}");
    }
}

#[test]
fn the_text_of_every_error_is_what_it_was() {
    let cases = [
        (CallError::Unreachable("session unreachable for amy: connect refused".to_string()), true),
        (CallError::TimedOut("worker for amy did not answer in time (busy script or wedged session)".to_string()), false),
        (CallError::Protocol("expected value at line 1".to_string()), false),
        (CallError::Refused(Refusal::parse("busy: market is running")), false),
    ];
    for (error, unreachable) in cases {
        assert_eq!(error.to_string(), error.text());
        assert_eq!(String::from(error.clone()), error.text());
        assert_eq!(error.is_unreachable(), unreachable);
    }
    assert!(CallError::Refused(Refusal::parse("busy: x")).never_ran());
    assert!(!CallError::Unreachable("busy".to_string()).never_ran(), "a transport error proves nothing");
}
