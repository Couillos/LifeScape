//! The cached account reads equal the database read after every kind of
//! change: this connection's inserts, updates and cancels, deal moves, and
//! a commit from another connection of the same file.

use crate::deals::{DealStatus, NewDeal};
use crate::orders::{NewOrder, OrderSide, OrderStatus};
use crate::{ensure_test_item, temp_database_path, Error, Store};

const ACCOUNTS: [&str; 3] = ["a_one", "b_two", "c_three"];

fn agrees(store: &Store, step: &str) {
    for account in ACCOUNTS {
        assert_eq!(store.account_orders(account).expect("cached"), store.fresh_account_orders(account).expect("fresh"), "{step}: orders of {account}");
        assert_eq!(store.open_deals_for_account(account).expect("cached"), store.fresh_open_deals_for_account(account).expect("fresh"), "{step}: deals of {account}");
    }
}

fn order(account: &str, side: OrderSide, price: i64, clock: i64) -> NewOrder {
    NewOrder { account: account.to_string(), side, item_id: 314, quantity: 10, price, clock, ..NewOrder::default() }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound.max(1)
    }
}

#[test]
fn the_cached_reads_equal_the_database_after_every_change() -> Result<(), Error> {
    let path = temp_database_path("views");
    let store = Store::open(&path)?;
    ensure_test_item(&store, 314)?;
    // A second connection of the same file (a pool reader that also writes).
    let other = Store::open_migrated(&path)?;
    agrees(&store, "empty");
    let mut rng = Rng(0x5EED_1234_ABCD);
    let (mut orders, mut deals): (Vec<i64>, Vec<i64>) = (Vec::new(), Vec::new());
    let (mut hits_before, _) = crate::account_views::account_view_stats();
    for step in 0..400u64 {
        let account = ACCOUNTS[rng.below(3) as usize];
        let label = format!("step {step}");
        match rng.below(8) {
            0 | 1 => orders.push(store.place_order(&order(account, if rng.below(2) == 0 { OrderSide::Buy } else { OrderSide::Sell }, 1 + rng.below(9) as i64, 100 + step as i64))?),
            2 => {
                if let Some(id) = orders.get(rng.below(orders.len() as u64 + 1) as usize) {
                    let _ = store.cancel_order(*id, 500 + step as i64);
                }
            }
            3 => {
                if let Some(id) = orders.get(rng.below(orders.len() as u64 + 1) as usize) {
                    let _ = store.set_order_status(*id, OrderStatus::Matched, rng.below(10) as i64, 600 + step as i64);
                }
            }
            4 => {
                let (buyer, seller) = (ACCOUNTS[rng.below(3) as usize], ACCOUNTS[rng.below(3) as usize]);
                if buyer != seller {
                    let deal = NewDeal { buyer: buyer.to_string(), seller: seller.to_string(), item_id: 314, quantity: 3, price: 2, meeting_x: 0, meeting_y: 0, meeting_level: 0, deadline_tick: 99, clock: 700 + step as i64, taker_side: OrderSide::Buy };
                    deals.push(store.create_deal(&deal)?);
                }
            }
            5 => {
                if let Some(id) = deals.get(rng.below(deals.len() as u64 + 1) as usize) {
                    let status = if rng.below(2) == 0 { DealStatus::Settled } else { DealStatus::Trading };
                    let _ = store.set_deal_status(*id, status, "", 800 + step as i64);
                }
            }
            // A commit by the OTHER connection: the cache must see it.
            6 => orders.push(other.place_order(&order(account, OrderSide::Sell, 5, 900 + step as i64))?),
            _ => {
                if let Some(id) = orders.get(rng.below(orders.len() as u64 + 1) as usize) {
                    let _ = other.cancel_order(*id, 950 + step as i64);
                }
            }
        }
        // Read twice: the second read is the cached one.
        agrees(&store, &label);
        agrees(&store, &format!("{label} again"));
    }
    let (hits_after, _) = crate::account_views::account_view_stats();
    assert!(hits_after > hits_before, "the cache served nothing");
    hits_before = hits_after;
    let _ = hits_before;
    drop(store);
    drop(other);
    let _ = std::fs::remove_file(&path);
    Ok(())
}

#[test]
fn a_read_with_no_change_in_between_is_served_from_the_cache() -> Result<(), Error> {
    let store = Store::open_in_memory()?;
    ensure_test_item(&store, 314)?;
    store.place_order(&order("a_one", OrderSide::Buy, 3, 100))?;
    let first = store.account_orders("a_one")?;
    let (hits, misses) = crate::account_views::account_view_stats();
    let second = store.account_orders("a_one")?;
    let (hits_after, misses_after) = crate::account_views::account_view_stats();
    assert_eq!(first, second);
    assert!(hits_after > hits && misses_after >= misses, "the second read hit the database");
    // A write by this connection moves the stamp: the next read is fresh.
    store.place_order(&order("a_one", OrderSide::Sell, 9, 101))?;
    assert_eq!(store.account_orders("a_one")?.len(), 2);
    Ok(())
}
