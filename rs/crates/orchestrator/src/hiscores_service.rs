#![forbid(unsafe_code)]
//! Hiscores snapshot service: one background thread computes the
//! `hiscores` snapshot at most every 30 minutes (first run 240 s after
//! the daemon start, or later when the persisted snapshot is younger
//! than 30 minutes), keeps it in memory as ready-to-send bytes and
//! persists it atomically to `run/hiscores.json` (served at once after
//! a restart). `/api/hiscores` only copies the cached bytes.
//!
//! Sources, cheapest first: the pushed live snapshot of each daemon job
//! (only `skills`, `inv`, `bank`, `worn` are cloned, never the whole
//! frame) when fresher than 10 minutes, else the engine save file. A
//! live snapshot with an empty bank (bank not opened since login)
//! borrows the save's bank, like `holdings`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lifescape_json::{self as json, Value, object, str_value};

use crate::hiscores::{self, Holder, INTERVAL_SECONDS, LIFE_ACTIVE_SECONDS, SKILLS, START_DELAY_SECONDS};

/// A pushed snapshot older than this reads from the save file instead.
const LIVE_FRESH_SECONDS: f64 = 600.0;

static CACHE: Mutex<Option<Arc<Vec<u8>>>> = Mutex::new(None);

pub fn snapshot_path() -> PathBuf {
    crate::paths::run_dir().join("hiscores.json")
}

fn now_secs() -> i64 {
    crate::jobs::now_secs() as i64
}

/// Serialised snapshot currently served, None before the first one.
pub fn cached_body() -> Option<Arc<Vec<u8>>> {
    CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
}

pub fn publish(snapshot: &Value) {
    let bytes = Arc::new(json::stringify(snapshot).into_bytes());
    *CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(bytes);
}

/// `/api/hiscores` body: the cached bytes, or a typed "pending" error.
pub fn api_body() -> Vec<u8> {
    match cached_body() {
        Some(bytes) => bytes.as_ref().clone(),
        None => json::stringify(&object(vec![
            ("ok", Value::Bool(false)),
            ("error", str_value("hiscores: first snapshot pending (computed 240 s after the daemon start)")),
        ]))
        .into_bytes(),
    }
}

// -- player selection (pure) -------------------------------------------------

/// Excluded: staff/private/listed accounts and the `proof-*` test lives.
pub fn is_excluded(account: &str, excluded: &BTreeSet<String>) -> bool {
    let lower = account.to_lowercase();
    excluded.contains(&lower) || lower.starts_with("proof")
}

/// Players: daemon jobs plus lives written within `LIFE_ACTIVE_SECONDS`
/// (`lives` = (account, age in seconds)), deduplicated case-blind,
/// sorted, exclusions removed.
pub fn select_accounts(jobs: &[String], lives: &[(String, u64)], excluded: &BTreeSet<String>) -> Vec<String> {
    let mut chosen: BTreeMap<String, String> = BTreeMap::new();
    let active = lives.iter().filter(|(_, age)| *age < LIFE_ACTIVE_SECONDS).map(|(account, _)| account);
    for account in jobs.iter().chain(active) {
        if account.is_empty() || is_excluded(account, excluded) {
            continue;
        }
        chosen.entry(account.to_lowercase()).or_insert_with(|| account.clone());
    }
    chosen.into_values().collect()
}

// -- holders (pure) -----------------------------------------------------------

fn stacks(rows: &Value) -> Vec<(i64, i64)> {
    rows.as_arr()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| Some((item.get("id").as_i64()?, item.get("count").as_i64()?)))
                .filter(|(_, count)| *count > 0)
                .collect()
        })
        .unwrap_or_default()
}

/// Holder from a pushed snapshot (`skills` `{id, level, xp}` rows with
/// display XP, `inv`/`bank`/`worn` `{id, count}` rows). None without
/// skills (worker not logged in yet). An empty bank takes `save_bank`.
pub fn holder_from_live(account: &str, iron: bool, live: &Value, save_bank: Option<Vec<(i64, i64)>>) -> Option<Holder> {
    let rows = live.get("skills").as_arr()?;
    if rows.is_empty() {
        return None;
    }
    let mut xp = [0i64; 19];
    for row in rows {
        if let (Some(slot), Some(amount)) = (row.get("id").as_i64().and_then(hiscores::skill_slot), row.get("xp").as_i64()) {
            xp[slot] = amount.max(0);
        }
    }
    let mut items = stacks(live.get("inv"));
    items.extend(stacks(live.get("worn")));
    let bank = stacks(live.get("bank"));
    if bank.is_empty() {
        items.extend(save_bank.unwrap_or_default());
    } else {
        items.extend(bank);
    }
    Some(Holder { account: account.to_string(), iron, source: "live", xp, items })
}

fn save_stacks(bucket: &std::collections::HashMap<u16, u32>) -> Vec<(i64, i64)> {
    bucket.iter().map(|(obj, count)| (i64::from(*obj), i64::from(*count))).filter(|(_, count)| *count > 0).collect()
}

/// Holder from an engine save (server XP, 10x the display value).
pub fn holder_from_save(account: &str, iron: bool, save: &lifescape_sdk::save::Save) -> Holder {
    let mut xp = [0i64; 19];
    for (slot, (stat, _)) in SKILLS.iter().enumerate() {
        xp[slot] = save.stats.get(stat).map(|(_, server_xp)| i64::from(*server_xp).max(0) / 10).unwrap_or(0);
    }
    let mut items = save_stacks(&save.inv);
    items.extend(save_stacks(&save.worn));
    items.extend(save_stacks(&save.bank));
    Holder { account: account.to_string(), iron, source: "save", xp, items }
}

// -- gathering (daemon) -------------------------------------------------------

/// The four arrays a hiscore needs, cloned from a job's pushed snapshot
/// when it is fresh; never blocks on a busy job.
fn live_view(job: &crate::jobs::Job, now: f64) -> Option<Value> {
    let guard = job.inner.try_lock().ok()?;
    let live = guard.live.as_ref()?;
    let fresh = live.get("updated").as_f64().is_some_and(|at| now - at < LIVE_FRESH_SECONDS);
    if !fresh {
        return None;
    }
    Some(object(vec![
        ("skills", live.get("skills").clone()),
        ("inv", live.get("inv").clone()),
        ("bank", live.get("bank").clone()),
        ("worn", live.get("worn").clone()),
    ]))
}

/// (account, age in seconds) of every life file.
fn life_ages(dir: &Path) -> Vec<(String, u64)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let now = std::time::SystemTime::now();
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let account = name.strip_suffix(".json")?.to_string();
            let modified = entry.metadata().ok()?.modified().ok()?;
            let age = now.duration_since(modified).map(|span| span.as_secs()).unwrap_or(0);
            Some((account, age))
        })
        .collect()
}

fn is_iron(account: &str) -> bool {
    std::fs::read_to_string(crate::brain::life_path(account)).map(|text| crate::holdings::life_is_iron(&text)).unwrap_or(false)
}

/// Every player's holder; accounts without live data nor save are
/// reported in `errors`.
fn gather(orch: &crate::daemon::Orchestrator, errors: &mut Vec<String>) -> Vec<Holder> {
    let jobs: BTreeMap<String, Arc<crate::jobs::Job>> =
        orch.jobs.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).iter().map(|(name, job)| (name.clone(), job.clone())).collect();
    let job_names: Vec<String> = jobs.keys().cloned().collect();
    let accounts = select_accounts(&job_names, &life_ages(&crate::brain::lives_dir()), &crate::economy_guard::excluded_accounts());
    let now = crate::jobs::now_secs();
    let mut holders = Vec::new();
    for account in accounts {
        let iron = is_iron(&account);
        let live = jobs.get(&account).and_then(|job| live_view(job, now));
        let load = || lifescape_sdk::save::load_account(&account, None);
        // The save is read only when needed: no live snapshot, or a live
        // one whose bank was not opened since login.
        let mut save = None;
        if live.as_ref().is_some_and(|view| stacks(view.get("bank")).is_empty()) {
            save = load();
        }
        let from_live = live.as_ref().and_then(|view| holder_from_live(&account, iron, view, save.as_ref().map(|saved| save_stacks(&saved.bank))));
        let holder = match from_live {
            Some(holder) => Some(holder),
            None => save.or_else(load).map(|saved| holder_from_save(&account, iron, &saved)),
        };
        match holder {
            Some(holder) => holders.push(holder),
            None => errors.push(format!("{account}: no live snapshot and no save at {}", lifescape_sdk::save::account_path(&account, None))),
        }
    }
    holders
}

/// Market unit price: catalogue items that traded or rest in the book
/// take the current price (`MarketService::reference`, the bots' price);
/// anything else is worth 0.
fn market_price(service: &crate::market_service::MarketService, obj: i64) -> i64 {
    // One book per object: a banknote is priced as its object.
    let obj = crate::market_audit::book_item(obj);
    let quoted = {
        let engine = service.lock_engine();
        if engine.find_item(obj).is_none() {
            return 0;
        }
        engine.books.get(&obj).map(|book| book.best()).is_some_and(|(bid, ask)| bid.is_some() || ask.is_some())
    };
    let reference = service.reference(obj);
    if reference.traded || quoted {
        reference.price
    } else {
        0
    }
}

/// Computes, publishes and persists one snapshot; logs its cost and
/// adds it to the `cpu-profile pool` line (`hiscores.snapshot`).
pub fn run_once(orch: &crate::daemon::Orchestrator, next_at: i64) -> Value {
    let probe = crate::cpu_profile::Probe::start();
    let mut errors = Vec::new();
    let holders = gather(orch, &mut errors);
    let market = crate::market_daemon::shared();
    let mut price_of = |obj: i64| -> Option<i64> {
        let service = market.as_ref()?;
        let tradeable = lifescape_gamedata::obj_by_id(obj.max(0) as usize).is_some_and(|row| row.tradeable);
        Some(if tradeable { market_price(service, obj) } else { 0 })
    };
    let display_name = |account: &str| crate::jobs::humanize(account);
    let mut snapshot = hiscores::build(
        &holders,
        hiscores::Context {
            generated_at: now_secs(),
            next_at,
            price_of: &mut price_of,
            item_name: &crate::holdings::item_name,
            display_name: &display_name,
            errors,
        },
    );
    let (wall_ns, cpu_ns) = probe.elapsed();
    if let Value::Obj(map) = &mut snapshot {
        map.insert(
            "cost".to_string(),
            object(vec![
                ("cpu_ms", Value::Float((cpu_ns as f64 / 1e5).round() / 10.0)),
                ("wall_ms", Value::Float((wall_ns as f64 / 1e5).round() / 10.0)),
            ]),
        );
    }
    publish(&snapshot);
    if let Err(error) = crate::paths::write_json(&snapshot_path(), &snapshot) {
        crate::logln!("hiscores: snapshot not persisted to {}: {error}", snapshot_path().display());
    }
    let (wall_total, cpu_total) = probe.elapsed();
    crate::cpu_profile::pool().record("hiscores.snapshot", wall_total, cpu_total);
    crate::logln!(
        "hiscores: snapshot {} players, cpu {:.1} ms, wall {:.1} ms",
        holders.len(),
        cpu_total as f64 / 1e6,
        wall_total as f64 / 1e6
    );
    snapshot
}

/// When the first snapshot runs: `START_DELAY_SECONDS` after `started`,
/// never sooner than 30 minutes after the persisted one.
pub fn first_due(started: i64, persisted_at: Option<i64>) -> i64 {
    let delayed = started + START_DELAY_SECONDS;
    match persisted_at {
        Some(at) => delayed.max(at + INTERVAL_SECONDS),
        None => delayed,
    }
}

/// Loads the persisted snapshot (served at once, `next_at` rewritten to
/// `first_due`); returns its `generated_at`.
pub fn load_persisted(path: &Path, started: i64) -> Option<i64> {
    let mut snapshot = crate::paths::read_json(path)?;
    let generated_at = snapshot.get("generated_at").as_i64()?;
    if let Value::Obj(map) = &mut snapshot {
        map.insert("next_at".to_string(), Value::Int(first_due(started, Some(generated_at))));
    }
    publish(&snapshot);
    Some(generated_at)
}

/// Starts the snapshot thread (daemon only).
pub fn start(orch: Arc<crate::daemon::Orchestrator>) {
    let started = now_secs();
    let persisted = load_persisted(&snapshot_path(), started);
    let mut due = first_due(started, persisted);
    std::thread::Builder::new()
        .name("hiscores".to_string())
        .spawn(move || loop {
            while now_secs() < due {
                if !orch.running.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_secs(10));
            }
            let next = now_secs() + INTERVAL_SECONDS;
            run_once(&orch, next);
            due = next;
        })
        .expect("hiscores thread spawns");
}
