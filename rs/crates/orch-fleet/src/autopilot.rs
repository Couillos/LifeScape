#![forbid(unsafe_code)]
//! What the daemon and the web server ask of the brain (layering step C,
//! 2026-11-24): one trait, implemented by `orch-brain` and registered by
//! the binary at startup (before any thread, like `log_sink`). The daemon
//! and the web crate name this trait, never the brain: a one-line edit in
//! a `brain_*.rs` file no longer recompiles them.
//!
//! Without a registration `current()` answers with `NoAutopilot`, which
//! does nothing (daemon and web tests that never need a brain); the binary
//! calls `assert_registered` so a missing registration is loud at startup.

use std::sync::{Arc, OnceLock};

use lifescape_json::Value;

use crate::fleet::Fleet;
use crate::jobs::Job;

/// The brain's entry points for the daemon and the web server.
pub trait Autopilot: Send + Sync {
    /// One supervisor visit of `job`: the report it just filed (if any) and
    /// the brain pass it triggers (`brain::on_supervise`).
    fn on_supervise(&self, fleet: &Arc<dyn Fleet>, job: &Arc<Job>, report: Option<Value>);
    /// A report was pushed on the heartbeat channel and folded by the
    /// daemon: wake the brain for it (`brain::on_report_push`).
    fn on_report_push(&self, fleet: &Arc<dyn Fleet>, job: &Arc<Job>) -> bool;
}

/// The answer while no brain is registered: nothing happens.
pub struct NoAutopilot;

impl Autopilot for NoAutopilot {
    fn on_supervise(&self, _fleet: &Arc<dyn Fleet>, _job: &Arc<Job>, _report: Option<Value>) {}

    fn on_report_push(&self, _fleet: &Arc<dyn Fleet>, _job: &Arc<Job>) -> bool {
        false
    }
}

static AUTOPILOT: OnceLock<Arc<dyn Autopilot>> = OnceLock::new();
static NONE: NoAutopilot = NoAutopilot;

/// Register the brain; the first registration wins, later ones are no-ops.
pub fn register(autopilot: Arc<dyn Autopilot>) {
    let _ = AUTOPILOT.set(autopilot);
}

pub fn is_registered() -> bool {
    AUTOPILOT.get().is_some()
}

/// Panics when no brain is registered (called by `main` once registered).
pub fn assert_registered() {
    assert!(is_registered(), "autopilot not registered: the brain would silently never run");
}

/// The registered brain, or `NoAutopilot`.
pub fn current() -> &'static dyn Autopilot {
    match AUTOPILOT.get() {
        Some(autopilot) => autopilot.as_ref(),
        None => &NONE,
    }
}
