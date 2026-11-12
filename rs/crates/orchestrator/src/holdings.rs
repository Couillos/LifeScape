#![forbid(unsafe_code)]
//! Item holdings: live totals plus hourly history for /api/items/history.
//!
//! Live totals come from the supervised snapshots (in-memory describes,
//! pushed by the workers many times a second) plus the engine save files
//! for every other account, exactly like the old Python `holdings.py`.
//! History lives in the store (`holdings_hourly`, 8 days, one point per
//! item and hour; `holdings_daily`, 400 days, last measure of each day).
//! The frozen Python-era `holdings.json` (30-minute points, 48 kept) and
//! its `holdings.sqlite` ancestor are imported once, then set aside.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lifescape_json::{self as json, Value, object, str_value};

use crate::jobs;

pub const INTERVAL_SECONDS: i64 = 60 * 60;
pub const COINS: i64 = 995;
const LIVE_FRESH_SECONDS: f64 = 600.0;
const TOP_ITEMS: usize = 500;

fn items_dir() -> PathBuf {
    crate::paths::run_dir().join("items")
}

fn history_path(items: &Path) -> PathBuf {
    items.join("holdings.json")
}

fn imported_path(items: &Path) -> PathBuf {
    items.join("holdings.json.imported")
}

fn sqlite_path(items: &Path) -> PathBuf {
    items.join("holdings.sqlite")
}

fn store_path(items: &Path) -> PathBuf {
    items
        .parent()
        .map(|run| run.join("lifescape.db"))
        .unwrap_or_else(|| crate::paths::run_dir().join("lifescape.db"))
}

// -- live holdings -----------------------------------------------------------

/// A life file belongs to an Ironman when its profile says so. Pure for
/// the test; the aggregate below reads the files.
pub fn life_is_iron(text: &str) -> bool {
    json::parse(text).map(|value| value.get("profile").get("ironman").as_bool() == Some(true)).unwrap_or(false)
}

/// Iron accounts (lowercased) from their life files. Their yields still
/// count in rates, but their coins and stock leave the public economy
/// aggregates (D9 economy pages, masse monétaire).
fn iron_accounts() -> HashSet<String> {
    let mut iron = HashSet::new();
    let Ok(entries) = std::fs::read_dir(crate::brain::lives_dir()) else { return iron };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        if life_is_iron(&text) {
            if let Some(account) = path.file_stem().and_then(|stem| stem.to_str()) {
                iron.insert(account.to_lowercase());
            }
        }
    }
    iron
}

/// Aggregate one snapshot's inv/worn/bank arrays (`{id, count}` rows).
fn add_snapshot(totals: &mut BTreeMap<i64, i64>, snapshot: &Value) {
    for key in ["inv", "worn", "bank"] {
        let items = snapshot.get(key).as_arr().cloned().unwrap_or_default();
        for item in &items {
            let (Some(obj_id), Some(count)) = (item.get("id").as_i64(), item.get("count").as_i64()) else {
                continue;
            };
            if count <= 0 {
                continue;
            }
            *totals.entry(obj_id).or_insert(0) = totals.get(&obj_id).unwrap_or(&0).saturating_add(count);
        }
    }
}

fn snapshot_bank_empty(snapshot: &Value) -> bool {
    let items = snapshot.get("bank").as_arr().cloned().unwrap_or_default();
    items.iter().all(|item| item.get("count").as_i64().unwrap_or(0) <= 0)
}

/// `{obj_id: count}` over supervised snapshots plus engine saves.
/// `described` are daemon `Job::describe()` values (in-memory pushes
/// first, heartbeat files as fallback): only fresh ones count as live,
/// the rest fall through to their save file like any idle account.
pub fn current_holdings(described: &[Value], save_dir: &str) -> BTreeMap<i64, i64> {
    let now = jobs::now_secs();
    let iron = iron_accounts();
    // Staff/private plus the persisted list (`run/economy-excluded.txt`).
    let excluded = crate::economy_guard::excluded_accounts();
    let mut live: HashMap<String, Value> = HashMap::new();
    for state in described {
        let account = state.get("account").as_str().unwrap_or("").to_lowercase();
        if account.is_empty() || excluded.contains(&account) || iron.contains(&account) {
            continue;
        }
        let fresh = state.get("updated").as_f64().is_some_and(|at| now - at < LIVE_FRESH_SECONDS);
        if fresh {
            live.insert(account, state.clone());
        }
    }
    let mut totals = BTreeMap::new();
    for state in live.values() {
        add_snapshot(&mut totals, state);
    }
    let saves = std::fs::read_dir(save_dir).map(|entries| entries.flatten().collect::<Vec<_>>()).unwrap_or_default();
    let mut names: Vec<String> = saves
        .iter()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.ends_with(".sav"))
        .collect();
    names.sort();
    for name in names {
        let account = name[..name.len() - 4].to_lowercase();
        if excluded.contains(&account) || iron.contains(&account) {
            continue;
        }
        let path = format!("{save_dir}/{name}");
        let Some(saved) = lifescape_sdk::save::parse_file(&path) else {
            continue;
        };
        if let Some(state) = live.get(&account) {
            // Stale save: only its bank helps until the bot opens the bank.
            if snapshot_bank_empty(state) {
                for (obj_id, count) in &saved.bank {
                    *totals.entry(i64::from(*obj_id)).or_insert(0) =
                        totals.get(&i64::from(*obj_id)).unwrap_or(&0).saturating_add(i64::from(*count));
                }
            }
            continue;
        }
        for bucket in [&saved.inv, &saved.worn, &saved.bank] {
            for (obj_id, count) in bucket {
                *totals.entry(i64::from(*obj_id)).or_insert(0) =
                    totals.get(&i64::from(*obj_id)).unwrap_or(&0).saturating_add(i64::from(*count));
            }
        }
    }
    totals
}

pub(crate) fn item_name(obj_id: i64) -> String {
    if obj_id >= 0 {
        if let Some(obj) = lifescape_gamedata::obj_by_id(obj_id as usize) {
            if let Some(display) = obj.display {
                if !display.is_empty() {
                    return display.to_string();
                }
            }
            if let Some(name) = obj.name {
                let shown = jobs::humanize(&name);
                if !shown.is_empty() {
                    return shown;
                }
            }
        }
    }
    format!("obj {obj_id}")
}

// -- history (JSON, one-shot SQLite migration) --------------------------------

struct History {
    /// Oldest first: (taken_at seconds, {obj_id: count}).
    samples: Vec<(i64, BTreeMap<i64, i64>)>,
}

fn history_from_value(data: &Value) -> Result<History, String> {
    let mut samples = Vec::new();
    let raw = data.get("samples").as_arr().cloned().unwrap_or_default();
    for sample in &raw {
        let taken_at = sample.get("time").as_i64().ok_or_else(|| "history sample without time".to_string())?;
        let mut items = BTreeMap::new();
        let entries = sample.get("items").as_obj().cloned().unwrap_or_default();
        for (key, count) in &entries {
            let (Ok(obj_id), Some(count)) = (key.parse::<i64>(), count.as_i64()) else {
                continue;
            };
            if count > 0 {
                items.insert(obj_id, count);
            }
        }
        samples.push((taken_at, items));
    }
    samples.sort_by_key(|(taken_at, _)| *taken_at);
    Ok(History { samples })
}

/// Read the frozen JSON history, with the frozen Python-era SQLite as
/// fallback. A broken store reads empty (logged): the live holdings
/// below are always served, history just shows fewer points.
fn read_history(items: &Path) -> History {
    let path = history_path(items);
    if path.exists() {
        match std::fs::read_to_string(&path).map_err(|error| error.to_string()).and_then(|text| json::parse(&text)) {
            Ok(data) => match history_from_value(&data) {
                Ok(history) => return history,
                Err(error) => eprintln!("holdings: ignoring {path:?}: {error}"),
            },
            Err(error) => eprintln!("holdings: ignoring {path:?}: {error}"),
        }
    }
    let sqlite = sqlite_path(items);
    if sqlite.exists() {
        match read_sqlite_history(&sqlite) {
            Ok(history) => return history,
            Err(error) => eprintln!("holdings: ignoring {sqlite:?}: {error}"),
        }
    }
    History { samples: Vec::new() }
}

/// One-shot import of the frozen JSON history into the store: every
/// sample in time order, so hourly keeps the last write of its hour and
/// daily the last measure of its day. Runs only into an empty store;
/// the JSON file is then set aside (never deleted, never read again).
pub(crate) fn ensure_imported(items: &Path) {
    let history = history_path(items);
    if !history.exists() || imported_path(items).exists() {
        return;
    }
    let db_path = store_path(items);
    let store = match lifescape_store::Store::open(&db_path) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("holdings: import skipped, store unreachable: {error}");
            return;
        }
    };
    match store.holdings_latest() {
        Ok(None) => {}
        Ok(Some(_)) => return,
        Err(error) => {
            eprintln!("holdings: import skipped, latest unreadable: {error}");
            return;
        }
    }
    let samples = read_history(items).samples;
    let imported = samples.len();
    let mut failures = 0;
    for (taken_at, point) in samples {
        let pairs: Vec<(i64, i64)> = point.into_iter().collect();
        if let Err(error) = store.record_holdings(taken_at, &pairs) {
            failures += 1;
            eprintln!("holdings: import drops point at {taken_at}: {error}");
        }
    }
    if failures == 0 {
        println!("holdings: imported {imported} JSON samples into {}", db_path.display());
    }
    if let Err(error) = std::fs::rename(&history, imported_path(items)) {
        eprintln!("holdings: import kept, JSON not set aside: {error}");
    }
}

// -- minimal read-only SQLite walk --------------------------------------------
//
// Enough for the sampler-era store: a 4 KiB-page database with two rowid
// tables (`samples`, `holdings`) holding small integer records. Text
// decoding covers only `sqlite_master`; integer records reject floats,
// blobs and overflow pages loudly instead of guessing.

#[derive(Debug, PartialEq)]
enum Cell {
    Null,
    Int(i64),
    Text(String),
}

fn varint(data: &[u8], mut pos: usize) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    for index in 0..9 {
        let byte = *data.get(pos)?;
        pos += 1;
        if index == 8 {
            value = (value << 8) | u64::from(byte);
            break;
        }
        value = (value << 7) | u64::from(byte & 0x7F);
        if byte & 0x80 == 0 {
            break;
        }
    }
    Some((value, pos))
}

fn parse_record(payload: &[u8]) -> Option<Vec<Cell>> {
    let (header_len, mut pos) = varint(payload, 0)?;
    let mut serials = Vec::new();
    while (pos as u64) < header_len {
        let (serial, next) = varint(payload, pos)?;
        serials.push(serial);
        pos = next;
    }
    let mut cells = Vec::with_capacity(serials.len());
    for serial in serials {
        match serial {
            0 => cells.push(Cell::Null),
            1 | 2 | 3 | 4 | 5 | 6 => {
                let len = [0, 1, 2, 3, 4, 6, 8][serial as usize];
                let chunk = payload.get(pos..pos + len)?;
                let mut padded = [0u8; 8];
                let start = 8 - len;
                padded[start..].copy_from_slice(chunk);
                if chunk[0] & 0x80 != 0 {
                    padded[..start].fill(0xFF);
                }
                cells.push(Cell::Int(i64::from_be_bytes(padded)));
                pos += len;
            }
            8 | 9 => cells.push(Cell::Int(serial as i64 - 8)),
            _ if serial >= 12 => {
                let len = if serial % 2 == 0 { (serial - 12) / 2 } else { (serial - 13) / 2 };
                let text = std::str::from_utf8(payload.get(pos..pos + len as usize)?).ok()?;
                cells.push(Cell::Text(text.to_string()));
                pos += len as usize;
            }
            _ => return None,
        }
    }
    Some(cells)
}

struct Pages {
    data: Vec<u8>,
    page_size: usize,
}

impl Pages {
    fn load(path: &Path) -> Result<Self, String> {
        let data = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        if data.len() < 100 || &data[..16] != b"SQLite format 3\x00" {
            return Err(format!("{}: not a SQLite file", path.display()));
        }
        let raw_size = u16::from_be_bytes(data[16..18].try_into().map_err(|_| "short header".to_string())?) as usize;
        let page_size = if raw_size == 1 { 65536 } else { raw_size };
        if page_size < 512 || data.len() % page_size != 0 {
            return Err(format!("{}: bad page size {page_size}", path.display()));
        }
        Ok(Pages { data, page_size })
    }

    fn walk_table(&self, root: u32) -> Result<Vec<(u64, Vec<Cell>)>, String> {
        let mut rows = Vec::new();
        let mut stack = vec![root];
        while let Some(page) = stack.pop() {
            let start = (page as usize - 1) * self.page_size;
            let head = start + if page == 1 { 100 } else { 0 };
            let kind = *self.data.get(head).ok_or_else(|| format!("page {page} out of range"))?;
            let count = u16::from_be_bytes(
                self.data.get(head + 3..head + 5).ok_or_else(|| format!("page {page} short header"))?.try_into().map_err(|_| "short".to_string())?,
            ) as usize;
            if kind == 0x05 {
                for index in 0..count {
                    let at = head + 12 + 2 * index;
                    let offset = u16::from_be_bytes(
                        self.data.get(at..at + 2).ok_or_else(|| format!("page {page} short cell"))?.try_into().map_err(|_| "short".to_string())?,
                    ) as usize;
                    let cell = start + offset;
                    let child = u32::from_be_bytes(self.data.get(cell..cell + 4).ok_or_else(|| format!("page {page} short child"))?.try_into().map_err(|_| "short".to_string())?);
                    stack.push(child);
                }
                let at = head + 8;
                let child = u32::from_be_bytes(self.data.get(at..at + 4).ok_or_else(|| format!("page {page} short tail"))?.try_into().map_err(|_| "short".to_string())?);
                stack.push(child);
            } else if kind == 0x0D {
                for index in 0..count {
                    let at = head + 8 + 2 * index;
                    let offset = u16::from_be_bytes(self.data.get(at..at + 2).ok_or_else(|| format!("page {page} short cell"))?.try_into().map_err(|_| "short".to_string())?) as usize;
                    let mut pos = start + offset;
                    let (payload_len, next) = varint(&self.data, pos).ok_or_else(|| format!("page {page} short payload"))?;
                    pos = next;
                    let (rowid, next) = varint(&self.data, pos).ok_or_else(|| format!("page {page} short rowid"))?;
                    pos = next;
                    let end = pos + payload_len as usize;
                    if end > start + self.page_size {
                        return Err(format!("page {page} overflow cell ({payload_len} bytes): unsupported"));
                    }
                    let record = parse_record(self.data.get(pos..end).ok_or_else(|| format!("page {page} short record"))?).ok_or_else(|| format!("page {page} bad record"))?;
                    rows.push((rowid, record));
                }
            } else {
                return Err(format!("page {page} unexpected type {kind:#x}"));
            }
        }
        Ok(rows)
    }
}

fn read_sqlite_history(path: &Path) -> Result<History, String> {
    let pages = Pages::load(path)?;
    let mut roots: HashMap<String, u32> = HashMap::new();
    for (_rowid, record) in pages.walk_table(1)? {
        if record.first() != Some(&Cell::Text("table".to_string())) {
            continue;
        }
        if let [_, Cell::Text(name), _, Cell::Int(root), ..] = record.as_slice() {
            roots.insert(name.clone(), *root as u32);
        }
    }
    let samples_root = roots.get("samples").copied().ok_or_else(|| "no samples table".to_string())?;
    let holdings_root = roots.get("holdings").copied().ok_or_else(|| "no holdings table".to_string())?;
    let mut samples: Vec<(i64, i64)> = Vec::new();
    for (rowid, record) in pages.walk_table(samples_root)? {
        if let [_, Cell::Int(taken_at), ..] = record.as_slice() {
            samples.push((rowid as i64, *taken_at));
        } else {
            return Err("bad samples record".to_string());
        }
    }
    samples.sort();
    let mut by_sample: HashMap<i64, BTreeMap<i64, i64>> = HashMap::new();
    for (_rowid, record) in pages.walk_table(holdings_root)? {
        if let [Cell::Int(sample_id), Cell::Int(obj_id), Cell::Int(count), ..] = record.as_slice() {
            if *count > 0 {
                by_sample.entry(*sample_id).or_default().insert(*obj_id, *count);
            }
        } else {
            return Err("bad holdings record".to_string());
        }
    }
    let mut ordered = Vec::with_capacity(samples.len());
    for (sample_id, taken_at) in samples {
        ordered.push((taken_at, by_sample.remove(&sample_id).unwrap_or_default()));
    }
    let mut history = History { samples: ordered };
    history.samples.sort_by_key(|(taken_at, _)| *taken_at);
    Ok(history)
}

// -- snapshot + sampling --------------------------------------------------------

fn snapshot_in(described: &[Value], _items: &Path, save_dir: &str) -> Value {
    let live = current_holdings(described, save_dir);
    let live_total: i64 = live.values().sum();
    let mut ids: Vec<i64> = live.keys().copied().collect();
    ids.push(COINS);
    ids.sort();
    ids.dedup();
    let mut ranked = ids;
    ranked.sort_by(|left, right| {
        live.get(left).copied().unwrap_or(0).cmp(&live.get(right).copied().unwrap_or(0)).reverse().then(left.cmp(right))
    });
    ranked.truncate(TOP_ITEMS);
    let items_list: Vec<Value> = ranked
        .iter()
        .map(|obj_id| {
            object(vec![
                ("id", Value::Int(*obj_id)),
                ("name", str_value(&item_name(*obj_id))),
                ("count", Value::Int(live.get(obj_id).copied().unwrap_or(0))),
            ])
        })
        .collect();
    object(vec![("liveTotal", Value::Int(live_total)), ("items", Value::Arr(items_list))])
}

/// The dashboard payload: live totals plus the ranked item list.
/// History rides the store now (`holdings_hourly`/`holdings_daily`),
/// served per item by the items history endpoint.
pub fn snapshot(described: &[Value]) -> Value {
    snapshot_in(described, &items_dir(), &lifescape_sdk::save::default_save_dir())
}

fn maybe_sample_in(described: &[Value], items: &Path, save_dir: &str, now: i64) -> bool {
    ensure_imported(items);
    let holdings = current_holdings(described, save_dir);
    let pairs: Vec<(i64, i64)> = holdings.into_iter().filter(|(_, count)| *count > 0).collect();
    if pairs.is_empty() {
        return false;
    }
    let store = match lifescape_store::Store::open(&store_path(items)) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("holdings: sample dropped, store unreachable: {error}");
            return false;
        }
    };
    match store.holdings_latest() {
        Ok(Some(latest)) if now - latest < INTERVAL_SECONDS => return false,
        Ok(_) => {}
        Err(error) => {
            eprintln!("holdings: sample dropped, latest unreadable: {error}");
            return false;
        }
    }
    match store.record_holdings(now, &pairs) {
        Ok(()) => {
            println!("holdings: {} item kinds across all accounts", pairs.len());
            true
        }
        Err(error) => {
            eprintln!("holdings: sample dropped: {error}");
            false
        }
    }
}

/// Append one point when the last is older than one hour. True when
/// a point was written.
pub fn maybe_sample(described: &[Value]) -> bool {
    maybe_sample_in(described, &items_dir(), &lifescape_sdk::save::default_save_dir(), jobs::now_secs() as i64)
}

/// Live describes for the endpoint and the sampler: pushed snapshots
/// first (tens of ms fresh), heartbeat files as fallback.
pub(crate) fn described_jobs(orch: &crate::daemon::Orchestrator) -> Vec<Value> {
    let live: Vec<std::sync::Arc<crate::jobs::Job>> = orch.jobs.lock().unwrap().values().cloned().collect();
    live.iter().map(|job| job.describe()).collect()
}

/// Background hourly sampler, started once by the daemon. It reads
/// the live describes like the endpoint, so running bots count their
/// pushed snapshots instead of their stale save files.
pub fn start_sampler(orch: Arc<crate::daemon::Orchestrator>) {
    std::thread::Builder::new()
        .name("holdings-sampler".to_string())
        .spawn(move || {
            while orch.running.load(std::sync::atomic::Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_secs(60));
                if !orch.running.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                maybe_sample(&described_jobs(&orch));
            }
        })
        .expect("holdings sampler spawns");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lifescape-holdings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn described(account: &str, updated: f64, inv: Vec<(i64, i64)>, bank: Vec<(i64, i64)>) -> Value {
        let rows = |entries: Vec<(i64, i64)>| {
            Value::Arr(entries.into_iter().map(|(id, count)| object(vec![("id", Value::Int(id)), ("count", Value::Int(count))])).collect())
        };
        object(vec![
            ("account", str_value(account)),
            ("updated", Value::Float(updated)),
            ("inv", rows(inv)),
            ("worn", Value::Arr(vec![])),
            ("bank", rows(bank)),
        ])
    }

    /// Minimal engine save (format version 5): only the inventories matter.
    fn write_sav(dir: &Path, account: &str, inv: &[(u16, u32)], worn: &[(u16, u32)], bank: &[(u16, u32)]) {
        let mut data = Vec::new();
        data.extend_from_slice(&0x2004u16.to_be_bytes());
        data.extend_from_slice(&5u16.to_be_bytes());
        data.extend_from_slice(&3200u16.to_be_bytes()); // x
        data.extend_from_slice(&3200u16.to_be_bytes()); // z
        data.push(0); // level
        data.extend_from_slice(&[0u8; 7]); // body
        data.extend_from_slice(&[0u8; 5]); // colours
        data.push(0); // gender
        data.extend_from_slice(&100u16.to_be_bytes()); // energy
        data.extend_from_slice(&0i32.to_be_bytes()); // playtime
        for _ in 0..21 {
            data.extend_from_slice(&0i32.to_be_bytes()); // xp
            data.push(1); // level
        }
        data.extend_from_slice(&0u16.to_be_bytes()); // varps
        let mut inventories: Vec<(u16, Vec<(u16, u32)>)> = vec![(0, vec![(0, 0); 28]), (1, vec![(0, 0); 14]), (2, vec![(0, 0); 100])];
        for (slot, (id, count)) in inv.iter().enumerate() {
            inventories[0].1[slot] = (*id, *count);
        }
        for (slot, (id, count)) in worn.iter().enumerate() {
            inventories[1].1[slot] = (*id, *count);
        }
        for (slot, (id, count)) in bank.iter().enumerate() {
            inventories[2].1[slot] = (*id, *count);
        }
        data.push(inventories.len() as u8);
        for (kind, slots) in &inventories {
            data.extend_from_slice(&kind.to_be_bytes());
            data.extend_from_slice(&(slots.len() as u16).to_be_bytes());
            for (id, count) in slots {
                data.extend_from_slice(&(id + 1).to_be_bytes()); // engine stores id+1, 0 is empty
                if *count < 255 {
                    data.push(*count as u8);
                } else {
                    data.push(255);
                    data.extend_from_slice(&(*count as i32).to_be_bytes());
                }
            }
        }
        std::fs::write(dir.join(format!("{account}.sav")), data).unwrap();
    }

    #[test]
    fn varint_and_record_decode() {
        assert_eq!(varint(&[0x00], 0), Some((0, 1)));
        assert_eq!(varint(&[0x7F], 0), Some((127, 1)));
        assert_eq!(varint(&[0x81, 0x00], 0), Some((128, 2)));
        // Header (3 bytes: NULL + 1-byte int) then the int payload 42.
        assert_eq!(parse_record(&[0x03, 0x00, 0x01, 0x2A]), Some(vec![Cell::Null, Cell::Int(42)]));
        assert_eq!(parse_record(&[0x02, 0x09, 0x2A]), Some(vec![Cell::Int(1)]));
        assert!(parse_record(&[0x02, 0x07]).is_none(), "floats are rejected");
    }

    /// Hand-built 3-page database: master (page 1) -> interior (page 2)
    /// -> leaf (page 3). Covers the interior child walk, the path the
    /// multi-page holdings table takes.
    fn tiny_db() -> Vec<u8> {
        const PS: usize = 512;
        fn enc_varint(mut value: u64) -> Vec<u8> {
            let mut tail = vec![(value & 0x7F) as u8];
            value >>= 7;
            while value > 0 {
                tail.push(0x80 | ((value & 0x7F) as u8));
                value >>= 7;
            }
            tail.reverse();
            tail
        }
        fn record(cells: &[Cell]) -> Vec<u8> {
            let mut head = Vec::new();
            let mut body = Vec::new();
            for cell in cells {
                match cell {
                    Cell::Null => head.push(0),
                    Cell::Int(n) => {
                        let bytes = if *n == 0 || *n == 1 {
                            head.push(8 + *n as u8);
                            vec![]
                        } else if *n >= -128 && *n <= 127 {
                            head.push(1);
                            vec![*n as u8]
                        } else {
                            head.push(4);
                            (*n as i32).to_be_bytes().to_vec()
                        };
                        body.extend(bytes);
                    }
                    Cell::Text(text) => {
                        head.push(13 + 2 * text.len() as u8);
                        body.extend(text.as_bytes());
                    }
                }
            }
            let mut head_len = enc_varint((head.len() + 1) as u64);
            head_len.extend(head);
            head_len.extend(body);
            head_len
        }
        fn leaf_cell(rowid: u64, cells: &[Cell]) -> Vec<u8> {
            let payload = record(cells);
            let mut out = enc_varint(payload.len() as u64);
            out.extend(enc_varint(rowid));
            out.extend(payload);
            out
        }
        let mut db = vec![0u8; 4 * PS];
        db[..16].copy_from_slice(b"SQLite format 3\x00");
        db[16..18].copy_from_slice(&(PS as u16).to_be_bytes());
        // Page 1: master leaf with one entry (table t, root 2).
        let master = leaf_cell(
            1,
            &[Cell::Text("table".to_string()), Cell::Text("t".to_string()), Cell::Text("t".to_string()), Cell::Int(2), Cell::Text("CREATE TABLE t(a)".to_string())],
        );
        let at = PS - master.len();
        db[at..at + master.len()].copy_from_slice(&master);
        db[100] = 0x0D;
        db[103..105].copy_from_slice(&1u16.to_be_bytes());
        db[105..107].copy_from_slice(&(at as u16).to_be_bytes());
        db[108..110].copy_from_slice(&(at as u16).to_be_bytes());
        // Page 2: interior, cell -> page 3 plus rightmost -> page 4.
        db[PS + 0] = 0x05;
        db[PS + 3..PS + 5].copy_from_slice(&1u16.to_be_bytes());
        db[PS + 5..PS + 7].copy_from_slice(&((PS - 8) as u16).to_be_bytes());
        let cell_at = 2 * PS - 8;
        db[cell_at..cell_at + 4].copy_from_slice(&3u32.to_be_bytes());
        db[cell_at + 4] = 0x04; // rowid key 4: page 3 holds 1..2
        db[PS + 8..PS + 12].copy_from_slice(&4u32.to_be_bytes());
        db[PS + 12..PS + 14].copy_from_slice(&((cell_at - PS) as u16).to_be_bytes());
        // Page 3: leaf with two rows.
        let first = leaf_cell(1, &[Cell::Int(7)]);
        let second = leaf_cell(2, &[Cell::Int(9)]);
        let second_at = 3 * PS - second.len();
        let first_at = second_at - first.len();
        db[first_at..first_at + first.len()].copy_from_slice(&first);
        db[second_at..second_at + second.len()].copy_from_slice(&second);
        db[2 * PS] = 0x0D;
        db[2 * PS + 3..2 * PS + 5].copy_from_slice(&2u16.to_be_bytes());
        db[2 * PS + 5..2 * PS + 7].copy_from_slice(&((first_at - 2 * PS) as u16).to_be_bytes());
        db[2 * PS + 8..2 * PS + 10].copy_from_slice(&((first_at - 2 * PS) as u16).to_be_bytes());
        db[2 * PS + 10..2 * PS + 12].copy_from_slice(&((second_at - 2 * PS) as u16).to_be_bytes());
        // Page 4: rightmost leaf with one row.
        let third = leaf_cell(6, &[Cell::Int(11)]);
        let third_at = 4 * PS - third.len();
        db[third_at..third_at + third.len()].copy_from_slice(&third);
        db[3 * PS] = 0x0D;
        db[3 * PS + 3..3 * PS + 5].copy_from_slice(&1u16.to_be_bytes());
        db[3 * PS + 5..3 * PS + 7].copy_from_slice(&((third_at - 3 * PS) as u16).to_be_bytes());
        db[3 * PS + 8..3 * PS + 10].copy_from_slice(&((third_at - 3 * PS) as u16).to_be_bytes());
        db
    }

    #[test]
    fn btree_walk_covers_interior_pages() {
        let dir = temp_dir("btree");
        let path = dir.join("tiny.sqlite");
        std::fs::write(&path, tiny_db()).unwrap();
        let pages = Pages::load(&path).expect("tiny db loads");
        let mut rows = pages.walk_table(2).expect("interior walk");
        rows.sort_by_key(|(rowid, _)| *rowid);
        assert_eq!(rows, vec![(1, vec![Cell::Int(7)]), (2, vec![Cell::Int(9)]), (6, vec![Cell::Int(11)])]);
        let master = pages.walk_table(1).expect("master walk");
        assert_eq!(master.len(), 1);
    }

    #[test]
    fn live_beats_save_and_excluded_are_skipped() {
        let dir = temp_dir("current");
        let saves = dir.join("saves");
        std::fs::create_dir_all(&saves).unwrap();
        let save_arg = saves.to_string_lossy().to_string();
        // t01: save holds bank coins; admin/bite saves must never count.
        write_sav(&saves, "t01", &[], &[], &[(995, 100)]);
        write_sav(&saves, "admin", &[(995, 1000)], &[], &[]);
        write_sav(&saves, "bite", &[(995, 500)], &[], &[]);
        write_sav(&saves, "bob", &[(995, 5)], &[], &[]);
        let now = jobs::now_secs();
        // Fresh live t01: its save inv is ignored, its save bank is added.
        let live = vec![described("t01", now, vec![(995, 7)], vec![])];
        let totals = current_holdings(&live, &save_arg);
        assert_eq!(totals.get(&995), Some(&112), "live 7 + save bank 100 + bob save 5, got {totals:?}");
        // Stale live t01: the whole save counts instead.
        let stale = vec![described("t01", now - 3600.0, vec![(995, 7)], vec![])];
        let totals = current_holdings(&stale, &save_arg);
        assert_eq!(totals.get(&995), Some(&105), "t01 save bank 100 + bob save 5, got {totals:?}");
    }

    #[test]
    fn iron_life_flags_parse() {
        assert!(life_is_iron(r#"{"profile":{"ironman":true}}"#), "iron flag reads");
        assert!(!life_is_iron(r#"{"profile":{"ironman":false}}"#), "normal play counts");
        assert!(!life_is_iron(r#"{"profile":{}}"#), "missing flag counts");
        assert!(!life_is_iron("not json"), "garbage counts");
    }

    #[test]
    fn per_slot_stacks_add_up() {        let dir = temp_dir("slots");
        let now = jobs::now_secs();
        // Three oak-log slots of one log each, like the live t01 snapshot.
        let live = vec![described("t01", now, vec![(1521, 1), (1521, 1), (1521, 1)], vec![])];
        let totals = current_holdings(&live, dir.join("empty-saves").to_string_lossy().as_ref());
        assert_eq!(totals.get(&1521), Some(&3));
    }

    #[test]
    fn snapshot_lists_live_counts_without_history() {
        let dir = temp_dir("snapshot");
        let items = dir.join("items");
        std::fs::create_dir_all(&items).unwrap();
        let now = jobs::now_secs();
        let live = vec![described("t01", now, vec![(1521, 3)], vec![])];
        let save_arg = dir.join("saves").to_string_lossy().to_string();
        let data = snapshot_in(&live, &items, &save_arg);
        assert_eq!(data.get("liveTotal").as_i64(), Some(3));
        let rows = data.get("items").as_arr().unwrap();
        assert_eq!(rows.len(), 2, "live item plus the coins row");
        assert_eq!(rows[0].get("id").as_i64(), Some(1521), "live count ranks first");
        assert_eq!(rows[0].get("count").as_i64(), Some(3));
        assert_eq!(rows[0].get("name").as_str(), Some("Oak logs"));
        assert!(rows[0].get("series").is_null(), "series ride the store now, not the snapshot");
        assert!(data.get("samples").is_null() && data.get("total").is_null());
        assert_eq!(rows[1].get("id").as_i64(), Some(995));
        assert_eq!(rows[1].get("name").as_str(), Some("Coins"));
    }

    #[test]
    fn sample_writes_hourly_then_respects_interval() {
        let dir = temp_dir("sampling");
        let items = dir.join("items");
        std::fs::create_dir_all(&items).unwrap();
        let saves = dir.join("saves");
        std::fs::create_dir_all(&saves).unwrap();
        let save_arg = saves.to_string_lossy().to_string();
        let now = 1_762_675_200;
        assert!(!maybe_sample_in(&[], &items, &save_arg, now), "no holdings means no point");
        assert!(!store_path(&items).exists(), "no store file without holdings");
        let live = vec![described("t01", jobs::now_secs(), vec![(995, 7)], vec![])];
        assert!(maybe_sample_in(&live, &items, &save_arg, now));
        assert!(!maybe_sample_in(&live, &items, &save_arg, now), "same hour: no second point");
        assert!(!maybe_sample_in(&live, &items, &save_arg, now + INTERVAL_SECONDS - 1), "59 min later: still no point");
        assert!(maybe_sample_in(&live, &items, &save_arg, now + INTERVAL_SECONDS), "one hour later: new point");
        let store = lifescape_store::Store::open(&store_path(&items)).unwrap();
        assert_eq!(store.hourly_points(Some(995)).unwrap().len(), 2);
        assert_eq!(store.daily_points(Some(995)).unwrap().len(), 1, "same day: last measure wins");
    }

    #[test]
    fn frozen_json_imports_once_then_steps_aside() {
        let dir = temp_dir("import");
        let items = dir.join("items");
        std::fs::create_dir_all(&items).unwrap();
        let saves = dir.join("saves");
        std::fs::create_dir_all(&saves).unwrap();
        let save_arg = saves.to_string_lossy().to_string();
        // Two 30-minute samples inside one hour, one the next day.
        std::fs::write(
            history_path(&items),
            r#"{"samples": [{"time": 1000, "items": {"995": 7, "1521": 2}}, {"time": 1900, "items": {"995": 5}}, {"time": 90000, "items": {"995": 9}}]}"#,
        )
        .unwrap();
        // Import alone: hourly keeps the last write of each hour.
        ensure_imported(&items);
        assert!(!history_path(&items).exists(), "JSON set aside after import");
        assert!(imported_path(&items).exists());
        let store = lifescape_store::Store::open(&store_path(&items)).unwrap();
        // 1000 and 1900 share an hour: last write wins; 90000 (next day, 01:00) lands alone.
        assert_eq!(store.hourly_points(Some(995)).unwrap(), vec![(0, 5), (90000, 9)]);
        assert_eq!(store.daily_points(Some(995)).unwrap(), vec![(0, 5), (86400, 9)]);
        assert_eq!(store.hourly_points(Some(1521)).unwrap(), vec![(0, 2)]);
        // A fresh sample records on top and prunes past retention.
        let live = vec![described("t01", jobs::now_secs(), vec![(995, 1)], vec![])];
        assert!(maybe_sample_in(&live, &items, &save_arg, 1_762_675_200));
        assert_eq!(store.hourly_points(Some(995)).unwrap(), vec![(1_762_675_200, 1)]);
        assert_eq!(store.daily_points(Some(995)).unwrap(), vec![((1_762_675_200 / 86400) * 86400, 1)]);
        // Second run sees the store filled: no reimport, no rename failure.
        assert!(!maybe_sample_in(&live, &items, &save_arg, 1_762_675_200), "same hour: quiet");
        assert!(imported_path(&items).exists());
    }

    /// The frozen sampler-era database parses: 48 points, positive counts.
    #[test]
    fn real_sqlite_history_parses() {
        let sqlite = sqlite_path(&crate::paths::run_dir().join("items"));
        if !sqlite.exists() {
            return;
        }
        let history = read_sqlite_history(&sqlite).expect("frozen sqlite parses");
        assert_eq!(history.samples.len(), 48);
        assert!(history.samples.windows(2).all(|pair| pair[0].0 < pair[1].0), "points ordered");
        assert!(history.samples.iter().all(|(_, point)| point.values().all(|count| *count > 0)));
    }
}

