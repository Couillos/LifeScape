#![forbid(unsafe_code)]
//! The brain as the daemon and the web server see it (layering step C,
//! 2026-11-24): `BrainAutopilot` implements `fleet::autopilot::Autopilot`
//! and the binary registers it at startup (`install`), before any thread.
//! One method per entry point, each a plain delegation: no behaviour of
//! its own.

use std::sync::Arc;

use lifescape_json::Value;
use lifescape_orch_fleet::autopilot::{self, Autopilot};

use crate::fleet::Fleet;
use crate::jobs::Job;

pub struct BrainAutopilot;

/// Register the brain as the autopilot (first call wins).
pub fn install() {
    autopilot::register(Arc::new(BrainAutopilot));
}

impl Autopilot for BrainAutopilot {
    fn on_supervise(&self, fleet: &Arc<dyn Fleet>, job: &Arc<Job>, report: Option<Value>) {
        crate::brain::on_supervise(fleet, job, report);
    }

    fn on_report_push(&self, fleet: &Arc<dyn Fleet>, job: &Arc<Job>) -> bool {
        crate::brain::on_report_push(fleet, job)
    }
}
