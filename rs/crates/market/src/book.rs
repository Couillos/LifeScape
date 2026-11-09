#![forbid(unsafe_code)]
//! One limit order book per item. Price priority, then arrival order
//! (`seq`); partial fills; the trade price is the resting order's price.
//! Amend is cancel + replace (the new order loses its queue position).

use crate::error::Error;

/// Buy or sell side of a limit order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Side {
    Buy,
    Sell,
}

/// One resting or incoming limit order. `remaining` drops with each fill.
#[derive(Clone, Debug, PartialEq)]
pub struct Order {
    pub id: u64,
    pub account: String,
    pub item: i64,
    pub side: Side,
    pub quantity: i64,
    pub remaining: i64,
    pub price: i64,
    pub seq: u64,
}

/// One agreed quantity at the resting order's price.
#[derive(Clone, Debug, PartialEq)]
pub struct Fill {
    pub buy_id: u64,
    pub sell_id: u64,
    pub buyer: String,
    pub seller: String,
    pub item: i64,
    pub quantity: i64,
    pub price: i64,
}

/// Order book for a single item. Orders live here until filled, amended
/// (cancel + replace) or cancelled.
#[derive(Clone, Debug, Default)]
pub struct Book {
    buys: Vec<Order>,
    sells: Vec<Order>,
    next_seq: u64,
}

impl Book {
    /// Empty book.
    pub fn new() -> Self {
        Book { buys: Vec::new(), sells: Vec::new(), next_seq: 0 }
    }

    /// Resting depth for one side: (price, total qty), best first.
    pub fn depth(&self, side: Side, limit: usize) -> Vec<(i64, i64)> {
        let mut rows: Vec<(i64, i64)> = Vec::new();
        let orders = match side {
            Side::Buy => &self.buys,
            Side::Sell => &self.sells,
        };
        for order in orders {
            match rows.last_mut() {
                Some(last) if last.0 == order.price => last.1 += order.remaining,
                _ => rows.push((order.price, order.remaining)),
            }
            if rows.len() >= limit {
                break;
            }
        }
        rows
    }

    /// Best bid and best ask, when both sides rest.
    pub fn best(&self) -> (Option<i64>, Option<i64>) {
        (self.buys.first().map(|order| order.price), self.sells.first().map(|order| order.price))
    }

    /// Open orders of one account, oldest first.
    pub fn orders_of(&self, account: &str) -> Vec<Order> {
        let mut found: Vec<Order> = self
            .buys
            .iter()
            .chain(self.sells.iter())
            .filter(|order| order.account == account)
            .cloned()
            .collect();
        found.sort_by_key(|order| order.seq);
        found
    }

    /// Total resting quantity on one side (merchant cap denominator).
    pub fn resting_qty(&self, side: Side) -> i64 {
        match side {
            Side::Buy => self.buys.iter().map(|order| order.remaining).sum(),
            Side::Sell => self.sells.iter().map(|order| order.remaining).sum(),
        }
    }

    fn insert_sorted(orders: &mut Vec<Order>, order: Order, buy: bool) {
        let position = orders
            .iter()
            .position(|resting| {
                if buy {
                    resting.price < order.price
                        || (resting.price == order.price && resting.seq > order.seq)
                } else {
                    resting.price > order.price
                        || (resting.price == order.price && resting.seq > order.seq)
                }
            })
            .unwrap_or(orders.len());
        orders.insert(position, order);
    }

    fn take(&mut self, id: u64) -> Option<Order> {
        if let Some(position) = self.buys.iter().position(|order| order.id == id) {
            return Some(self.buys.remove(position));
        }
        if let Some(position) = self.sells.iter().position(|order| order.id == id) {
            return Some(self.sells.remove(position));
        }
        None
    }

    /// Places a limit order, matching immediately against the other side.
    /// Returns the fills plus whether the remainder rests (false = fully
    /// filled). Rejects nonsense quantities and prices loudly.
    pub fn place(
        &mut self,
        id: u64,
        account: &str,
        item: i64,
        side: Side,
        quantity: i64,
        price: i64,
    ) -> Result<(Vec<Fill>, bool), Error> {
        if account.is_empty() {
            return Err(Error::Invalid("empty account".to_string()));
        }
        if quantity <= 0 {
            return Err(Error::Invalid(format!("bad quantity {quantity}")));
        }
        if price < 1 {
            return Err(Error::Invalid(format!("bad price {price}")));
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        let mut incoming = Order {
            id,
            account: account.to_string(),
            item,
            side,
            quantity,
            remaining: quantity,
            price,
            seq,
        };
        let mut fills = Vec::new();
        loop {
            let crosses = match side {
                Side::Buy => self.sells.first().is_some_and(|ask| ask.price <= incoming.price),
                Side::Sell => self.buys.first().is_some_and(|bid| bid.price >= incoming.price),
            };
            if !crosses {
                break;
            }
            let (resting_price, resting_qty, resting_id, resting_owner) = match side {
                Side::Buy => {
                    let ask = self.sells.first().expect("ask checked");
                    (ask.price, ask.remaining, ask.id, ask.account.clone())
                }
                Side::Sell => {
                    let bid = self.buys.first().expect("bid checked");
                    (bid.price, bid.remaining, bid.id, bid.account.clone())
                }
            };
            if resting_owner == incoming.account {
                break;
            }
            let take = incoming.remaining.min(resting_qty);
            let (buy_id, sell_id, buyer, seller) = match side {
                Side::Buy => (incoming.id, resting_id, incoming.account.clone(), resting_owner),
                Side::Sell => (resting_id, incoming.id, resting_owner, incoming.account.clone()),
            };
            fills.push(Fill {
                buy_id,
                sell_id,
                buyer,
                seller,
                item,
                quantity: take,
                price: resting_price,
            });
            incoming.remaining -= take;
            match side {
                Side::Buy => {
                    let ask = self.sells.first_mut().expect("ask checked");
                    ask.remaining -= take;
                    if ask.remaining == 0 {
                        self.sells.remove(0);
                    }
                }
                Side::Sell => {
                    let bid = self.buys.first_mut().expect("bid checked");
                    bid.remaining -= take;
                    if bid.remaining == 0 {
                        self.buys.remove(0);
                    }
                }
            }
            if incoming.remaining == 0 {
                break;
            }
        }
        let rests = incoming.remaining > 0;
        if rests {
            match side {
                Side::Buy => Self::insert_sorted(&mut self.buys, incoming, true),
                Side::Sell => Self::insert_sorted(&mut self.sells, incoming, false),
            }
        }
        Ok((fills, rests))
    }

    /// Remaining quantity of one resting order, `None` when gone.
    pub fn remaining(&self, id: u64) -> Option<i64> {
        self.buys
            .iter()
            .chain(self.sells.iter())
            .find(|order| order.id == id)
            .map(|order| order.remaining)
    }

    /// Cancels one resting order. Fills already taken stay valid.
    pub fn cancel(&mut self, id: u64) -> Result<Order, Error> {
        self.take(id).ok_or_else(|| Error::NotFound(format!("order {id}")))
    }

    /// Amends a price: cancel + replace, so the order loses its queue
    /// position. Returns the replacement order plus any immediate fills.
    pub fn amend(&mut self, id: u64, new_price: i64, new_id: u64) -> Result<(Order, Vec<Fill>), Error> {
        if new_price < 1 {
            return Err(Error::Invalid(format!("bad amended price {new_price}")));
        }
        let old = self.take(id).ok_or_else(|| Error::NotFound(format!("order {id}")))?;
        let (fills, rests) = self.place(new_id, &old.account, old.item, old.side, old.remaining, new_price)?;
        let replacement = Order {
            id: new_id,
            account: old.account,
            item: old.item,
            side: old.side,
            quantity: old.remaining,
            remaining: if rests { old.remaining - fills.iter().map(|fill| fill.quantity).sum::<i64>() } else { 0 },
            price: new_price,
            seq: self.next_seq - 1,
        };
        Ok((replacement, fills))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(book: &mut Book, id: u64, account: &str, side: Side, qty: i64, price: i64) -> (Vec<Fill>, bool) {
        book.place(id, account, 314, side, qty, price).expect("places")
    }

    #[test]
    fn price_then_time_priority_with_resting_price() {
        let mut book = Book::new();
        place(&mut book, 1, "seller-a", Side::Sell, 10, 5);
        place(&mut book, 2, "seller-b", Side::Sell, 10, 5);
        place(&mut book, 3, "seller-c", Side::Sell, 10, 4);
        // Cheapest ask first, then oldest at equal price; trade price is
        // the resting order's.
        let (fills, rests) = place(&mut book, 4, "buyer", Side::Buy, 25, 5);
        assert!(!rests);
        assert_eq!(fills.len(), 3);
        assert_eq!((fills[0].quantity, fills[0].price), (10, 4));
        assert_eq!(fills[1].seller, "seller-a");
        assert_eq!(fills[2].seller, "seller-b");
        assert_eq!(fills[2].quantity, 5);
    }

    #[test]
    fn partial_fill_leaves_the_rest_resting() {
        let mut book = Book::new();
        place(&mut book, 1, "seller", Side::Sell, 100, 5);
        let (fills, rests) = place(&mut book, 2, "buyer", Side::Buy, 30, 6);
        assert!(!rests, "incoming fully filled");
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].price, 5, "resting ask sets the price");
        assert_eq!(book.depth(Side::Sell, 4), vec![(5, 70)]);
    }

    #[test]
    fn self_trade_never_matches() {
        let mut book = Book::new();
        place(&mut book, 1, "t01", Side::Sell, 10, 5);
        let (fills, rests) = place(&mut book, 2, "t01", Side::Buy, 10, 6);
        assert!(fills.is_empty());
        assert!(rests);
        assert_eq!(book.orders_of("t01").len(), 2);
    }

    #[test]
    fn amend_loses_time_priority() {
        let mut book = Book::new();
        place(&mut book, 1, "a", Side::Sell, 5, 5);
        place(&mut book, 2, "b", Side::Sell, 5, 5);
        // Seller A reprices to the same level: now behind B, so the next
        // buyer fills B first and A keeps resting.
        let (_, fills) = book.amend(1, 5, 3).expect("amends");
        assert!(fills.is_empty());
        let (fills, _) = place(&mut book, 4, "buyer", Side::Buy, 5, 5);
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].seller, "b");
        let asks: Vec<String> = book.sells.iter().map(|order| order.account.clone()).collect();
        assert_eq!(asks, vec!["a".to_string()]);
    }

    #[test]
    fn cancel_drops_only_the_named_order() {
        let mut book = Book::new();
        place(&mut book, 1, "a", Side::Sell, 5, 5);
        assert!(book.cancel(99).is_err());
        book.cancel(1).expect("cancels");
        assert!(book.depth(Side::Sell, 4).is_empty());
    }
}
