//! Lot 4: fairness under replan-storm + operator metrics, no live sessions.
//!
//! One faulty account hammers the service (cap flood + fail->replan loop
//! with edge bans, the `travel.rs` replan replayed submit by submit) while
//! victim accounts plan normally: victims stay 100 % served, only the
//! culprit is refused (account-cap) or served from the negative cache.
//! Companion: cancel releases the cap, and every lot 4 gauge reads through
//! the public `stats` op (counters + queue + RAM, no log spam).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lifescape_nav::plan::{EdgeId, NavOptions, NoRouteReason, Segment, Tile};
use lifescape_nav::service::{PlanClient, Poll, Service, Snapshot, Submit, ACCOUNT_CAP};
use lifescape_nav::static_world::StaticWorld;
use lifescape_nav::tables::Tables;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

fn setup() -> (Arc<StaticWorld>, Arc<Tables>) {
    let root = workspace_root();
    let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
    let gametypes =
        std::fs::read_to_string(root.join("LifeScape/gamedata/data/gametypes.json")).expect("gametypes");
    let overrides =
        std::fs::read_to_string(root.join("LifeScape/gamedata/data/nav_overrides.json")).expect("overrides");
    let defs = lifescape_nav::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
    let world = StaticWorld::open(&zip, defs).expect("static world");
    let zones =
        lifescape_nav::f2p::load_zones(&root.join("lostcity/content/maps/free2play.csv")).expect("f2p");
    world.set_f2p(zones);
    let tables = Tables::load(&root.join("LifeScape/gamedata/data")).expect("tables");
    world.set_sky_floors(tables.sky_floors.clone());
    (world, Arc::new(tables))
}

fn shared_world() -> (Arc<StaticWorld>, Arc<Tables>) {
    static WORLD: std::sync::OnceLock<(Arc<StaticWorld>, Arc<Tables>)> = std::sync::OnceLock::new();
    WORLD.get_or_init(setup).clone()
}

fn short_options() -> NavOptions {
    NavOptions {
        allow_paid: false,
        f2p_only: true,
        satisfied: HashSet::new(),
        excluded_edges: HashSet::new(),
    }
}

fn start_service(threads: usize) -> Arc<Service> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let (world, tables) = shared_world();
    let sock = std::env::temp_dir().join(format!(
        "lifescape-lot4-{}-{threads}-{}.sock",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    ));
    let _ = std::fs::remove_file(&sock);
    Service::start(Snapshot::new(world, tables, "lot4".to_string()), &sock, threads)
        .expect("service starts")
}

/// Submit, then poll to a terminal outcome (bounded: cancel + Unknown past
/// the bound, never a hang).
fn await_terminal(client: &PlanClient, id: u64) -> Poll {
    let start = std::time::Instant::now();
    loop {
        match client.poll(id).expect("poll answers").0 {
            Poll::Pending => {
                assert!(
                    start.elapsed() < Duration::from_secs(60),
                    "ticket {id} never lands"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
            terminal => return terminal,
        }
    }
}

fn door_leg() -> (Tile, Tile) {
    (
        Tile { x: 3228, z: 3214, level: 0 },
        Tile { x: 3224, z: 3214, level: 0 },
    )
}

fn first_door_edge(plan: &lifescape_nav::plan::Plan) -> Option<EdgeId> {
    plan.segments.iter().find_map(|segment| match segment {
        Segment::OpenDoor { door, .. } => Some(EdgeId::Door {
            loc: door.loc,
            x: door.x,
            z: door.z,
            level: door.level,
        }),
        _ => None,
    })
}

/// Storm, phase 1: the culprit fires 10 distinct submits without polling.
/// Cap 2 admits exactly 2, the other 8 refuse account-cap. Deterministic:
/// the first job pays the cold L2 build (~3-digit ms), so the pool cannot
/// drain either admission before the 8 later submits land.
#[test]
fn storm_cap_flood_refuses_only_the_culprit() {
    let service = start_service(2);
    let culprit = PlanClient::new(service.socket_path(), "storm-culprit");
    let (from, _) = door_leg();
    let mut accepted = 0u32;
    let mut refused = 0u32;
    let mut ids = Vec::new();
    for nth in 0..10 {
        let dest = Tile { x: 3200 + nth, z: 3200, level: 0 };
        match culprit.submit(from, dest, &short_options(), "lot4").expect("submit answers") {
            Submit::Accepted { id, .. } => {
                accepted += 1;
                ids.push(id);
            }
            Submit::Refused { reason, .. } => {
                refused += 1;
                assert!(reason.contains("account-cap"), "cap flood refuses by cap, got {reason}");
            }
        }
    }
    assert_eq!(accepted, ACCOUNT_CAP as u32, "cap admits exactly {ACCOUNT_CAP}");
    assert_eq!(refused, 10 - ACCOUNT_CAP as u32, "the rest refuses, nobody queues behind");
    let stats = service.stats();
    assert!(stats.refused_cap >= refused as u64, "refusals are counted");
    // Victims plan while the culprit holds its 2 tickets: full service.
    for victim in ["storm-alice", "storm-bob"] {
        let client = PlanClient::new(service.socket_path(), victim);
        let (from, dest) = door_leg();
        let id = match client.submit(from, dest, &short_options(), "lot4").expect("submit") {
            Submit::Accepted { id, .. } => id,
            Submit::Refused { reason, .. } => panic!("victim {victim} refused under storm: {reason}"),
        };
        match await_terminal(&client, id) {
            Poll::Ready(plan) => assert!(!plan.segments.is_empty(), "victim {victim} plans"),
            other => panic!("victim {victim} must plan, got {other:?}"),
        }
    }
    for id in ids {
        let _ = culprit.cancel(id);
    }
}

/// Storm, phase 2: the fail->replan loop replayed submit by submit (ban the
/// edge the plan swung through, resubmit: the `travel.rs` replan without a
/// live bot). Victims interleave every round and stay 100 % served; the
/// culprit's verdicts stay terminal (Ready, then Disconnected negatives
/// from the shared cache, never a stall, never a victim refusal).
#[test]
fn storm_replan_loop_starves_nobody() {
    let service = start_service(2);
    let culprit = PlanClient::new(service.socket_path(), "replan-culprit");
    let witness = PlanClient::new(service.socket_path(), "replan-witness");
    let (from, dest) = door_leg();
    let mut options = short_options();
    let mut failures = 0u32;
    for round in 0..6 {
        let id = match culprit.submit(from, dest, &options, "lot4").expect("submit") {
            Submit::Accepted { id, .. } => id,
            Submit::Refused { reason, .. } => panic!("culprit refused round {round}: {reason}"),
        };
        match await_terminal(&culprit, id) {
            Poll::Ready(plan) => {
                // Execution "fails" on the first door: ban it, replan.
                match first_door_edge(&plan) {
                    Some(edge) => {
                        options.excluded_edges.insert(edge);
                    }
                    None => panic!("round {round}: door leg plans doorless"),
                }
            }
            Poll::Failed(no) => {
                assert_eq!(no.reason, NoRouteReason::Disconnected, "banned door seals the pocket");
                failures += 1;
            }
            Poll::Unknown | Poll::Pending => panic!("round {round}: lost ticket"),
        }
        // The witness plans between every culprit round: zero famine.
        let witness_id = match witness.submit(from, dest, &short_options(), "lot4").expect("submit") {
            Submit::Accepted { id, .. } => id,
            Submit::Refused { reason, .. } => panic!("witness refused round {round}: {reason}"),
        };
        match await_terminal(&witness, witness_id) {
            Poll::Ready(plan) => assert!(!plan.segments.is_empty(), "witness plans round {round}"),
            other => panic!("witness must plan round {round}, got {other:?}"),
        }
    }
    assert!(failures >= 4, "the loop replays N failures, got {failures}");
    let stats = service.stats();
    println!(
        "replan-storm: failures={failures} hits={} trunc={} joins={} miss={} refused_cap={}",
        stats.hits, stats.trunc_hits, stats.joins, stats.misses, stats.refused_cap,
    );
    // The sealed pocket is a shared negative now: replaying it computes nothing.
    let misses_before = stats.misses;
    let id = match culprit.submit(from, dest, &options, "lot4").expect("submit") {
        Submit::Accepted { id, .. } => id,
        Submit::Refused { reason, .. } => panic!("replay refused: {reason}"),
    };
    match await_terminal(&culprit, id) {
        Poll::Failed(no) => assert_eq!(no.reason, NoRouteReason::Disconnected),
        other => panic!("sealed pocket stays sealed, got {other:?}"),
    }
    assert_eq!(service.stats().misses, misses_before, "negative served from L1");
}

/// Queue-full is counted and observable: one exhaustive NoRoute pins the
/// single pool lane (~1.5 s budget kill), then 9 accounts x 2 submits over-
/// fill the 16-slot queue. Exactly the last 2 refuse queue-full (never a
/// block), the gauge shows depth 16, and the service heals after cancels.
#[test]
fn queue_full_refuses_and_counts() {
    let service = start_service(1);
    let options = short_options();
    let blocker = PlanClient::new(service.socket_path(), "wall-blocker");
    let block_from = Tile { x: 3219, z: 3218, level: 0 };
    let block_dest = Tile { x: 3215, z: 3218, level: 0 };
    let block_id = match blocker.submit(block_from, block_dest, &options, "lot4").expect("submit") {
        Submit::Accepted { id, .. } => id,
        Submit::Refused { reason, .. } => panic!("blocker refused: {reason}"),
    };
    // The lane is pinned past the budget kill; the flood below lands in ms.
    let mut flooded = vec![block_id];
    let mut refused_full = 0u32;
    for account in 0..9 {
        for nth in 0..2 {
            let client = PlanClient::new(service.socket_path(), &format!("wall-{account}"));
            let dest = Tile { x: 3200 + account * 10 + nth, z: 3200, level: 0 };
            match client.submit(block_from, dest, &options, "lot4").expect("submit") {
                Submit::Accepted { id, .. } => flooded.push(id),
                Submit::Refused { reason, .. } => {
                    assert!(reason.contains("queue-full"), "overflow refuses queue-full, got {reason}");
                    refused_full += 1;
                }
            }
        }
    }
    assert_eq!(flooded.len(), 17, "1 computing + 16 queued, never a block");
    assert_eq!(refused_full, 2, "only the overflow refuses");
    assert_eq!(service.queue_depth(), 16, "gauge shows the full queue");
    assert!(service.stats().refused_full >= 2, "queue-full refusals are counted");
    for id in flooded {
        let _ = blocker.cancel(id);
    }
    // Cancel frees the cap at once but the queue slots free as the pool
    // drops the dead jobs (sync_channel has no removal): wait the drain out.
    let start = std::time::Instant::now();
    while service.queue_depth() > 0 {
        assert!(start.elapsed() < Duration::from_secs(10), "pool drops cancelled jobs");
        std::thread::sleep(Duration::from_millis(1));
    }
    // Heals: a fast leg lands once the pool drops the cancelled jobs.
    let healer = PlanClient::new(service.socket_path(), "wall-healer");
    let (from, dest) = door_leg();
    let heal_id = match healer.submit(from, dest, &options, "lot4").expect("submit") {
        Submit::Accepted { id, .. } => id,
        Submit::Refused { reason, .. } => panic!("healer refused after drain: {reason}"),
    };
    match await_terminal(&healer, heal_id) {
        Poll::Ready(plan) => assert!(!plan.segments.is_empty(), "healthy after queue-full"),
        other => panic!("must heal, got {other:?}"),
    }
}

/// Cancel propagates: filling the cap then cancelling both tickets frees
/// the account (2 fresh submits accepted) and the dead tickets poll Unknown.
#[test]
fn cancel_releases_the_account_cap() {
    let service = start_service(1);
    let client = PlanClient::new(service.socket_path(), "cancel-cap");
    let (from, _) = door_leg();
    let mut ids = Vec::new();
    for nth in 0..ACCOUNT_CAP {
        let dest = Tile { x: 3300 + nth as i32, z: 3300, level: 0 };
        match client.submit(from, dest, &short_options(), "lot4").expect("submit") {
            Submit::Accepted { id, .. } => ids.push(id),
            Submit::Refused { reason, .. } => panic!("cap room expected, got {reason}"),
        }
    }
    assert_eq!(ids.len(), ACCOUNT_CAP, "cap fills");
    for id in &ids {
        client.cancel(*id).expect("cancel answers");
    }
    for id in ids {
        match client.poll(id).expect("poll").0 {
            Poll::Unknown => {}
            Poll::Pending => panic!("cancelled ticket {id} still pending"),
            terminal => panic!("cancelled ticket {id} answered {terminal:?}"),
        }
    }
    // Admission released: the account submits again, no wedge.
    let mut fresh = Vec::new();
    for nth in 0..ACCOUNT_CAP {
        let dest = Tile { x: 3400 + nth as i32, z: 3400, level: 0 };
        match client.submit(from, dest, &short_options(), "lot4").expect("submit") {
            Submit::Accepted { id, .. } => fresh.push(id),
            Submit::Refused { reason, .. } => panic!("cap must release after cancel, got {reason}"),
        }
    }
    for id in fresh {
        let _ = await_terminal(&client, id);
    }
    assert_eq!(service.queue_depth(), 0, "pool drains the test submits");
    assert_eq!(service.inflight_total(), 0, "nothing in flight once polled");
}

/// Every lot 4 gauge reads through one `stats` op: counters, queue, L1 age,
/// RSS, painted squares. Operator-readable, no log spam (the test reads the
/// socket op, exactly like `orchestrator nav-stats` does through the daemon).
#[test]
fn metrics_gauge_reads_counters_queue_and_ram() {
    let service = start_service(1);
    let client = PlanClient::new(service.socket_path(), "gauge");
    let (from, dest) = door_leg();
    let id = match client.submit(from, dest, &short_options(), "lot4").expect("submit") {
        Submit::Accepted { id, .. } => id,
        Submit::Refused { reason, .. } => panic!("refused: {reason}"),
    };
    match await_terminal(&client, id) {
        Poll::Ready(_) => {}
        other => panic!("must plan, got {other:?}"),
    }
    assert!(service.stats().misses >= 1, "one compute counted");
    assert!(service.l1_len() >= 1, "one plan cached");
    assert!(service.l1_bytes() > 0, "cached bytes counted");
    assert!(service.l1_oldest_age_ms().is_some(), "age reads on a warm cache");
    assert!(Service::rss_bytes() > 0, "RSS reads on Linux");
    assert!(service.painted_squares() > 0, "planning paints squares");
    assert_eq!(service.queue_depth(), 0, "idle service, empty queue");
    assert_eq!(service.inflight_total(), 0, "idle service, nothing in flight");
    println!(
        "gauge: l1_len={} l1_bytes={} age_ms={:?} rss={}B painted={} refused_cap/full/stale={}/{}/{}",
        service.l1_len(),
        service.l1_bytes(),
        service.l1_oldest_age_ms(),
        Service::rss_bytes(),
        service.painted_squares(),
        service.stats().refused_cap,
        service.stats().refused_full,
        service.stats().refused_stale,
    );
}
