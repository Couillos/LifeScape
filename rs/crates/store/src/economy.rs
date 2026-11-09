#![forbid(unsafe_code)]
//! Account economy profile plus reliability counters, and public trend scores.

use crate::{Error, Store};

/// One row of `accounts_econ`: profile knobs and deal reliability.
#[derive(Clone, Debug, PartialEq)]
pub struct AccountEcon {
    pub account: String,
    pub iron: bool,
    pub deals_settled: i64,
    pub deals_failed: i64,
    pub market_affinity: f64,
    pub price_patience: f64,
    pub archetype: String,
}

fn map_account(row: &rusqlite::Row<'_>) -> Result<AccountEcon, rusqlite::Error> {
    let iron_flag: i64 = row.get(1)?;
    Ok(AccountEcon {
        account: row.get(0)?,
        iron: iron_flag != 0,
        deals_settled: row.get(2)?,
        deals_failed: row.get(3)?,
        market_affinity: row.get(4)?,
        price_patience: row.get(5)?,
        archetype: row.get(6)?,
    })
}

/// One row of `trends`: shortage/momentum score for an item and a period
/// (`"24h"` or `"7d"` by convention, free text otherwise).
#[derive(Clone, Debug, PartialEq)]
pub struct Trend {
    pub item_id: i64,
    pub period: String,
    pub shortage: f64,
    pub momentum: f64,
    pub score: f64,
    pub computed_clock: i64,
}

fn map_trend(row: &rusqlite::Row<'_>) -> Result<Trend, rusqlite::Error> {
    Ok(Trend {
        item_id: row.get(0)?,
        period: row.get(1)?,
        shortage: row.get(2)?,
        momentum: row.get(3)?,
        score: row.get(4)?,
        computed_clock: row.get(5)?,
    })
}

impl Store {
    /// Inserts the account row or replaces its profile fields on conflict.
    /// Reliability counters are preserved by the conflict branch.
    pub fn upsert_account(&self, account: &AccountEcon) -> Result<(), Error> {
        if account.account.is_empty() {
            return Err(Error::Invalid("account name is empty".to_string()));
        }
        self.connection.execute(
            "INSERT INTO accounts_econ(account, iron, deals_settled, deals_failed, \
             market_affinity, price_patience, archetype) \
             VALUES (?1, ?2, 0, 0, ?3, ?4, ?5) \
             ON CONFLICT(account) DO UPDATE SET iron=excluded.iron, \
             market_affinity=excluded.market_affinity, \
             price_patience=excluded.price_patience, archetype=excluded.archetype",
            rusqlite::params![
                account.account,
                i64::from(account.iron),
                account.market_affinity,
                account.price_patience,
                account.archetype,
            ],
        )?;
        Ok(())
    }

    /// One account row, `None` when the account never traded.
    pub fn get_account(&self, account: &str) -> Result<Option<AccountEcon>, Error> {
        let mut statement = self.connection.prepare(
            "SELECT account, iron, deals_settled, deals_failed, market_affinity, \
             price_patience, archetype FROM accounts_econ WHERE account = ?1",
        )?;
        let mut found_rows = statement.query_map(rusqlite::params![account], map_account)?;
        match found_rows.next() {
            None => Ok(None),
            Some(found) => Ok(Some(found?)),
        }
    }

    /// Bumps the settled/failed counter after a deal closes. The account
    /// row must exist (created at account setup via `upsert_account`).
    pub fn record_deal_outcome(&self, account: &str, settled: bool) -> Result<(), Error> {
        let column = if settled { "deals_settled" } else { "deals_failed" };
        let query = format!(
            "UPDATE accounts_econ SET {column} = {column} + 1 WHERE account = ?1"
        );
        let touched = self
            .connection
            .execute(&query, rusqlite::params![account])?;
        if touched == 0 {
            return Err(Error::NotFound(format!("account {account}")));
        }
        Ok(())
    }

    /// Inserts or replaces the trend score for one (item, period) pair.
    pub fn upsert_trend(&self, trend: &Trend) -> Result<(), Error> {
        if trend.period.is_empty() {
            return Err(Error::Invalid("trend period is empty".to_string()));
        }
        self.connection.execute(
            "INSERT INTO trends(item_id, period, shortage, momentum, score, computed_clock) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(item_id, period) DO UPDATE SET shortage=excluded.shortage, \
             momentum=excluded.momentum, score=excluded.score, \
             computed_clock=excluded.computed_clock",
            rusqlite::params![
                trend.item_id,
                trend.period,
                trend.shortage,
                trend.momentum,
                trend.score,
                trend.computed_clock,
            ],
        )?;
        Ok(())
    }

    /// Every trend period scored for one item.
    pub fn trends_for_item(&self, item_id: i64) -> Result<Vec<Trend>, Error> {
        let mut statement = self.connection.prepare(
            "SELECT item_id, period, shortage, momentum, score, computed_clock \
             FROM trends WHERE item_id = ?1 ORDER BY period",
        )?;
        let mapped = statement.query_map(rusqlite::params![item_id], map_trend)?;
        let mut scored = Vec::new();
        for mapped_row in mapped {
            scored.push(mapped_row?);
        }
        Ok(scored)
    }
}

#[cfg(test)]
mod economy_tests {
    use super::*;

    #[test]
    fn account_reliability_and_trend_roundtrip() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        crate::ensure_test_item(&store, 314)?;
        store.upsert_account(&AccountEcon {
            account: "t01".to_string(),
            iron: false,
            deals_settled: 0,
            deals_failed: 0,
            market_affinity: 0.7,
            price_patience: 0.4,
            archetype: "producer".to_string(),
        })?;
        store.record_deal_outcome("t01", true)?;
        store.record_deal_outcome("t01", false)?;
        let fetched = store
            .get_account("t01")?
            .ok_or_else(|| Error::NotFound("account t01".to_string()))?;
        assert_eq!((fetched.deals_settled, fetched.deals_failed), (1, 1));
        store.upsert_trend(&Trend {
            item_id: 314,
            period: "24h".to_string(),
            shortage: 0.8,
            momentum: 0.1,
            score: 0.9,
            computed_clock: 1_700_000_000,
        })?;
        assert_eq!(store.trends_for_item(314)?.len(), 1);
        Ok(())
    }
}
