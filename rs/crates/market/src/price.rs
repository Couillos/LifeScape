#![forbid(unsafe_code)]
//! Reference price and OHLCV buckets. The reference prefers traded
//! reality (VWAP, then the last print ever), then the live book
//! midpoint, then 1 coin: an object nobody ever traded is worth 1 until
//! buyers bid it up (PRD3: NPC shop prices never anchor the market).
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

/// Reference price with its provenance: `traded` when it comes from
/// real fills (only then does the 5x / 0.2x guard band apply).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reference {
    pub price: i64,
    pub traded: bool,
}

/// Reference price: VWAP of `recent`, else the `last_print` ever, else
/// the book midpoint (both sides needed), else 1 coin.
pub fn reference_price(recent: &[PricedFill], last_print: Option<i64>, best_bid: Option<i64>, best_ask: Option<i64>) -> Reference {
    if let Some(average) = vwap(recent) {
        return Reference { price: average.max(1), traded: true };
    }
    if let Some(last) = last_print.filter(|price| *price >= 1) {
        return Reference { price: last, traded: true };
    }
    if let (Some(bid), Some(ask)) = (best_bid, best_ask) {
        if bid >= 1 && ask >= 1 {
            return Reference { price: ((bid + ask) / 2).max(1), traded: false };
        }
    }
    Reference { price: 1, traded: false }
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

    #[test]
    fn reference_falls_back_in_order() {
        let traded = |price| super::Reference { price, traded: true };
        let quoted = |price| super::Reference { price, traded: false };
        assert_eq!(reference_price(&[fill(10, 7, 1)], Some(3), Some(1), Some(2)), traded(7));
        assert_eq!(reference_price(&[], Some(3), Some(4), Some(6)), traded(3), "last print beats the book");
        assert_eq!(reference_price(&[], None, Some(4), Some(6)), quoted(5));
        assert_eq!(reference_price(&[], None, Some(4), None), quoted(1), "one-sided book: never traded, 1 coin");
        assert_eq!(reference_price(&[], None, None, None), quoted(1), "never traded: 1 coin, no shop anchor");
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
