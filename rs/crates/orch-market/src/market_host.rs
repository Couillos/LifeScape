#![forbid(unsafe_code)]
//! The daemon-side hooks of the market (live positions, worker pose, locks
//! push), written against `Fleet` so the market layer does not name the
//! daemon: an extension trait with a blanket impl, replacing the three
//! inherent `impl Orchestrator` blocks that lived in `market_audit` and
//! `market_convene` (layering step A5, 2026-11-23). Bodies unchanged.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;


use crate::fleet::Fleet;
use crate::jobs::Job;
use crate::market_audit::push_locks_with;
use crate::market_service::MarketService;

pub trait MarketHost: Fleet {
    /// Daemon side of the locks: one `set_locks` per live worker each round.
    fn push_locks(&self, service: &MarketService, accounts: &[String]);
    /// Live positions for the market convene loop: account to (own tick
    /// epoch, level, x, z). Ticks are per-login epochs, never comparable
    /// across bots: departs and deadlines ride each bot's own clock, the
    /// sweep rides the daemon wall. Only connected workers with a pushed
    /// snapshot appear, so offline bots never convene (their orders stay
    /// dormant, matches wait).
    fn market_live_positions(&self) -> HashMap<String, (u64, i32, i32, i32)>;
    /// Pose one market convocation on a worker: the `market deliver`
    /// script runs inline at the next safe point, one convocation at a
    /// time (a newer pose replaces the older). True when the worker acked.
    fn pose_market_script(&self, account: &str, args: &[String]) -> bool;
}

impl<T: Fleet + ?Sized> MarketHost for T {
    fn push_locks(&self, service: &MarketService, accounts: &[String]) {
        let failures = push_locks_with(service, accounts, &mut |account, request| {
            self.send_worker(account, request, Duration::from_secs(2)).map(|_| ()).map_err(String::from)
        });
        for (account, error) in failures {
            crate::logln!("market-locks: set_locks to {account} failed: {error}");
        }
    }

    fn market_live_positions(&self) -> HashMap<String, (u64, i32, i32, i32)> {
        // Never hold the jobs map while locking a job: a wedged job
        // would freeze every jobs.lock() taker (dispatch, ingest,
        // convene itself). Snapshot the Arcs, release, then read.
        // Contended inners keep their last position (skip this sweep).
        let jobs: Vec<Arc<Job>> = self.jobs().lock().unwrap().values().cloned().collect();
        let mut parties = HashMap::new();
        for job in &jobs {
            let Ok(inner) = job.inner.try_lock() else {
                continue;
            };
            let Some(live) = inner.live.as_ref() else {
                continue;
            };
            let ticks = live.get("ticks").as_i64().unwrap_or(0).max(0) as u64;
            let position = live.get("position").as_arr().cloned().unwrap_or_default();
            if position.len() < 3 {
                continue;
            }
            let (level, x, z) = (
                position[0].as_i64().unwrap_or(0) as i32,
                position[1].as_i64().unwrap_or(0) as i32,
                position[2].as_i64().unwrap_or(0) as i32,
            );
            parties.insert(job.account.clone(), (ticks, level, x, z));
        }
        parties
    }

    fn pose_market_script(&self, account: &str, args: &[String]) -> bool {
        let request = crate::worker_command::WorkerCommand::PoseMarket { script: "market".to_string(), args: args.to_vec() }.encode();
        match self.send_worker(account, &request, Duration::from_secs(10)) {
            Ok(reply) => reply.get("ok").as_bool() == Some(true),
            Err(_) => false,
        }
    }
}
