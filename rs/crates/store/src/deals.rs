#![forbid(unsafe_code)]
//! Physical delivery deals: creation, status moves, per-account reads.

use crate::{Error, Store};

/// Deal lifecycle from match to settlement or typed failure.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DealStatus {
    Matched,
    Convened,
    BothArrived,
    Trading,
    Settled,
    Failed,
}

impl DealStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            DealStatus::Matched => "matched",
            DealStatus::Convened => "convened",
            DealStatus::BothArrived => "both_arrived",
            DealStatus::Trading => "trading",
            DealStatus::Settled => "settled",
            DealStatus::Failed => "failed",
        }
    }

    fn parse(text: &str) -> Result<Self, Error> {
        match text {
            "matched" => Ok(DealStatus::Matched),
            "convened" => Ok(DealStatus::Convened),
            "both_arrived" => Ok(DealStatus::BothArrived),
            "trading" => Ok(DealStatus::Trading),
            "settled" => Ok(DealStatus::Settled),
            "failed" => Ok(DealStatus::Failed),
            other => Err(Error::Invalid(format!("bad deal status {other}"))),
        }
    }
}

/// Caller-supplied fields for a new deal.
#[derive(Clone, Debug, PartialEq)]
pub struct NewDeal {
    pub buyer: String,
    pub seller: String,
    pub item_id: i64,
    pub quantity: i64,
    pub price: i64,
    pub meeting_x: i64,
    pub meeting_y: i64,
    pub meeting_level: i64,
    pub deadline_tick: i64,
    pub clock: i64,
}

/// Stored deal row.
#[derive(Clone, Debug, PartialEq)]
pub struct Deal {
    pub deal_id: i64,
    pub buyer: String,
    pub seller: String,
    pub item_id: i64,
    pub quantity: i64,
    pub price: i64,
    pub meeting_x: i64,
    pub meeting_y: i64,
    pub meeting_level: i64,
    pub status: DealStatus,
    pub fail_reason: String,
    pub deadline_tick: i64,
    pub created_clock: i64,
    pub updated_clock: i64,
}

fn map_deal(row: &rusqlite::Row<'_>) -> Result<Deal, Error> {
    let status_text: String = row.get(9)?;
    Ok(Deal {
        deal_id: row.get(0)?,
        buyer: row.get(1)?,
        seller: row.get(2)?,
        item_id: row.get(3)?,
        quantity: row.get(4)?,
        price: row.get(5)?,
        meeting_x: row.get(6)?,
        meeting_y: row.get(7)?,
        meeting_level: row.get(8)?,
        status: DealStatus::parse(&status_text)?,
        fail_reason: row.get(10)?,
        deadline_tick: row.get(11)?,
        created_clock: row.get(12)?,
        updated_clock: row.get(13)?,
    })
}

const DEAL_COLUMNS: &str = "deal_id, buyer, seller, item_id, quantity, price, \
     meeting_x, meeting_y, meeting_level, status, fail_reason, deadline_tick, \
     created_clock, updated_clock";

impl Store {
    /// Records a fresh match between two orders, returns the deal id.
    pub fn create_deal(&self, new_deal: &NewDeal) -> Result<i64, Error> {
        if new_deal.buyer.is_empty() || new_deal.seller.is_empty() {
            return Err(Error::Invalid("deal buyer/seller is empty".to_string()));
        }
        if new_deal.buyer == new_deal.seller {
            return Err(Error::Invalid("deal buyer equals seller".to_string()));
        }
        if new_deal.quantity <= 0 || new_deal.price < 1 {
            return Err(Error::Invalid(format!(
                "bad deal qty/price {}/{}",
                new_deal.quantity, new_deal.price
            )));
        }
        self.connection.execute(
            "INSERT INTO deals(buyer, seller, item_id, quantity, price, meeting_x, \
             meeting_y, meeting_level, status, fail_reason, deadline_tick, \
             created_clock, updated_clock) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'matched', '', ?9, ?10, ?10)",
            rusqlite::params![
                new_deal.buyer,
                new_deal.seller,
                new_deal.item_id,
                new_deal.quantity,
                new_deal.price,
                new_deal.meeting_x,
                new_deal.meeting_y,
                new_deal.meeting_level,
                new_deal.deadline_tick,
                new_deal.clock,
            ],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    /// Moves a deal forward; a `Failed` status requires a reason.
    pub fn set_deal_status(
        &self,
        deal_id: i64,
        status: DealStatus,
        fail_reason: &str,
        clock: i64,
    ) -> Result<(), Error> {
        if status == DealStatus::Failed && fail_reason.is_empty() {
            return Err(Error::Invalid("failed deal needs a reason".to_string()));
        }
        let touched = self.connection.execute(
            "UPDATE deals SET status = ?1, fail_reason = ?2, updated_clock = ?3 \
             WHERE deal_id = ?4",
            rusqlite::params![status.as_str(), fail_reason, clock, deal_id],
        )?;
        if touched == 0 {
            return Err(Error::NotFound(format!("deal {deal_id}")));
        }
        Ok(())
    }

    /// One deal by id.
    pub fn get_deal(&self, deal_id: i64) -> Result<Option<Deal>, Error> {
        let query = format!("SELECT {DEAL_COLUMNS} FROM deals WHERE deal_id = ?1");
        let mut statement = self.connection.prepare(&query)?;
        let mut found_rows = statement.query(rusqlite::params![deal_id])?;
        match found_rows.next()? {
            None => Ok(None),
            Some(row) => Ok(Some(map_deal(row)?)),
        }
    }

    /// Open deals where the account is buyer or seller, oldest first.
    pub fn open_deals_for_account(&self, account: &str) -> Result<Vec<Deal>, Error> {
        let query = format!(
            "SELECT {DEAL_COLUMNS} FROM deals \
             WHERE (buyer = ?1 OR seller = ?1) AND status NOT IN ('settled','failed') \
             ORDER BY created_clock"
        );
        let mut statement = self.connection.prepare(&query)?;
        let mut found_rows = statement.query(rusqlite::params![account])?;
        let mut open_deals = Vec::new();
        while let Some(row) = found_rows.next()? {
            open_deals.push(map_deal(row)?);
        }
        Ok(open_deals)
    }

    /// Latest deals first (newest `created_clock`, `deal_id` tiebreak),
    /// every status: the market page feed shows buyer and seller pseudos.
    /// Limit clamps to one page (1..=100); callers pass the `limit` query.
    pub fn recent_deals(&self, limit: i64) -> Result<Vec<Deal>, Error> {
        let capped = limit.clamp(1, 100);
        let query = format!(
            "SELECT {DEAL_COLUMNS} FROM deals \
             ORDER BY created_clock DESC, deal_id DESC LIMIT ?1"
        );
        let mut statement = self.connection.prepare(&query)?;
        let mut found_rows = statement.query(rusqlite::params![capped])?;
        let mut latest = Vec::new();
        while let Some(row) = found_rows.next()? {
            latest.push(map_deal(row)?);
        }
        Ok(latest)
    }

    /// Fails every non-final deal left by a dead daemon (both parties
    /// disconnected with it, no handover can resume). Returns the count.
    /// The market service calls this on boot so `my_deals` never shows
    /// phantom handovers after a restart.
    pub fn fail_stale_deals(&self, reason: &str, clock: i64) -> Result<usize, Error> {
        if reason.is_empty() {
            return Err(Error::Invalid("failed deal needs a reason".to_string()));
        }
        let touched = self.connection.execute(
            "UPDATE deals SET status = 'failed', fail_reason = ?1, updated_clock = ?2 \
             WHERE status NOT IN ('settled','failed')",
            rusqlite::params![reason, clock],
        )?;
        Ok(touched)
    }

    /// Boot-time variant of `fail_stale_deals` that gives the lots back:
    /// every non-final deal fails with `reason` and both parties get an
    /// open order of the deal's quantity at the deal's price, created at
    /// the deal's clock so the reload keeps its rank (same outcome as a
    /// neutral runtime failure: both relist). The carnet rows behind a
    /// match were settled at match time; without this the quantity of a
    /// deal killed by a daemon restart vanished for both sides. One
    /// transaction: a crash mid-way fails nothing and relists nothing.
    /// Returns the deals failed.
    pub fn fail_stale_deals_relisting(&self, reason: &str, clock: i64) -> Result<Vec<Deal>, Error> {
        if reason.is_empty() {
            return Err(Error::Invalid("failed deal needs a reason".to_string()));
        }
        self.connection.execute_batch("BEGIN IMMEDIATE")?;
        match self.fail_and_relist_inner(reason, clock) {
            Ok(failed) => {
                self.connection.execute_batch("COMMIT")?;
                Ok(failed)
            }
            Err(error) => {
                let _ignored = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    fn fail_and_relist_inner(&self, reason: &str, clock: i64) -> Result<Vec<Deal>, Error> {
        let query = format!(
            "SELECT {DEAL_COLUMNS} FROM deals WHERE status NOT IN ('settled','failed') \
             ORDER BY created_clock, deal_id"
        );
        let mut stale = Vec::new();
        {
            let mut statement = self.connection.prepare(&query)?;
            let mut found_rows = statement.query(rusqlite::params![])?;
            while let Some(row) = found_rows.next()? {
                stale.push(map_deal(row)?);
            }
        }
        for deal in &stale {
            self.connection.execute(
                "UPDATE deals SET status = 'failed', fail_reason = ?1, updated_clock = ?2 \
                 WHERE deal_id = ?3",
                rusqlite::params![reason, clock, deal.deal_id],
            )?;
            for (account, side) in [(&deal.buyer, crate::OrderSide::Buy), (&deal.seller, crate::OrderSide::Sell)] {
                self.place_order(&crate::NewOrder {
                    account: account.clone(),
                    side,
                    item_id: deal.item_id,
                    quantity: deal.quantity,
                    price: deal.price,
                    dormant: false,
                    clock: deal.created_clock,
                })?;
            }
        }
        Ok(stale)
    }
}

#[cfg(test)]
mod deals_tests {
    use super::*;

    #[test]
    fn deal_lifecycle_roundtrip() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        crate::ensure_test_item(&store, 314)?;
        let deal_id = store.create_deal(&NewDeal {
            buyer: "t01".to_string(),
            seller: "t02".to_string(),
            item_id: 314,
            quantity: 50,
            price: 3,
            meeting_x: 3212,
            meeting_y: 3428,
            meeting_level: 0,
            deadline_tick: 9999,
            clock: 1_700_000_000,
        })?;
        assert_eq!(store.open_deals_for_account("t01")?.len(), 1);
        store.set_deal_status(deal_id, DealStatus::Convened, "", 1_700_000_010)?;
        store.set_deal_status(deal_id, DealStatus::Settled, "", 1_700_000_020)?;
        assert!(store.open_deals_for_account("t01")?.is_empty());
        assert_eq!(
            store.get_deal(deal_id)?.map(|found| found.status),
            Some(DealStatus::Settled)
        );
        Ok(())
    }

    #[test]
    fn stale_deals_fail_and_relist_both_sides_once() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        crate::ensure_test_item(&store, 314)?;
        let fresh = |status: DealStatus| -> Result<i64, Error> {
            let deal_id = store.create_deal(&NewDeal {
                buyer: "t01".to_string(),
                seller: "t02".to_string(),
                item_id: 314,
                quantity: 7,
                price: 3,
                meeting_x: 3212,
                meeting_y: 3428,
                meeting_level: 0,
                deadline_tick: 9999,
                clock: 1_700_000_000,
            })?;
            store.set_deal_status(deal_id, status, "", 1_700_000_005)?;
            Ok(deal_id)
        };
        let live = fresh(DealStatus::Convened)?;
        fresh(DealStatus::Settled)?;
        let failed = store.fail_stale_deals_relisting("daemon restart", 1_700_000_100)?;
        assert_eq!(failed.iter().map(|deal| deal.deal_id).collect::<Vec<_>>(), vec![live]);
        let rows = |account: &str| -> Result<Vec<(crate::OrderSide, i64, i64, i64)>, Error> {
            Ok(store.account_orders(account)?.iter().map(|row| (row.side, row.remaining, row.price, row.created_clock)).collect())
        };
        assert_eq!(rows("t01")?, vec![(crate::OrderSide::Buy, 7, 3, 1_700_000_000)]);
        assert_eq!(rows("t02")?, vec![(crate::OrderSide::Sell, 7, 3, 1_700_000_000)]);
        assert!(store.fail_stale_deals_relisting("daemon restart", 1_700_000_200)?.is_empty(), "idempotent");
        assert_eq!(store.account_orders("t01")?.len(), 1);
        Ok(())
    }
}
