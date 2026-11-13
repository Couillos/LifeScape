#![forbid(unsafe_code)]
//! Order books backed by real holdings (user rule, 2026-11-13: "nettoyer
//! les carnets d'ordre impossibles en verifiant chaque compte").
//!
//! Live 2026-11-12/13: lara_lane and ryan_park sold raw shrimps they did
//! not own at all (0 banked, 0 carried) across several deals; every
//! handover died and the phantom order relisted forever. Each audit round
//! weighs every account's open orders against what it really holds
//! (pack + bank, the engine save when offline), minus what in-flight
//! deals already promise, and cancels the orders that cover nothing:
//! oldest first keep their claim, the rest go.

use std::collections::{BTreeMap, HashMap};

use lifescape_json::Value;
use lifescape_store::{Order, OrderSide};

use crate::jobs::Job;
use crate::market_service::MarketService;

pub const COINS_ID: i64 = 995;

/// Coins and per-object stock of one live snapshot (pack, bank, worn:
/// numeric ids). The bank row is the last known bank (the heartbeat keeps
/// it once closed): banked coins fund bids, banked stock covers asks.
pub(crate) fn ledger_holdings(live: &Value, coins_id: i64) -> (i64, Vec<(i64, i64)>) {
    let mut coins = 0i64;
    let mut stock: HashMap<i64, i64> = HashMap::new();
    for key in ["inv", "bank", "worn"] {
        for entry in live.get(key).as_arr().cloned().unwrap_or_default() {
            if !matches!(entry, Value::Obj(_)) {
                continue;
            }
            let id = entry.get("id").as_i64().unwrap_or(-1);
            let count = entry.get("count").as_i64().unwrap_or(1).max(0);
            if id < 0 {
                continue;
            }
            if id == coins_id {
                coins += count;
            } else {
                *stock.entry(id).or_insert(0) += count;
            }
        }
    }
    let mut stock: Vec<(i64, i64)> = stock.into_iter().collect();
    stock.sort();
    (coins, stock)
}

/// Base object of a carried id: a banknote counts for the object it
/// notes (`cert_<name>`), anything else for itself.
pub fn base_of(id: i64) -> i64 {
    let Ok(index) = usize::try_from(id) else { return id };
    let name = lifescape_gamedata::obj_by_id(index).and_then(|entry| entry.name).unwrap_or_default();
    match name.strip_prefix("cert_") {
        Some(base) => lifescape_gamedata::objs_by_name(base).into_iter().next().map(|entry| entry.obj_id as i64).unwrap_or(id),
        None => id,
    }
}

/// Sellable holdings of one account: pack and bank (never worn), notes
/// folded into their base object, coins apart.
pub fn sellable(inv: &[(i64, i64)], bank: &[(i64, i64)]) -> (i64, BTreeMap<i64, i64>) {
    let mut coins = 0i64;
    let mut stock: BTreeMap<i64, i64> = BTreeMap::new();
    for (id, count) in inv.iter().chain(bank.iter()) {
        if *id == COINS_ID {
            coins += *count;
        } else {
            *stock.entry(base_of(*id)).or_insert(0) += *count;
        }
    }
    (coins, stock)
}

/// Orders one account cannot honour. `coins`/`stock` are what it holds;
/// `promised_coins`/`promised` what its in-flight deals already take.
/// Oldest orders keep their claim (they were placed first); an order
/// only partly covered goes whole: a book never shows lots it cannot
/// hand over.
pub fn uncovered(orders: &[Order], coins: i64, stock: &BTreeMap<i64, i64>, promised_coins: i64, promised: &BTreeMap<i64, i64>) -> Vec<i64> {
    let mut sorted: Vec<&Order> = orders.iter().collect();
    sorted.sort_by_key(|order| (order.created_clock, order.order_id));
    let mut purse = coins - promised_coins;
    let mut left: BTreeMap<i64, i64> = stock.clone();
    for (item, qty) in promised {
        *left.entry(*item).or_insert(0) -= qty;
    }
    let mut cancel = Vec::new();
    for order in sorted {
        let remaining = order.remaining.max(0);
        if order.side == OrderSide::Buy {
            let cost = remaining * order.price.max(0);
            if purse >= cost {
                purse -= cost;
            } else {
                cancel.push(order.order_id);
            }
        } else {
            let have = left.entry(order.item_id).or_insert(0);
            if *have >= remaining {
                *have -= remaining;
            } else {
                cancel.push(order.order_id);
            }
        }
    }
    cancel
}

fn rows_of(live: &Value, key: &str) -> Vec<(i64, i64)> {
    live.get(key)
        .as_arr()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| Some((entry.get("id").as_i64()?, entry.get("count").as_i64().unwrap_or(1).max(0))))
        .collect()
}

/// What the account really holds, from the freshest witness: a live
/// snapshot whose bank streamed this session, else the engine save (the
/// server writes it at logout; a live bot's save bank stands in until it
/// opens its bank). None when nothing witnesses the account.
fn witness(account: &str, live: Option<&Value>, save_dir: &str) -> Option<(i64, BTreeMap<i64, i64>)> {
    let save = lifescape_sdk::save::load_account(account, Some(save_dir));
    let saved = |bucket: &HashMap<u16, u32>| -> Vec<(i64, i64)> { bucket.iter().map(|(id, n)| (i64::from(*id), i64::from(*n))).collect() };
    match live {
        Some(state) => {
            let inv = rows_of(state, "inv");
            let bank = if state.get("bank_streamed").as_bool() == Some(true) {
                rows_of(state, "bank")
            } else {
                save.as_ref().map(|found| saved(&found.bank)).unwrap_or_default()
            };
            Some(sellable(&inv, &bank))
        }
        None => save.map(|found| sellable(&saved(&found.inv), &saved(&found.bank))),
    }
}

impl MarketService {
    /// One audit round over every account with open orders: cancels the
    /// orders its real holdings cannot cover. Accounts with no witness at
    /// all (no live snapshot, no save: deleted) lose every order.
    pub(crate) fn audit_round(&self, jobs: &[std::sync::Arc<Job>], save_dir: &str) -> usize {
        let now = crate::jobs::now_secs();
        let mut lives: HashMap<String, Value> = HashMap::new();
        for job in jobs {
            if let Ok(Some(live)) = job.inner.try_lock().map(|inner| inner.live.clone()) {
                if live.get("updated").as_f64().is_some_and(|at| now - at < 120.0) {
                    lives.insert(job.account.to_lowercase(), live);
                }
            }
        }
        let open: Vec<Order> = self
            .lock_store()
            .resting_orders()
            .unwrap_or_default()
            .into_iter()
            // Open rows and the resting remainder of partly filled ones.
            .filter(|order| order.remaining > 0 && matches!(order.status, lifescape_store::OrderStatus::Open | lifescape_store::OrderStatus::Matched))
            .collect();
        let mut by_account: BTreeMap<String, Vec<Order>> = BTreeMap::new();
        for order in open {
            by_account.entry(order.account.clone()).or_default().push(order);
        }
        let mut cancelled = 0usize;
        for (account, orders) in by_account {
            let Some((coins, stock)) = witness(&account, lives.get(&account.to_lowercase()), save_dir) else {
                cancelled += self.cancel_rows(&account, &orders.iter().map(|order| order.order_id).collect::<Vec<_>>(), "no witness (account gone)");
                continue;
            };
            let deals = self.lock_store().open_deals_for_account(&account).unwrap_or_default();
            let mut promised_coins = 0i64;
            let mut promised: BTreeMap<i64, i64> = BTreeMap::new();
            for deal in &deals {
                if deal.buyer == account {
                    promised_coins += deal.quantity * deal.price;
                } else {
                    *promised.entry(deal.item_id).or_insert(0) += deal.quantity;
                }
            }
            let bad = uncovered(&orders, coins, &stock, promised_coins, &promised);
            if !bad.is_empty() {
                cancelled += self.cancel_rows(&account, &bad, "not covered by its holdings");
            }
        }
        cancelled
    }

    /// What each live account's open orders and in-flight deals promise:
    /// coins for bids and buyer deals, base objects for sells and seller
    /// deals. Pushed to the workers (`set_locks`) so no script spends them
    /// outside the trade procedure (user rule 2026-11-13).
    pub(crate) fn locks_for(&self, accounts: &[String]) -> BTreeMap<String, (i64, BTreeMap<i64, i64>)> {
        let open: Vec<Order> = self
            .lock_store()
            .resting_orders()
            .unwrap_or_default()
            .into_iter()
            // Open rows and the resting remainder of partly filled ones.
            .filter(|order| order.remaining > 0 && matches!(order.status, lifescape_store::OrderStatus::Open | lifescape_store::OrderStatus::Matched))
            .collect();
        let mut out: BTreeMap<String, (i64, BTreeMap<i64, i64>)> = BTreeMap::new();
        for account in accounts {
            let mut coins = 0i64;
            let mut items: BTreeMap<i64, i64> = BTreeMap::new();
            for order in open.iter().filter(|order| &order.account == account) {
                if order.side == OrderSide::Buy {
                    coins += order.remaining.max(0) * order.price.max(0);
                } else {
                    *items.entry(order.item_id).or_insert(0) += order.remaining.max(0);
                }
            }
            for deal in self.lock_store().open_deals_for_account(account).unwrap_or_default() {
                if &deal.buyer == account {
                    coins += deal.quantity * deal.price;
                } else {
                    *items.entry(deal.item_id).or_insert(0) += deal.quantity;
                }
            }
            out.insert(account.clone(), (coins, items));
        }
        out
    }

    fn cancel_rows(&self, account: &str, ids: &[i64], why: &str) -> usize {
        let mut done = 0usize;
        for id in ids {
            let reply = self.handle(&lifescape_json::object(vec![
                ("op", lifescape_json::str_value("cancel")),
                ("account", lifescape_json::str_value(account)),
                ("order", Value::Int(*id)),
            ]));
            if reply.get("ok").as_bool() == Some(true) {
                done += 1;
            }
        }
        if done > 0 {
            crate::logln!("market-audit: {account}: {done} order(s) cancelled, {why} ({ids:?})");
        }
        done
    }

    /// Funds the ledger from the last pushed holdings (L11): the
    /// coverage check in `op_place` runs against the ledger, but nothing
    /// ever credited live bots, so every live order failed coverage. Each
    /// convene round mirrors inv+bank+worn item counts (numeric ids from
    /// the snapshot) into the ledger; match-time settlement still moves
    /// it between rounds. Stale (>30 s) or missing snapshots skip: no
    /// funding without a live witness. SET semantics over-commit window
    /// documented on `sync_account`.
    pub(crate) fn sync_ledger_from_jobs(&self, jobs: &[std::sync::Arc<Job>]) {
        let coins_id = lifescape_gamedata::objs_by_name("Coins")
            .into_iter()
            .next()
            .map(|entry| entry.obj_id as i64)
            .unwrap_or(995);
        let now = crate::jobs::now_secs();
        for job in jobs {
            let live = match job.inner.try_lock().map(|inner| inner.live.clone()) {
                Ok(Some(live)) => live,
                _ => continue,
            };
            if !live.get("updated").as_f64().is_some_and(|at| now - at < 30.0) {
                continue;
            }
            let (coins, stock) = ledger_holdings(&live, coins_id);
            self.lock_engine().ledger.sync_account(&job.account, coins, &stock);
        }
    }
}

/// Daemon side of the locks: one `set_locks` per live worker each round.
impl crate::daemon::Orchestrator {
    pub(crate) fn push_locks(&self, service: &MarketService, accounts: &[String]) {
        for (account, (coins, items)) in service.locks_for(accounts) {
            let rows: Vec<Value> = items
                .iter()
                .map(|(id, qty)| Value::Arr(vec![Value::Int(*id), Value::Int(*qty)]))
                .collect();
            let request = lifescape_json::object(vec![
                ("cmd", lifescape_json::str_value("set_locks")),
                ("coins", Value::Int(coins)),
                ("items", Value::Arr(rows)),
            ]);
            let _ = self.send_worker(&account, &request, std::time::Duration::from_secs(2));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{uncovered, COINS_ID};
    use lifescape_store::{Order, OrderSide, OrderStatus};
    use std::collections::BTreeMap;

    fn order(id: i64, side: OrderSide, item: i64, remaining: i64, price: i64, clock: i64) -> Order {
        Order {
            order_id: id,
            account: "lara_lane".to_string(),
            side,
            item_id: item,
            quantity: remaining,
            remaining,
            price,
            status: OrderStatus::Open,
            dormant: false,
            created_clock: clock,
            updated_clock: clock,
        }
    }

    /// Live 2026-11-13: lara_lane listed raw shrimps (317) she did not own.
    /// Nothing held: every sell goes; a covered order stays.
    #[test]
    fn sells_without_stock_are_cancelled() {
        let orders = vec![order(1, OrderSide::Sell, 317, 14, 1, 10), order(2, OrderSide::Sell, 1511, 5, 1, 11)];
        let stock = BTreeMap::from([(1511i64, 5i64)]);
        assert_eq!(uncovered(&orders, 0, &stock, 0, &BTreeMap::new()), vec![1]);
    }

    /// Oldest orders keep their claim; stock promised to an in-flight
    /// deal is not counted twice.
    #[test]
    fn oldest_claim_first_and_promised_stock_is_taken() {
        let orders = vec![order(7, OrderSide::Sell, 317, 10, 1, 20), order(5, OrderSide::Sell, 317, 10, 1, 10)];
        let stock = BTreeMap::from([(317i64, 25i64)]);
        let promised = BTreeMap::from([(317i64, 10i64)]);
        assert_eq!(uncovered(&orders, 0, &stock, 0, &promised), vec![7], "15 left: the older order 5 stays");
    }

    /// Bids are covered by the purse, oldest first, minus coins already
    /// promised to deals.
    #[test]
    fn bids_need_the_coins() {
        let orders = vec![order(1, OrderSide::Buy, 317, 10, 2, 10), order(2, OrderSide::Buy, 1511, 5, 3, 11)];
        // 10 x 2 + 5 x 3 = 35 coins.
        assert_eq!(uncovered(&orders, 35, &BTreeMap::new(), 0, &BTreeMap::new()), Vec::<i64>::new(), "35 covers both");
        assert_eq!(uncovered(&orders, 35, &BTreeMap::new(), 17, &BTreeMap::new()), vec![1], "18 left: the 20-coin bid goes, the 15-coin one fits");
        let _ = COINS_ID;
    }
}
