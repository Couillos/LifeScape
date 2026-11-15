#![forbid(unsafe_code)]
//! Current price and OHLCV buckets. The current price is what a player
//! reads (user 2026-11-14): the median of the last five fills, clamped
//! by the impact prices of the live book (the price to fill the usual
//! lot, never a dust order's); never traded, the book (midpoint, else
//! its one side), then 1 coin (PRD3: NPC shop prices never anchor the market).
//! The VWAP stays for the market page's 24 h change only (display).
//! Buckets align on multiples of the width since the epoch, in clock
//! seconds; every fill also carries its server tick for later joins.

/// One settled fill, enough to price and to bucket.
#[derive(Clone, Debug, PartialEq)]
pub struct PricedFill {
    pub quantity: i64,
    pub price: i64,
    pub clock: i64,
}

/// One OHLCV bucket over a clock window.
#[derive(Clone, Debug, PartialEq)]
pub struct Candle {
    pub window_start: i64,
    pub open: i64,
    pub high: i64,
    pub low: i64,
    pub close: i64,
    pub volume: i64,
}

/// Quantity-weighted average price, rounded to the nearest coin.
pub fn vwap(fills: &[PricedFill]) -> Option<i64> {
    let mut coins: i128 = 0;
    let mut qty: i128 = 0;
    for fill in fills {
        if fill.quantity <= 0 || fill.price < 1 {
            continue;
        }
        coins += (fill.quantity as i128) * (fill.price as i128);
        qty += fill.quantity as i128;
    }
    if qty == 0 {
        return None;
    }
    Some(((coins + qty / 2) / qty).max(1) as i64)
}

/// Current price with its provenance: `traded` when it comes from real
/// fills.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reference {
    pub price: i64,
    pub traded: bool,
}

/// Fills a player looks at to price an object (user 2026-11-14: "the
/// last trades I saw", never a 24 h average).
pub const PRICE_FILLS: usize = 5;

/// One of the last fills the current price reads: its price and its lot
/// size (the impact size comes from the lot sizes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LastFill {
    pub price: i64,
    pub quantity: i64,
}

/// Largest impact size: past 1000 units a lot no longer says more about
/// what a meaningful order is (user 2026-11-14).
pub const IMPACT_CAP: i64 = 1000;

/// Median of a non-empty list; an even count averages its two middles,
/// rounded up.
fn median_of(mut values: Vec<i64>) -> Option<i64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let middle = values.len() / 2;
    Some(if values.len() % 2 == 1 { values[middle] } else { (values[middle - 1] + values[middle] + 1) / 2 })
}

/// Median of the last `PRICE_FILLS` prices (`prices` oldest first, any
/// age); an even count averages its two middles. `None` without fills.
pub fn median_price(prices: &[i64]) -> Option<i64> {
    let start = prices.len().saturating_sub(PRICE_FILLS);
    median_of(prices[start..].iter().copied().filter(|price| *price >= 1).collect()).map(|median| median.max(1))
}

/// Impact size of an item (user 2026-11-14, the derivatives "impact
/// notional"): the median quantity of its last `PRICE_FILLS` fills, at
/// least 1, at most `IMPACT_CAP`; never traded, 1 unit.
pub fn impact_size(last_fills: &[LastFill]) -> i64 {
    let start = last_fills.len().saturating_sub(PRICE_FILLS);
    let quantities = last_fills[start..].iter().filter(|fill| fill.price >= 1 && fill.quantity >= 1).map(|fill| fill.quantity).collect();
    median_of(quantities).unwrap_or(1).clamp(1, IMPACT_CAP)
}

/// Impact price of one side: the price of the `size`-th unit walking the
/// resting `levels` (price, quantity) from the best. `None` when the
/// side holds fewer than `size` units (a thin side sets no price).
pub fn impact_price(levels: &[(i64, i64)], size: i64) -> Option<i64> {
    let mut covered = 0;
    for (price, quantity) in levels.iter().copied().filter(|(price, quantity)| *price >= 1 && *quantity >= 1) {
        covered += quantity;
        if covered >= size.max(1) {
            return Some(price);
        }
    }
    None
}

/// Current price as a player reads it (user 2026-11-14, replaces the
/// 24 h VWAP reference in every bot decision): the median of the last
/// five fills (`last_fills`, oldest first, any age), clamped into
/// [impact bid, impact ask] (the HKEX nominal price, with the
/// derivatives' impact prices instead of the top of the book: one unit
/// listed at 1 coin no longer drags an item traded in lots of 100 down
/// to 1). The impact size is `impact_size(last_fills)`; a side holding
/// fewer units does not clamp. `bids` / `asks` are the resting levels
/// (price, quantity) best first; a caller may stop once the impact size
/// is covered. Never traded: the book midpoint, else its one side, else
/// 1 coin (`traded: false`: the opening rules of the callers apply,
/// `zip::seller_open` opens at twice the reserve).
pub fn current_price(last_fills: &[LastFill], bids: &[(i64, i64)], asks: &[(i64, i64)]) -> Reference {
    let start = last_fills.len().saturating_sub(PRICE_FILLS);
    let prices: Vec<i64> = last_fills[start..].iter().map(|fill| fill.price).collect();
    let size = impact_size(last_fills);
    let impact_bid = impact_price(bids, size);
    let impact_ask = impact_price(asks, size);
    if let Some(median) = median_price(&prices) {
        let mut price = median;
        if let Some(bid) = impact_bid.filter(|bid| *bid > price) {
            price = bid;
        }
        if let Some(ask) = impact_ask.filter(|ask| *ask < price) {
            price = ask;
        }
        return Reference { price: price.max(1), traded: true };
    }
    let best = |levels: &[(i64, i64)]| levels.iter().find(|(price, quantity)| *price >= 1 && *quantity >= 1).map(|(price, _)| *price);
    let bid = impact_bid.or_else(|| best(bids));
    let ask = impact_ask.or_else(|| best(asks));
    let price = match (bid, ask) {
        (Some(bid), Some(ask)) => (bid + ask) / 2,
        (Some(side), None) | (None, Some(side)) => side,
        (None, None) => 1,
    };
    Reference { price: price.max(1), traded: false }
}

/// 24 h change of the displayed reference (thousandths): today's VWAP
/// against the previous 24 h VWAP, or against today's first print when
/// nothing traded the day before. Measured on the same VWAP the list shows
/// as its price (2026-11-13: wool listed at 2 showed +800 % because the
/// change compared the first print, 1, with a lone last print at 9).
pub fn change_thousandths(today: &[PricedFill], previous_day: &[PricedFill]) -> i64 {
    let Some(now) = vwap(today) else { return 0 };
    let before = vwap(previous_day).or_else(|| today.iter().find(|fill| fill.quantity > 0 && fill.price >= 1).map(|fill| fill.price));
    match before {
        Some(base) if base >= 1 => (((now - base) as f64 / base as f64) * 1000.0).round() as i64,
        _ => 0,
    }
}

/// Groups ordered fills into OHLCV buckets of `width_seconds`.
pub fn bucketize(fills: &[PricedFill], width_seconds: i64) -> Vec<Candle> {
    if width_seconds <= 0 {
        return Vec::new();
    }
    let mut buckets: Vec<Candle> = Vec::new();
    for fill in fills {
        if fill.quantity <= 0 || fill.price < 1 || fill.clock < 0 {
            continue;
        }
        let start = (fill.clock / width_seconds) * width_seconds;
        match buckets.last_mut() {
            Some(current) if current.window_start == start => {
                if fill.price > current.high {
                    current.high = fill.price;
                }
                if fill.price < current.low {
                    current.low = fill.price;
                }
                current.close = fill.price;
                current.volume += fill.quantity;
            }
            _ => buckets.push(Candle {
                window_start: start,
                open: fill.price,
                high: fill.price,
                low: fill.price,
                close: fill.price,
                volume: fill.quantity,
            }),
        }
    }
    buckets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill(qty: i64, price: i64, clock: i64) -> PricedFill {
        PricedFill { quantity: qty, price, clock }
    }

    #[test]
    fn change_follows_the_displayed_vwap() {
        // Wool 2026-11-13: listed at its VWAP 2, the old change compared
        // the first print (1) with a lone last print (9): +800 %.
        let day_before = [fill(100, 1, -5)];
        assert_eq!(change_thousandths(&[fill(10, 1, 1), fill(10, 3, 2)], &day_before), 1000, "1 → 2 is +100 %");
        assert_eq!(change_thousandths(&[fill(10, 1, 1), fill(10, 3, 2)], &[]), 1000, "no previous day: first print is the base");
        assert_eq!(change_thousandths(&[], &day_before), 0, "nothing traded today: no change");
        assert_eq!(change_thousandths(&[fill(5, 4, 1)], &[fill(5, 4, -1)]), 0);
    }

    #[test]
    fn vwap_weights_by_quantity() {
        assert_eq!(vwap(&[fill(100, 3, 1), fill(100, 5, 2)]), Some(4));
        assert_eq!(vwap(&[fill(300, 2, 1), fill(100, 6, 2)]), Some(3));
        assert_eq!(vwap(&[]), None);
    }

    fn lots(prices: &[i64], quantity: i64) -> Vec<LastFill> {
        prices.iter().map(|price| LastFill { price: *price, quantity }).collect()
    }

    #[test]
    fn current_price_is_the_median_of_five_fills_clamped_by_the_book() {
        let traded = |price| super::Reference { price, traded: true };
        // Only the last five count, whatever their age.
        assert_eq!(current_price(&lots(&[900, 1, 7, 8, 9, 10, 11], 1), &[], &[]), traded(9));
        assert_eq!(current_price(&lots(&[3, 9], 1), &[], &[]), traded(6), "even count: the middles averaged");
        assert_eq!(current_price(&lots(&[5, 5, 5], 1), &[(8, 1)], &[]), traded(8), "a richer bid is the price now");
        assert_eq!(current_price(&lots(&[5, 5, 5], 1), &[], &[(3, 1)]), traded(3), "a cheaper ask is the price now");
        assert_eq!(current_price(&lots(&[5, 5, 5], 1), &[(4, 1)], &[(6, 1)]), traded(5), "the book inside the fills: the fills");
    }

    #[test]
    fn impact_size_is_the_median_lot_capped() {
        assert_eq!(impact_size(&[]), 1, "never traded: one unit");
        assert_eq!(impact_size(&lots(&[5, 5, 5], 100)), 100);
        assert_eq!(impact_size(&[LastFill { price: 5, quantity: 1 }, LastFill { price: 5, quantity: 100 }, LastFill { price: 5, quantity: 300 }]), 100);
        assert_eq!(impact_size(&lots(&[5], 50_000)), IMPACT_CAP, "capped at 1000");
        assert_eq!(impact_size(&lots(&[5, 5, 5, 5, 5, 5, 5], 3)), 3);
        assert_eq!(impact_price(&[(1, 1), (4, 60), (6, 100)], 100), Some(6), "the 100th unit sits at 6");
        assert_eq!(impact_price(&[(1, 1), (4, 99)], 100), Some(4), "exactly covered");
        assert_eq!(impact_price(&[(1, 1), (4, 98)], 100), None, "thin side: no impact price");
    }

    #[test]
    fn a_dust_ask_does_not_move_a_lot_traded_item() {
        // The historic 1-coin ratchet: one unit listed at 1 coin used to
        // price feathers traded in lots of 100 at 1 for every bot.
        let feathers = lots(&[4, 4, 5, 4, 4], 100);
        let dust = [(1, 1), (4, 500)];
        assert_eq!(current_price(&feathers, &[], &dust).price, 4, "the 100th unit is at 4");
        assert_eq!(current_price(&feathers, &[], &[(1, 1)]).price, 4, "a lone dust ask is a thin side");
        // A real deep ask below the median is the price now.
        assert_eq!(current_price(&feathers, &[], &[(1, 1), (2, 150)]).price, 2);
        // Lots of one: the single unit at 1 coin is the whole impact size.
        assert_eq!(current_price(&lots(&[4, 4, 4], 1), &[], &[(1, 1)]).price, 1);
    }

    #[test]
    fn a_thin_bid_side_is_ignored() {
        let feathers = lots(&[4, 4, 4], 100);
        assert_eq!(current_price(&feathers, &[(9, 10), (8, 20)], &[]).price, 4, "30 units of bids do not lift a lot of 100");
        assert_eq!(current_price(&feathers, &[(9, 10), (7, 90), (2, 500)], &[]).price, 7, "a deep bid above the median lifts it to its 100th unit");
        assert_eq!(current_price(&feathers, &[(3, 100)], &[]).price, 4, "a deep bid below the median: the fills");
    }

    #[test]
    fn never_traded_reads_the_book_then_one_coin() {
        let quoted = |price| super::Reference { price, traded: false };
        assert_eq!(current_price(&[], &[(4, 1)], &[(6, 1)]), quoted(5), "never traded: the midpoint");
        assert_eq!(current_price(&[], &[(4, 1)], &[]), quoted(4), "never traded, one side: that side");
        assert_eq!(current_price(&[], &[], &[(9, 3), (12, 5)]), quoted(9), "impact size 1: the best ask");
        assert_eq!(current_price(&[], &[], &[]), quoted(1), "never traded: 1 coin, no shop anchor");
    }

    #[test]
    fn buckets_split_on_the_hour_and_track_ohlc() {
        let fills = vec![fill(100, 3, 36000 + 60), fill(50, 5, 36000 + 120), fill(10, 7, 39600 + 10)];
        let candles = bucketize(&fills, 3600);
        assert_eq!(candles.len(), 2);
        assert_eq!(
            candles[0],
            Candle { window_start: 36000, open: 3, high: 5, low: 3, close: 5, volume: 150 }
        );
        assert_eq!(candles[1].open, 7);
    }
}
