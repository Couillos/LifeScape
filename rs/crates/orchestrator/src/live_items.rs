//! Live stacks read by object id (tour 3, point 7).
//!
//! The heartbeat's `inv` / `bank` / `worn` entries carry the object id and
//! its DISPLAY name; several objects share one display ("Emerald amulet"
//! is both `unstrung_emerald_amulet` and `strung_emerald_amulet`). Live
//! 2026-11-17 12:11 will_field: "n2496 holds 7 strung_emerald_amulet,
//! done without launch" while it held 7 UNSTRUNG amulets; the stringing
//! node then blocked, ran short, and was "rescued on the market". Class
//! C (the brain's holdings read, not the map nor the SDK): an entry is
//! the object its id names, the name only for an entry without id (an
//! older snapshot shape). Same rule as the guides' `quote_of` (7f034de).

#![forbid(unsafe_code)]

use lifescape_json::Value;

/// Debug name of a live entry's object id (`None`: no id, or one the
/// dump does not know).
fn id_name(entry: &Value) -> Option<&'static str> {
    let id = entry.get("id").as_i64()?;
    usize::try_from(id).ok().and_then(lifescape_gamedata::obj_by_id).and_then(|obj| obj.name)
}

/// True when the live `entry` is a stack of `item_debug` (its banknote
/// counts as the object: the same goods). By id when the entry has one,
/// else by its debug or display name.
pub(crate) fn entry_is(entry: &Value, item_debug: &str) -> bool {
    let wanted = item_debug.to_lowercase();
    let same = |name: &str| {
        let name = name.to_lowercase();
        name == wanted || name.strip_prefix("cert_") == Some(wanted.as_str())
    };
    match entry {
        Value::Obj(_) => match id_name(entry) {
            Some(name) => same(name),
            None => entry.get("name").as_str().is_some_and(|name| same(name) || name.eq_ignore_ascii_case(&display(item_debug))),
        },
        Value::Str(name) => same(name) || name.eq_ignore_ascii_case(&display(item_debug)),
        _ => false,
    }
}

fn display(item_debug: &str) -> String {
    lifescape_gamedata::objs_by_name(item_debug).into_iter().next().and_then(|obj| obj.display.or(obj.name)).unwrap_or(item_debug).to_string()
}

/// Units of `item_debug` the live snapshot holds (pack, bank, worn),
/// listed or not.
pub(crate) fn live_units(live: &Value, item_debug: &str) -> i64 {
    let mut total = 0i64;
    for key in ["inv", "bank", "worn"] {
        for entry in live.get(key).as_arr().cloned().unwrap_or_default() {
            if !entry_is(&entry, item_debug) {
                continue;
            }
            total += match &entry {
                Value::Obj(_) => entry.get("count").as_i64().unwrap_or(1).max(0),
                _ => 1,
            };
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::{entry_is, live_units};
    use lifescape_json::{object, str_value, Value};

    fn stack(name: &str, count: i64) -> Value {
        let obj = lifescape_gamedata::objs_by_name(name).into_iter().next().expect("in the dump");
        object(vec![("id", Value::Int(obj.obj_id as i64)), ("name", str_value(obj.display.unwrap_or(name))), ("count", Value::Int(count))])
    }

    /// Live will_field 12:11: 7 UNSTRUNG emerald amulets ("Emerald
    /// amulet") never read as 7 strung ones (same display name).
    #[test]
    fn homonyms_are_read_by_id() {
        let live = object(vec![("inv", Value::Arr(vec![stack("unstrung_emerald_amulet", 7)])), ("bank", Value::Arr(vec![stack("strung_emerald_amulet", 1)]))]);
        assert_eq!(live_units(&live, "strung_emerald_amulet"), 1);
        assert_eq!(live_units(&live, "unstrung_emerald_amulet"), 7);
    }

    /// A banknote is the goods; an entry without id reads by name.
    #[test]
    fn notes_and_old_shapes_still_count() {
        let note = stack("cert_unstrung_emerald_amulet", 3);
        assert!(entry_is(&note, "unstrung_emerald_amulet"));
        assert!(!entry_is(&note, "strung_emerald_amulet"));
        let bare = object(vec![("name", str_value("Yew logs")), ("count", Value::Int(4))]);
        assert!(entry_is(&bare, "yew_logs"));
        assert!(entry_is(&str_value("coins"), "coins"));
    }
}
