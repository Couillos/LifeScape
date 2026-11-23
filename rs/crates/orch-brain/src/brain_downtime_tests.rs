#![forbid(unsafe_code)]
//! The fill wait stops while the daemon is down (user 2026-11-23: 38 buys
//! expired at once after a 7 h stop).

use std::path::Path;
use std::sync::Arc;

use lifescape_planner::{Archetype, GoalKind, HistoryEntry, Life, Node, NodeKind, NodeStatus, Profile};

use crate::downtime::{self, Downtime};
use crate::jobs::Job;

const HOUR: i64 = 3600;

fn deadline() -> i64 {
    (crate::brain_await::AWAIT_FILL_TICKS * crate::market_eta::live_tick_ms() / 1000) as i64
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
}

fn entry(node: &str, outcome: &str, at: i64) -> HistoryEntry {
    HistoryEntry { node: node.to_string(), started: at, ended: at, outcome: outcome.to_string() }
}

/// A parked market buy of 5 logs placed at `placed`.
fn parked(placed: i64) -> Life {
    let mut node = Node::new("n9100", NodeKind::ObtainItems { item: "logs".to_string(), count: 5 }, Vec::new(), 5);
    node.status = NodeStatus::Ready;
    node.method = Some("market:buy".to_string());
    Life {
        schema: 1,
        account: "down_t".to_string(),
        created_at: 0,
        seed: 3,
        profile: Profile::new(false, 0.5, 0.5, 3, Archetype::Producer),
        goal: GoalKind::MakeMoney,
        nodes: vec![node],
        engagement: None,
        active: None,
        history: vec![entry("n9100", "awaiting_fill", placed)],
        ambition: None,
        goal_origin: None,
        memory: Default::default(),
    }
}

fn probe_job(tag: &str) -> Arc<Job> {
    Arc::new(Job::new(&format!("{}down_{tag}", crate::brain_commerce::PROBE_PREFIX)))
}

fn outcomes(life: &Life) -> Vec<String> {
    life.history.iter().map(|history| history.outcome.clone()).collect()
}

/// Buy placed, 7 h stop, restart: still waiting, with the pre-stop and
/// post-restart time only. The control (no downtime recorded) expires.
#[test]
fn a_buy_placed_before_a_seven_hour_stop_keeps_waiting() {
    let at = now();
    let pre = deadline() / 4;
    let post = 30;
    let placed = at - 7 * HOUR - pre - post;
    downtime::set_for_test(vec![(placed + pre, placed + pre + 7 * HOUR)]);
    assert_eq!(downtime::awake_secs(placed, at), pre + post);
    let job = probe_job("seven");
    let mut life = parked(placed);
    let (changed, waiting) = crate::brain_filler::settle_parked(&job, &mut life);
    assert!(!changed, "nothing moved: {:?}", outcomes(&life));
    assert_eq!(waiting, vec!["n9100".to_string()]);
    assert_eq!(outcomes(&life), vec!["awaiting_fill".to_string()]);
    // The running-node reader (production) agrees, and the remaining time is the deadline minus the awake wait.
    let waited = crate::brain_launch::seconds_since_history(&life, "n9100", "awaiting_fill").expect("waiting");
    assert_eq!(waited, pre + post);
    assert_eq!(deadline() - waited, deadline() - pre - post);
    // Control: the same life with no downtime recorded expires (the live bug).
    downtime::set_for_test(Vec::new());
    let mut control = parked(placed);
    let (changed, waiting) = crate::brain_filler::settle_parked(&job, &mut control);
    assert!(changed && waiting.is_empty(), "control must expire");
    assert!(outcomes(&control).iter().any(|outcome| outcome == "failed" || outcome == "abandoned"));
}

/// The wait counts only the up periods across two restarts, and expires
/// once their sum passes the deadline.
#[test]
fn a_wait_across_two_restarts_sums_the_up_periods() {
    let at = now();
    let d = deadline();
    let third = d / 3;
    let job = probe_job("two");
    // up `third`, down 5 h, up `third`, down 8 h, up `last`; total awake
    // = 2 * third + last, d - 10 (still waiting) then d + 10 (expired).
    for (awake_total, expired) in [(d - 10, false), (d + 10, true)] {
        let last = awake_total - 2 * third;
        let placed = at - (third + 5 * HOUR + third + 8 * HOUR + last);
        let (a, b) = (placed + third, placed + third + 5 * HOUR);
        let (c, e) = (b + third, b + third + 8 * HOUR);
        downtime::set_for_test(vec![(a, b), (c, e)]);
        assert_eq!(downtime::awake_secs(placed, at), awake_total);
        let mut life = parked(placed);
        let (changed, waiting) = crate::brain_filler::settle_parked(&job, &mut life);
        assert_eq!(changed, expired, "awake {awake_total} vs deadline {d}: {:?}", outcomes(&life));
        assert_eq!(waiting.len(), usize::from(!expired));
    }
    downtime::set_for_test(Vec::new());
}

/// Normal case: no downtime installed, the wait is the wall clock.
#[test]
fn without_downtime_the_wait_is_the_wall_clock() {
    downtime::set_for_test(Vec::new());
    let at = now();
    assert_eq!(downtime::awake_secs(at - 500, at), 500);
    let life = parked(at - 500);
    assert_eq!(crate::brain_launch::seconds_since_history(&life, "n9100", "awaiting_fill"), Some(500));
    let job = probe_job("plain");
    let mut fresh = parked(at - 10);
    assert_eq!(crate::brain_filler::settle_parked(&job, &mut fresh), (false, vec!["n9100".to_string()]));
    let mut old = parked(at - deadline() - 10);
    assert!(crate::brain_filler::settle_parked(&job, &mut old).0, "an expired wait still expires");
}

fn write_life(dir: &Path, account: &str, ended: &[i64]) {
    let mut life = parked(0);
    life.account = account.to_string();
    life.history = ended.iter().map(|&at| entry("n1", "launched", at)).collect();
    std::fs::write(dir.join(format!("{account}.json")), lifescape_json::stringify(&lifescape_planner::life_to_value(&life))).expect("write life");
}

/// No liveness file: the newest life stamp is the last alive. The buys
/// before it wait only their pre-stop time; the reverted run after it
/// (store rows only) is not credited.
#[test]
fn the_fallback_reads_the_newest_life_stamp() {
    let dir = std::env::temp_dir().join(format!("lifescape-downtime-lives-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let stop = 1_000_000;
    let restart = stop + 7 * HOUR + 3000;
    write_life(&dir, "a", &[stop - 5000, stop - 40]);
    write_life(&dir, "b", &[stop - 900, stop]);
    write_life(&dir, "future", &[restart + 10_000]);
    std::fs::write(dir.join("broken.json"), "{not json").expect("write");
    std::fs::write(dir.join("notes.txt"), "x").expect("write");
    assert_eq!(super::newest_life_stamp(&dir, restart), Some(stop), "future stamps and junk ignored");
    assert_eq!(super::newest_life_stamp(&dir.join("absent"), restart), None);
    let after = Downtime::default().restarted(super::newest_life_stamp(&dir, restart), restart);
    assert_eq!(after.intervals, vec![(stop, restart)]);
    // A buy placed 20 min before the stop, read 100 s after the restart.
    let placed = stop - 1200;
    assert_eq!(crate::downtime::awake_in(&after.intervals, placed, restart + 100), 1200 + 100);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Production inputs: the upkeep clock the brain builds carries the
/// installed downtime (not a test-only field).
#[test]
fn production_readers_see_the_installed_downtime() {
    use crate::brain_upkeep_fx::UpkeepFx;
    let at = now();
    downtime::set_for_test(vec![(at - 5000, at - 1000)]);
    let live = lifescape_json::Value::Null;
    let fx = crate::brain_upkeep_fx::LiveUpkeepFx::bare(&live);
    let clock = fx.clock();
    assert_eq!((clock.down)(at - 6000, at), 4000);
    assert_eq!(clock.awake_ticks_since(at - 6000), clock.ticks_since(at - 2000));
    downtime::set_for_test(Vec::new());
}
