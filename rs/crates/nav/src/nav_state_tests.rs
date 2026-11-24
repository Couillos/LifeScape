//! Caches kept across restarts (lot N4): a restored service holds what
//! the saved one held, serves the same frames, and ignores a file made
//! for other data. Needs the engine pack like the other world tests.

use std::path::PathBuf;
use std::sync::Arc;

use lifescape_json as json;

use crate::plan::{NavOptions, Tile};
use crate::service::{encode_plan, test_socket, PlanClient, Poll, Service, Snapshot};

fn tile(x: i32, z: i32) -> Tile {
    Tile { x, z, level: 0 }
}

fn options() -> NavOptions {
    NavOptions { f2p_only: true, ..NavOptions::default() }
}

fn state_file(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("nav-state-{name}-{}.json", std::process::id()))
}

fn service_on(version: &str, name: &str) -> Arc<Service> {
    let (snapshot, _world, _tables) = crate::service_replay_tests::real_snapshot();
    let snapshot = Snapshot::new(snapshot.world.clone(), snapshot.tables.clone(), version.to_string());
    Service::start(snapshot, &test_socket(name), 2).expect("service starts")
}

/// Plans of a few kinds: a long walk, a door leg, a ride, a sealed pocket
/// (open verdict recorded), an adjusted destination.
const LEGS: [((i32, i32), (i32, i32)); 7] = [
    ((3222, 3218), (3185, 3436)),
    ((3228, 3214), (3224, 3214)),
    ((3222, 3218), (3213, 3428)),
    ((3092, 3243), (3094, 3491)),
    ((3185, 3436), (3162, 3484)),
    ((3222, 3218), (3253, 3267)),
    ((3109, 3167), (3131, 3249)),
];

fn compute_all(service: &Service) -> Vec<String> {
    let client = PlanClient::new(service.socket_path(), "state");
    let version = service.current_version();
    LEGS.iter()
        .map(|((from_x, from_z), (dest_x, dest_z))| {
            let outcome = client.plan_once(tile(*from_x, *from_z), tile(*dest_x, *dest_z), &options(), &version, 30_000).expect("plan answers");
            match outcome.0 {
                Poll::Ready(plan) => json::stringify(&encode_plan(&plan)),
                other => format!("{other:?}"),
            }
        })
        .collect()
}

/// Rows in the order they would be saved, as comparable text.
fn rows_of(service: &Service) -> (Vec<String>, Vec<u64>, Vec<(u64, Option<Vec<u64>>)>) {
    let (_snapshot, routes, pockets) = service.state_rows();
    let plans: Vec<String> = routes
        .iter()
        .map(|(_, _, entry)| {
            format!(
                "{:?}->{:?} f2p={} paid={} {} path={} cum={:?}",
                entry.from,
                entry.dest,
                entry.f2p_only,
                entry.allow_paid,
                json::stringify(&encode_plan(&entry.plan)),
                entry.info.path.len(),
                entry.info.cumulative
            )
        })
        .collect();
    // Recency: ids by increasing stamp.
    let mut by_stamp: Vec<(u64, usize)> = routes.iter().enumerate().map(|(index, (_, stamp, _))| (*stamp, index)).collect();
    by_stamp.sort_unstable();
    let recency = by_stamp.iter().map(|(_, index)| *index as u64).collect();
    (plans, recency, pockets)
}

#[test]
fn restored_service_holds_what_the_saved_one_held() {
    let first = service_on("state-v1", "state-a");
    let computed = compute_all(&first);
    // A cache hit refreshes recency: touch the first leg again so the LRU
    // order differs from the insertion order.
    let again = compute_all(&first);
    assert_eq!(computed, again, "served from the cache bit for bit");
    let (plans, recency, pockets) = rows_of(&first);
    assert!(plans.len() >= 5, "the legs cached {} plans", plans.len());
    let file = state_file("hold");
    let saved = first.save_state(&file).expect("saves");
    assert_eq!(saved.routes, plans.len());
    assert_eq!(saved.pockets, pockets.len());
    println!("saved {saved:?}");
    assert!(saved.squares > 3, "painted squares recorded: {}", saved.squares);

    let second = service_on("state-v1", "state-b");
    let restored = second.restore_state(&file).expect("restores");
    assert_eq!((restored.routes, restored.pockets, restored.squares), (saved.routes, saved.pockets, saved.squares));
    let (plans_back, recency_back, pockets_back) = rows_of(&second);
    assert_eq!(plans_back, plans, "same plans, same insertion order, same path analysis");
    assert_eq!(recency_back, recency, "same recency order");
    assert_eq!(pockets_back, pockets, "same pocket verdicts");
    assert_eq!(second.route_stats().inserted, 0, "a restore is not traffic");

    // Served from the restored cache: the same frames, no computation.
    let before = second.stats();
    let served = compute_all(&second);
    let after = second.stats();
    assert_eq!(served, computed, "restored = recomputed, bit for bit");
    assert_eq!(after.misses - before.misses, 0, "every leg is a cache hit after a restore");
    let _ = std::fs::remove_file(&file);
}

#[test]
fn a_file_of_other_data_is_ignored() {
    let first = service_on("state-v1", "state-c");
    compute_all(&first);
    let file = state_file("other");
    first.save_state(&file).expect("saves");
    let other = service_on("state-v2", "state-d");
    let error = other.restore_state(&file).expect_err("another data version is refused");
    assert!(error.contains("data version"), "{error}");
    assert_eq!(other.route_stats().entries, 0, "nothing was loaded");
    let missing = other.restore_state(&state_file("absent")).expect_err("no file");
    assert!(missing.contains("no state file"), "{missing}");
    // Wrong format and garbage: ignored too.
    std::fs::write(&file, r#"{"format":"nav-state-0","version":"state-v2"}"#).unwrap();
    assert!(other.restore_state(&file).expect_err("format").contains("format"));
    std::fs::write(&file, "not json").unwrap();
    assert!(other.restore_state(&file).is_err());
    assert_eq!(other.route_stats().entries, 0);
    let _ = std::fs::remove_file(&file);
}
