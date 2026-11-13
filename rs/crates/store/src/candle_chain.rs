#![forbid(unsafe_code)]
//! Continuous candles (user rule 2026-11-13: "des bougies cohérentes,
//! pas de trou"): every candle opens at the previous candle's close (its
//! high/low widen to include that open), and every window without a fill
//! is a flat candle at the last close (volume 0), from the first known
//! price up to the live window. The `candles` table keeps the raw
//! per-window aggregates; the chain is applied when history is read, so
//! the trailing flats always reach "now".

use crate::{Candle, Error, Store, YieldCandleResolution};

/// First window start at or after `clock` (`candles_for` reads
/// `window_start >= from`).
fn first_window_from(clock: i64, width: i64) -> i64 {
    clock.div_euclid(width) * width + if clock.rem_euclid(width) == 0 { 0 } else { width }
}

/// Window holding the last clock before the half-open edge `to_clock`.
fn last_window_before(to_clock: i64, width: i64) -> i64 {
    (to_clock - 1).div_euclid(width) * width
}

/// Chains sorted raw candles over the windows `first_window..=last_window`.
/// `previous_close` is the last price before `first_window` (carries the
/// leading empty windows and opens the first candle); without it the chain
/// starts at the first raw candle. Raw candles outside the range are
/// ignored. Pure helper, unit-testable.
pub fn chain_candles(
    item_id: i64,
    resolution: YieldCandleResolution,
    raw: &[Candle],
    previous_close: Option<i64>,
    first_window: i64,
    last_window: i64,
) -> Vec<Candle> {
    let width = resolution.width_seconds();
    let mut traded = raw
        .iter()
        .filter(|candle| candle.window_start >= first_window && candle.window_start <= last_window)
        .peekable();
    let mut window = match (previous_close, traded.peek()) {
        (Some(_), _) => first_window,
        (None, Some(first)) => first.window_start,
        (None, None) => return Vec::new(),
    };
    let mut close = previous_close;
    let mut chained = Vec::new();
    while window <= last_window {
        let here = match traded.peek() {
            Some(candle) if candle.window_start == window => traded.next(),
            _ => None,
        };
        match (here, close) {
            (Some(candle), Some(previous)) => {
                let mut linked = candle.clone();
                linked.open_price = previous;
                linked.high_price = linked.high_price.max(previous);
                linked.low_price = linked.low_price.min(previous);
                close = Some(candle.close_price);
                chained.push(linked);
            }
            (Some(candle), None) => {
                close = Some(candle.close_price);
                chained.push(candle.clone());
            }
            (None, Some(previous)) => chained.push(Candle {
                item_id,
                resolution: resolution.as_str().to_string(),
                window_start: window,
                open_price: previous,
                high_price: previous,
                low_price: previous,
                close_price: previous,
                volume: 0,
                buy_volume: 0,
                sell_volume: 0,
            }),
            (None, None) => {}
        }
        window += width;
    }
    chained
}

impl Store {
    /// History candles of one item over `[from_clock, to_clock)`: the
    /// cached (or rebuilt) raw candles, chained open-on-previous-close with
    /// one flat candle per trade-less window, the last window being the
    /// one holding `to_clock - 1` (callers cap `to_clock` at the live edge).
    /// Returns the candles and the close carried in from before the range.
    pub fn continuous_candles(
        &mut self,
        item_id: i64,
        resolution: YieldCandleResolution,
        from_clock: i64,
        to_clock: i64,
    ) -> Result<(Vec<Candle>, Option<i64>), Error> {
        let width = resolution.width_seconds();
        let first_window = first_window_from(from_clock, width);
        let last_window = last_window_before(to_clock, width);
        let raw = self.candles_or_rebuild(item_id, resolution, first_window, last_window + width)?;
        let previous_close = self.last_price_before(item_id, first_window)?;
        let chained = chain_candles(item_id, resolution, &raw, previous_close, first_window, last_window);
        Ok((chained, previous_close))
    }
}

#[cfg(test)]
mod candle_chain_tests {
    use super::*;
    use crate::NewExecution;

    const HOUR: i64 = 3600;

    fn raw(window: i64, open: i64, high: i64, low: i64, close: i64, volume: i64) -> Candle {
        Candle {
            item_id: 1017,
            resolution: "1h".to_string(),
            window_start: window,
            open_price: open,
            high_price: high,
            low_price: low,
            close_price: close,
            volume,
            buy_volume: 0,
            sell_volume: 0,
        }
    }

    fn ohlcv(candles: &[Candle]) -> Vec<(i64, i64, i64, i64, i64, i64)> {
        candles
            .iter()
            .map(|candle| {
                (
                    candle.window_start,
                    candle.open_price,
                    candle.high_price,
                    candle.low_price,
                    candle.close_price,
                    candle.volume,
                )
            })
            .collect()
    }

    /// Wizards hat live 2026-11-13: 135, then 101 the next hour, nothing
    /// for an hour, then 59 twice. Each candle opens at the previous close
    /// (101 and 59 are red), the empty hour is a flat 101, and flats run on
    /// to the live hour.
    #[test]
    fn wizards_hat_opens_on_previous_close_without_gaps() {
        let base = 498_492 * HOUR;
        let stored = vec![
            raw(base, 135, 135, 135, 135, 1),
            raw(base + HOUR, 101, 101, 101, 101, 1),
            raw(base + 3 * HOUR, 59, 59, 59, 59, 2),
        ];
        let chained = chain_candles(1017, YieldCandleResolution::Hour1, &stored, None, base - 2 * HOUR, base + 4 * HOUR);
        assert_eq!(
            ohlcv(&chained),
            vec![
                (base, 135, 135, 135, 135, 1),
                (base + HOUR, 135, 135, 101, 101, 1),
                (base + 2 * HOUR, 101, 101, 101, 101, 0),
                (base + 3 * HOUR, 101, 101, 59, 59, 2),
                (base + 4 * HOUR, 59, 59, 59, 59, 0),
            ],
            "no price before the first trade: the chain starts there"
        );
    }

    /// Bronze axe live 2026-11-13: 16 16 12 20 then 8 8 8 6. The second
    /// hour opens at 20 (the first hour's close), not at its first fill.
    #[test]
    fn bronze_axe_second_hour_opens_at_the_first_close() {
        let base = 498_494 * HOUR;
        let stored = vec![raw(base, 16, 20, 12, 20, 4), raw(base + HOUR, 8, 8, 6, 6, 4)];
        let chained = chain_candles(1351, YieldCandleResolution::Hour1, &stored, None, base, base + HOUR);
        assert_eq!(ohlcv(&chained), vec![(base, 16, 20, 12, 20, 4), (base + HOUR, 20, 20, 6, 6, 4)]);
    }

    /// A close from before the range opens the first window and carries
    /// the leading empty ones; no known price at all draws nothing.
    #[test]
    fn previous_close_carries_the_leading_windows() {
        let base = 1_000 * HOUR;
        let stored = vec![raw(base + HOUR, 9, 12, 9, 11, 3)];
        let chained = chain_candles(1, YieldCandleResolution::Hour1, &stored, Some(7), base, base + HOUR);
        assert_eq!(ohlcv(&chained), vec![(base, 7, 7, 7, 7, 0), (base + HOUR, 7, 12, 7, 11, 3)]);
        assert!(chain_candles(1, YieldCandleResolution::Hour1, &[], None, base, base + 5 * HOUR).is_empty());
        let flat = chain_candles(1, YieldCandleResolution::Hour1, &[], Some(4), base, base + HOUR);
        assert_eq!(ohlcv(&flat), vec![(base, 4, 4, 4, 4, 0), (base + HOUR, 4, 4, 4, 4, 0)], "price known, no trade: flats");
    }

    /// Store path: the close before `from` comes from executions, windows
    /// align on the resolution, the last window holds `to - 1`.
    #[test]
    fn store_serves_continuous_candles() -> Result<(), Error> {
        let mut store = Store::open_in_memory()?;
        crate::ensure_test_item(&store, 314)?;
        let fill = |clock: i64, price: i64| NewExecution {
            item_id: 314,
            quantity: 1,
            price,
            clock,
            server_tick: clock,
            taker_side: None,
        };
        let base = 2_000 * HOUR;
        store.record_execution(&fill(base - 5 * HOUR, 30))?;
        store.record_execution(&fill(base + HOUR + 10, 40))?;
        let (candles, previous_close) =
            store.continuous_candles(314, YieldCandleResolution::Hour1, base - 30, base + 2 * HOUR + 1)?;
        assert_eq!(previous_close, Some(30));
        assert_eq!(
            ohlcv(&candles),
            vec![
                (base, 30, 30, 30, 30, 0),
                (base + HOUR, 30, 40, 30, 40, 1),
                (base + 2 * HOUR, 40, 40, 40, 40, 0),
            ]
        );
        Ok(())
    }
}
