#![forbid(unsafe_code)]
//! Brain planning context (split from brain.rs, same behavior):
//! public-data inputs for node resolution (money-method pick, price and
//! yield context, wallet and bank gauges). Never another bot's state.

use crate::jobs::Job;
use lifescape_json::Value;
use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::{Life, Node, NodeKind};

/// Fixed round-trip estimate to Varrock for cost comparisons.
// shortcut: fixed 800-tick round trip; replaced by measured convene
// ETAs once the brain reads them (L8).
pub const TRIP_TICKS: u64 = 800;

/// Pure money-method pick over published profits: the trend bonus
/// (score × revenue × weight) can flip a close call toward a shortage
/// item without ever reading another bot.
pub fn choose_money(profits: &[(String, f64, f64, f64)], trend_weight: f64) -> Option<String> {
    let mut best: Option<(String, f64)> = None;
    for (method, profit, revenue, trend) in profits {
        let score = lifescape_planner::rank_money_method(*profit, trend.clamp(-1.0, 1.0), *revenue, trend_weight);
        if best.as_ref().is_none_or(|(_, top)| score > *top) {
            best = Some((method.clone(), score));
        }
    }
    best.map(|(method, _)| method)
}

fn display_of(item_debug: &str) -> String {
    lifescape_gamedata::objs_by_name(item_debug)
        .into_iter()
        .next()
        .and_then(|entry| entry.display.or(entry.name))
        .unwrap_or(item_debug)
        .to_string()
}

fn shop_floor(item_debug: &str) -> i64 {
    let mut floor = i64::MAX;
    for shop in lifescape_gamedata::f2p::f2p_shops() {
        for stock in &shop.stocks {
            if stock.obj == item_debug {
                // Static shop price lookup by debug name.
                if let Some(entry) = lifescape_gamedata::objs_by_name(item_debug).into_iter().next() {
                    floor = floor.min(entry.price.max(1));
                }
            }
        }
    }
    if floor == i64::MAX { 1 } else { floor }
}

/// Public-data planning context for one node: market reference (or shop
/// fallback), published yields (or cold-start priors), trend score.
pub(crate) fn ready_ctx(life: &Life, node: &Node) -> ReadyCtx {
    let mut ctx = ReadyCtx::blind(life.profile.market_affinity);
    ctx.ironman = life.profile.ironman;
    ctx.trip_ticks = TRIP_TICKS;
    let (item_debug, band) = match &node.kind {
        NodeKind::ObtainItems { item, .. } => (item.clone(), 1),
        NodeKind::ReachLevel { level, .. } => (String::new(), crate::rates_service::level_band(i64::from(*level))),
        _ => (String::new(), 1),
    };
    let snapshot = crate::rates_service::live_snapshot();
    if let Ok(store) = lifescape_store::Store::open(&crate::market_service::db_path()) {
        // Best published coin rate prices the grind (fallback 0.5).
        let profits = crate::rates_service::profit_rows(&store, &snapshot);
        if let Some(top) = profits.iter().map(|row| row.profit).max_by(|left, right| left.total_cmp(right)) {
            if top > 0.0 {
                ctx.coins_per_tick = (top / crate::rates_service::NORMALIZE_TICKS).max(0.01);
            }
        }
        if !item_debug.is_empty() {
            let display = display_of(&item_debug);
            let (price, _) = crate::rates_service::price_estimate(&store, &display);
            ctx.price = price.max(1.0) as i64;
            ctx.shop_price = shop_floor(&item_debug);
            let farm_method = node.alternatives.iter().find(|alt| alt.method.starts_with("farm:")).map(|alt| alt.method.clone()).unwrap_or_default();
            let key_item = if farm_method.starts_with("farm:") { display.clone() } else { display.clone() };
            ctx.yield_per_6000 = crate::rates_service::measured_yield(&snapshot, &farm_method, &key_item, band)
                .or_else(|| crate::rates_service::prior_for(&farm_method, &key_item.to_lowercase()).map(|(yield_per, _)| yield_per))
                .unwrap_or(200.0);
        } else if matches!(&node.kind, NodeKind::MakeMoney) {
            // Money leaves price the best published profit; the flip leg
            // stays a flat engagement (measured once merchants report).
            let _ = &snapshot;
        }
    }
    ctx
}

pub(crate) fn holdings_count(job: &Job, item_debug: &str) -> i64 {
    let display = display_of(item_debug).to_lowercase();
    let debug = item_debug.to_lowercase();
    let live = job.inner.try_lock().map(|inner| inner.live.clone()).unwrap_or(None).unwrap_or(Value::Null);
    let mut total = 0i64;
    for key in ["inv", "bank", "worn"] {
        for entry in live.get(key).as_arr().cloned().unwrap_or_default() {
            match &entry {
                Value::Obj(_) => {
                    let name = entry.get("name").as_str().unwrap_or("").to_lowercase();
                    if name == display || name == debug {
                        total += entry.get("count").as_i64().unwrap_or(1).max(0);
                    }
                }
                Value::Str(name) if name.to_lowercase() == display || name.to_lowercase() == debug => {
                    total += 1;
                }
                _ => {}
            }
        }
    }
    total
}

/// Distinct banked types with a positive count (the space-pressure gauge,
/// L8: cleanup at 55 of 60 F2P slots). Name or id keys the type, so both
/// live snapshot shapes count.
pub(crate) fn bank_types(job: &Job) -> usize {
    let live = job.inner.try_lock().map(|inner| inner.live.clone()).unwrap_or(None).unwrap_or(Value::Null);
    let mut types = std::collections::HashSet::new();
    for entry in live.get("bank").as_arr().cloned().unwrap_or_default() {
        match &entry {
            Value::Obj(_) => {
                if entry.get("count").as_i64().unwrap_or(1) <= 0 {
                    continue;
                }
                if let Some(name) = entry.get("name").as_str() {
                    types.insert(name.to_lowercase());
                } else if let Some(id) = entry.get("id").as_i64() {
                    types.insert(format!("id{id}"));
                }
            }
            Value::Str(name) => {
                types.insert(name.to_lowercase());
            }
            _ => {}
        }
    }
    types.len()
}

#[cfg(test)]
mod tests {
    use super::choose_money;

    #[test]
    fn trend_bonus_flips_a_close_money_call() {
        let profits = vec![
            ("gather:fishing/raw_lobster".to_string(), 500.0, 1000.0, 0.0),
            ("gather:woodcut/yew_logs".to_string(), 510.0, 1000.0, -0.5),
        ];
        assert_eq!(choose_money(&profits, 0.1).as_deref(), Some("gather:fishing/raw_lobster"), "flat profits pick the top");
        let shortage = vec![
            ("gather:fishing/raw_lobster".to_string(), 500.0, 1000.0, 0.6),
            ("gather:woodcut/yew_logs".to_string(), 510.0, 1000.0, -0.5),
        ];
        assert_eq!(choose_money(&shortage, 0.1).as_deref(), Some("gather:fishing/raw_lobster"), "shortage bonus keeps lobster");
        let flip = vec![
            ("gather:fishing/raw_lobster".to_string(), 500.0, 1000.0, -0.5),
            ("gather:woodcut/yew_logs".to_string(), 480.0, 1000.0, 0.8),
        ];
        assert_eq!(choose_money(&flip, 0.1).as_deref(), Some("gather:woodcut/yew_logs"), "trend flips the money call");
    }
}
