#![forbid(unsafe_code)]
//! Method health registry (audit 2026-11-15, chantier 3: "registre de
//! santé des méthodes"). Live, whole families failed at every launch for
//! days and nobody saw it: milking 0/1694, tanning 0/56, the jewellery
//! furnace 0/38, the mill 1.3 %. Every folded report counts for its
//! planner method (`record`); a method under `SICK_RATE` over at least
//! `MIN_LAUNCHES` launches is quarantined: the planner never resolves it
//! (`ReadyCtx::quarantined`), the operator is told once (log + error
//! event), no silent substitute. A live `check-method` that works after
//! the quarantine (`run/method_checks.jsonl`) lifts it and clears the
//! counts: the fix is proven, not presumed.
//!
//! Counted: a run that ended its work (`ok`) or failed on its own
//! (`fail`, `contract`). A block on missing inputs is the plan's, not
//! the method's, and does not count.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use lifescape_json::{self as json, object, str_value, Value};

/// Launches before a method can be judged.
pub const MIN_LAUNCHES: u32 = 20;
/// Success share under which a judged method is quarantined.
pub const SICK_RATE: f64 = 0.10;
/// Distinct failing accounts before a method can be judged (plan P4,
/// live 2026-11-17: two hot-looping bots quarantined
/// `fight:melee/chickens` for the whole fleet).
pub const MIN_ACCOUNTS: usize = 3;

/// One method's counts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Health {
    pub ok: u32,
    pub fail: u32,
    /// Unix seconds of the quarantine, 0 = open.
    pub quarantined_at: i64,
    /// Last failure text seen (the operator's first clue).
    pub last_error: String,
    /// Accounts that failed the method, each with the shape of its last
    /// failure (digits aside; empty after one of its runs worked): an
    /// identical failure repeated by the same account counts once.
    pub accounts: BTreeMap<String, String>,
}

impl Health {
    pub fn launches(&self) -> u32 {
        self.ok + self.fail
    }

    /// Sick: judged (launches and distinct failing accounts) and under
    /// the success floor.
    pub fn sick(&self) -> bool {
        self.launches() >= MIN_LAUNCHES && self.accounts.len() >= MIN_ACCOUNTS && f64::from(self.ok) < SICK_RATE * f64::from(self.launches())
    }
}

/// One fold outcome for a method.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    Ok,
    Fail,
}

/// Failure text without its digits (positions, counts, ticks).
fn error_shape(error: &str) -> String {
    let shape: String = error.chars().filter(|glyph| !glyph.is_ascii_digit()).collect();
    let shape = shape.split_whitespace().collect::<Vec<_>>().join(" ");
    if shape.is_empty() {
        "?".to_string()
    } else {
        shape
    }
}

/// Pure update: counts `outcome` of `account`, quarantines a method that
/// turned sick at `now`. Returns true when this call quarantined it.
pub fn apply(table: &mut BTreeMap<String, Health>, account: &str, method: &str, outcome: Outcome, error: &str, now: i64) -> bool {
    let entry = table.entry(method.to_string()).or_default();
    match outcome {
        Outcome::Ok => {
            entry.ok = entry.ok.saturating_add(1);
            if let Some(last) = entry.accounts.get_mut(account) {
                last.clear();
            }
        }
        Outcome::Fail => {
            let shape = error_shape(error);
            if entry.accounts.get(account) == Some(&shape) {
                return false;
            }
            entry.accounts.insert(account.to_string(), shape);
            entry.fail = entry.fail.saturating_add(1);
            if !error.is_empty() {
                entry.last_error = error.chars().take(240).collect();
            }
        }
    }
    if entry.quarantined_at == 0 && entry.sick() {
        entry.quarantined_at = now;
        return true;
    }
    false
}

/// Pure lift: a method whose live check worked after its quarantine
/// starts afresh. Returns the methods lifted.
pub fn lift_proven(table: &mut BTreeMap<String, Health>, checks: &[(String, bool, i64)]) -> Vec<String> {
    let mut lifted = Vec::new();
    for (method, health) in table.iter_mut() {
        if health.quarantined_at == 0 {
            continue;
        }
        let proven = checks.iter().any(|(checked, worked, at)| checked == method && *worked && *at >= health.quarantined_at);
        if proven {
            *health = Health::default();
            lifted.push(method.clone());
        }
    }
    lifted
}

fn path() -> std::path::PathBuf {
    crate::paths::run_dir().join("method_health.json")
}

fn to_value(table: &BTreeMap<String, Health>) -> Value {
    object(vec![(
        "methods",
        Value::Obj(
            table
                .iter()
                .map(|(method, health)| {
                    (
                        method.clone(),
                        object(vec![
                            ("ok", Value::Int(i64::from(health.ok))),
                            ("fail", Value::Int(i64::from(health.fail))),
                            ("quarantined_at", Value::Int(health.quarantined_at)),
                            ("last_error", str_value(&health.last_error)),
                            ("accounts", Value::Obj(health.accounts.iter().map(|(account, shape)| (account.clone(), str_value(shape))).collect())),
                        ]),
                    )
                })
                .collect(),
        ),
    )])
}

fn from_value(value: &Value) -> BTreeMap<String, Health> {
    let mut out = BTreeMap::new();
    if let Some(methods) = value.get("methods").as_obj() {
        for (method, row) in methods.iter() {
            out.insert(
                method.clone(),
                Health {
                    ok: u32::try_from(row.get("ok").as_i64().unwrap_or(0)).unwrap_or(0),
                    fail: u32::try_from(row.get("fail").as_i64().unwrap_or(0)).unwrap_or(0),
                    quarantined_at: row.get("quarantined_at").as_i64().unwrap_or(0),
                    last_error: row.get("last_error").as_str().unwrap_or("").to_string(),
                    accounts: row
                        .get("accounts")
                        .as_obj()
                        .map(|accounts| accounts.iter().map(|(account, shape)| (account.clone(), shape.as_str().unwrap_or("").to_string())).collect())
                        .unwrap_or_default(),
                },
            );
        }
    }
    out
}

fn table() -> &'static Mutex<BTreeMap<String, Health>> {
    static TABLE: OnceLock<Mutex<BTreeMap<String, Health>>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let loaded = std::fs::read_to_string(path()).ok().and_then(|text| json::parse(&text).ok()).map(|value| from_value(&value)).unwrap_or_default();
        Mutex::new(loaded)
    })
}

fn save(table: &BTreeMap<String, Health>) {
    let file = path();
    let staging = file.with_extension("json.tmp");
    let written = std::fs::write(&staging, json::stringify_pretty(&to_value(table))).and_then(|_| std::fs::rename(&staging, &file));
    if let Err(error) = written {
        crate::logln!("method health: cannot save {}: {error}", file.display());
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
}

/// Counts one folded report of `method` for `account`.
pub fn record(account: &str, method: &str, outcome: Outcome, error: &str) {
    if method.is_empty() || crate::economy_guard::is_staff(account) {
        return;
    }
    let mut guard = table().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = now_secs();
    if apply(&mut guard, account, method, outcome, error, now) {
        let health = guard.get(method).cloned().unwrap_or_default();
        crate::logln!(
            "method health: {method} QUARANTINED ({}/{} ok over {} launches by {} accounts, under {:.0} %); last error: {}; re-tested on admin at the next deploy",
            health.ok,
            health.launches(),
            health.launches(),
            health.accounts.len(),
            SICK_RATE * 100.0,
            health.last_error
        );
    }
    save(&guard);
}

/// Quarantine stamp of every quarantined method (D1 re-test order).
pub fn quarantine_stamps() -> BTreeMap<String, i64> {
    let guard = table().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.iter().filter(|(_, health)| health.quarantined_at > 0).map(|(method, health)| (method.clone(), health.quarantined_at)).collect()
}

/// Live checks from `run/method_checks.jsonl`: `(method, worked, at)`.
pub fn live_checks() -> Vec<(String, bool, i64)> {
    checks()
}

fn checks() -> Vec<(String, bool, i64)> {
    let text = std::fs::read_to_string(crate::method_check::registry_path()).unwrap_or_default();
    text.lines()
        .filter_map(|line| json::parse(line).ok())
        .map(|row| (row.get("method").as_str().unwrap_or("").to_string(), row.get("worked").as_bool().unwrap_or(false), row.get("at").as_i64().unwrap_or(0)))
        .collect()
}

/// Quarantined methods now (proven fixes lifted first).
pub fn quarantined() -> Vec<String> {
    let mut guard = table().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.values().any(|health| health.quarantined_at > 0) {
        let lifted = lift_proven(&mut guard, &checks());
        if !lifted.is_empty() {
            crate::logln!("method health: quarantine lifted by a live check: {}", lifted.join(", "));
            save(&guard);
        }
    }
    guard.iter().filter(|(_, health)| health.quarantined_at > 0).map(|(method, _)| method.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::{apply, lift_proven, Health, Outcome, MIN_ACCOUNTS, MIN_LAUNCHES};

    /// Plan P4 red test: 21 failures of one bot (the same error, a hot
    /// loop) never quarantine for the fleet; three bots failing do.
    #[test]
    fn one_looping_bot_never_quarantines_a_method() {
        let mut table = BTreeMap::new();
        for launch in 0..21 {
            let error = format!("combat bank trip left the pack too full to hunt ({} free)", launch % 3);
            assert!(!apply(&mut table, "hill_eric", "fight:melee/chickens", Outcome::Fail, &error, launch), "one bot never judges");
        }
        assert_eq!(table["fight:melee/chickens"].fail, 1, "the same failure of the same bot counts once");
        for launch in 0..21 {
            let error = if launch % 2 == 0 { "no chickens near" } else { "pack too full" };
            assert!(!apply(&mut table, "hill_eric", "fight:melee/chickens", Outcome::Fail, error, launch));
        }
        assert!(table["fight:melee/chickens"].launches() >= 20, "a changed failure counts");
        assert!(!table["fight:melee/chickens"].sick(), "one account only");
        let mut fleet = BTreeMap::new();
        let mut quarantined = false;
        for launch in 0..MIN_LAUNCHES as i64 + 3 {
            let bot = ["a", "b", "c"][launch as usize % MIN_ACCOUNTS];
            let error = if launch % 2 == 0 { "no recipe" } else { "bad usage" };
            quarantined |= apply(&mut fleet, bot, "tan:cow_hide", Outcome::Fail, error, launch);
        }
        assert!(quarantined, "three bots failing every launch quarantine it");
    }
    use std::collections::BTreeMap;

    #[test]
    fn a_method_failing_every_launch_is_quarantined_once() {
        let mut table = BTreeMap::new();
        let bots = ["a", "b", "c", "d"];
        for launch in 0..MIN_LAUNCHES - 1 {
            let bot = bots[launch as usize % bots.len()];
            assert!(!apply(&mut table, bot, "milk:bucket_milk", Outcome::Fail, if launch % 8 < 4 { "no cows near" } else { "bucket lost" }, i64::from(launch)), "not judged before {MIN_LAUNCHES}");
        }
        assert!(apply(&mut table, "e", "milk:bucket_milk", Outcome::Fail, "no cows near", 100));
        assert!(!apply(&mut table, "f", "milk:bucket_milk", Outcome::Fail, "no cows near", 101), "told once");
        assert_eq!(table["milk:bucket_milk"].quarantined_at, 100);
        assert_eq!(table["milk:bucket_milk"].last_error, "no cows near");
    }

    #[test]
    fn a_healthy_method_stays_open() {
        let mut table = BTreeMap::new();
        for launch in 0..40 {
            let outcome = if launch % 3 == 0 { Outcome::Ok } else { Outcome::Fail };
            assert!(!apply(&mut table, &format!("bot{}", launch % 5), "farm:coal", outcome, &format!("contention {launch}"), launch));
        }
        assert!(!table["farm:coal"].sick(), "a third succeeds");
    }

    #[test]
    fn a_live_check_after_the_quarantine_lifts_it() {
        let mut table = BTreeMap::new();
        table.insert("tan:cow_hide".to_string(), Health { ok: 0, fail: 56, quarantined_at: 500, ..Health::default() });
        assert!(lift_proven(&mut table, &[("tan:cow_hide".to_string(), true, 400)]).is_empty(), "a check from before proves nothing");
        assert!(lift_proven(&mut table, &[("tan:cow_hide".to_string(), false, 600)]).is_empty(), "a failed check proves nothing");
        assert_eq!(lift_proven(&mut table, &[("tan:cow_hide".to_string(), true, 600)]), vec!["tan:cow_hide".to_string()]);
        assert_eq!(table["tan:cow_hide"], Health::default());
    }
}
