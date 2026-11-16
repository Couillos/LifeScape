#![forbid(unsafe_code)]
//! Per-run flows behind a method's profit (2026-11-15, `bid-why`
//! diagnosis). The old profit row of a (method, band) summed the mean
//! yield of EVERY object ever seen in its bucket, as if one run made them
//! all: a cooking run cooks one fish, yet "cook band 16" added shrimps,
//! meat, trout, sardines, anchovies and a misattributed steel bar, 422 083
//! coins per 6000 ticks, the coin rate of every cook-16 bot (70 coins a
//! tick: no lot ever beat it, smiths bought steel bars rather than smelt
//! them). Here each run keeps its own flows (products positive, inputs
//! negative, per 6000 ticks); the profit is the median run's net value at
//! today's prices. The last `RUNS_KEPT` runs per bucket are kept and
//! persisted with the rates snapshot; a cold start rebuilds them from the
//! store's reports (`rebuild`).

use std::collections::{BTreeMap, VecDeque};

use lifescape_json::{object, str_value, Value};

/// Runs kept per (method, band).
pub const RUNS_KEPT: usize = 50;
/// Store reports read back at a cold start (seconds).
pub const REBUILD_WINDOW_SECS: i64 = 3 * 24 * 3600;

/// One run: (object display name, signed quantity per 6000 ticks).
pub type Run = Vec<(String, f64)>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Runs {
    pub by_bucket: BTreeMap<(String, i64), VecDeque<Run>>,
}

/// The median run of a bucket priced: (revenue, input cost, its product
/// rows `(per 6000, price)`).
#[derive(Clone, Debug, PartialEq)]
pub struct PricedRun {
    pub revenue: f64,
    pub input_cost: f64,
    pub revenue_items: Vec<(f64, f64)>,
    /// Object of each `revenue_items` row, same order.
    pub products: Vec<String>,
}

impl Runs {
    /// Adds one run (an empty run adds nothing).
    pub fn record(&mut self, method: &str, band: i64, run: Run) {
        if run.is_empty() {
            return;
        }
        let bucket = self.by_bucket.entry((method.to_string(), band)).or_default();
        bucket.push_back(run);
        while bucket.len() > RUNS_KEPT {
            bucket.pop_front();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.by_bucket.is_empty()
    }

    /// The bucket's median run by net value at `price_of` (`None`: no run).
    pub fn median(&self, method: &str, band: i64, price_of: &mut dyn FnMut(&str) -> f64) -> Option<PricedRun> {
        let bucket = self.by_bucket.get(&(method.to_string(), band))?;
        let mut priced: Vec<PricedRun> = bucket
            .iter()
            .map(|run| {
                let mut revenue_items = Vec::new();
                let mut products = Vec::new();
                let mut input_cost = 0.0;
                for (item, quantity) in run {
                    let price = price_of(item);
                    if *quantity >= 0.0 {
                        revenue_items.push((*quantity, price));
                        products.push(item.clone());
                    } else {
                        input_cost += quantity.abs() * price;
                    }
                }
                let revenue = revenue_items.iter().map(|(quantity, price)| quantity * price).sum();
                PricedRun { revenue, input_cost, revenue_items, products }
            })
            .collect();
        priced.sort_by(|left, right| (left.revenue - left.input_cost).total_cmp(&(right.revenue - right.input_cost)));
        priced.get(priced.len() / 2).cloned()
    }

    pub fn to_value(&self) -> Value {
        Value::Arr(
            self.by_bucket
                .iter()
                .map(|((method, band), runs)| {
                    object(vec![
                        ("method", str_value(method)),
                        ("band", Value::Int(*band)),
                        (
                            "runs",
                            Value::Arr(
                                runs.iter()
                                    .map(|run| Value::Arr(run.iter().map(|(item, quantity)| Value::Arr(vec![str_value(item), Value::Float(*quantity)])).collect()))
                                    .collect(),
                            ),
                        ),
                    ])
                })
                .collect(),
        )
    }

    pub fn from_value(value: &Value) -> Runs {
        let mut out = Runs::default();
        for bucket in value.as_arr().cloned().unwrap_or_default() {
            let method = bucket.get("method").as_str().unwrap_or("").to_string();
            let band = bucket.get("band").as_i64().unwrap_or(1);
            for run in bucket.get("runs").as_arr().cloned().unwrap_or_default() {
                let flows: Run = run
                    .as_arr()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|pair| {
                        let pair = pair.as_arr()?;
                        Some((pair.first()?.as_str()?.to_string(), pair.get(1)?.as_f64()?))
                    })
                    .collect();
                out.record(&method, band, flows);
            }
        }
        out
    }
}

/// One report's run: farm flows positive, consumed inputs negative, per
/// 6000 ticks of `work_ticks` (`None`: no work).
pub fn run_of(flows: &[(String, String, i64)], work_ticks: i64) -> Option<Run> {
    if work_ticks <= 0 {
        return None;
    }
    let mut run: Run = Vec::new();
    for (item, provenance, quantity) in flows {
        let Some(per) = crate::rates_service::normalize_per_6000(*quantity, work_ticks) else { continue };
        if crate::rates_service::is_farm_provenance(provenance) && per > 0.0 {
            run.push((item.clone(), per));
        } else if crate::rates_service::is_input_provenance(provenance) {
            run.push((item.clone(), -per.abs()));
        }
    }
    Some(run)
}

/// Cold start: the runs of the store's recent reports.
pub fn rebuild(store: &lifescape_store::Store, now_secs: i64) -> Runs {
    let mut out = Runs::default();
    for (method, ticks, level, flows) in store.report_flows_since(now_secs - REBUILD_WINDOW_SECS).unwrap_or_default() {
        if crate::rates_service::is_trading_method(&method) || crate::rates_service::is_unrated_method(&method) {
            continue;
        }
        if let Some(run) = run_of(&flows, ticks) {
            out.record(&method, crate::rates_service::level_band(level), run);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{run_of, Runs, RUNS_KEPT};

    /// Live cook band 16: five kinds of food in one bucket summed to a
    /// 422 083-coin row. The median run prices one run, never the sum.
    #[test]
    fn the_profit_is_one_run_never_every_product_summed() {
        let mut runs = Runs::default();
        runs.record("cook", 16, vec![("Shrimps".to_string(), 377.0), ("Raw shrimps".to_string(), -445.0)]);
        runs.record("cook", 16, vec![("Trout".to_string(), 136.0), ("Raw trout".to_string(), -207.0)]);
        runs.record("cook", 16, vec![("Cooked meat".to_string(), 521.0), ("Raw beef".to_string(), -684.0)]);
        let mut price = |item: &str| match item {
            "Shrimps" => 179.0,
            "Raw shrimps" => 77.0,
            "Trout" => 60.0,
            "Raw trout" => 40.0,
            "Cooked meat" => 4.0,
            "Raw beef" => 2.0,
            _ => 1.0,
        };
        let median = runs.median("cook", 16, &mut price).expect("runs");
        let net = median.revenue - median.input_cost;
        // Nets: trout -120, meat 716, shrimps 33 218: the middle is meat.
        assert!((net - (521.0 * 4.0 - 684.0 * 2.0)).abs() < 1e-6, "the middle run (meat), got {net}");
        assert!(median.revenue < 377.0 * 179.0 + 136.0 * 60.0, "never the sum");
    }

    #[test]
    fn buckets_keep_their_last_runs_and_round_trip() {
        let mut runs = Runs::default();
        for index in 0..RUNS_KEPT + 5 {
            runs.record("mining", 31, vec![("Coal".to_string(), index as f64)]);
        }
        assert_eq!(runs.by_bucket[&("mining".to_string(), 31)].len(), RUNS_KEPT);
        assert_eq!(Runs::from_value(&runs.to_value()), runs);
        let flows = vec![("Coal".to_string(), "produced".to_string(), 30), ("Bread".to_string(), "consumed".to_string(), -2), ("Coins".to_string(), "traded".to_string(), 5)];
        let run = run_of(&flows, 6000).expect("work");
        assert_eq!(run, vec![("Coal".to_string(), 30.0), ("Bread".to_string(), -2.0)]);
    }
}
