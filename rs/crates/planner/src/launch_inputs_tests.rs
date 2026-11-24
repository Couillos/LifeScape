//! Tests of `launch_inputs`: every counting helper equals the formula the
//! launch pass used on the raw live snapshot (the old functions are
//! restated here as reference implementations).

use super::{base_of, read_stacks, LaunchInputs};
use lifescape_json::{object, str_value, Value};

fn stack(name: &str, count: i64) -> Value {
    let obj = lifescape_gamedata::objs_by_name(name).into_iter().next().unwrap_or_else(|| panic!("{name} not in the dump"));
    object(vec![("id", Value::Int(obj.obj_id as i64)), ("name", str_value(obj.display.unwrap_or(name))), ("count", Value::Int(count))])
}

fn bare(name: &str, count: i64) -> Value {
    object(vec![("name", str_value(name)), ("count", Value::Int(count))])
}

/// Old `live_items::entry_is`.
fn old_entry_is(entry: &Value, item: &str) -> bool {
    let wanted = item.to_lowercase();
    let same = |name: &str| {
        let name = name.to_lowercase();
        name == wanted || name.strip_prefix("cert_") == Some(wanted.as_str())
    };
    let display = |item: &str| lifescape_gamedata::objs_by_name(item).into_iter().next().and_then(|obj| obj.display.or(obj.name)).unwrap_or(item).to_string();
    match entry {
        Value::Obj(_) => {
            let id_name = entry.get("id").as_i64().and_then(|id| usize::try_from(id).ok()).and_then(lifescape_gamedata::obj_by_id).and_then(|obj| obj.name);
            match id_name {
                Some(name) => same(name),
                None => entry.get("name").as_str().is_some_and(|name| same(name) || name.eq_ignore_ascii_case(&display(item))),
            }
        }
        Value::Str(name) => same(name) || name.eq_ignore_ascii_case(&display(item)),
        _ => false,
    }
}

/// Old `live_items::live_units`.
fn old_live_units(live: &Value, item: &str) -> i64 {
    let mut total = 0i64;
    for key in ["inv", "bank", "worn"] {
        for entry in live.get(key).as_arr().map(Vec::as_slice).unwrap_or_default() {
            if !old_entry_is(entry, item) {
                continue;
            }
            total += match entry {
                Value::Obj(_) => entry.get("count").as_i64().unwrap_or(1).max(0),
                _ => 1,
            };
        }
    }
    total
}

fn inputs_of(live: &Value, locks: &[(&str, i64)], listed: &[(&str, i64)]) -> LaunchInputs {
    let id = |name: &str| lifescape_gamedata::objs_by_name(name).into_iter().next().expect("in the dump").obj_id as i64;
    LaunchInputs {
        inv: read_stacks(live, "inv"),
        bank: read_stacks(live, "bank"),
        wearing: read_stacks(live, "worn"),
        locks: locks.iter().map(|(name, units)| (base_of(id(name)), *units)).collect(),
        listed: listed.iter().map(|(name, units)| (id(name), *units)).collect(),
        ..LaunchInputs::blank_for_tests()
    }
}

fn sample() -> Value {
    object(vec![
        ("inv", Value::Arr(vec![stack("bronze_pickaxe", 1), stack("coins", 5000), stack("cert_yew_logs", 3), bare("Yew logs", 2), str_value("bronze_axe"), Value::Int(7)])),
        ("bank", Value::Arr(vec![stack("steel_pickaxe", 1), stack("yew_logs", 10), stack("airrune", 0), stack("unstrung_emerald_amulet", 7)])),
        ("worn", Value::Arr(vec![stack("iron_pickaxe", 1), str_value("bronze_sword")])),
    ])
}

const NAMES: [&str; 12] = ["bronze_pickaxe", "steel_pickaxe", "iron_pickaxe", "coins", "yew_logs", "airrune", "bronze_axe", "bronze_sword", "unstrung_emerald_amulet", "strung_emerald_amulet", "staff_of_air", "nothing_here"];

/// Raw holdings equal `raw_holdings_count` (`live_units`) on every name,
/// banknotes, bare names, zero counts and homonyms included.
#[test]
fn raw_units_equal_the_old_live_units() {
    let live = sample();
    let inputs = inputs_of(&live, &[], &[]);
    for name in NAMES {
        assert_eq!(inputs.raw_units(name), old_live_units(&live, name), "{name}");
    }
    assert_eq!(inputs.raw_units("yew_logs"), 3 + 2 + 10);
    assert_eq!(inputs.raw_units("unstrung_emerald_amulet"), 7);
    assert_eq!(inputs.raw_units("strung_emerald_amulet"), 0);
}

/// Holdings equal `holdings_count`: raw minus locks (clamped at 0), coins
/// never locked.
#[test]
fn held_equals_the_old_holdings_count() {
    let live = sample();
    let locks = [("yew_logs", 4), ("bronze_pickaxe", 5), ("coins", 100)];
    let inputs = inputs_of(&live, &locks, &[]);
    let locked = |name: &str| locks.iter().find(|(known, _)| *known == name).map_or(0, |(_, units)| *units);
    for name in NAMES {
        let raw = old_live_units(&live, name);
        let old = if name == "coins" { raw } else { (raw - locked(name).max(0)).max(0) };
        assert_eq!(inputs.held(name), old, "{name}");
    }
    assert_eq!(inputs.held("coins"), 5000);
    assert_eq!(inputs.held("bronze_pickaxe"), 0);
}

/// A lock on a banknote's goods counts on the goods (`base_of`).
#[test]
fn a_lock_keys_on_the_base_object() {
    let live = sample();
    let inputs = inputs_of(&live, &[("yew_logs", 1)], &[]);
    assert_eq!(base_of(lifescape_gamedata::objs_by_name("cert_yew_logs").into_iter().next().expect("dump").obj_id as i64), lifescape_gamedata::objs_by_name("yew_logs").into_iter().next().expect("dump").obj_id as i64);
    assert_eq!(inputs.held("yew_logs"), 14);
}

/// Free of listings equals `(holdings_count - listed_units).max(0)`, the
/// family variant folds `family_held` over it.
#[test]
fn free_of_listings_equals_the_old_formula() {
    let live = sample();
    let listed = [("yew_logs", 20), ("steel_pickaxe", 1), ("bronze_axe", 0)];
    let inputs = inputs_of(&live, &[("yew_logs", 2)], &listed);
    let listed_of = |name: &str| listed.iter().find(|(known, _)| *known == name).map_or(0, |(_, units)| *units);
    for name in NAMES {
        assert_eq!(inputs.free_of_listings(name), (inputs.held(name) - listed_of(name)).max(0), "{name}");
        let free_of = |n: &str| (inputs.held(n) - listed_of(n)).max(0);
        assert_eq!(inputs.free_now(name, true), free_of(name), "exact {name}");
        assert_eq!(inputs.free_now(name, false), crate::tools::family_held(name, i64::MAX / 4, &free_of), "family {name}");
        assert_eq!(inputs.family_held(name), crate::tools::family_held(name, i64::MAX / 4, &|n| inputs.held(n)), "held family {name}");
    }
    assert_eq!(inputs.free_of_listings("yew_logs"), 0);
    assert_eq!(inputs.free_of_listings("steel_pickaxe"), 0);
    assert_eq!(inputs.listed_units("nothing_here"), 0);
}

/// A steel pickaxe meets `bronze_pickaxe` by family; the exact count
/// stays 1 for the piece itself.
#[test]
fn the_family_meets_a_better_piece() {
    let live = object(vec![("inv", Value::Arr(vec![stack("steel_pickaxe", 1)]))]);
    let inputs = inputs_of(&live, &[], &[]);
    assert_eq!(inputs.family_held("bronze_pickaxe"), 1);
    assert_eq!(inputs.held("bronze_pickaxe"), 0);
}

/// Carried tools equal `carried_tools`: pack + bank + worn names with a
/// positive count, first-seen order, a tool wholly locked dropped.
#[test]
fn carried_tools_equal_the_old_listing() {
    let live = sample();
    let inputs = inputs_of(&live, &[("bronze_pickaxe", 1), ("yew_logs", 15)], &[]);
    let names = inputs.carried_tools();
    // Old: entries (name, count>0) in inv, bank, worn order, merged by name.
    let expected: Vec<String> = ["coins", "Yew logs", "bronze_axe", "steel_pickaxe", "unstrung_emerald_amulet", "iron_pickaxe", "bronze_sword"].iter().map(|name| name.to_string()).collect();
    assert_eq!(names, expected);
    assert!(!names.contains(&"bronze_pickaxe".to_string()), "locked tool not carried");
    assert!(names.contains(&"steel_pickaxe".to_string()));
    assert!(!names.contains(&"airrune".to_string()), "zero count never rides");
}

/// Committed coins: every resting buy except the exception's, plus the
/// flat deal and gift part.
#[test]
fn committed_skips_the_exception_only() {
    let inputs = LaunchInputs { commit_rows: vec![(10, 300), (11, 50), (10, 7)], commit_flat: 1000, ..LaunchInputs::blank_for_tests() };
    assert_eq!(inputs.committed(None), 1357);
    assert_eq!(inputs.committed(Some(10)), 1050);
    assert_eq!(inputs.committed(Some(99)), 1357);
}
