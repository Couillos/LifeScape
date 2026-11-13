#![forbid(unsafe_code)]
//! Limit orders: placement, price amendment, cancellation, reads.

use crate::{Error, Store};

/// Buy or sell side of an order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OrderSide {
    Buy,
    Sell,
}

impl OrderSide {
    fn as_str(self) -> &'static str {
        match self {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        }
    }

    fn parse(text: &str) -> Result<Self, Error> {
        match text {
            "buy" => Ok(OrderSide::Buy),
            "sell" => Ok(OrderSide::Sell),
            other => Err(Error::Invalid(format!("bad order side {other}"))),
        }
    }
}

/// Lifecycle of an order. Priority loss on amend is enforced by the market
/// layer (cancel + replace); this crate only records the outcome.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OrderStatus {
    Open,
    Matched,
    Settled,
    Cancelled,
}

impl OrderStatus {
    fn as_str(self) -> &'static str {
        match self {
            OrderStatus::Open => "open",
            OrderStatus::Matched => "matched",
            OrderStatus::Settled => "settled",
            OrderStatus::Cancelled => "cancelled",
        }
    }

    fn parse(text: &str) -> Result<Self, Error> {
        match text {
            "open" => Ok(OrderStatus::Open),
            "matched" => Ok(OrderStatus::Matched),
            "settled" => Ok(OrderStatus::Settled),
            "cancelled" => Ok(OrderStatus::Cancelled),
            other => Err(Error::Invalid(format!("bad order status {other}"))),
        }
    }
}

/// Caller-supplied fields for a new order. Prices are floored at 1 coin.
#[derive(Clone, Debug, PartialEq)]
pub struct NewOrder {
    pub account: String,
    pub side: OrderSide,
    pub item_id: i64,
    pub quantity: i64,
    pub price: i64,
    pub dormant: bool,
    pub clock: i64,
}

/// Stored order row.
#[derive(Clone, Debug, PartialEq)]
pub struct Order {
    pub order_id: i64,
    pub account: String,
    pub side: OrderSide,
    pub item_id: i64,
    pub quantity: i64,
    pub remaining: i64,
    pub price: i64,
    pub status: OrderStatus,
    pub dormant: bool,
    pub created_clock: i64,
    pub updated_clock: i64,
}

fn map_order(row: &rusqlite::Row<'_>) -> Result<Order, Error> {
    let side_text: String = row.get(2)?;
    let status_text: String = row.get(7)?;
    let dormant_flag: i64 = row.get(8)?;
    Ok(Order {
        order_id: row.get(0)?,
        account: row.get(1)?,
        side: OrderSide::parse(&side_text)?,
        item_id: row.get(3)?,
        quantity: row.get(4)?,
        remaining: row.get(5)?,
        price: row.get(6)?,
        status: OrderStatus::parse(&status_text)?,
        dormant: dormant_flag != 0,
        created_clock: row.get(9)?,
        updated_clock: row.get(10)?,
    })
}

const ORDER_COLUMNS: &str = "order_id, account, side, item_id, quantity, remaining, \
     price, status, dormant, created_clock, updated_clock";

impl Store {
    /// Records a new open order, returns its id.
    pub fn place_order(&self, new_order: &NewOrder) -> Result<i64, Error> {
        if new_order.account.is_empty() {
            return Err(Error::Invalid("order account is empty".to_string()));
        }
        if new_order.quantity <= 0 {
            return Err(Error::Invalid(format!(
                "bad order quantity {}",
                new_order.quantity
            )));
        }
        if new_order.price < 1 {
            return Err(Error::Invalid(format!("bad order price {}", new_order.price)));
        }
        self.connection.execute(
            "INSERT INTO orders(account, side, item_id, quantity, remaining, price, \
             status, dormant, created_clock, updated_clock) \
             VALUES (?1, ?2, ?3, ?4, ?4, ?5, 'open', ?6, ?7, ?7)",
            rusqlite::params![
                new_order.account,
                new_order.side.as_str(),
                new_order.item_id,
                new_order.quantity,
                new_order.price,
                i64::from(new_order.dormant),
                new_order.clock,
            ],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    /// Changes the limit price of an open order and stamps the update clock.
    pub fn amend_order_price(
        &self,
        order_id: i64,
        new_price: i64,
        clock: i64,
    ) -> Result<(), Error> {
        if new_price < 1 {
            return Err(Error::Invalid(format!("bad amended price {new_price}")));
        }
        let touched = self.connection.execute(
            "UPDATE orders SET price = ?1, updated_clock = ?2 \
             WHERE order_id = ?3 AND status = 'open'",
            rusqlite::params![new_price, clock, order_id],
        )?;
        if touched == 0 {
            return Err(Error::NotFound(format!("open order {order_id}")));
        }
        Ok(())
    }

    /// Marks an order cancelled. Fills stay recorded in deals/executions.
    pub fn cancel_order(&self, order_id: i64, clock: i64) -> Result<(), Error> {
        let touched = self.connection.execute(
            // A partly filled order (`matched`) still rests its remainder
            // on the book: it must stay cancellable, or the remainder can
            // never be withdrawn (audit, brain, operator).
            "UPDATE orders SET status = 'cancelled', updated_clock = ?1 \
             WHERE order_id = ?2 AND status IN ('open','matched')",
            rusqlite::params![clock, order_id],
        )?;
        if touched == 0 {
            return Err(Error::NotFound(format!("open order {order_id}")));
        }
        Ok(())
    }

    /// Marks an order matched or settled after a deal moves forward.
    pub fn set_order_status(
        &self,
        order_id: i64,
        status: OrderStatus,
        remaining: i64,
        clock: i64,
    ) -> Result<(), Error> {
        if remaining < 0 {
            return Err(Error::Invalid(format!("bad remaining {remaining}")));
        }
        let touched = self.connection.execute(
            "UPDATE orders SET status = ?1, remaining = ?2, updated_clock = ?3 \
             WHERE order_id = ?4",
            rusqlite::params![status.as_str(), remaining, clock, order_id],
        )?;
        if touched == 0 {
            return Err(Error::NotFound(format!("order {order_id}")));
        }
        Ok(())
    }

    /// One order by id.
    pub fn get_order(&self, order_id: i64) -> Result<Option<Order>, Error> {
        let query = format!("SELECT {ORDER_COLUMNS} FROM orders WHERE order_id = ?1");
        let mut statement = self.connection.prepare(&query)?;
        let mut found_rows = statement.query(rusqlite::params![order_id])?;
        match found_rows.next()? {
            None => Ok(None),
            Some(row) => Ok(Some(map_order(row)?)),
        }
    }

    /// Open book for one item and side, best price then oldest first.
    pub fn open_book(&self, item_id: i64, side: OrderSide) -> Result<Vec<Order>, Error> {
        let ordering = match side {
            OrderSide::Buy => "price DESC, created_clock ASC",
            OrderSide::Sell => "price ASC, created_clock ASC",
        };
        let query = format!(
            "SELECT {ORDER_COLUMNS} FROM orders \
             WHERE item_id = ?1 AND side = ?2 AND status = 'open' AND dormant = 0 \
             ORDER BY {ordering}"
        );
        let mut statement = self.connection.prepare(&query)?;
        let mut found_rows = statement.query(rusqlite::params![item_id, side.as_str()])?;
        let mut book = Vec::new();
        while let Some(row) = found_rows.next()? {
            book.push(map_order(row)?);
        }
        Ok(book)
    }

    /// Every non-final order of one account.
    pub fn account_orders(&self, account: &str) -> Result<Vec<Order>, Error> {
        let query = format!(
            "SELECT {ORDER_COLUMNS} FROM orders \
             WHERE account = ?1 AND status IN ('open','matched') ORDER BY created_clock"
        );
        let mut statement = self.connection.prepare(&query)?;
        let mut found_rows = statement.query(rusqlite::params![account])?;
        let mut account_book = Vec::new();
        while let Some(row) = found_rows.next()? {
            account_book.push(map_order(row)?);
        }
        Ok(account_book)
    }

    /// Every resting order of the whole carnet (open, plus partially
    /// filled rows stranded by a daemon restart), oldest first. The
    /// market service rebuilds its in-memory books from these on boot:
    /// without this a restart orphans every resting order (visible in
    /// `my_orders`, invisible to matching).
    pub fn resting_orders(&self) -> Result<Vec<Order>, Error> {
        let query = format!(
            "SELECT {ORDER_COLUMNS} FROM orders \
             WHERE status IN ('open','matched') AND dormant = 0 ORDER BY created_clock"
        );
        let mut statement = self.connection.prepare(&query)?;
        let mut found_rows = statement.query(rusqlite::params![])?;
        let mut resting = Vec::new();
        while let Some(row) = found_rows.next()? {
            resting.push(map_order(row)?);
        }
        Ok(resting)
    }
}

#[cfg(test)]
mod orders_tests {
    use super::*;
    use crate::temp_database_path;

    fn sample_order(account: &str) -> NewOrder {
        NewOrder {
            account: account.to_string(),
            side: OrderSide::Buy,
            item_id: 314,
            quantity: 100,
            price: 3,
            dormant: false,
            clock: 1_700_000_000,
        }
    }

    #[test]
    fn crash_without_close_loses_nothing() -> Result<(), Error> {
        let path = temp_database_path("crash");
        {
            // Simulated crash: drop the store without any close call.
            let store = Store::open(&path)?;
            crate::ensure_test_item(&store, 314)?;
            let order_id = store.place_order(&sample_order("t01"))?;
            assert_eq!(store.get_order(order_id)?.is_some(), true);
        }
        let reopened = Store::open(&path)?;
        let reopened_orders = reopened.account_orders("t01")?;
        assert_eq!(reopened_orders.len(), 1);
        assert_eq!(reopened_orders[0].remaining, 100);
        std::fs::remove_file(&path).map_err(Error::from)?;
        Ok(())
    }

    #[test]
    fn amend_and_cancel_flow() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        crate::ensure_test_item(&store, 314)?;
        let order_id = store.place_order(&sample_order("t02"))?;
        store.amend_order_price(order_id, 5, 1_700_000_100)?;
        assert_eq!(store.get_order(order_id)?.map(|found| found.price), Some(5));
        store.cancel_order(order_id, 1_700_000_200)?;
        assert_eq!(
            store.get_order(order_id)?.map(|found| found.status),
            Some(OrderStatus::Cancelled)
        );
        assert!(store.account_orders("t02")?.is_empty());
        Ok(())
    }
}
