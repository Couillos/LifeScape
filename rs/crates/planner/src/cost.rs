//! Tick costs (§5.2) with the player's coin rate. All inputs are plain
//! numbers the brain reads from public sources only (market reference,
//! published yields, trend scores): never another bot's state (D22).

/// `market:buy` cost in equivalent ticks: round trip to Varrock plus the
/// grind to pay the price at the player's own coin rate.
pub fn cost_market_buy(trip_ticks: u64, price: i64, qty: i64, coins_per_tick: f64) -> u64 {
    if qty <= 0 || price <= 0 {
        return trip_ticks.saturating_mul(2);
    }
    if coins_per_tick <= 0.0 {
        return u64::MAX / 2;
    }
    let grind = (price.max(0) as f64 * qty.max(0) as f64 / coins_per_tick).ceil() as u64;
    trip_ticks.saturating_mul(2).saturating_add(grind)
}

/// `farm:S` cost: quantity at the measured yield per 6000 ticks, plus
/// trips to the spot and bank.
pub fn cost_farm(qty: i64, yield_per_6000: f64, trip_ticks: u64) -> u64 {
    if qty <= 0 {
        return trip_ticks.saturating_mul(2);
    }
    if yield_per_6000 <= 0.0 {
        return u64::MAX / 2;
    }
    let work = (qty.max(0) as f64 / yield_per_6000 * 6000.0).ceil() as u64;
    work.saturating_add(trip_ticks.saturating_mul(2))
}

/// `craft:R` cost: summed input costs plus the craft work itself.
pub fn cost_craft(input_ticks: u64, qty: i64, yield_per_6000: f64, trip_ticks: u64) -> u64 {
    if yield_per_6000 <= 0.0 {
        return u64::MAX / 2;
    }
    let work = if qty <= 0 {
        0
    } else {
        (qty.max(0) as f64 / yield_per_6000 * 6000.0).ceil() as u64
    };
    input_ticks.saturating_add(work).saturating_add(trip_ticks)
}

/// Market/farm pick weighted by the fixed trait (D15): affinity 0 pays
/// 1.5x the market cost, affinity 1 only 0.5x. A 10x gap still decides
/// for every profile; a close call follows the personality. Iron never
/// reaches here (market alternatives are filtered at expansion).
pub fn choose_market_or_farm(market_ticks: u64, farm_ticks: u64, market_affinity: f64) -> bool {
    let affinity = market_affinity.clamp(0.0, 1.0);
    let weighted = market_ticks as f64 * (1.5 - affinity);
    weighted < farm_ticks as f64
}

/// Trend-adjusted profit pick for `make_money`: revenue plus the public
/// trend bonus (score × revenue × weight) minus friction, per method.
/// The caller feeds published profits and public trend scores only.
pub fn rank_money_method(profit: f64, trend_score: f64, revenue: f64, trend_weight: f64) -> f64 {
    profit + trend_score.clamp(-1.0, 1.0) * revenue * trend_weight
}

#[cfg(test)]
mod tests {
    use super::{choose_market_or_farm, cost_craft, cost_farm, cost_market_buy, rank_money_method};

    #[test]
    fn market_cost_is_trip_plus_grind() {
        // 200 ticks round trip + 3000 coins at 1 coin/tick.
        assert_eq!(cost_market_buy(100, 30, 100, 1.0), 200 + 3000);
        assert_eq!(cost_market_buy(100, 0, 10, 1.0), 200, "free lots cost the trip only");
        assert_eq!(cost_market_buy(100, 30, 100, 0.0), u64::MAX / 2, "no income means no market");
    }

    #[test]
    fn farm_cost_scales_with_yield() {
        assert_eq!(cost_farm(300, 300.0, 100), 6000 + 200);
        assert!(cost_farm(300, 150.0, 100) > cost_farm(300, 300.0, 100), "slower yield costs more");
        assert_eq!(cost_farm(300, 0.0, 100), u64::MAX / 2, "unknown yield is unusable");
    }

    #[test]
    fn craft_adds_inputs() {
        assert_eq!(cost_craft(1000, 100, 100.0, 50), 1000 + 6000 + 50);
    }

    #[test]
    fn affinity_weights_close_calls_only() {
        // PRD example: 1000 market vs 1200 farm. High affinity buys,
        // low affinity (x1.5 = 1500) farms.
        assert!(choose_market_or_farm(1000, 1200, 0.72), "market friend buys");
        assert!(!choose_market_or_farm(1000, 1200, 0.0), "market hater farms");
        assert!(choose_market_or_farm(100, 1200, 0.0), "a 10x gap still buys for everyone");
        assert!(!choose_market_or_farm(10000, 1200, 1.0), "a 10x gap still farms for everyone");
    }

    #[test]
    fn trend_bonus_lifts_shortage_items() {
        let base = rank_money_method(500.0, 0.0, 1000.0, 0.1);
        let shortage = rank_money_method(500.0, 0.6, 1000.0, 0.1);
        assert!((shortage - base - 60.0).abs() < 1e-9, "bonus = score x revenue x weight");
    }
}
