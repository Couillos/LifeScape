#![forbid(unsafe_code)]
//! Settled executions (source of truth) and OHLCV candles rebuilt from them.
//!
//! Clocks are unix seconds; each execution also carries the server tick so
//! price history can be crossed with tick-based yield measurements.

use crate::{Error, OrderSide, Store};

/// Candle resolution. Buckets align on multiples of the width since epoch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum YieldCandleResolution {
    Hour1,
    Day1,
}

impl YieldCandleResolution {
    /// Short code stored in the `candles` table.
    pub fn as_str(self) -> &'static str {
        match self {
            YieldCandleResolution::Hour1 => "1h",
            YieldCandleResolution::Day1 => "1d",
        }
    }

    /// Bucket width in clock seconds.
    pub fn width_seconds(self) -> i64 {
        match self {
            YieldCandleResolution::Hour1 => 3600,
            YieldCandleResolution::Day1 => 86400,
        }
    }
}

/// Caller-supplied fields for one settled fill.
#[derive(Clone, Debug, PartialEq)]
pub struct NewExecution {
    pub item_id: i64,
    pub quantity: i64,
    pub price: i64,
    pub clock: i64,
    pub server_tick: i64,
    /// Side of the order that consumed the book (the deal's taker);
    /// `None` only when the deal predates the split (unknown).
    pub taker_side: Option<OrderSide>,
}

/// Stored execution row.
#[derive(Clone, Debug, PartialEq)]
pub struct Execution {
    pub exec_id: i64,
    pub item_id: i64,
    pub quantity: i64,
    pub price: i64,
    pub clock: i64,
    pub server_tick: i64,
    /// `None`: settled before migration 4, side unknown.
    pub taker_side: Option<OrderSide>,
}

/// One OHLCV bucket. `volume` is the total; `buy_volume` (buyer took the
/// asks) and `sell_volume` (seller hit the bids) split the executions
/// whose taker side is known, so they may sum below `volume`.
#[derive(Clone, Debug, PartialEq)]
pub struct Candle {
    pub item_id: i64,
    pub resolution: String,
    pub window_start: i64,
    pub open_price: i64,
    pub high_price: i64,
    pub low_price: i64,
    pub close_price: i64,
    pub volume: i64,
    pub buy_volume: i64,
    pub sell_volume: i64,
}

fn map_candle(row: &rusqlite::Row<'_>) -> Result<Candle, rusqlite::Error> {
    Ok(Candle {
        item_id: row.get(0)?,
        resolution: row.get(1)?,
        window_start: row.get(2)?,
        open_price: row.get(3)?,
        high_price: row.get(4)?,
        low_price: row.get(5)?,
        close_price: row.get(6)?,
        volume: row.get(7)?,
        buy_volume: row.get(8)?,
        sell_volume: row.get(9)?,
    })
}

impl Store {
    /// Appends one settled fill, returns its id.
    pub fn record_execution(&self, new_execution: &NewExecution) -> Result<i64, Error> {
        if new_execution.quantity <= 0 {
            return Err(Error::Invalid(format!(
                "bad execution quantity {}",
                new_execution.quantity
            )));
        }
        if new_execution.price < 1 {
            return Err(Error::Invalid(format!(
                "bad execution price {}",
                new_execution.price
            )));
        }
        self.connection.execute(
            "INSERT INTO executions(item_id, quantity, price, clock, server_tick, taker_side) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                new_execution.item_id,
                new_execution.quantity,
                new_execution.price,
                new_execution.clock,
                new_execution.server_tick,
                new_execution.taker_side.map(OrderSide::as_str),
            ],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    /// Price of the last fill of one item strictly before `clock` (the
    /// close a flat, trade-less candle carries forward), `None` if none.
    pub fn last_price_before(&self, item_id: i64, clock: i64) -> Result<Option<i64>, Error> {
        let mut statement = self.connection.prepare(
            "SELECT price FROM executions WHERE item_id = ?1 AND clock < ?2 \
             ORDER BY clock DESC, exec_id DESC LIMIT 1",
        )?;
        let mut rows = statement.query(rusqlite::params![item_id, clock])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }

    /// Fills for one item in a clock range, oldest first.
    pub fn executions_for_item(
        &self,
        item_id: i64,
        from_clock: i64,
        to_clock: i64,
    ) -> Result<Vec<Execution>, Error> {
        let mut statement = self.connection.prepare(
            "SELECT exec_id, item_id, quantity, price, clock, server_tick, taker_side \
             FROM executions WHERE item_id = ?1 AND clock >= ?2 AND clock < ?3 \
             ORDER BY clock, exec_id",
        )?;
        let mut found_rows = statement.query(rusqlite::params![item_id, from_clock, to_clock])?;
        let mut fills = Vec::new();
        while let Some(row) = found_rows.next()? {
            fills.push(Execution {
                exec_id: row.get(0)?,
                item_id: row.get(1)?,
                quantity: row.get(2)?,
                price: row.get(3)?,
                clock: row.get(4)?,
                server_tick: row.get(5)?,
                taker_side: OrderSide::parse_taker(row.get(6)?)?,
            });
        }
        Ok(fills)
    }

    /// Every item id with at least one recorded execution, oldest
    /// source of truth for the "traded items only" list filter (24h
    /// volume hides goods whose last fill settled more than a day ago).
    pub fn items_with_executions(&self) -> Result<Vec<i64>, Error> {
        let mut statement = self
            .connection
            .prepare("SELECT DISTINCT item_id FROM executions ORDER BY item_id")?;
        let mapped = statement.query_map(rusqlite::params![], |row| row.get(0))?;
        let mut traded = Vec::new();
        for mapped_row in mapped {
            traded.push(mapped_row?);
        }
        Ok(traded)
    }

    /// Stored candles for one item and resolution in a window range.
    pub fn candles_for(
        &self,
        item_id: i64,
        resolution: YieldCandleResolution,
        from_window: i64,
        to_window: i64,
    ) -> Result<Vec<Candle>, Error> {
        let mut statement = self.connection.prepare(
            "SELECT item_id, resolution, window_start, open_price, high_price, \
             low_price, close_price, volume, buy_volume, sell_volume FROM candles \
             WHERE item_id = ?1 AND resolution = ?2 \
             AND window_start >= ?3 AND window_start < ?4 ORDER BY window_start",
        )?;
        let mapped = statement.query_map(
            rusqlite::params![item_id, resolution.as_str(), from_window, to_window],
            |row| map_candle(row).map_err(|source| source),
        )?;
        let mut buckets = Vec::new();
        for mapped_row in mapped {
            buckets.push(mapped_row?);
        }
        Ok(buckets)
    }

    /// Cached candles, rebuilt from executions when the cache holds
    /// nothing for the window but fills exist (post-restart backfill:
    /// old executions predate the live repaint, so their items would
    /// otherwise serve zero candles forever).
    pub fn candles_or_rebuild(
        &mut self,
        item_id: i64,
        resolution: YieldCandleResolution,
        from_window: i64,
        to_window: i64,
    ) -> Result<Vec<Candle>, Error> {
        let cached = self.candles_for(item_id, resolution, from_window, to_window)?;
        if !cached.is_empty() || self.executions_for_item(item_id, from_window, to_window)?.is_empty() {
            return Ok(cached);
        }
        self.rebuild_candles(item_id, resolution)?;
        self.candles_for(item_id, resolution, from_window, to_window)
    }

    /// Executions are settled handovers only (user rule 2026-11-13: the
    /// market shows what happened in game). Rebuilds the execution log
    /// from the settled deals (clock = settlement) and repaints every
    /// candle of every item it touched, before or after. Skipped when the
    /// log already matches the settled deals. Returns the rows kept.
    pub fn resync_executions_from_deals(&mut self) -> Result<usize, Error> {
        let settled: (i64, i64) = self.connection.query_row(
            "SELECT COUNT(*), COALESCE(SUM(quantity), 0) FROM deals WHERE status = 'settled'",
            rusqlite::params![],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let logged: (i64, i64) = self.connection.query_row(
            "SELECT COUNT(*), COALESCE(SUM(quantity), 0) FROM executions",
            rusqlite::params![],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if settled == logged {
            return Ok(settled.0 as usize);
        }
        let mut touched = self.items_with_executions()?;
        self.connection.execute_batch(
            "DELETE FROM executions; \
             INSERT INTO executions(item_id, quantity, price, clock, server_tick, taker_side) \
             SELECT item_id, quantity, price, updated_clock, updated_clock, taker_side FROM deals \
             WHERE status = 'settled' ORDER BY updated_clock, deal_id;",
        )?;
        touched.extend(self.items_with_executions()?);
        touched.sort_unstable();
        touched.dedup();
        for item in touched {
            self.rebuild_candles(item, YieldCandleResolution::Hour1)?;
            self.rebuild_candles(item, YieldCandleResolution::Day1)?;
        }
        Ok(settled.0 as usize)
    }

    /// Recomputes every candle of one item and resolution from executions.
    /// Needs `&mut` for the delete+insert transaction; everything else on
    /// this store takes `&self`.
    pub fn rebuild_candles(
        &mut self,
        item_id: i64,
        resolution: YieldCandleResolution,
    ) -> Result<Vec<Candle>, Error> {
        let fills = self.executions_for_item(item_id, 0, i64::MAX)?;
        let rebuilt = aggregate_fills(item_id, resolution, &fills);
        let transaction = self
            .connection
            .transaction()
            .map_err(|source| Error::Sql(source.to_string()))?;
        transaction.execute(
            "DELETE FROM candles WHERE item_id = ?1 AND resolution = ?2",
            rusqlite::params![item_id, resolution.as_str()],
        )?;
        for bucket in &rebuilt {
            transaction.execute(
                "INSERT INTO candles(item_id, resolution, window_start, open_price, \
                 high_price, low_price, close_price, volume, buy_volume, sell_volume) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    bucket.item_id,
                    bucket.resolution,
                    bucket.window_start,
                    bucket.open_price,
                    bucket.high_price,
                    bucket.low_price,
                    bucket.close_price,
                    bucket.volume,
                    bucket.buy_volume,
                    bucket.sell_volume,
                ],
            )?;
        }
        transaction
            .commit()
            .map_err(|source| Error::Sql(source.to_string()))?;
        Ok(rebuilt)
    }
}

/// Buy and sell share of one fill by its taker side; unknown counts in
/// neither (it stays in the total volume only).
fn sided_volume(fill: &Execution) -> (i64, i64) {
    match fill.taker_side {
        Some(OrderSide::Buy) => (fill.quantity, 0),
        Some(OrderSide::Sell) => (0, fill.quantity),
        None => (0, 0),
    }
}

/// Buckets ordered fills into OHLCV candles. Pure helper, unit-testable.
fn aggregate_fills(
    item_id: i64,
    resolution: YieldCandleResolution,
    fills: &[Execution],
) -> Vec<Candle> {
    let width = resolution.width_seconds();
    let mut buckets: Vec<Candle> = Vec::new();
    for fill in fills {
        let window_start = (fill.clock / width) * width;
        let (bought, sold) = sided_volume(fill);
        match buckets.last_mut() {
            Some(current) if current.window_start == window_start => {
                if fill.price > current.high_price {
                    current.high_price = fill.price;
                }
                if fill.price < current.low_price {
                    current.low_price = fill.price;
                }
                current.close_price = fill.price;
                current.volume += fill.quantity;
                current.buy_volume += bought;
                current.sell_volume += sold;
            }
            _ => buckets.push(Candle {
                item_id,
                resolution: resolution.as_str().to_string(),
                window_start,
                open_price: fill.price,
                high_price: fill.price,
                low_price: fill.price,
                close_price: fill.price,
                volume: fill.quantity,
                buy_volume: bought,
                sell_volume: sold,
            }),
        }
    }
    buckets
}

#[cfg(test)]
mod candles_tests {
    use super::*;

    fn sample_fill(clock: i64, price: i64, quantity: i64) -> NewExecution {
        NewExecution {
            item_id: 314,
            quantity,
            price,
            clock,
            server_tick: clock * 10,
            taker_side: None,
        }
    }

    /// Buy volume = units a buyer took off the asks, sell volume = units
    /// a seller hit on the bids; an unknown side (pre-split rows) stays
    /// in the total only.
    #[test]
    fn candles_split_volume_by_taker_side() -> Result<(), Error> {
        let mut store = Store::open_in_memory()?;
        crate::ensure_test_item(&store, 314)?;
        let sided = |clock: i64, quantity: i64, side: Option<crate::OrderSide>| NewExecution {
            taker_side: side,
            ..sample_fill(clock, 4, quantity)
        };
        store.record_execution(&sided(10 * 3600 + 1, 7, Some(crate::OrderSide::Buy)))?;
        store.record_execution(&sided(10 * 3600 + 2, 3, Some(crate::OrderSide::Sell)))?;
        store.record_execution(&sided(10 * 3600 + 3, 2, None))?;
        store.record_execution(&sided(10 * 3600 + 4, 1, Some(crate::OrderSide::Buy)))?;
        store.record_execution(&sided(11 * 3600, 6, Some(crate::OrderSide::Sell)))?;
        let fills = store.executions_for_item(314, 0, i64::MAX)?;
        assert_eq!(fills[1].taker_side, Some(crate::OrderSide::Sell), "execution keeps its side");
        assert_eq!(fills[2].taker_side, None);
        let rebuilt = store.rebuild_candles(314, YieldCandleResolution::Hour1)?;
        let split: Vec<(i64, i64, i64)> =
            rebuilt.iter().map(|candle| (candle.volume, candle.buy_volume, candle.sell_volume)).collect();
        assert_eq!(split, vec![(13, 8, 3), (6, 0, 6)]);
        assert_eq!(store.candles_for(314, YieldCandleResolution::Hour1, 0, i64::MAX)?, rebuilt, "stored split matches");
        Ok(())
    }

    #[test]
    fn last_price_before_reads_the_previous_close() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        crate::ensure_test_item(&store, 314)?;
        assert_eq!(store.last_price_before(314, 100)?, None);
        store.record_execution(&sample_fill(10, 3, 1))?;
        store.record_execution(&sample_fill(20, 7, 1))?;
        assert_eq!(store.last_price_before(314, 20)?, Some(3), "strictly before");
        assert_eq!(store.last_price_before(314, 21)?, Some(7));
        Ok(())
    }

    /// User rule 2026-11-13: the execution log is settled handovers only.
    /// Match-time rows of failed deals go; settled deals stay, at their
    /// settlement clock; candles follow; a second pass changes nothing.
    #[test]
    fn executions_resync_to_settled_deals() -> Result<(), Error> {
        let mut store = Store::open_in_memory()?;
        crate::ensure_test_item(&store, 314)?;
        let deal = |store: &Store, qty: i64, clock: i64| -> Result<i64, Error> {
            store.create_deal(&crate::NewDeal {
                buyer: "b".to_string(),
                seller: "s".to_string(),
                item_id: 314,
                quantity: qty,
                price: 2,
                meeting_x: 0,
                meeting_y: 0,
                meeting_level: 0,
                deadline_tick: 0,
                clock,
                taker_side: crate::OrderSide::Sell,
            })
        };
        let settled = deal(&store, 5, 10 * 3600)?;
        let failed = deal(&store, 7, 10 * 3600 + 30)?;
        store.set_deal_status(settled, crate::DealStatus::Settled, "", 10 * 3600 + 600)?;
        store.set_deal_status(failed, crate::DealStatus::Failed, "no_show", 10 * 3600 + 700)?;
        // The old match-time log printed both.
        store.record_execution(&sample_fill(10 * 3600, 2, 5))?;
        store.record_execution(&sample_fill(10 * 3600 + 30, 2, 7))?;
        store.rebuild_candles(314, YieldCandleResolution::Hour1)?;
        assert_eq!(store.resync_executions_from_deals()?, 1);
        let fills = store.executions_for_item(314, 0, i64::MAX)?;
        assert_eq!(fills.len(), 1, "only the settled handover: {fills:?}");
        assert_eq!((fills[0].quantity, fills[0].clock), (5, 10 * 3600 + 600));
        assert_eq!(fills[0].taker_side, Some(crate::OrderSide::Sell), "the deal's taker side carries over");
        let candles = store.candles_for(314, YieldCandleResolution::Hour1, 0, i64::MAX)?;
        assert_eq!(candles.iter().map(|candle| candle.volume).sum::<i64>(), 5, "candles repainted");
        assert_eq!(store.resync_executions_from_deals()?, 1, "idempotent");
        Ok(())
    }

    #[test]
    fn rebuild_candles_from_executions() -> Result<(), Error> {
        let mut store = Store::open_in_memory()?;
        crate::ensure_test_item(&store, 314)?;
        // Two fills in hour 10, one in hour 11 (clocks are epoch seconds).
        store.record_execution(&sample_fill(10 * 3600 + 60, 3, 100))?;
        store.record_execution(&sample_fill(10 * 3600 + 120, 5, 50))?;
        store.record_execution(&sample_fill(10 * 3600 + 300, 4, 25))?;
        store.record_execution(&sample_fill(11 * 3600 + 10, 7, 10))?;

        let rebuilt = store.rebuild_candles(314, YieldCandleResolution::Hour1)?;
        assert_eq!(rebuilt.len(), 2);
        assert_eq!(
            rebuilt[0],
            Candle {
                item_id: 314,
                resolution: "1h".to_string(),
                window_start: 10 * 3600,
                open_price: 3,
                high_price: 5,
                low_price: 3,
                close_price: 4,
                volume: 175,
                buy_volume: 0,
                sell_volume: 0,
            }
        );
        assert_eq!(rebuilt[1].open_price, 7);
        assert_eq!(rebuilt[1].volume, 10);

        // Stored rows match, and a second rebuild from scratch agrees.
        let stored = store.candles_for(314, YieldCandleResolution::Hour1, 0, i64::MAX)?;
        assert_eq!(stored, rebuilt);
        store.connection.execute("DELETE FROM candles", [])?;
        let rebuilt_again = store.rebuild_candles(314, YieldCandleResolution::Hour1)?;
        assert_eq!(rebuilt_again, rebuilt);
        Ok(())
    }
}
