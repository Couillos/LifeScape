#![forbid(unsafe_code)]
//! Limit orders: placement, price amendment, cancellation, reads.

use crate::{Error, Store};

/// Buy or sell side of an order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum OrderSide {
    #[default]
    Buy,
    Sell,
}

impl OrderSide {
    /// Stored text: "buy" or "sell" (orders, deal and execution taker side).
    pub fn as_str(self) -> &'static str {
        match self {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        }
    }

    pub(crate) fn parse(text: &str) -> Result<Self, Error> {
        match text {
            "buy" => Ok(OrderSide::Buy),
            "sell" => Ok(OrderSide::Sell),
            other => Err(Error::Invalid(format!("bad order side {other}"))),
        }
    }

    /// Nullable taker column: NULL = side unknown (rows before migration 4).
    pub(crate) fn parse_taker(text: Option<String>) -> Result<Option<Self>, Error> {
        text.as_deref().map(Self::parse).transpose()
    }
}

/// Lifecycle of an order. Priority loss on amend is enforced by the market
/// layer (cancel + replace); this crate only records the outcome.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum OrderStatus {
    #[default]
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
/// `replaces`: the order an amend replaces (schema 6, decision Q1): the
/// new row joins its lineage (same root, same t0); `None` opens a new
/// lineage. `motive`: why the order exists (`need`, `gear`, `surplus`…,
/// empty when the caller does not say), kept by every amend. `ceiling`:
/// a buy's limit per lot (schema 10, the 10 % walk after a restart),
/// kept by every amend; `None` when the caller does not say.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewOrder {
    pub account: String,
    pub side: OrderSide,
    pub item_id: i64,
    pub quantity: i64,
    pub price: i64,
    pub dormant: bool,
    pub clock: i64,
    pub replaces: Option<i64>,
    pub motive: String,
    pub ceiling: Option<i64>,
    /// A sell's limit per lot (schema 11): the seller's own value of the
    /// unit, where its ask stops walking down; `None` when unsaid.
    pub ask_limit: Option<i64>,
}

/// Cancel reason of the row an amend closed (schema 6): the row is
/// `cancelled` (status unchanged, Q1) but its lineage goes on.
pub const AMENDED_REASON: &str = "amended";

/// Cancel reason of the fragments a merge round folded into one order
/// (bookkeeping: the quantity goes on resting, nothing was withdrawn).
pub const MERGED_REASON: &str = "merged";

/// Cancel reason of a listing withdrawn so the stack relists as one lot
/// (bookkeeping: the same units are listed again at once).
pub const REGROUPED_REASON: &str = "regrouped";

/// Reasons that are bookkeeping, never a withdrawn trade: the measures
/// of cancellations leave them out. `amended` only exists on rows older
/// than schema 12 (a reprice now rewrites its row, `order_amends`).
pub const BOOKKEEPING_REASONS: [&str; 3] = [AMENDED_REASON, MERGED_REASON, REGROUPED_REASON];

/// Stored order row.
#[derive(Clone, Debug, Default, PartialEq)]
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
    /// Order this row replaced (amend), `None` for a lineage root.
    pub replaces: Option<i64>,
    /// First order of the lineage (itself for a root).
    pub root_id: i64,
    /// Clock the lineage opened (t0), kept by every amend.
    pub root_clock: i64,
    pub motive: String,
    /// Why a cancelled row closed (`amended`, `gear_plan`, `audit`…;
    /// empty when the canceller did not say).
    pub cancel_reason: String,
    /// A buy's limit per lot (schema 10), `None` when unsaid.
    pub ceiling: Option<i64>,
    /// A sell's walk limit per lot (schema 11), `None` when unsaid.
    pub ask_limit: Option<i64>,
}

impl Order {
    /// t0 of the lineage: its root's clock (the row's own creation for a
    /// row the schema-6 migration never saw).
    pub fn lineage_clock(&self) -> i64 {
        if self.root_clock > 0 {
            self.root_clock
        } else {
            self.created_clock
        }
    }
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
        replaces: row.get(11)?,
        root_id: row.get(12)?,
        root_clock: row.get(13)?,
        motive: row.get(14)?,
        cancel_reason: row.get(15)?,
        ceiling: row.get(16)?,
        ask_limit: row.get(17)?,
    })
}

const ORDER_COLUMNS: &str = "order_id, account, side, item_id, quantity, remaining, \
     price, status, dormant, created_clock, updated_clock, replaces, root_id, root_clock, \
     motive, cancel_reason, ceiling, ask_limit";

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
        let (root_id, root_clock) = match new_order.replaces {
            Some(previous) => self.lineage_of(previous, new_order)?,
            None => (0, new_order.clock),
        };
        self.connection.execute(
            "INSERT INTO orders(account, side, item_id, quantity, remaining, price, \
             status, dormant, created_clock, updated_clock, replaces, root_id, root_clock, motive, ceiling, ask_limit) \
             VALUES (?1, ?2, ?3, ?4, ?4, ?5, 'open', ?6, ?7, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params![
                new_order.account,
                new_order.side.as_str(),
                new_order.item_id,
                new_order.quantity,
                new_order.price,
                i64::from(new_order.dormant),
                new_order.clock,
                new_order.replaces,
                root_id,
                root_clock,
                new_order.motive,
                new_order.ceiling,
                new_order.ask_limit,
            ],
        )?;
        let order_id = self.connection.last_insert_rowid();
        if root_id == 0 {
            // A fresh order is the root of its own lineage.
            self.connection.execute("UPDATE orders SET root_id = ?1 WHERE order_id = ?1", rusqlite::params![order_id])?;
        }
        Ok(order_id)
    }

    /// Root and t0 of the lineage `previous` belongs to, for the amend
    /// `new_order` (same account, side and object, or refused).
    fn lineage_of(&self, previous: i64, new_order: &NewOrder) -> Result<(i64, i64), Error> {
        let Some(old) = self.get_order(previous)? else {
            return Err(Error::NotFound(format!("replaced order {previous}")));
        };
        if old.account != new_order.account || old.side != new_order.side || old.item_id != new_order.item_id {
            return Err(Error::Invalid(format!(
                "order {previous} ({} {} {}) is another lineage than {} {} {}",
                old.account,
                old.side.as_str(),
                old.item_id,
                new_order.account,
                new_order.side.as_str(),
                new_order.item_id
            )));
        }
        let root = if old.root_id > 0 { old.root_id } else { old.order_id };
        Ok((root, old.lineage_clock()))
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
        self.cancel_order_because(order_id, clock, "")
    }

    /// `cancel_order` recording why (`amended` for the row an amend
    /// replaced: the measure of true cancellations reads it, schema 6).
    pub fn cancel_order_because(&self, order_id: i64, clock: i64, reason: &str) -> Result<(), Error> {
        let touched = self.connection.execute(
            // A partly filled order (`matched`) still rests its remainder
            // on the book: it must stay cancellable, or the remainder can
            // never be withdrawn (audit, brain, operator).
            "UPDATE orders SET status = 'cancelled', updated_clock = ?1, cancel_reason = ?3 \
             WHERE order_id = ?2 AND status IN ('open','matched')",
            rusqlite::params![clock, order_id, reason],
        )?;
        if touched == 0 {
            return Err(Error::NotFound(format!("open order {order_id}")));
        }
        Ok(())
    }

    /// Rewrites the ceiling kept with a resting buy (B1, lot 3 marché:
    /// the owner's limit moved, a restart must adopt the new one). In the
    /// book's unit (coins per lot), like the column it writes.
    pub fn set_order_ceiling(&self, order_id: i64, ceiling: i64) -> Result<(), Error> {
        self.connection.execute("UPDATE orders SET ceiling = ?1 WHERE order_id = ?2 AND status IN ('open','matched')", rusqlite::params![ceiling.max(1), order_id])?;
        Ok(())
    }

    /// Gives a resting order the motive it lacks (an owner adopting an
    /// unsaid row); a motive already said is never overwritten.
    pub fn set_order_motive(&self, order_id: i64, motive: &str) -> Result<(), Error> {
        self.connection.execute("UPDATE orders SET motive = ?1 WHERE order_id = ?2 AND motive = '' AND status IN ('open','matched')", rusqlite::params![motive, order_id])?;
        Ok(())
    }

    /// Rewrites the walk limit kept with a resting sell (the seller's own
    /// value moved), in the book's unit (coins per lot).
    pub fn set_order_ask_limit(&self, order_id: i64, ask_limit: i64) -> Result<(), Error> {
        self.connection.execute("UPDATE orders SET ask_limit = ?1 WHERE order_id = ?2 AND status IN ('open','matched')", rusqlite::params![ask_limit.max(1), order_id])?;
        Ok(())
    }

    /// Marks an order matched or settled after a deal moves forward.
    /// Final rows (`settled`, `cancelled`) never move again: a late
    /// refresh racing a cancel would otherwise revive a withdrawn order
    /// (review 2026-11-13). Refused writes report `NotFound`.
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
             WHERE order_id = ?4 AND status IN ('open','matched')",
            rusqlite::params![status.as_str(), remaining, clock, order_id],
        )?;
        if touched == 0 {
            return Err(Error::NotFound(format!("non-final order {order_id}")));
        }
        Ok(())
    }

    /// Moves the resting orders (open, partly matched) of `from` to the
    /// book of `to` (a banknote's orders to its base object: one book per
    /// object, 2026-11-16). Final rows keep their history. Returns the
    /// rows moved.
    pub fn rebase_resting_orders(&self, from: i64, to: i64) -> Result<usize, Error> {
        Ok(self.connection.execute(
            "UPDATE orders SET item_id = ?2 WHERE item_id = ?1 AND status IN ('open','matched')",
            rusqlite::params![from, to],
        )?)
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
            ..NewOrder::default()
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

    /// Schema 6 (plan marché M1): an amend joins the lineage of the row
    /// it replaces (root and t0 kept, motive carried by the caller); the
    /// replaced row closes `amended`; a fresh order is its own root.
    #[test]
    fn amends_keep_root_and_t0() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        crate::ensure_test_item(&store, 314)?;
        let mut first = sample_order("t04");
        first.motive = "need".to_string();
        let root = store.place_order(&first)?;
        let step = |previous: i64, price: i64, clock: i64| -> Result<i64, Error> {
            let next = store.place_order(&NewOrder { price, clock, replaces: Some(previous), motive: "need".to_string(), ..sample_order("t04") })?;
            store.cancel_order_because(previous, clock, AMENDED_REASON)?;
            Ok(next)
        };
        let second = step(root, 4, 1_700_002_000)?;
        let third = step(second, 5, 1_700_004_000)?;
        let row = store.get_order(third)?.expect("third row");
        assert_eq!((row.root_id, row.root_clock, row.replaces, row.motive.as_str()), (root, 1_700_000_000, Some(second), "need"));
        assert_eq!(row.lineage_clock(), 1_700_000_000, "t0 is the root's clock, not the amend's");
        let replaced = store.get_order(second)?.expect("second row");
        assert_eq!((replaced.status, replaced.cancel_reason.as_str()), (OrderStatus::Cancelled, AMENDED_REASON));
        let fresh = store.get_order(root)?.expect("root row");
        assert_eq!((fresh.root_id, fresh.root_clock, fresh.replaces), (root, 1_700_000_000, None));
        let foreign = store.place_order(&NewOrder { replaces: Some(third), ..sample_order("t05") });
        assert!(matches!(foreign, Err(Error::Invalid(_))), "another account never joins the lineage: {foreign:?}");
        Ok(())
    }

    #[test]
    fn final_rows_never_move_again() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        crate::ensure_test_item(&store, 314)?;
        let cancelled = store.place_order(&sample_order("t03"))?;
        store.cancel_order(cancelled, 1_700_000_100)?;
        assert!(store.set_order_status(cancelled, OrderStatus::Matched, 40, 1_700_000_200).is_err());
        let settled = store.place_order(&sample_order("t03"))?;
        store.set_order_status(settled, OrderStatus::Settled, 0, 1_700_000_100)?;
        assert!(store.set_order_status(settled, OrderStatus::Open, 100, 1_700_000_200).is_err());
        assert_eq!(store.get_order(cancelled)?.map(|found| found.status), Some(OrderStatus::Cancelled));
        assert_eq!(store.get_order(settled)?.map(|found| (found.status, found.remaining)), Some((OrderStatus::Settled, 0)));
        Ok(())
    }

    /// Schema 10 (user 2026-11-20, the 10 % walk after a restart): a buy
    /// keeps its ceiling, read back with the row; unsaid, none.
    /// A sell keeps its own-value limit (schema 11), read back with the row.
    #[test]
    fn a_sell_keeps_its_ask_limit() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        crate::ensure_test_item(&store, 314)?;
        let said = store.place_order(&NewOrder { side: OrderSide::Sell, ask_limit: Some(15), ..sample_order("t06") })?;
        assert_eq!(store.get_order(said)?.and_then(|row| row.ask_limit), Some(15));
        store.set_order_ask_limit(said, 22)?;
        assert_eq!(store.get_order(said)?.and_then(|row| row.ask_limit), Some(22));
        Ok(())
    }

    #[test]
    fn a_buy_keeps_its_ceiling() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        crate::ensure_test_item(&store, 314)?;
        let said = store.place_order(&NewOrder { ceiling: Some(120), ..sample_order("t05") })?;
        let unsaid = store.place_order(&sample_order("t05"))?;
        assert_eq!(store.get_order(said)?.and_then(|row| row.ceiling), Some(120));
        assert_eq!(store.get_order(unsaid)?.and_then(|row| row.ceiling), None);
        assert!(store.resting_orders()?.iter().any(|row| row.order_id == said && row.ceiling == Some(120)));
        store.set_order_ceiling(said, 40)?;
        assert_eq!(store.get_order(said)?.and_then(|row| row.ceiling), Some(40), "the owner's new limit is kept");
        Ok(())
    }
}
