#![forbid(unsafe_code)]
//! Yield estimator (plan rendements R2): one bucket per D2 key (planner
//! method, combat style, site, tool tier, band of 5 levels of the
//! method's skill), fed by contract-2 reports only.
//!
//! A bucket holds decayed sums, never samples (bounded memory): the
//! signed flows by (item, cause), the XP-bearing unit counters, the work
//! and pause ticks, the run counters. Estimate = Σ quantities / Σ work
//! ticks × 6000. Decay (D7): before a run of `t` work ticks lands, every
//! sum is multiplied by 0.5^(t / `HALF_LIFE_WORK_TICKS`): the bucket
//! forgets by its own cumulated work, not by wall time. A run shorter
//! than `SHORT_RUN_TICKS` weighs `t / SHORT_RUN_TICKS`. Failed and
//! blocked runs never enter a rate: they count in the failure rate.
//!
//! Each run also lands in two roll-ups of its key: all sites (`site =
//! "*"`) and all sites and tools (`tool = "*"`), so a read that falls
//! back (no site known, a thin bucket) reads an aggregate of the same
//! decay, never a figure made up. The number of buckets is bounded
//! (`MAX_BUCKETS`): the least recently fed exact bucket goes first.

use std::collections::BTreeMap;

/// Work ticks after which a run weighs half (D7, to calibrate live).
pub const HALF_LIFE_WORK_TICKS: f64 = 60_000.0;
/// Runs shorter than this weigh `ticks / SHORT_RUN_TICKS` (plan §6).
pub const SHORT_RUN_TICKS: f64 = 300.0;
/// Most buckets kept (exact keys and roll-ups together).
pub const MAX_BUCKETS: usize = 20_000;
/// Site or tool of a roll-up key.
pub const ANY: &str = "*";

/// D2 key of a bucket. `band` is the first level of the band of the
/// method's skill (1, 6, 11...), 0 for a method no skill gates.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct YieldKey {
    pub method: String,
    pub style: String,
    pub site: String,
    pub tool: String,
    pub band: i64,
}

impl YieldKey {
    /// The all-sites roll-up of this key.
    pub fn any_site(&self) -> YieldKey {
        YieldKey { site: ANY.to_string(), ..self.clone() }
    }

    /// The all-sites, all-tools roll-up of this key.
    pub fn any_site_tool(&self) -> YieldKey {
        YieldKey { site: ANY.to_string(), tool: ANY.to_string(), ..self.clone() }
    }

    pub fn is_rollup(&self) -> bool {
        self.site == ANY || self.tool == ANY
    }
}

/// One accepted run (`rates_measure`): its work, flows and units.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RunSample {
    pub work_ticks: i64,
    pub pause_ticks: i64,
    /// (display item, cause, signed quantity).
    pub flows: Vec<(String, String, i64)>,
    /// (unit kind, subject, quantity).
    pub units: Vec<(String, String, i64)>,
    /// Wall clock of the ingest (seconds).
    pub clock: i64,
}

/// Decayed sums of one key.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Bucket {
    /// Accepted runs (raw count: the confidence's n).
    pub runs: u64,
    /// Σ run weights (decayed).
    pub weight: f64,
    /// Σ weight × work ticks (decayed): the estimator's denominator.
    pub work_ticks: f64,
    /// Σ weight × pause ticks (decayed).
    pub pause_ticks: f64,
    /// (display item, cause) -> Σ weight × quantity (decayed).
    pub flows: BTreeMap<(String, String), f64>,
    /// (unit kind, subject) -> Σ weight × quantity (decayed).
    pub units: BTreeMap<(String, String), f64>,
    /// Σ weight × r and Σ weight × r² of the run's carrier rate (units
    /// of every kind per 6000): the dispersion of the confidence.
    pub rate_sum: f64,
    pub rate_squares: f64,
    /// Decayed counts of finished runs and of failed/blocked ones.
    pub finished: f64,
    pub failed: f64,
    /// Samples rejected above the physical ceiling (raw count).
    pub rejected: u64,
    /// Wall clock of the last accepted run (seconds).
    pub last_clock: i64,
}

/// Weight of a run of `work_ticks`.
pub fn run_weight(work_ticks: i64) -> f64 {
    (work_ticks.max(0) as f64 / SHORT_RUN_TICKS).min(1.0)
}

/// Decay factor of `work_ticks` new ticks of work.
pub fn decay_factor(work_ticks: i64) -> f64 {
    0.5_f64.powf(work_ticks.max(0) as f64 / HALF_LIFE_WORK_TICKS)
}

impl Bucket {
    fn decay(&mut self, factor: f64) {
        self.weight *= factor;
        self.work_ticks *= factor;
        self.pause_ticks *= factor;
        self.rate_sum *= factor;
        self.rate_squares *= factor;
        self.flows.values_mut().for_each(|sum| *sum *= factor);
        self.units.values_mut().for_each(|sum| *sum *= factor);
    }

    /// Folds one accepted run in (decay first, then the weighted run).
    pub fn add(&mut self, sample: &RunSample) {
        if sample.work_ticks <= 0 {
            return;
        }
        let factor = decay_factor(sample.work_ticks);
        self.decay(factor);
        self.finished = self.finished * factor + 1.0;
        self.failed *= factor;
        let weight = run_weight(sample.work_ticks);
        self.runs += 1;
        self.weight += weight;
        self.work_ticks += weight * sample.work_ticks as f64;
        self.pause_ticks += weight * sample.pause_ticks.max(0) as f64;
        for (item, cause, qty) in &sample.flows {
            *self.flows.entry((item.clone(), cause.clone())).or_insert(0.0) += weight * *qty as f64;
        }
        let mut carried = 0.0;
        for (kind, subject, qty) in &sample.units {
            *self.units.entry((kind.clone(), subject.clone())).or_insert(0.0) += weight * *qty as f64;
            carried += *qty as f64;
        }
        let rate = carried * crate::rates_parse::NORMALIZE_TICKS / sample.work_ticks as f64;
        self.rate_sum += weight * rate;
        self.rate_squares += weight * rate * rate;
        self.last_clock = self.last_clock.max(sample.clock);
    }

    /// A failed or blocked run: never a rate, one more failure.
    pub fn add_failure(&mut self, work_ticks: i64) {
        let factor = decay_factor(work_ticks);
        self.finished = self.finished * factor + 1.0;
        self.failed = self.failed * factor + 1.0;
    }

    /// Σ / Σ per 6000 work ticks of every (item, cause).
    pub fn flow_rates(&self) -> Vec<(String, String, f64)> {
        if self.work_ticks <= 0.0 {
            return Vec::new();
        }
        let scale = crate::rates_parse::NORMALIZE_TICKS / self.work_ticks;
        self.flows.iter().map(|((item, cause), sum)| (item.clone(), cause.clone(), sum * scale)).collect()
    }

    /// Σ / Σ per 6000 work ticks of every (kind, subject).
    pub fn unit_rates(&self) -> Vec<(String, String, f64)> {
        if self.work_ticks <= 0.0 {
            return Vec::new();
        }
        let scale = crate::rates_parse::NORMALIZE_TICKS / self.work_ticks;
        self.units.iter().map(|((kind, subject), sum)| (kind.clone(), subject.clone(), sum * scale)).collect()
    }

    /// Weighted standard deviation of the run carrier rate.
    pub fn dispersion(&self) -> f64 {
        if self.weight <= 0.0 {
            return 0.0;
        }
        let mean = self.rate_sum / self.weight;
        (self.rate_squares / self.weight - mean * mean).max(0.0).sqrt()
    }

    /// Share of recent runs that failed or blocked.
    pub fn failure_rate(&self) -> f64 {
        if self.finished <= 0.0 { 0.0 } else { self.failed / self.finished }
    }

    /// Pause ticks per 6000 work ticks.
    pub fn pause_per_6000(&self) -> f64 {
        if self.work_ticks <= 0.0 { 0.0 } else { self.pause_ticks * crate::rates_parse::NORMALIZE_TICKS / self.work_ticks }
    }
}

/// Every bucket, exact keys and roll-ups.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct YieldBook {
    pub buckets: BTreeMap<YieldKey, Bucket>,
}

impl YieldBook {
    /// One accepted run under `key` and its two roll-ups.
    pub fn record(&mut self, key: &YieldKey, sample: &RunSample) {
        for target in [key.clone(), key.any_site(), key.any_site_tool()] {
            self.buckets.entry(target).or_default().add(sample);
        }
        self.bound();
    }

    /// One failed or blocked run under `key` and its roll-ups.
    pub fn record_failure(&mut self, key: &YieldKey, work_ticks: i64) {
        for target in [key.clone(), key.any_site(), key.any_site_tool()] {
            self.buckets.entry(target).or_default().add_failure(work_ticks);
        }
        self.bound();
    }

    /// One sample rejected above the ceiling (counted, never rated).
    pub fn record_rejected(&mut self, key: &YieldKey) {
        self.buckets.entry(key.clone()).or_default().rejected += 1;
        self.bound();
    }

    pub fn get(&self, key: &YieldKey) -> Option<&Bucket> {
        self.buckets.get(key)
    }

    /// Drops the least recently fed buckets beyond `MAX_BUCKETS`.
    fn bound(&mut self) {
        if self.buckets.len() <= MAX_BUCKETS {
            return;
        }
        // One sorted pass down to 90 %: least recently fed first, an
        // exact key before the roll-ups of the same age.
        let mut ages: Vec<(i64, bool, YieldKey)> = self.buckets.iter().map(|(key, bucket)| (bucket.last_clock, key.is_rollup(), key.clone())).collect();
        ages.sort();
        let excess = self.buckets.len() - MAX_BUCKETS * 9 / 10;
        for (_, _, key) in ages.into_iter().take(excess) {
            self.buckets.remove(&key);
        }
    }
}

#[cfg(test)]
#[path = "rates_estimator_tests.rs"]
mod tests;
