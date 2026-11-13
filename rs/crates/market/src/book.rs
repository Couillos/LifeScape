#![forbid(unsafe_code)]
//! One limit order book per item. Price priority, then arrival order
//! (`seq`); partial fills; the trade price is the resting order's price.
//! Amend is cancel + replace (the new order loses its queue position).
//! Never crossed: an incoming or relisted order skips its own resting
//! orders (no self-trade) and the pairs the caller keeps apart, and
//! matches everything else it crosses; `uncross` heals what a raw
//! `relist` (boot reload) or an expired pair guard leaves crossed.

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
    /// Arrival stamp per resting order id: bumped at every (re)entry
    /// (place, relist, fold, boot reload), unlike `seq`, which a relist
    /// keeps for queue rank. `uncross` makes the later arrival the taker.
    arrivals: std::collections::HashMap<u64, u64>,
    next_arrival: u64,
}

/// Pair guard that keeps nobody apart (plain `place`).
fn never_kept_apart(_buyer: &str, _seller: &str) -> bool {
    false
}

impl Book {
    /// Empty book.
    pub fn new() -> Self {
        Book::default()
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
        self.arrivals.remove(&id);
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
        self.place_guarded(id, account, item, side, quantity, price, &never_kept_apart)
    }

    /// `place` with a pair guard: resting orders of the same account, or
    /// of a counterparty `kept_apart(buyer, seller)` refuses, are skipped
    /// (never a wall: the next eligible resting order still matches), so
    /// the remainder only ever rests crossed against those.
    #[allow(clippy::too_many_arguments)]
    pub fn place_guarded(
        &mut self,
        id: u64,
        account: &str,
        item: i64,
        side: Side,
        quantity: i64,
        price: i64,
        kept_apart: &dyn Fn(&str, &str) -> bool,
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
        let fills = self.take_as_taker(&mut incoming, kept_apart);
        let rests = incoming.remaining > 0;
        if rests {
            self.rest(incoming);
        }
        Ok((fills, rests))
    }

    fn rest(&mut self, order: Order) {
        self.arrivals.insert(order.id, self.next_arrival);
        self.next_arrival += 1;
        match order.side {
            Side::Buy => Self::insert_sorted(&mut self.buys, order, true),
            Side::Sell => Self::insert_sorted(&mut self.sells, order, false),
        }
    }

    /// Matches `incoming` against the opposite side in priority order, at
    /// each resting order's price, skipping its own orders and the
    /// counterparties `kept_apart` refuses. `incoming.remaining` drops.
    fn take_as_taker(&mut self, incoming: &mut Order, kept_apart: &dyn Fn(&str, &str) -> bool) -> Vec<Fill> {
        let mut fills = Vec::new();
        while incoming.remaining > 0 {
            let opposite = match incoming.side {
                Side::Buy => &self.sells,
                Side::Sell => &self.buys,
            };
            let found = opposite
                .iter()
                .take_while(|resting| match incoming.side {
                    Side::Buy => resting.price <= incoming.price,
                    Side::Sell => resting.price >= incoming.price,
                })
                .position(|resting| {
                    resting.account != incoming.account && {
                        let (buyer, seller) = match incoming.side {
                            Side::Buy => (incoming.account.as_str(), resting.account.as_str()),
                            Side::Sell => (resting.account.as_str(), incoming.account.as_str()),
                        };
                        !kept_apart(buyer, seller)
                    }
                });
            let Some(position) = found else {
                break;
            };
            let opposite = match incoming.side {
                Side::Buy => &mut self.sells,
                Side::Sell => &mut self.buys,
            };
            let resting = &mut opposite[position];
            let take = incoming.remaining.min(resting.remaining);
            let (buy_id, sell_id, buyer, seller) = match incoming.side {
                Side::Buy => (incoming.id, resting.id, incoming.account.clone(), resting.account.clone()),
                Side::Sell => (resting.id, incoming.id, resting.account.clone(), incoming.account.clone()),
            };
            fills.push(Fill { buy_id, sell_id, buyer, seller, item: incoming.item, quantity: take, price: resting.price });
            incoming.remaining -= take;
            resting.remaining -= take;
            if resting.remaining == 0 {
                let gone = opposite.remove(position);
                self.arrivals.remove(&gone.id);
            }
        }
        fills
    }

    /// Remaining quantity of one resting order, `None` when gone.
    pub fn remaining(&self, id: u64) -> Option<i64> {
        self.buys
            .iter()
            .chain(self.sells.iter())
            .find(|order| order.id == id)
            .map(|order| order.remaining)
    }

    /// Every resting order on both sides, oldest first. Snapshot before a
    /// match to remember consumed rows for a priority-preserving relist.
    pub fn orders(&self) -> Vec<Order> {
        let mut all: Vec<Order> = self.buys.iter().chain(self.sells.iter()).cloned().collect();
        all.sort_by_key(|order| order.seq);
        all
    }

    /// Next arrival rank (the `seq` the following placed order takes).
    /// Read before a match to stamp the taker's relist row with the rank
    /// it actually consumed.
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Puts an order back at its original queue rank WITHOUT matching
    /// (it counts as the latest arrival for `uncross`). Only for orders
    /// that rested uncrossed at that very rank a moment ago (rollback of
    /// a withdrawal, fold of same-price fragments) or a boot reload
    /// followed by `uncross`, in last-update order: anything else uses
    /// `relist_matching`, or the book stays crossed (live 2026-11-13,
    /// bronze axe: a relisted bid at 20 above asks at 16).
    pub fn relist(&mut self, order: Order) {
        self.next_seq = self.next_seq.max(order.seq.saturating_add(1));
        self.rest(order);
    }

    /// Re-lists an order after a failed handover: it re-enters as a taker
    /// (fills at the resting orders' prices, own orders and pairs
    /// `kept_apart` refuses skipped), and its remainder rests at its
    /// original `seq` (queue rank among its own side kept).
    pub fn relist_matching(&mut self, order: Order, kept_apart: &dyn Fn(&str, &str) -> bool) -> Vec<Fill> {
        self.next_seq = self.next_seq.max(order.seq.saturating_add(1));
        let mut incoming = order;
        let fills = self.take_as_taker(&mut incoming, kept_apart);
        if incoming.remaining > 0 {
            self.rest(incoming);
        }
        fills
    }

    /// Matches every crossed pair of resting orders (boot reload, a pair
    /// cooldown that expired, any leftover): best bid first, then the
    /// best eligible ask under it (own orders and pairs `kept_apart`
    /// refuses skipped). The order that (re)entered the book last is the
    /// taker: it trades at the price of the one already resting (user
    /// rule 2026-11-13: a bid relisted at 20 pays the 16 ask, never 20),
    /// whatever `seq` rank it kept. After it, only own or kept-apart
    /// pairs rest crossed.
    pub fn uncross(&mut self, kept_apart: &dyn Fn(&str, &str) -> bool) -> Vec<Fill> {
        let mut fills = Vec::new();
        loop {
            let mut pair: Option<(usize, usize)> = None;
            'bids: for (bid_index, bid) in self.buys.iter().enumerate() {
                // Bids run best first: once one sits under the best ask,
                // no later one crosses.
                if self.sells.first().map_or(true, |ask| ask.price > bid.price) {
                    break;
                }
                for (ask_index, ask) in self.sells.iter().enumerate() {
                    if ask.price > bid.price {
                        break;
                    }
                    if ask.account != bid.account && !kept_apart(&bid.account, &ask.account) {
                        pair = Some((bid_index, ask_index));
                        break 'bids;
                    }
                }
            }
            let Some((bid_index, ask_index)) = pair else {
                return fills;
            };
            let take = self.buys[bid_index].remaining.min(self.sells[ask_index].remaining);
            let (bid, ask) = (&self.buys[bid_index], &self.sells[ask_index]);
            let arrival = |order: &Order| self.arrivals.get(&order.id).copied().unwrap_or(0);
            let price = if arrival(bid) < arrival(ask) { bid.price } else { ask.price };
            fills.push(Fill {
                buy_id: bid.id,
                sell_id: ask.id,
                buyer: bid.account.clone(),
                seller: ask.account.clone(),
                item: bid.item,
                quantity: take,
                price,
            });
            self.buys[bid_index].remaining -= take;
            self.sells[ask_index].remaining -= take;
            if self.buys[bid_index].remaining == 0 {
                let gone = self.buys.remove(bid_index);
                self.arrivals.remove(&gone.id);
            }
            if self.sells[ask_index].remaining == 0 {
                let gone = self.sells.remove(ask_index);
                self.arrivals.remove(&gone.id);
            }
        }
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
    fn own_resting_order_is_skipped_not_a_wall() {
        let mut book = Book::new();
        place(&mut book, 1, "t01", Side::Sell, 1, 15);
        place(&mut book, 2, "t02", Side::Sell, 1, 16);
        let (fills, rests) = place(&mut book, 3, "t01", Side::Buy, 1, 20);
        assert!(!rests);
        assert_eq!((fills[0].seller.as_str(), fills[0].price), ("t02", 16));
        assert_eq!(book.best(), (None, Some(15)), "only the own ask rests");
    }

    #[test]
    fn relist_matching_takes_at_the_resting_price_and_keeps_its_rank() {
        let mut book = Book::new();
        place(&mut book, 1, "early", Side::Buy, 1, 10);
        place(&mut book, 2, "third", Side::Sell, 1, 16);
        let kept_apart = |buyer: &str, seller: &str| buyer == "lara" && seller == "partner";
        place(&mut book, 3, "partner", Side::Sell, 1, 15);
        let relisted = Order { id: 9, account: "lara".to_string(), item: 314, side: Side::Buy, quantity: 2, remaining: 2, price: 20, seq: 0 };
        let fills = book.relist_matching(relisted, &kept_apart);
        assert_eq!(fills.len(), 1, "partner kept apart, third matched once");
        assert_eq!((fills[0].seller.as_str(), fills[0].price, fills[0].quantity), ("third", 16, 1));
        let buyers: Vec<String> = book.buys.iter().map(|order| order.account.clone()).collect();
        assert_eq!(buyers, vec!["lara".to_string(), "early".to_string()], "remainder rests at its old rank");
        assert_eq!(book.best(), (Some(20), Some(15)), "crossed only against the kept-apart partner");
    }

    #[test]
    fn uncross_matches_at_the_older_price_and_respects_guards() {
        let mut book = Book::new();
        let order = |id: u64, account: &str, side: Side, price: i64, seq: u64| Order {
            id,
            account: account.to_string(),
            item: 314,
            side,
            quantity: 1,
            remaining: 1,
            price,
            seq,
        };
        book.relist(order(1, "seller", Side::Sell, 16, 1));
        book.relist(order(2, "partner", Side::Sell, 15, 3));
        book.relist(order(3, "buyer", Side::Buy, 20, 2));
        book.relist(order(4, "buyer", Side::Sell, 14, 4));
        let kept_apart = |buyer: &str, seller: &str| buyer == "buyer" && seller == "partner";
        let fills = book.uncross(&kept_apart);
        assert_eq!(fills.len(), 1);
        assert_eq!((fills[0].seller.as_str(), fills[0].price), ("seller", 16), "older ask is the maker");
        assert!(book.uncross(&kept_apart).is_empty(), "no double match");
        // Guard lifted: the partner now matches; the own ask never does.
        assert!(book.uncross(&|_: &str, _: &str| false).is_empty(), "buyer bid consumed");
        book.relist(order(5, "buyer", Side::Buy, 20, 5));
        let fills = book.uncross(&|_: &str, _: &str| false);
        assert_eq!(fills.len(), 1);
        assert_eq!((fills[0].seller.as_str(), fills[0].price), ("partner", 15), "older ask sets the price");
        assert_eq!(book.best(), (None, Some(14)), "own ask untouched");
        book.relist(order(6, "buyer", Side::Buy, 20, 6));
        assert!(book.uncross(&|_: &str, _: &str| false).is_empty(), "self never matches");
        assert_eq!(book.best(), (Some(20), Some(14)), "own pair may rest crossed");
    }

    /// User rule 2026-11-13 (lara_lane, bronze axe): a bid back at 20
    /// with its old rank pays the resting asks, 15 then 16, never 20 —
    /// as a relist and as a boot heal alike.
    #[test]
    fn late_arrival_pays_the_resting_asks_whatever_its_rank() {
        let lara = |id: u64| Order { id, account: "lara".to_string(), item: 314, side: Side::Buy, quantity: 2, remaining: 2, price: 20, seq: 0 };
        let mut relisted = Book::new();
        place(&mut relisted, 1, "third-a", Side::Sell, 1, 16);
        place(&mut relisted, 2, "third-b", Side::Sell, 1, 15);
        let fills = relisted.relist_matching(lara(9), &|_: &str, _: &str| false);
        let prices: Vec<i64> = fills.iter().map(|fill| fill.price).collect();
        assert_eq!(prices, vec![15, 16], "relist: best ask first, at the asks");
        let mut healed = Book::new();
        let ask = |id: u64, account: &str, price: i64, seq: u64| Order {
            id,
            account: account.to_string(),
            item: 314,
            side: Side::Sell,
            quantity: 1,
            remaining: 1,
            price,
            seq,
        };
        healed.relist(ask(1, "third-a", 16, 5));
        healed.relist(ask(2, "third-b", 15, 6));
        healed.relist(lara(9));
        let fills = healed.uncross(&|_: &str, _: &str| false);
        let prices: Vec<i64> = fills.iter().map(|fill| fill.price).collect();
        assert_eq!(prices, vec![15, 16], "boot heal: the latest arrival is the taker despite seq 0");
        assert_eq!(healed.best(), (None, None));
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
