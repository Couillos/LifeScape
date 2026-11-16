//! Method profitability (split from `rates_service`, rule 4) and its
//! shared cache (CPU 2026-11-14: every seller quote, ready context and
//! bid draw rebuilt the whole table — ~900 catalogue loads per build,
//! ~800 ms of CPU per `seller_quote`).
//!
//! `profit_rows` prices each distinct item once per build.
//! `cached_profit_rows` serves the daemon's live table: rebuilt when the
//! rates snapshot changed (`invalidate`, every ingest) or after
//! `PROFIT_TTL` (market prints move the prices), else shared as is.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use lifescape_store::Store;

use super::{live_snapshot, price_estimate, profit_per_6000, Snapshot, NORMALIZE_TICKS, TREND_WEIGHT};

/// Longest a cached table serves without a rebuild (market prints).
pub const PROFIT_TTL: Duration = Duration::from_secs(10);

/// Public trend score of an item display name (24 h period preferred),
/// 0.0 when unscored.
pub fn trend_score(store: &Store, item: &str) -> f64 {
    let Ok(Some(item_id)) = store.item_id_by_name(item) else {
        return 0.0;
    };
    let scores = store.trends_for_item(item_id).unwrap_or_default();
    scores.iter().find(|trend| trend.period == "24h").map(|trend| trend.score).unwrap_or_else(|| {
        scores.first().map(|trend| trend.score).unwrap_or(0.0)
    })
}

/// One profitability row per (method, band): revenue at estimated
/// prices, input cost at shop prices, pause friction, trend bonus.
#[derive(Clone, Debug, PartialEq)]
pub struct ProfitRow {
    pub method: String,
    pub band: i64,
    pub revenue: f64,
    pub input_cost: f64,
    pub friction: f64,
    pub trend_bonus: f64,
    pub profit: f64,
    pub price_prior: bool,
}

/// Builds profitability from the snapshot, pricing through the store
/// (each distinct item priced and scored once per build).
pub fn profit_rows(store: &Store, snapshot: &Snapshot) -> Vec<ProfitRow> {
    let mut prices: HashMap<String, (f64, bool)> = HashMap::new();
    let mut trends: HashMap<String, f64> = HashMap::new();
    let mut price_of = |item: &str| *prices.entry(item.to_string()).or_insert_with(|| price_estimate(store, item));
    let mut bands: BTreeMap<(String, i64), Vec<(String, f64)>> = BTreeMap::new();
    for ((method, item, band), stat) in &snapshot.yields {
        bands.entry((method.clone(), *band)).or_default().push((item.clone(), stat.mean()));
    }
    let mut rows = Vec::new();
    for ((method, band), products) in bands {
        // The median run priced (`rates_runs`), never the sum of every
        // object the bucket ever saw (cook 16: 422 083 coins per 6000).
        let mut price_prior = false;
        let mut priced = |item: &str| {
            let (price, prior) = price_of(item);
            price_prior = price_prior || prior;
            price
        };
        let Some(run) = snapshot.runs.median(&method, band, &mut priced) else { continue };
        let revenue_items = run.revenue_items.clone();
        let revenue = run.revenue;
        let input_cost = run.input_cost;
        let mut trend_weighted = 0.0;
        for (item, (quantity, price)) in run.products.iter().zip(run.revenue_items.iter()) {
            let trend = *trends.entry(item.clone()).or_insert_with(|| trend_score(store, item));
            trend_weighted += quantity * price * trend;
        }
        let _ = &products;
        let pause_per = snapshot.pause_per_6000(&method, band);
        let score = if revenue > 0.0 { trend_weighted / revenue } else { 0.0 };
        let profit = profit_per_6000(&revenue_items, input_cost, pause_per, score, TREND_WEIGHT);
        let friction = pause_per * revenue / NORMALIZE_TICKS;
        let trend_bonus = score * revenue * TREND_WEIGHT;
        rows.push(ProfitRow {
            method: method.clone(),
            band,
            revenue,
            input_cost,
            friction,
            trend_bonus,
            profit,
            price_prior,
        });
    }
    rows.sort_by(|left, right| (left.method.clone(), left.band).cmp(&(right.method.clone(), right.band)));
    rows
}

/// Rates snapshot generation: bumped on every ingest.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// The snapshot changed: the next `cached_profit_rows` rebuilds.
pub fn invalidate() {
    GENERATION.fetch_add(1, Ordering::Relaxed);
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
/// open (no rows, no decision — as before the cache). Built under the
/// lock: concurrent callers wait for one build, never run their own.
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
    use super::*;

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

    /// The memoised build prices like one `price_estimate` per row did:
    /// same rows on a store with prints, bids and unknown items.
    #[test]
    fn memoised_build_matches_per_row_pricing() {
        let store = Store::open_in_memory().expect("store");
        for (item_id, name) in [(1511, "Logs"), (1521, "Oak logs"), (440, "Iron ore")] {
            store.upsert_item(&lifescape_store::Item { item_id, name: name.to_string(), shop_price: 4, alch_value: 1 }).expect("item");
        }
        let mut snapshot = Snapshot::new();
        let mut bucket = super::super::BucketStat::default();
        bucket.add(120.0);
        snapshot.yields.insert(("woodcut".to_string(), "Logs".to_string(), 1), bucket.clone());
        snapshot.yields.insert(("woodcut".to_string(), "Oak logs".to_string(), 3), bucket.clone());
        snapshot.yields.insert(("mining".to_string(), "Iron ore".to_string(), 3), bucket.clone());
        snapshot.yields.insert(("mining".to_string(), "Unknown rock".to_string(), 3), bucket.clone());
        snapshot.inputs.insert(("smelt".to_string(), "Iron ore".to_string(), 3), bucket);
        // One run per product (2026-11-15): two mining runs, two objects.
        snapshot.runs.record("woodcut", 1, vec![("Logs".to_string(), 120.0)]);
        snapshot.runs.record("woodcut", 3, vec![("Oak logs".to_string(), 120.0)]);
        snapshot.runs.record("mining", 3, vec![("Iron ore".to_string(), 120.0)]);
        snapshot.runs.record("mining", 3, vec![("Unknown rock".to_string(), 120.0)]);
        let rows = profit_rows(&store, &snapshot);
        assert_eq!(rows.len(), 3, "one row per (method, band)");
        for row in &rows {
            let mut price = |item: &str| price_estimate(&store, item).0;
            let median = snapshot.runs.median(&row.method, row.band, &mut price).expect("runs");
            assert_eq!(row.revenue, median.revenue, "{} {}: the median run, never the sum", row.method, row.band);
        }
        let mining = rows.iter().find(|row| row.method == "mining").expect("mining");
        let both: f64 = ["Iron ore", "Unknown rock"].iter().map(|item| 120.0 * price_estimate(&store, item).0).sum();
        assert!(mining.revenue < both, "two runs are not one run of both objects");
    }
}
