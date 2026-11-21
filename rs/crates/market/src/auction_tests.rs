//! Grouped auction, mode hysteresis and anchor (lot 3 marché, B2-B4).

use super::*;
use crate::book::{Book, Side};

fn no_pair_kept_apart(_buyer: &str, _seller: &str) -> bool {
    false
}

/// B2: the price that trades the most units, the middle of the
/// equilibrium interval on a tie.
#[test]
fn the_clearing_price_trades_the_most_units() {
    // Buyers 5 at 10 and 5 at 8, sellers 3 at 6 and 5 at 9: at 9 or 10
    // five units change hands (at 8 only the three cheap ones).
    let found = clearing(&[(10, 5), (8, 5)], &[(6, 3), (9, 5)]).expect("crosses");
    assert_eq!((found.volume, found.lowest, found.highest), (5, 9, 10));
    assert_eq!(found.price, 9, "middle of [9, 10], rounded down");
    let lone = clearing(&[(10, 1)], &[(4, 1)]).expect("crosses");
    assert_eq!((lone.volume, lone.price), (1, 7), "one buyer at 10, one seller at 4: the middle");
}

#[test]
fn nothing_crossing_clears_nothing() {
    assert_eq!(clearing(&[(5, 3)], &[(9, 3)]), None, "bid under ask");
    assert_eq!(clearing(&[(5, 3)], &[]), None, "a buyer without a seller");
    assert_eq!(clearing(&[], &[(5, 3)]), None, "a seller without a buyer");
    assert_eq!(clearing(&[], &[]), None);
}

/// The chosen price is never one where fewer units trade.
#[test]
fn the_clearing_price_always_reaches_the_maximum_volume() {
    let bids: [(i64, i64); 4] = [(12, 2), (9, 4), (7, 1), (3, 6)];
    let asks: [(i64, i64); 4] = [(2, 1), (6, 3), (8, 5), (13, 2)];
    let found = clearing(&bids, &asks).expect("crosses");
    let traded = |price: i64| {
        let demand: i64 = bids.iter().filter(|(bid, _)| *bid >= price).map(|(_, qty)| qty).sum();
        let supply: i64 = asks.iter().filter(|(ask, _)| *ask <= price).map(|(_, qty)| qty).sum();
        demand.min(supply)
    };
    assert_eq!(traded(found.price), found.volume);
    assert!((1..=20).all(|price| traded(price) <= found.volume), "no price trades more");
}

/// B2: between two auctions the book does not match; the auction trades
/// everyone who agrees at one price, best prices and oldest orders first.
#[test]
fn a_batch_book_rests_crossed_until_its_auction() {
    let mut book = Book::new();
    book.set_batch(true);
    let (fills, _) = book.place(1, "seller-a", 314, Side::Sell, 3, 6).expect("places");
    assert!(fills.is_empty());
    let (fills, rests) = book.place(2, "seller-b", 314, Side::Sell, 5, 9).expect("places");
    assert!(fills.is_empty() && rests);
    let (fills, rests) = book.place(3, "buyer-a", 314, Side::Buy, 5, 10).expect("places");
    assert!(fills.is_empty() && rests, "a crossing bid rests: no continuous match");
    book.place(4, "buyer-b", 314, Side::Buy, 5, 8).expect("places");
    assert!(book.uncross(&no_pair_kept_apart).is_empty(), "uncross leaves a batch book alone");
    let fills = book.run_auction(&no_pair_kept_apart);
    assert_eq!(fills.iter().map(|fill| fill.quantity).sum::<i64>(), 5);
    assert!(fills.iter().all(|fill| fill.price == 9), "one price for everybody: {fills:?}");
    assert_eq!(fills.iter().find(|fill| fill.seller == "seller-a").map(|fill| fill.quantity), Some(3), "the cheapest ask first");
    assert_eq!(book.remaining(3), None, "the best bid was served in full");
    assert_eq!(book.remaining(4), Some(5), "the bid under the price waits");
    assert_eq!(book.remaining(2), Some(3), "the rest of the 9 ask waits");
}

#[test]
fn an_account_never_trades_with_itself_in_an_auction() {
    let mut book = Book::new();
    book.set_batch(true);
    book.place(1, "solo", 314, Side::Sell, 2, 5).expect("places");
    book.place(2, "solo", 314, Side::Buy, 2, 9).expect("places");
    assert!(book.run_auction(&no_pair_kept_apart).is_empty());
    book.place(3, "other", 314, Side::Buy, 2, 9).expect("places");
    let fills = book.run_auction(&|_buyer: &str, seller: &str| seller == "never");
    assert_eq!(fills.iter().map(|fill| (fill.buyer.as_str(), fill.seller.as_str(), fill.quantity)).collect::<Vec<_>>(), vec![("other", "solo", 2)]);
}

#[test]
fn a_continuous_book_is_not_batched_by_default() {
    let mut book = Book::new();
    book.place(1, "seller", 314, Side::Sell, 1, 5).expect("places");
    let (fills, _) = book.place(2, "buyer", 314, Side::Buy, 1, 9).expect("places");
    assert_eq!(fills.len(), 1, "plain books still match on arrival");
}

/// B3: the mean time between two trades against one walk window, with
/// a hysteresis between the two thresholds.
#[test]
fn the_mode_follows_the_trade_gap_with_hysteresis() {
    // 60 ms ticks: a window of 2 000 ticks = 120 s.
    let tick_ms = 60.0;
    let slow = trade_gap_ticks(&[1000, 1300, 1600], 1700, tick_ms).expect("measured");
    assert!((slow - 5000.0).abs() < 1.0, "300 s apart = 5 000 ticks: {slow}");
    assert!(next_mode(false, Some(slow)), "slower than a window: auction");
    let brisk = trade_gap_ticks(&[1000, 1050, 1100, 1150], 1160, tick_ms).expect("measured");
    assert!((brisk - 833.0).abs() < 1.0, "50 s apart: {brisk}");
    assert!(!next_mode(true, Some(brisk)), "twice as frequent as a window: continuous again");
    // Between 1 000 and 2 000 ticks the mode stays: no flip-flop.
    let between = trade_gap_ticks(&[1000, 1090, 1180], 1200, tick_ms).expect("measured");
    assert!(between > EXIT_GAP_TICKS && between <= ENTER_GAP_TICKS, "{between}");
    assert!(next_mode(true, Some(between)) && !next_mode(false, Some(between)));
}

#[test]
fn a_burst_then_silence_and_a_lone_trade_read_as_rare() {
    let tick_ms = 60.0;
    let silent = trade_gap_ticks(&[1000, 1001, 1002], 1000 + 3600, tick_ms).expect("measured");
    assert!(next_mode(false, Some(silent)), "an hour of silence after a burst: rare ({silent})");
    assert_eq!(trade_gap_ticks(&[1000], 1010, tick_ms), None, "one trade: no mean");
    assert!(next_mode(false, None), "never or once traded: auction");
}

/// B4: the volume-weighted mean; an isolated one-coin sale barely moves it.
#[test]
fn an_isolated_one_coin_sale_barely_moves_the_anchor() {
    let steady = [(100, 40), (102, 30), (98, 50), (101, 20)];
    assert_eq!(anchor_price(&steady), Some(100));
    let mut with_dump = steady.to_vec();
    with_dump.push((1, 1));
    let moved = anchor_price(&with_dump).expect("anchor");
    assert!(moved >= 99, "one unit at 1 coin among 140: {moved}");
    assert_eq!(anchor_price(&[(7, 0), (0, 5)]), None, "nothing usable");
    assert_eq!(anchor_price(&[]), None);
}
