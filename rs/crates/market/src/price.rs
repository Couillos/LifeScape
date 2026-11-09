#![forbid(unsafe_code)]
//! Reference price and OHLCV buckets. The reference prefers traded
//! reality (VWAP), then the live book midpoint, then the shop price.
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

/// Reference price: VWAP, else book midpoint, else the shop price.
pub fn reference_price(
    recent: &[PricedFill],
    best_bid: Option<i64>,
    best_ask: Option<i64>,
    shop_price: i64,
) -> i64 {
    if let Some(average) = vwap(recent) {
        return average.max(1);
    }
    if let (Some(bid), Some(ask)) = (best_bid, best_ask) {
        if bid >= 1 && ask >= 1 {
            return ((bid + ask) / 2).max(1);
        }
    }
    shop_price.max(1)
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
    fn vwap_weights_by_quantity() {
        assert_eq!(vwap(&[fill(100, 3, 1), fill(100, 5, 2)]), Some(4));
        assert_eq!(vwap(&[fill(300, 2, 1), fill(100, 6, 2)]), Some(3));
        assert_eq!(vwap(&[]), None);
    }

    #[test]
    fn reference_falls_back_in_order() {
        assert_eq!(reference_price(&[fill(10, 7, 1)], Some(1), Some(2), 99), 7);
        assert_eq!(reference_price(&[], Some(4), Some(6), 99), 5);
        assert_eq!(reference_price(&[], Some(4), None, 99), 99);
        assert_eq!(reference_price(&[], None, None, 0), 1);
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
