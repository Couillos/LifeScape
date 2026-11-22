#![forbid(unsafe_code)]
//! XP / tick snapshot service, same mechanism as `hiscores_service`: one
//! background thread computes the `xp_rates` snapshot at most every 30
//! minutes (first run 240 s after the daemon start, or later when the
//! persisted snapshot is younger than 30 minutes), keeps it in memory as
//! ready-to-send bytes and persists it atomically to `run/xp.json`.
//! `/api/xp` only copies the cached bytes.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lifescape_json::{self as json, Value, object, str_value};

use crate::xp_rates::{self, INTERVAL_SECONDS, START_DELAY_SECONDS};

static CACHE: Mutex<Option<Arc<Vec<u8>>>> = Mutex::new(None);

pub fn snapshot_path() -> PathBuf {
    crate::paths::run_dir().join("xp.json")
}

fn now_secs() -> i64 {
    crate::jobs::now_secs() as i64
}

fn publish(snapshot: &Value) {
    let bytes = Arc::new(json::stringify(snapshot).into_bytes());
    *CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(bytes);
}

/// `/api/xp` body: the cached bytes, or a typed "pending" error.
pub fn api_body() -> Vec<u8> {
    let cached = CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    match cached {
        Some(bytes) => bytes.as_ref().clone(),
        None => json::stringify(&object(vec![
            ("ok", Value::Bool(false)),
            ("error", str_value("xp: first snapshot pending (computed 240 s after the daemon start)")),
        ]))
        .into_bytes(),
    }
}

/// When the first snapshot runs, like `hiscores_service::first_due`.
pub fn first_due(started: i64, persisted_at: Option<i64>) -> i64 {
    let delayed = started + START_DELAY_SECONDS;
    persisted_at.map_or(delayed, |at| delayed.max(at + INTERVAL_SECONDS))
}

/// Computes one snapshot from the store: hourly sums over the last two
/// weeks (chart, 24 h and 7 d periods and their previous ones). The "all"
/// period reads the same rows when the window holds the whole history,
/// else one more query folding the whole history into a single bucket.
pub fn compute(now: i64, next_at: i64) -> Result<Value, String> {
    let store = lifescape_store::Store::open(&crate::market_service::db_path()).map_err(|error| format!("xp: store open: {error}"))?;
    let since = xp_rates::since_clock(now);
    let rows = store.skill_buckets_since(since, xp_rates::BUCKET_SECONDS).map_err(|error| format!("xp: skill sums query: {error}"))?;
    let buckets = store.bucket_work_since(since, xp_rates::BUCKET_SECONDS).map_err(|error| format!("xp: work ticks query: {error}"))?;
    let oldest = store.oldest_work_clock().map_err(|error| format!("xp: oldest report query: {error}"))?.unwrap_or(now);
    let excluded = crate::economy_guard::excluded_accounts();
    if oldest >= since {
        let history = xp_rates::History { rows: &rows, buckets: &buckets, oldest_clock: oldest };
        return Ok(xp_rates::build(&rows, &buckets, &history, now, next_at, &excluded));
    }
    let all_rows = store.skill_buckets_since(0, i64::MAX).map_err(|error| format!("xp: all-time skill sums query: {error}"))?;
    let all_buckets = store.bucket_work_since(0, i64::MAX).map_err(|error| format!("xp: all-time work ticks query: {error}"))?;
    let history = xp_rates::History { rows: &all_rows, buckets: &all_buckets, oldest_clock: oldest };
    Ok(xp_rates::build(&rows, &buckets, &history, now, next_at, &excluded))
}

/// Computes, publishes and persists one snapshot; a failure is logged and
/// the previous snapshot keeps being served.
pub fn run_once(next_at: i64) {
    let started = std::time::Instant::now();
    match compute(now_secs(), next_at) {
        Ok(snapshot) => {
            publish(&snapshot);
            if let Err(error) = crate::paths::write_json(&snapshot_path(), &snapshot) {
                crate::logln!("xp: snapshot not persisted to {}: {error}", snapshot_path().display());
            }
            crate::logln!("xp: snapshot computed in {:.0} ms", started.elapsed().as_secs_f64() * 1000.0);
        }
        Err(error) => crate::logln!("{error}"),
    }
}

/// Starts the snapshot thread (daemon only): the persisted snapshot is
/// served at once, its `next_at` rewritten to the first due time.
pub fn start(orch: Arc<crate::daemon::Orchestrator>) {
    let started = now_secs();
    let persisted = crate::paths::read_json(&snapshot_path()).and_then(|mut snapshot| {
        let generated_at = snapshot.get("generated_at").as_i64()?;
        if let Value::Obj(map) = &mut snapshot {
            map.insert("next_at".to_string(), Value::Int(first_due(started, Some(generated_at))));
        }
        publish(&snapshot);
        Some(generated_at)
    });
    let mut due = first_due(started, persisted);
    std::thread::Builder::new()
        .name("xp".to_string())
        .spawn(move || loop {
            while now_secs() < due {
                if !orch.running.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_secs(10));
            }
            let next = now_secs() + INTERVAL_SECONDS;
            run_once(next);
            due = next;
        })
        .expect("xp thread spawns");
}
