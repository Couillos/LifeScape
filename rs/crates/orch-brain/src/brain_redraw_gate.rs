#![forbid(unsafe_code)]
//! The redraw of a closed goal, retried only when its inputs changed (lot
//! 3.3 of the architecture plan; analysis 1.3: `succession.redraw` 1 941
//! calls, 2 091 s of CPU, retried on every pass while nothing is drawable).
//!
//! `brain_succession::redraw` is a function of the life (history, goal,
//! ambition), the live levels and stacks, the bans and cooldowns (which
//! expire with the clock), the market (books, executions), the guides and
//! the rates, and the fleet (levels, holdings point, seats, claims). The
//! fingerprint (`Fingerprint`) hashes each of those six families. A retry
//! whose fingerprint equals the fingerprint of the last attempt that drew
//! nothing, made less than `MAX_SKIP_SECS` ago, could only draw nothing
//! again: it is the one the gate skips. The age bound covers any input the
//! fingerprint does not name (the time-value measure, the fleet's piles):
//! no state is trusted for more than two minutes.
//!
//! Three modes, `LIFESCAPE_REDRAW_GATE` (read once at the first redraw):
//! - `shadow` (the default): the old code decides, the gate computes
//!   beside it and counts what it WOULD have skipped. A skip the old code
//!   answered with a drawn goal or a changed life is a DIVERGENCE: logged
//!   with its fingerprint and counted. Switch to `on` only after a live
//!   hour with zero divergences.
//! - `on`: a skippable retry returns `(None, false)` without drawing.
//! - `off`: no gate, no fingerprint (the code as before).
//!
//! A summary line (`redraw-gate: ...`) goes to the daemon log every ten
//! minutes in `shadow` and `on` mode.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::Hasher;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use lifescape_json::Value;
use lifescape_planner::Life;

/// A recorded attempt is trusted this long (wall seconds).
pub(crate) const MAX_SKIP_SECS: i64 = 120;

/// Seconds between two summary lines.
const SUMMARY_EVERY_SECS: i64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Off,
    Shadow,
    On,
}

impl From<crate::config::RedrawGate> for Mode {
    fn from(gate: crate::config::RedrawGate) -> Mode {
        match gate {
            crate::config::RedrawGate::Off => Mode::Off,
            crate::config::RedrawGate::Shadow => Mode::Shadow,
            crate::config::RedrawGate::On => Mode::On,
        }
    }
}

impl Mode {
    /// `on` / `off`, anything else (and unset) is `shadow`
    /// (`config::RedrawGate::parse`).
    #[cfg(test)]
    pub(crate) fn parse(text: &str) -> Mode {
        Mode::from(crate::config::RedrawGate::parse(text))
    }

    fn label(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Shadow => "shadow",
            Mode::On => "on",
        }
    }
}

static MODE: OnceLock<Mode> = OnceLock::new();

/// Sets the mode of this process (`main`, from `Config::redraw_gate`,
/// `LIFESCAPE_REDRAW_GATE`); the first call wins.
pub fn install(gate: crate::config::RedrawGate) {
    let _ = MODE.set(Mode::from(gate));
}

/// The mode of this process: what `install` set, `shadow` before.
pub(crate) fn mode() -> Mode {
    *MODE.get_or_init(|| Mode::Shadow)
}

/// What a redraw reads, one hash per family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fingerprint {
    pub life: u64,
    pub bans: u64,
    pub live: u64,
    pub market: u64,
    pub guides: u64,
    pub fleet: u64,
}

fn digest(feed: impl FnOnce(&mut DefaultHasher)) -> u64 {
    let mut hasher = DefaultHasher::new();
    feed(&mut hasher);
    hasher.finish()
}

fn write_debug(hasher: &mut DefaultHasher, value: &dyn std::fmt::Debug) {
    hasher.write(format!("{value:?}").as_bytes());
    hasher.write_u8(0xFF);
}

fn sorted<K: Ord + Clone, V: Clone>(map: &HashMap<K, V>) -> Vec<(K, V)> {
    let mut rows: Vec<(K, V)> = map.iter().map(|(key, value)| (key.clone(), value.clone())).collect();
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    rows
}

/// Wall seconds now.
fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
}

/// The fingerprint of the inputs of `redraw(life, live)` now.
pub(crate) fn fingerprint(life: &Life, live: &Value, deps: &crate::brain_deps::BrainDeps) -> Fingerprint {
    let now = now_secs();
    let ticks_since = |stamp: i64| crate::brain_stuck::seconds_to_ticks(now - stamp);
    // The life: the goal and the ambition the draw reads, the history it
    // appends to (its length and tail), the graph it closes over.
    let life_print = digest(|hasher| {
        write_debug(hasher, &life.goal);
        write_debug(hasher, &life.goal_origin);
        write_debug(hasher, &life.ambition);
        write_debug(hasher, &life.memory);
        write_debug(hasher, &life.profile.ironman);
        write_debug(hasher, &life.profile.archetype);
        hasher.write_usize(life.history.len());
        for entry in life.history.iter().rev().take(3) {
            write_debug(hasher, entry);
        }
        write_debug(hasher, &life.active);
        for node in &life.nodes {
            write_debug(hasher, &(&node.id, &node.status));
        }
    });
    // Bans and cooldowns expire with the clock: their states, not their
    // stamps, so the print moves exactly when one runs out.
    let bans = digest(|hasher| {
        write_debug(hasher, &lifescape_planner::bans::ban_states(&life.history, &ticks_since));
        write_debug(hasher, &lifescape_planner::cooldown::cooled_keys(&life.history, &ticks_since));
        write_debug(hasher, &crate::brain_cool::active(life));
        write_debug(hasher, &crate::method_health::quarantined());
    });
    // The bot's own state: live levels, what it holds, what its listings
    // lock, the quests it finished.
    let live_print = digest(|hasher| {
        let levels: Vec<u8> = (0u8..23).map(|stat| crate::brain_succession::live_level(live, lifescape_sdk::provenance::skill_name(stat))).collect();
        write_debug(hasher, &levels);
        let mut stacks = crate::brain_plan::stacks_of(live);
        stacks.sort();
        write_debug(hasher, &stacks);
        write_debug(hasher, &crate::brain_locks::locked_items_in(deps.market.as_deref(), &life.account));
        write_debug(hasher, &crate::brain_rescue::quests_done(life));
    });
    // The market: the books' generation, the execution log, the service.
    let market = digest(|hasher| match deps.market.clone() {
        Some(service) => {
            hasher.write_u64(service.instance_id());
            hasher.write_u64(crate::brain_market_view::books_generation(&service));
            hasher.write_i64(service.read_store().executions_stamp().unwrap_or(-1));
        }
        None => hasher.write_u8(0),
    });
    // The guides and the rates the draw prices with.
    let guides = digest(|hasher| {
        hasher.write_i64(deps.guides.clone().map_or(-1, |set| set.built_unix_ms));
        hasher.write_u64(crate::rates_service::generation());
    });
    // The fleet: the best level of each skill, the last holdings point
    // (the surplus), the seats and the claims of the others, the measured
    // time value.
    let fleet = digest(|hasher| {
        let best: Vec<i64> = (0u8..23).map(|stat| crate::brain_gear_fleet::best_now(lifescape_sdk::provenance::skill_name(stat))).collect();
        write_debug(hasher, &best);
        if let Some(service) = deps.market.clone() {
            hasher.write_i64(service.read_store().holdings_stamp().unwrap_or(-1));
        }
        write_debug(hasher, &deps.crowd.others(&life.account));
        write_debug(hasher, &deps.crowd.seats(&life.account));
        write_debug(hasher, &sorted(&crate::market_claims::claimed_by_others(&life.account)));
        write_debug(hasher, &sorted(&crate::market_claims::purchases_by_others(&life.account)));
        hasher.write_usize(crate::time_value::current().map_or(0, |values| std::sync::Arc::as_ptr(&values) as usize));
    });
    Fingerprint { life: life_print, bans, live: live_print, market, guides, fleet }
}

/// The last attempt of an account that drew nothing: the fingerprint of
/// the state it left, and when.
struct Seen {
    print: Fingerprint,
    at: i64,
}

/// What the gate saw.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    /// Retries the fingerprint did not allow to skip.
    pub retried: u64,
    /// Skippable retries the old code answered with nothing at all.
    pub skip_agreed: u64,
    /// Skippable retries the old code answered with a drawn goal: the
    /// DIVERGENCE the shadow hour must show none of.
    pub skip_drew: u64,
    /// Skippable retries the old code answered with a changed life (no goal).
    pub skip_changed: u64,
    /// Retries skipped for real (`on`).
    pub skipped: u64,
}

/// The attempts of every account and the counters (one per process; the
/// tests build their own).
#[derive(Default)]
pub(crate) struct Gate {
    table: Mutex<HashMap<String, Seen>>,
    retried: AtomicU64,
    agreed: AtomicU64,
    drew: AtomicU64,
    changed: AtomicU64,
    skipped: AtomicU64,
    last_summary: Mutex<i64>,
}

fn global() -> &'static Gate {
    static GATE: OnceLock<Gate> = OnceLock::new();
    GATE.get_or_init(Gate::default)
}

/// `brain_succession::redraw` behind the gate, in this process's mode.
pub(crate) fn redraw(life: &mut Life, live: &Value, old: impl FnOnce(&mut Life, &Value, &crate::brain_deps::BrainDeps) -> (Option<String>, bool), deps: &crate::brain_deps::BrainDeps) -> (Option<String>, bool) {
    global().redraw_in(mode(), now_secs(), life, live, old, deps)
}

impl Gate {
    pub(crate) fn counts(&self) -> Counts {
        Counts {
            retried: self.retried.load(Ordering::Relaxed),
            skip_agreed: self.agreed.load(Ordering::Relaxed),
            skip_drew: self.drew.load(Ordering::Relaxed),
            skip_changed: self.changed.load(Ordering::Relaxed),
            skipped: self.skipped.load(Ordering::Relaxed),
        }
    }

    fn maybe_summary(&self, now: i64, mode: Mode) {
        let mut last = self.last_summary.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if now - *last < SUMMARY_EVERY_SECS {
            return;
        }
        let first = *last == 0;
        *last = now;
        if first {
            return;
        }
        let seen = self.counts();
        crate::logln!(
            "redraw-gate: mode={} retried={} would_skip_agreed={} would_skip_drew={} would_skip_changed={} skipped={}",
            mode.label(),
            seen.retried,
            seen.skip_agreed,
            seen.skip_drew,
            seen.skip_changed,
            seen.skipped
        );
    }

    /// `redraw` in a given mode at a given wall clock (the tests').
    pub(crate) fn redraw_in(&self, mode: Mode, now: i64, life: &mut Life, live: &Value, old: impl FnOnce(&mut Life, &Value, &crate::brain_deps::BrainDeps) -> (Option<String>, bool), deps: &crate::brain_deps::BrainDeps) -> (Option<String>, bool) {
        if mode == Mode::Off {
            return old(life, live, deps);
        }
        let before = fingerprint(life, live, deps);
        let skippable = self.table.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&life.account).is_some_and(|seen| seen.print == before && now >= seen.at && now - seen.at < MAX_SKIP_SECS);
        if skippable && mode == Mode::On {
            self.skipped.fetch_add(1, Ordering::Relaxed);
            self.maybe_summary(now, mode);
            return (None, false);
        }
        let (drawn, changed) = old(life, live, deps);
        if !skippable {
            self.retried.fetch_add(1, Ordering::Relaxed);
        } else if drawn.is_some() || changed {
            let counter = if drawn.is_some() { &self.drew } else { &self.changed };
            counter.fetch_add(1, Ordering::Relaxed);
            crate::logln!(
                "redraw-gate: DIVERGENCE {}: the fingerprint was unchanged ({before:?}) but the old redraw {}",
                life.account,
                match &drawn {
                    Some(label) => format!("drew {label}"),
                    None => "changed the life".to_string(),
                }
            );
        } else {
            self.agreed.fetch_add(1, Ordering::Relaxed);
        }
        // An attempt that drew nothing is remembered under the state it left.
        let after = (drawn.is_none()).then(|| fingerprint(life, live, deps));
        let mut seen = self.table.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match after {
            Some(print) => {
                seen.insert(life.account.clone(), Seen { print, at: now });
            }
            None => {
                seen.remove(&life.account);
            }
        }
        drop(seen);
        self.maybe_summary(now, mode);
        (drawn, changed)
    }
}

#[cfg(test)]
#[path = "brain_redraw_gate_tests.rs"]
mod tests;
