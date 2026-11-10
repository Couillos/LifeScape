#![forbid(unsafe_code)]
//! Settled executions (source of truth) and OHLCV candles rebuilt from them.
//!
//! Clocks are unix seconds; each execution also carries the server tick so
//! price history can be crossed with tick-based yield measurements.

use crate::{Error, Store};

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
}

/// One OHLCV bucket.
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
            "INSERT INTO executions(item_id, quantity, price, clock, server_tick) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                new_execution.item_id,
                new_execution.quantity,
                new_execution.price,
                new_execution.clock,
                new_execution.server_tick,
            ],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    /// Fills for one item in a clock range, oldest first.
    pub fn executions_for_item(
        &self,
        item_id: i64,
        from_clock: i64,
        to_clock: i64,
    ) -> Result<Vec<Execution>, Error> {
        let mut statement = self.connection.prepare(
            "SELECT exec_id, item_id, quantity, price, clock, server_tick \
             FROM executions WHERE item_id = ?1 AND clock >= ?2 AND clock < ?3 \
             ORDER BY clock, exec_id",
        )?;
        let mapped = statement.query_map(
            rusqlite::params![item_id, from_clock, to_clock],
            |row| {
                Ok(Execution {
                    exec_id: row.get(0)?,
                    item_id: row.get(1)?,
                    quantity: row.get(2)?,
                    price: row.get(3)?,
                    clock: row.get(4)?,
                    server_tick: row.get(5)?,
                })
            },
        )?;
        let mut fills = Vec::new();
        for mapped_row in mapped {
            fills.push(mapped_row?);
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
             low_price, close_price, volume FROM candles \
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
                 high_price, low_price, close_price, volume) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    bucket.item_id,
                    bucket.resolution,
                    bucket.window_start,
                    bucket.open_price,
                    bucket.high_price,
                    bucket.low_price,
                    bucket.close_price,
                    bucket.volume,
                ],
            )?;
        }
        transaction
            .commit()
            .map_err(|source| Error::Sql(source.to_string()))?;
        Ok(rebuilt)
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
        }
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
