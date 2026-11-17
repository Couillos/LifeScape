//! Method profitability from the yield estimator (plan rendements R2,
//! §8) and its shared cache (CPU 2026-11-14: rebuilt when the snapshot
//! changed or after `PROFIT_TTL`, else shared as is).
//!
//! One row per (planner method, style, band): the all-sites, all-tools
//! estimate (`rates_read`, n ≥ 3 then blended with the prior, prior
//! alone below), priced at the current price (`price_estimate`):
//! revenue = Σ positive flows × price, input cost = Σ negative flows ×
//! price (inputs, runes, ammo, bait, food, fees, losses: the signed
//! balance), friction = paused ticks at the activity's revenue rate,
//! plus the public-trend bonus. Trading legs (`shop_*`, `traded`) are
//! transfers, `unattributed` an alert: neither enters a profit.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use lifescape_store::Store;

use super::{live_snapshot, price_estimate, Snapshot, NORMALIZE_TICKS, TREND_WEIGHT};

/// Longest a cached table serves without a rebuild (market prints).
pub const PROFIT_TTL: Duration = Duration::from_secs(10);
/// Causes that never price into a method's profit.
pub const UNPRICED_CAUSES: [&str; 4] = ["shop_bought", "shop_sold", "traded", "unattributed"];

/// Public trend score of an item display name (24 h period preferred),
/// 0.0 when unscored.
pub fn trend_score(store: &Store, item: &str) -> f64 {
    let Ok(Some(item_id)) = store.item_id_by_name(item) else {
        return 0.0;
    };
    let scores = store.trends_for_item(item_id).unwrap_or_default();
    scores.iter().find(|trend| trend.period == "24h").map(|trend| trend.score).unwrap_or_else(|| scores.first().map(|trend| trend.score).unwrap_or(0.0))
}

/// Profitability of one activity, in coins per 6000 ticks: revenue at
/// estimated sale price, minus input cost, minus the pause friction
/// (paused ticks at the activity's own revenue rate), plus the
/// public-trend bonus (score times revenue times weight).
pub fn profit_per_6000(revenue_items: &[(f64, f64)], input_cost: f64, pause_ticks_per_6000: f64, trend_score: f64, trend_weight: f64) -> f64 {
    let revenue: f64 = revenue_items.iter().map(|(yield_per, price)| yield_per * price).sum();
    let revenue_rate = revenue / NORMALIZE_TICKS;
    let friction = pause_ticks_per_6000 * revenue_rate;
    revenue - input_cost - friction + trend_score * revenue * trend_weight
}

/// Seconds between two notes of the same missing profit row.
pub const NOTE_EVERY_SECS: i64 = 3600;

type Noted = HashMap<(String, i64), i64>;

fn noted() -> &'static std::sync::Mutex<Noted> {
    static NOTED: std::sync::OnceLock<std::sync::Mutex<Noted>> = std::sync::OnceLock::new();
    NOTED.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Pure: true when `(method, band)` was not noted in the last
/// `NOTE_EVERY_SECS` (and stamps it now).
pub fn log_due(table: &mut Noted, method: &str, band: i64, now: i64) -> bool {
    let key = (method.to_string(), band);
    match table.get(&key) {
        Some(last) if now - *last < NOTE_EVERY_SECS => false,
        _ => {
            table.insert(key, now);
            true
        }
    }
}

/// One profitability row per (method, style, band).
#[derive(Clone, Debug, PartialEq)]
pub struct ProfitRow {
    pub method: String,
    pub style: String,
    pub band: i64,
    pub revenue: f64,
    pub input_cost: f64,
    pub friction: f64,
    pub trend_bonus: f64,
    pub profit: f64,
    pub price_prior: bool,
    /// The yields behind the row are the game prior alone (n < 3).
    pub prior: bool,
}

/// Builds profitability from the snapshot, pricing through the store
/// (each distinct item priced and scored once per build).
pub fn profit_rows(store: &Store, snapshot: &Snapshot) -> Vec<ProfitRow> {
    let mut prices: HashMap<String, (f64, bool)> = HashMap::new();
    let mut trends: HashMap<String, f64> = HashMap::new();
    let now = crate::rates_service::now_secs();
    let mut rows = Vec::new();
    for key in snapshot.book.buckets.keys().filter(|key| key.site == crate::rates_estimator::ANY && key.tool == crate::rates_estimator::ANY) {
        let estimate = match crate::rates_read::estimate_key(&snapshot.book, key, now) {
            Ok(estimate) => estimate,
            Err(error) => {
                // Once per (method, band) per hour (plan P18: 10 373
                // identical lines in 10 h for one unmeasured method).
                if log_due(&mut noted().lock().unwrap_or_else(|poisoned| poisoned.into_inner()), &key.method, key.band, now) {
                    crate::logln!("rates: no profit row for {} band {}: {error} (next note in {NOTE_EVERY_SECS} s)", key.method, key.band);
                }
                continue;
            }
        };
        let mut price_prior = false;
        let mut revenue_items = Vec::new();
        let mut input_cost = 0.0;
        let mut trend_weighted = 0.0;
        for (item, cause, rate) in &estimate.flows {
            if UNPRICED_CAUSES.contains(&cause.as_str()) || *rate == 0.0 {
                continue;
            }
            let (price, prior) = *prices.entry(item.clone()).or_insert_with(|| price_estimate(store, item));
            price_prior |= prior;
            if *rate > 0.0 {
                revenue_items.push((*rate, price));
                trend_weighted += rate * price * *trends.entry(item.clone()).or_insert_with(|| trend_score(store, item));
            } else {
                input_cost += rate.abs() * price;
            }
        }
        let revenue: f64 = revenue_items.iter().map(|(rate, price)| rate * price).sum();
        let score = if revenue > 0.0 { trend_weighted / revenue } else { 0.0 };
        rows.push(ProfitRow {
            method: key.method.clone(),
            style: key.style.clone(),
            band: key.band,
            revenue,
            input_cost,
            friction: estimate.pause_per_6000 * revenue / NORMALIZE_TICKS,
            trend_bonus: score * revenue * TREND_WEIGHT,
            profit: profit_per_6000(&revenue_items, input_cost, estimate.pause_per_6000, score, TREND_WEIGHT),
            price_prior,
            prior: estimate.prior,
        });
    }
    rows.sort_by(|left, right| (left.method.clone(), left.style.clone(), left.band).cmp(&(right.method.clone(), right.style.clone(), right.band)));
    rows
}

/// Rates snapshot generation: bumped on every ingest.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// The snapshot changed: the next `cached_profit_rows` rebuilds.
pub fn invalidate() {
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// Current snapshot generation (the rates page index rebuilds on change).
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// One built table and what it was built from (pure freshness rule).
pub struct ProfitCache {
    pub built: Instant,
    pub generation: u64,
    pub rows: Arc<Vec<ProfitRow>>,
}

impl ProfitCache {
    /// Serves while the snapshot is the same and `PROFIT_TTL` not past.
    pub fn fresh(&self, now: Instant, generation: u64) -> bool {
        self.generation == generation && now.saturating_duration_since(self.built) < PROFIT_TTL
    }
}

fn profit_cache() -> &'static Mutex<Option<ProfitCache>> {
    static CACHE: OnceLock<Mutex<Option<ProfitCache>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// The live profitability table (store at `market_service::db_path`,
/// live snapshot), shared until stale. Empty when the store does not
/// open (no rows, no decision). Built under the lock: concurrent callers
/// wait for one build, never run their own.
pub fn cached_profit_rows() -> Arc<Vec<ProfitRow>> {
    let mut slot = profit_cache().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = Instant::now();
    let generation = GENERATION.load(Ordering::Relaxed);
    if let Some(cache) = slot.as_ref().filter(|cache| cache.fresh(now, generation)) {
        return cache.rows.clone();
    }
    let Ok(store) = Store::open(&crate::market_service::db_path()) else {
        return Arc::new(Vec::new());
    };
    let rows = Arc::new(profit_rows(&store, &live_snapshot()));
    *slot = Some(ProfitCache { built: now, generation, rows: rows.clone() });
    rows
}

#[cfg(test)]
mod tests {
    /// Plan P18: one note per (method, band) per hour.
    #[test]
    fn a_missing_profit_row_is_noted_once_an_hour() {
        let mut table = std::collections::HashMap::new();
        assert!(super::log_due(&mut table, "mill:pot_flour", 21, 1000));
        assert!(!super::log_due(&mut table, "mill:pot_flour", 21, 1000 + 59 * 60));
        assert!(super::log_due(&mut table, "mill:pot_flour", 22, 1000), "another band");
        assert!(super::log_due(&mut table, "mill:pot_flour", 21, 1000 + super::NOTE_EVERY_SECS));
    }

    use super::*;
    use crate::rates_estimator::{RunSample, YieldKey};

    /// A cached table serves until the TTL or a snapshot change.
    #[test]
    fn cache_serves_until_ttl_or_new_snapshot() {
        let built = Instant::now();
        let cache = ProfitCache { built, generation: 4, rows: Arc::new(Vec::new()) };
        assert!(cache.fresh(built, 4));
        assert!(cache.fresh(built + PROFIT_TTL - Duration::from_millis(1), 4));
        assert!(!cache.fresh(built + PROFIT_TTL, 4), "TTL past: rebuild");
        assert!(!cache.fresh(built, 5), "new snapshot: rebuild");
    }

    /// The signed balance prices: cooked shrimps sell, raw shrimps and
    /// burnt ones cost (§8); one row per method band, never a sum of
    /// every object a script ever saw.
    #[test]
    fn profit_prices_the_signed_balance() {
        let store = Store::open_in_memory().expect("store");
        for (item_id, name) in [(315, "Shrimps"), (317, "Raw shrimps")] {
            store.upsert_item(&lifescape_store::Item { item_id, name: name.to_string(), shop_price: 4, alch_value: 1 }).expect("item");
        }
        let mut snapshot = Snapshot::new();
        let key = YieldKey { method: "cook:shrimp".to_string(), style: String::new(), site: "3209,3215,0".to_string(), tool: String::new(), band: 1 };
        for _ in 0..3 {
            let sample = RunSample {
                work_ticks: 3000,
                pause_ticks: 0,
                flows: vec![("Shrimps".to_string(), "product".to_string(), 200), ("Raw shrimps".to_string(), "input".to_string(), -250), ("Coins".to_string(), "traded".to_string(), 999)],
                units: vec![("product".to_string(), "Shrimps".to_string(), 200)],
                clock: 1,
            };
            snapshot.book.record(&key, &sample);
        }
        let rows = profit_rows(&store, &snapshot);
        assert_eq!(rows.len(), 1, "{rows:?}");
        let row = &rows[0];
        assert_eq!((row.method.as_str(), row.band), ("cook:shrimp", 1), "keyed by activity");
        assert!(row.input_cost > 0.0 && row.revenue > 0.0);
        assert!(!row.prior, "three runs");
        let (shrimp, _) = price_estimate(&store, "Shrimps");
        assert!(row.revenue < 600.0 * shrimp * 1.5, "traded coins never count: {row:?}");
    }
}
