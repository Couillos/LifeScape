//! Economy measurement queries (audit 2026-11-15, chantier 10): the coin
//! flows the task reports attribute (`task_yields` coin lines by
//! provenance, exact once the SDK coin ledger labels them), the money
//! supply series (`holdings_hourly`, staff excluded at capture) and the
//! fills behind a price index (`executions`).

use rusqlite::params;

use crate::{Error, Store};

/// Net coins per provenance in a window (the money's sources and sinks).
#[derive(Debug, Clone, PartialEq)]
pub struct CoinFlowTotal {
    pub provenance: String,
    pub coins: i64,
    pub lines: i64,
}

/// Volume-weighted mean fill price of one item in a window.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemFills {
    pub item_id: i64,
    pub quantity: i64,
    pub mean_price: f64,
}

impl Store {
    /// Coin lines of the task reports created in `[since, until)` (until
    /// 0 = now), summed by provenance, staff accounts left out.
    pub fn coin_flows_between(&self, since: i64, until: i64, excluded: &[String]) -> Result<Vec<CoinFlowTotal>, Error> {
        let mut statement = self.connection.prepare(
            "SELECT r.account, y.provenance, SUM(y.quantity), COUNT(*) FROM task_yields y JOIN task_reports r ON r.report_id = y.report_id \
             WHERE LOWER(y.item_name) = 'coins' AND r.created_clock >= ?1 AND (?2 <= 0 OR r.created_clock < ?2) \
             GROUP BY r.account, y.provenance",
        )?;
        let rows = statement.query_map(params![since, until], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?)))?;
        let mut out: Vec<CoinFlowTotal> = Vec::new();
        for row in rows {
            let (account, provenance, coins, lines) = row?;
            if excluded.iter().any(|name| name.eq_ignore_ascii_case(&account)) {
                continue;
            }
            match out.iter_mut().find(|total| total.provenance == provenance) {
                Some(total) => {
                    total.coins += coins;
                    total.lines += lines;
                }
                None => out.push(CoinFlowTotal { provenance, coins, lines }),
            }
        }
        out.sort_by(|left, right| right.coins.abs().cmp(&left.coins.abs()).then_with(|| left.provenance.cmp(&right.provenance)));
        Ok(out)
    }

    /// Item lines of the task reports created at or after `since`, summed
    /// by (item, provenance, sign): where every object came from and went
    /// (staff accounts left out; `item` filters one display name,
    /// case-insensitive).
    pub fn item_flows_since(&self, since: i64, excluded: &[String], item: Option<&str>) -> Result<Vec<(String, String, i64, i64)>, Error> {
        let mut statement = self.connection.prepare(
            "SELECT r.account, y.item_name, y.provenance, SUM(y.quantity), COUNT(*) FROM task_yields y JOIN task_reports r ON r.report_id = y.report_id \
             WHERE r.created_clock >= ?1 AND y.item_name != '' AND (?2 = '' OR LOWER(y.item_name) = LOWER(?2)) GROUP BY r.account, y.item_name, y.provenance, y.quantity > 0",
        )?;
        let rows = statement.query_map(params![since, item.unwrap_or("")], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, i64>(3)?, row.get::<_, i64>(4)?))
        })?;
        let mut out: Vec<(String, String, i64, i64)> = Vec::new();
        for row in rows {
            let (account, name, provenance, quantity, lines) = row?;
            if excluded.iter().any(|staff| staff.eq_ignore_ascii_case(&account)) {
                continue;
            }
            // In and out apart (user 2026-11-18: a trade between bots read
            // as a net sink): one row per name, provenance and sign.
            match out.iter_mut().find(|(known, origin, sum, _)| *known == name && *origin == provenance && (*sum > 0) == (quantity > 0)) {
                Some(entry) => {
                    entry.2 += quantity;
                    entry.3 += lines;
                }
                None => out.push((name, provenance, quantity, lines)),
            }
        }
        out.sort_by(|left, right| right.2.abs().cmp(&left.2.abs()).then_with(|| left.0.cmp(&right.0)));
        Ok(out)
    }

    /// Fills per item in `[since, until)`: quantity and mean price.
    pub fn fills_between(&self, since: i64, until: i64) -> Result<Vec<ItemFills>, Error> {
        let mut statement = self.connection.prepare(
            // Coins per unit: a price is per the item's lot (plan prix lot D).
            "SELECT e.item_id, SUM(e.quantity), SUM(e.quantity * e.price / COALESCE(i.lot, 1)) FROM executions e LEFT JOIN items i ON i.item_id = e.item_id \
             WHERE e.clock >= ?1 AND (?2 <= 0 OR e.clock < ?2) GROUP BY e.item_id HAVING SUM(e.quantity) > 0 ORDER BY e.item_id",
        )?;
        let rows = statement.query_map(params![since, until], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (item_id, quantity, value) = row?;
            out.push(ItemFills { item_id, quantity, mean_price: value as f64 / quantity as f64 });
        }
        Ok(out)
    }
}

/// Laspeyres price index of `now` against `base`: the base basket's
/// quantities priced now over priced then, items traded in both windows
/// only. `None` without a common item. 1.0 = flat, 1.1 = +10 %.
pub fn laspeyres(base: &[ItemFills], now: &[ItemFills]) -> Option<f64> {
    let mut then_value = 0.0;
    let mut now_value = 0.0;
    for old in base {
        let Some(new) = now.iter().find(|fills| fills.item_id == old.item_id) else { continue };
        then_value += old.mean_price * old.quantity as f64;
        now_value += new.mean_price * old.quantity as f64;
    }
    (then_value > 0.0).then(|| now_value / then_value)
}

#[cfg(test)]
mod tests {
    use super::{laspeyres, ItemFills};

    #[test]
    fn the_index_prices_the_base_basket() {
        let fills = |item_id: i64, quantity: i64, mean_price: f64| ItemFills { item_id, quantity, mean_price };
        let base = vec![fills(1, 100, 10.0), fills(2, 10, 100.0), fills(3, 5, 7.0)];
        let now = vec![fills(1, 50, 11.0), fills(2, 999, 100.0)];
        let index = laspeyres(&base, &now).expect("common items");
        assert!((index - (1100.0 + 1000.0) / (1000.0 + 1000.0)).abs() < 1e-9, "{index}");
        assert_eq!(laspeyres(&base, &[fills(9, 1, 1.0)]), None);
    }
}
