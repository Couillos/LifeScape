#![forbid(unsafe_code)]
//! Lot 1.3 proof: the sampler describes the bots only when a point is due;
//! the points written are those of the eager path (`holdings::tests`).

use super::*;

#[test]
fn the_sampler_describes_the_bots_only_when_a_point_is_due() {
    let dir = std::env::temp_dir().join(format!("lifescape-holdings-{}-sampling_lazy", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let items = dir.join("items");
    std::fs::create_dir_all(&items).unwrap();
    let saves = dir.join("saves");
    std::fs::create_dir_all(&saves).unwrap();
    let save_arg = saves.to_string_lossy().to_string();
    let now = 1_762_675_200;
    let rows = |entries: Vec<(i64, i64)>| Value::Arr(entries.into_iter().map(|(id, count)| object(vec![("id", Value::Int(id)), ("count", Value::Int(count))])).collect());
    let live = vec![object(vec![("account", str_value("t01")), ("updated", Value::Float(jobs::now_secs())), ("inv", rows(vec![(995, 7)])), ("worn", Value::Arr(vec![])), ("bank", rows(vec![]))])];
    let described_calls = std::cell::Cell::new(0);
    let describe = || {
        described_calls.set(described_calls.get() + 1);
        live.clone()
    };
    assert!(maybe_sample_lazy(&describe, &items, &save_arg, now), "no point yet: described, written");
    assert_eq!(described_calls.get(), 1);
    for minutes in 1..60 {
        assert!(!maybe_sample_lazy(&describe, &items, &save_arg, now + minutes * 60), "minute {minutes}: no point due");
    }
    assert_eq!(described_calls.get(), 1, "59 quiet wake-ups described nobody");
    assert!(maybe_sample_lazy(&describe, &items, &save_arg, now + INTERVAL_SECONDS));
    assert_eq!(described_calls.get(), 2);
    let store = lifescape_store::Store::open(&store_path(&items)).unwrap();
    assert_eq!(store.hourly_points(Some(995)).unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}
