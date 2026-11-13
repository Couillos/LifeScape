#![forbid(unsafe_code)]
//! Market daemon plumbing (split from `market_service`, rule 4): the
//! process-wide service registry, the socket server and `start`.

use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, OnceLock};

use lifescape_json::{object, str_value, Value};

use crate::market_seed::catalogue;
use crate::market_service::{db_path, socket_path, MarketService};
use crate::jobs::Job;
use crate::market_convene::{convene_round, wall_now, ConvocationPose, CONVENE_EVERY_SECS};
use crate::market_eta::{EtaCache, TickMeter};
use crate::daemon::Orchestrator;
use std::time::Duration;

fn registry() -> &'static OnceLock<Arc<MarketService>> {
    static SERVICE: OnceLock<Arc<MarketService>> = OnceLock::new();
    &SERVICE
}

/// Daemon-side handle for the convene loop (same process, no socket).
pub fn shared() -> Option<Arc<MarketService>> {
    registry().get().cloned()
}

/// Handle one market request on the daemon service. Down service is a
/// loud error, never an empty book.
pub fn handle(request: &Value) -> Value {
    match registry().get() {
        Some(service) => service.handle(request),
        None => object(vec![
            ("ok", Value::Bool(false)),
            ("error", str_value("market service not running")),
        ]),
    }
}

/// Serve one worker connection: a single JSON-lines frame in, a single
/// frame out, then close (CLI shape, no thread per message past the
/// bounded pool).
fn serve_one(service: Arc<MarketService>, stream: UnixStream) {
        let request = match crate::daemon_pool::read_frame(&stream) {
        Some(frame) if frame.as_obj().is_some() => frame,
        _ => object(vec![]),
    };
    let reply = service.handle(&request);
    let mut buf = lifescape_json::stringify(&reply);
    buf.push('\n');
    let _ = (&stream).write_all(buf.as_bytes());
}

/// Starts the daemon market: catalogue from the dumps, store opened
/// here alone, socket served on the
/// bounded pool. Idempotent: a live socket refuses a second server.
pub fn start() -> Result<Arc<MarketService>, String> {
    let items = catalogue();
    let service = Arc::new(MarketService::open(&db_path(), items).map_err(|error| format!("market open: {error}"))?);
    // Handover ring around the fountain via the nav batch op (walkable,
    // no busts or stalls). Read-only nav use; a down service leaves the
    // ring empty and every deal falls back to the static ring.
    if let Some(snapshot) = crate::plan_service::snapshot() {
        let ring = crate::market_convene::build_ring_from_snapshot(&snapshot);
        service.lock_engine().ring = ring;
    }
    let sock = socket_path();
    if sock.exists() {
        if UnixStream::connect(&sock).is_ok() {
            return Err(format!("market already serving on {}", sock.display()));
        }
        let _ = std::fs::remove_file(&sock);
    }
    let listener = UnixListener::bind(&sock).map_err(|error| format!("bind {}: {error}", sock.display()))?;
    let serving = service.clone();
    std::thread::Builder::new()
        .name("market-accept".to_string())
        .spawn(move || {
            crate::daemon_pool::serve_pooled(
                listener,
                Arc::new(move |stream: UnixStream| serve_one(serving.clone(), stream)),
                Arc::new(|| true), // daemon-lifetime thread: the main accept loop ends the process
            );
        })
        .map_err(|error| error.to_string())?;
    let _ = registry().set(service.clone());
    Ok(service)
}

/// Daemon convene thread: every round sweeps expired deals, convokes
/// what waited out the grouping window, and poses one `market deliver`
/// per bot. Unposeable convokes retry next round; expired ones sweep.
pub fn start_convene_loop(orch: Arc<Orchestrator>) {
    std::thread::Builder::new()
        .name("market-convene".to_string())
        .spawn(move || {
            // The registry lands when the market starts (just above the
            // call site); wait a minute before giving up, never spin.
            let mut service = None;
            for _ in 0..12 {
                if let Some(running) = crate::market_service::shared() {
                    service = Some(running);
                    break;
                }
                std::thread::sleep(Duration::from_secs(5));
            }
            let Some(service) = service else {
                return;
            };
            let mut eta = EtaCache::new();
            let mut meter = TickMeter::new();
            let mut unposed: Vec<ConvocationPose> = Vec::new();
            // Audit every AUDIT_EVERY rounds (~60 s), first round included:
            // the books only ever show lots the account can hand over.
            const AUDIT_EVERY: u64 = 6;
            let mut round: u64 = 0;
            let save_dir = lifescape_sdk::save::default_save_dir();
            loop {
                std::thread::sleep(Duration::from_secs(CONVENE_EVERY_SECS));
                let parties = orch.market_live_positions();
                // Live tick rate from the freshest snapshot: session
                // ticks are per-login epochs but advance at one per
                // server tick, so wall-per-tick calibrates grouping and
                // sweep with no env switch.
                let wall = wall_now();
                let Some(truth) = parties.values().map(|party| party.0).max() else {
                    continue;
                };
                let tick_ms = meter.observe(wall, truth);
                let nav_up = crate::plan_service::stats_json().is_some();
                // Never hold the jobs map while locking a job (same
                // discipline as `market_live_positions`).
                let jobs: Vec<Arc<Job>> = orch.jobs.lock().unwrap().values().cloned().collect();
                service.sync_ledger_from_jobs(&jobs);
                if round % AUDIT_EVERY == 0 {
                    let cancelled = service.audit_round(&jobs, &save_dir);
                    if cancelled > 0 {
                        eprintln!("market-audit: {cancelled} uncovered order(s) cancelled");
                    }
                    // Then fold same-price fragments into one order each.
                    let merged = service.consolidate_round();
                    if merged > 0 {
                        eprintln!("market-merge: {merged} fragment(s) folded");
                    }
                }
                round += 1;
                // Locks every round: what open orders and deals promise
                // never leaves a bank outside the trade procedure.
                let accounts: Vec<String> = jobs.iter().map(|job| job.account.clone()).collect();
                orch.push_locks(&service, &accounts);
                crate::market_reprice::reprice_round(&service, wall, tick_ms as f64);
                for pose in convene_round(&service, nav_up, &parties, &mut eta, tick_ms) {
                    eprintln!("market-convene: pose {} {:?}", pose.account, pose.args);
                    unposed.push(pose);
                }
                unposed.retain(|pose| {
                    if orch.pose_market_script(&pose.account, &pose.args) {
                        false
                    } else {
                        true
                    }
                });
                if unposed.len() > 200 {
                    unposed.drain(..unposed.len() - 200);
                }
            }
        })
        .expect("market convene thread spawns");
}
