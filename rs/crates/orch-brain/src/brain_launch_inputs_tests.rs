#![forbid(unsafe_code)]
//! Guard of the production launch inputs (2026-11-23 live bug: lot 3 added
//! `now`, `tick_ms`, `levels`, `market_up` and `filler_ticks` to
//! `LaunchInputs`, the production `gather` never set them, and only the
//! test harnesses did: every history stamp read 0, every market fill wait
//! expired at once, every level read 1). These tests call the PRODUCTION
//! readers on a live-shaped snapshot and fail on any plain-data field left
//! at its default.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lifescape_json::{object, Value};
use lifescape_planner::launch_apply::settle_market_buy;
use lifescape_planner::{LaunchInputs, Life, NodeKind, NodeStatus};

use crate::brain_launch_fx::{Fx, LiveFx};
use crate::fleet::Fleet;
use crate::jobs::Job;

/// A fleet that is never asked anything (the readers under test only read
/// the job).
struct IdleFleet {
    jobs: Mutex<HashMap<String, Arc<Job>>>,
}

impl Fleet for IdleFleet {
    fn jobs(&self) -> &Mutex<HashMap<String, Arc<Job>>> {
        &self.jobs
    }
    fn is_running(&self) -> bool {
        true
    }
    fn proc_alive(&self, _account: &str) -> bool {
        false
    }
    fn send_worker(&self, _account: &str, _request: &Value, _timeout: Duration) -> Result<Value, String> {
        Err("idle fleet".to_string())
    }
    fn run_job(&self, _account: &str, _script: &str, _args: &[Value]) -> Value {
        Value::Null
    }
    fn open_job(self: Arc<Self>, _account: &str, _no_login: bool) -> Value {
        Value::Null
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn now_wall() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
}

/// A live snapshot: every skill at a distinct level, a pack, a position.
fn live_snapshot() -> Value {
    let skills: Vec<Value> = (0u8..23).map(|stat| object(vec![("id", Value::Int(i64::from(stat))), ("xp", Value::Int(388)), ("level", Value::Int(i64::from(stat) + 5))])).collect();
    let coins = object(vec![("name", Value::Str("coins".to_string())), ("count", Value::Int(500))]);
    object(vec![
        ("skills", Value::Arr(skills)),
        ("inv", Value::Arr(vec![coins])),
        ("bank", Value::Arr(Vec::new())),
        ("worn", Value::Arr(Vec::new())),
        ("position", Value::Arr(vec![Value::Int(0), Value::Int(3222), Value::Int(3218)])),
        ("inv_streamed", Value::Bool(true)),
        ("bank_streamed", Value::Bool(true)),
    ])
}

fn probe_job(tag: &str) -> Arc<Job> {
    let job = Arc::new(Job::new(&format!("{}inputs_{tag}", crate::brain_commerce::PROBE_PREFIX)));
    job.inner.lock().expect("job").live = Some(live_snapshot());
    job
}

/// Every plain-data field the lot 3 harnesses set by hand must come out of
/// the production readers.
fn assert_plain_fields(inputs: &LaunchInputs, live: &Value) {
    let wall = now_wall();
    assert!((wall - inputs.now).abs() <= 5, "now {} is not the wall clock {wall}", inputs.now);
    assert!(inputs.tick_ms > 0, "tick_ms unset");
    assert_eq!(inputs.tick_ms, crate::market_eta::live_tick_ms());
    assert!(!inputs.levels.is_empty(), "levels empty: every skill would read level 1");
    assert_eq!(inputs.levels, crate::brain_launch_inputs::live_skill_levels(live));
    assert_eq!(inputs.levels.len(), 23);
    for (name, level) in &inputs.levels {
        assert_eq!(*level, crate::brain_succession::live_level(live, name), "{name}");
        assert_eq!(inputs.level_of(name), *level);
    }
    assert!(inputs.levels.iter().any(|(_, level)| *level > 1));
    assert_eq!(inputs.filler_ticks, crate::brain_filler::FILLER_TICKS);
    assert!(inputs.filler_ticks > 0);
    assert_eq!(inputs.market_up, crate::market_service::shared().is_some());
}

#[test]
fn gather_fills_every_plain_data_field() {
    let job = probe_job("gather");
    let live = live_snapshot();
    let inputs = crate::brain_launch_inputs::gather(&job, &live);
    assert_plain_fields(&inputs, &live);
    assert!(inputs.inv_streamed && inputs.bank_streamed);
    assert!(inputs.position.is_some());
}

#[test]
fn the_market_read_keeps_the_fields_and_reads_the_clock_again() {
    let job = probe_job("market");
    let live = live_snapshot();
    let mut inputs = crate::brain_launch_inputs::gather(&job, &live);
    inputs.now = 1;
    inputs.tick_ms = 0;
    crate::brain_launch_inputs::gather_market(&job, &mut inputs);
    assert_plain_fields(&inputs, &live);
}

#[test]
fn live_fx_refreshes_the_clock_before_each_decision() {
    let job = probe_job("fx");
    let live = live_snapshot();
    let orch: Arc<dyn Fleet> = Arc::new(IdleFleet { jobs: Mutex::new(HashMap::new()) });
    let mut fx = LiveFx::new(&orch, &job, &live);
    let mut inputs = LaunchInputs::default();
    assert_eq!(inputs.now, 0);
    fx.refresh_clock(&mut inputs);
    assert!((now_wall() - inputs.now).abs() <= 5);
    inputs.now = 0;
    fx.refresh_market(&mut inputs);
    assert!((now_wall() - inputs.now).abs() <= 5, "refresh_market left the clock stale");
}

/// A history entry written by the launch decision carries the wall clock,
/// and a market buy placed just now is neither expired nor settled on the
/// next pass (the live bug: `waited = now - 0`).
#[test]
fn a_placed_market_buy_is_stamped_now_and_not_expired() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lives");
    let mut life: Life = crate::brain_launch_equiv_tests::load_dir(&dir).into_iter().next().expect("fixture");
    life.history.clear();
    let mut node = lifescape_planner::Node::new("n9100", NodeKind::ObtainItems { item: "logs".to_string(), count: 5 }, Vec::new(), 5);
    node.status = NodeStatus::Ready;
    life.nodes.push(node);
    let position = life.nodes.len() - 1;
    let job = probe_job("buy");
    let live = live_snapshot();
    let orch: Arc<dyn Fleet> = Arc::new(IdleFleet { jobs: Mutex::new(HashMap::new()) });
    let mut fx = LiveFx::new(&orch, &job, &live);
    let mut inputs = crate::brain_launch_inputs::gather(&job, &live);
    fx.refresh_clock(&mut inputs);
    // What `launch_drive::finish` does with a placed buy.
    settle_market_buy(&mut life, position, 0, Ok(()), inputs.now);
    let entry = life.history.last().expect("history entry");
    assert_eq!(entry.outcome, "awaiting_fill");
    assert!(entry.started > 0 && entry.ended > 0, "unstamped history entry");
    assert!((now_wall() - entry.ended).abs() <= 5);
    // The next pass: the wait counts from the placement, not from 1970.
    let waited = crate::brain_launch::seconds_since_history(&life, "n9100", "awaiting_fill").expect("waiting");
    assert!((0..=5).contains(&waited), "waited {waited} s right after the placement");
    let deadline = (crate::brain_await::AWAIT_FILL_TICKS * crate::market_eta::live_tick_ms() / 1000) as i64;
    assert!(waited < deadline, "the fill wait expired at once ({waited} >= {deadline})");
    let mut probe = life.clone();
    let settled = crate::brain_await::settle_fill(&job, &mut probe, position, "logs", 5, waited);
    assert!(settled, "a fresh buy was settled as expired");
    assert!(probe.history.iter().all(|entry| entry.outcome != "failed" && entry.outcome != "abandoned"));
}
