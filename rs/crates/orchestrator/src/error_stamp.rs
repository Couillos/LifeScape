//! Honest `last_error`: every error carries the moment it happened (unix
//! seconds and game tick) and is retired to `previous_error` once a later
//! run works, so an hours-old failure never reads as current.

use lifescape_json::{str_value, Value};

/// Add `at_unix` and `tick` to an error object (other shapes pass through).
pub fn stamp(error: Value, unix_secs: f64, tick: u64) -> Value {
    match error {
        Value::Obj(mut fields) => {
            fields.entry("at_unix".to_string()).or_insert(Value::Float(unix_secs));
            fields.entry("tick".to_string()).or_insert(Value::Int(tick as i64));
            Value::Obj(fields)
        }
        other => other,
    }
}

/// The error to keep when a session reports `incoming`: a stamped one is
/// taken as is; an unstamped one that equals the current error keeps the
/// current stamp (a poll never refreshes the time), else it is stamped now.
pub fn adopt(current: Option<&Value>, incoming: &Value, now_unix: f64, tick: u64) -> Value {
    if incoming.get("at_unix").as_f64().is_some() {
        return incoming.clone();
    }
    if let Some(known) = current {
        let same = ["type", "message", "where"].iter().all(|key| known.get(key) == incoming.get(key));
        if same && known.get("at_unix").as_f64().is_some() {
            return known.clone();
        }
    }
    stamp(incoming.clone(), now_unix, tick)
}

/// Move the current error to the previous slot (kept with its time).
pub fn retire(last_error: &mut Option<Value>, previous_error: &mut Option<Value>) {
    if let Some(error) = last_error.take() {
        *previous_error = Some(error);
    }
}

/// Seconds since the error, when it carries a time.
pub fn age_secs(error: &Value, now_unix: f64) -> Option<u64> {
    let at = error.get("at_unix").as_f64()?;
    Some((now_unix - at).max(0.0) as u64)
}

/// French age wording: "il y a 2 h 13 min", "il y a 45 s".
pub fn age_text(secs: u64) -> String {
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    if hours > 0 {
        format!("il y a {hours} h {minutes:02} min")
    } else if minutes > 0 {
        format!("il y a {minutes} min")
    } else {
        format!("il y a {secs} s")
    }
}

/// `last_error_ago` field of the state JSON (null without a timed error).
pub fn age_field(error: Option<&Value>, now_unix: f64) -> Value {
    match error.and_then(|value| age_secs(value, now_unix)) {
        Some(secs) => str_value(&age_text(secs)),
        None => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lifescape_json::object;

    fn sample() -> Value {
        object(vec![("type", str_value("Blocked"))])
    }

    #[test]
    fn stamp_adds_time_and_tick_once() {
        let stamped = stamp(sample(), 1000.0, 42);
        assert_eq!(stamped.get("at_unix").as_f64(), Some(1000.0));
        assert_eq!(stamped.get("tick").as_i64(), Some(42));
        let again = stamp(stamped, 2000.0, 99);
        assert_eq!(again.get("at_unix").as_f64(), Some(1000.0), "first stamp wins");
    }

    #[test]
    fn retire_moves_error_to_previous_with_its_time() {
        let mut last = Some(stamp(sample(), 1000.0, 1));
        let mut previous = None;
        retire(&mut last, &mut previous);
        assert!(last.is_none());
        assert_eq!(previous.unwrap().get("at_unix").as_f64(), Some(1000.0));
        let mut empty = None;
        let mut kept = Some(sample());
        retire(&mut empty, &mut kept);
        assert!(kept.is_some(), "no current error leaves the previous one alone");
    }

    #[test]
    fn adopt_keeps_the_first_time_of_an_unchanged_error() {
        let first = adopt(None, &sample(), 1000.0, 5);
        let polled = adopt(Some(&first), &sample(), 2000.0, 9);
        assert_eq!(polled.get("at_unix").as_f64(), Some(1000.0));
        let other = object(vec![("type", str_value("Failed"))]);
        assert_eq!(adopt(Some(&first), &other, 2000.0, 9).get("at_unix").as_f64(), Some(2000.0));
        let worker = stamp(sample(), 3000.0, 1);
        assert_eq!(adopt(Some(&first), &worker, 4000.0, 2).get("at_unix").as_f64(), Some(3000.0));
    }

    #[test]
    fn age_wording_is_french() {
        assert_eq!(age_text(2 * 3600 + 13 * 60 + 5), "il y a 2 h 13 min");
        assert_eq!(age_text(300), "il y a 5 min");
        assert_eq!(age_text(45), "il y a 45 s");
        let error = stamp(sample(), 1000.0, 1);
        assert_eq!(age_secs(&error, 1090.0), Some(90));
        assert_eq!(age_secs(&sample(), 1090.0), None);
        assert!(age_field(None, 5.0).is_null());
    }
}
