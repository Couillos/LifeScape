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
                // Keyed by the object: a deal (`net_of_deals`) and the
                // books sit on the base id, a banknote is its object.
                *stock.entry(base_of(id)).or_insert(0) += count;
            }
        }
    }
    let mut stock: Vec<(i64, i64)> = stock.into_iter().collect();
    stock.sort();
    (coins, stock)
}

/// True when a pushed snapshot witnesses the account's holdings: fresh
/// (under 30 s), its pack streamed and its bank seen since the session
/// opened (`inv_streamed`, `bank_streamed`). A fresh worker knows no
/// bank until its first bank open: mirroring it then set the ledger to
/// 0 coins and 0 items and every order of the account failed coverage
/// (live 2026-11-19, thorn_vera after a redeploy). The last known
/// ledger stands until a witness comes.
pub(crate) fn witnesses_holdings(live: &Value, now: f64) -> bool {
    live.get("updated").as_f64().is_some_and(|at| now - at < 30.0)
        && live.get("inv_streamed").as_bool() == Some(true)
        && live.get("bank_streamed").as_bool() == Some(true)
}

/// Base object of a carried id: a banknote counts for the object it
/// notes (`cert_<name>`), anything else for itself. The planner's rule
/// (`lifescape_planner::base_of`, lot 3 of the launch refactor): one copy.
pub fn base_of(id: i64) -> i64 {
    lifescape_planner::base_of(id)
}

/// Book of an order placed on `id` (2026-11-16, found live by M9: the
/// carnets were cut in two, Gold bar ask 51 on 2357 and bid 59 on its
/// banknote 2358 never crossed): a banknote trades in its base object's
/// book. The handover carries the note anyway (`market_client::trade_id`)
/// and the bank holds base objects.
pub fn book_item(id: i64) -> i64 {
    base_of(id)
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

/// Orders one account cannot honour. `coins`/`stock` are what it holds
/// (base objects, see `sellable`); `promised_coins`/`promised` what its
/// in-flight deals already take. Orders and promises on a banknote count
/// against its base object (the bank holds base objects). Oldest orders
/// keep their claim (they were placed first); an order only partly
/// covered goes whole: a book never shows lots it cannot hand over.
pub fn uncovered(orders: &[Order], coins: i64, stock: &BTreeMap<i64, i64>, promised_coins: i64, promised: &BTreeMap<i64, i64>) -> Vec<i64> {
    let plan = plan_coverage(orders, coins, stock, promised_coins, promised, false);
    plan.cancel
}

/// What one audit round does to an account's orders (plan marché C4).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AuditPlan {
    /// Orders withdrawn: a sell its stock cannot cover, a bid the purse
    /// cannot keep at even 1 coin a unit.
    pub cancel: Vec<i64>,
    /// Bids lowered to what the purse still covers `(order, price)`: an
    /// ordered cut of the ceiling instead of a cancel (C4: the bot spent
    /// coins at a shop or a toll; its need and lineage go on).
    pub lower: Vec<(i64, i64)>,
}

/// `uncovered` that lowers an uncovered bid to the per-unit price the
/// purse left still covers (at least 1 coin) instead of cancelling it.
/// Lineages keep their claim by t0 (an amend never loses its rank here).
pub fn audit_plan(orders: &[Order], coins: i64, stock: &BTreeMap<i64, i64>, promised_coins: i64, promised: &BTreeMap<i64, i64>) -> AuditPlan {
    plan_coverage(orders, coins, stock, promised_coins, promised, true)
}

fn plan_coverage(orders: &[Order], coins: i64, stock: &BTreeMap<i64, i64>, promised_coins: i64, promised: &BTreeMap<i64, i64>, lower_bids: bool) -> AuditPlan {
    let mut sorted: Vec<&Order> = orders.iter().collect();
    sorted.sort_by_key(|order| (order.lineage_clock(), order.order_id));
    let mut purse = coins - promised_coins;
    let mut left: BTreeMap<i64, i64> = stock.clone();
    for (item, qty) in promised {
        *left.entry(base_of(*item)).or_insert(0) -= qty;
    }
    let mut plan = AuditPlan::default();
    let cancel = &mut plan.cancel;
    for order in sorted {
        let remaining = order.remaining.max(0);
        if order.side == OrderSide::Buy {
            // Coins and prices per lot (plan prix lot D).
            let lot = crate::market_units::lot_of(order.item_id);
            let cost = crate::market_units::deal_coins(order.item_id, remaining, order.price.max(0));
            let affordable = if remaining > 0 { purse.max(0).saturating_mul(lot) / remaining } else { 0 };
            if purse >= cost {
                purse -= cost;
            } else if lower_bids && affordable >= 1 {
                plan.lower.push((order.order_id, affordable));
                purse -= crate::market_units::deal_coins(order.item_id, remaining, affordable);
            } else {
                cancel.push(order.order_id);
            }
        } else {
            let have = left.entry(base_of(order.item_id)).or_insert(0);
            if *have >= remaining {
                *have -= remaining;
            } else {
                cancel.push(order.order_id);
            }
        }
    }
    plan
}

/// What a new order may still commit (live 2026-11-13, deals 1716/1722,
/// 1721, 1692/1720, 1772, 1643-1652): `coins`/`stock` are the ledger
/// (pack + bank minus in-flight deals, see `net_of_deals`), and the
/// account's resting orders of the same side already claim their share.
/// Before, a fully matched sell (remaining 0, goods still in the bank
/// until the handover) left the stock looking free: the brain listed it
/// again, the relist matched at once, and the seller came `short`.
/// Returns `(free units of the order's base object, free coins)`.
pub fn uncommitted(side: OrderSide, item: i64, coins: i64, stock: &[(i64, i64)], resting: &[Order]) -> (i64, i64) {
    let base = base_of(item);
    let held: i64 = stock.iter().filter(|(id, _)| base_of(*id) == base).map(|(_, qty)| *qty).sum();
    let mut claimed_units = 0i64;
    let mut claimed_coins = 0i64;
    for order in resting.iter().filter(|order| order.side == side && order.remaining > 0) {
        if side == OrderSide::Sell && base_of(order.item_id) == base {
            claimed_units += order.remaining;
        } else if side == OrderSide::Buy {
            claimed_coins += crate::market_units::deal_coins(order.item_id, order.remaining, order.price.max(0));
        }
    }
    ((held - claimed_units).max(0), (coins - claimed_coins).max(0))
}

/// Highest per-lot price in `[old, wanted]` a buy order of `remaining`
/// units may move to when its account has `free` coins beyond every
/// resting bid (this one at `old` included): an account never promises
/// more coins than it holds (user 2026-11-19, point 1; live hana_91:
/// 148 893 coins locked in 29 bids for 144 022 held, the 10 % walk had
/// raised them past its wallet). A cut always passes.
pub fn affordable_raise(item: i64, remaining: i64, old: i64, wanted: i64, free: i64) -> i64 {
    if wanted <= old {
        return wanted;
    }
    let extra = |price: i64| crate::market_units::deal_coins(item, remaining, price) - crate::market_units::deal_coins(item, remaining, old);
    if extra(wanted) <= free {
        return wanted;
    }
    // The cost only grows with the price: bisection on [old, wanted).
    let (mut low, mut high) = (old, wanted);
    while high - low > 1 {
        let middle = low + (high - low) / 2;
        if extra(middle) <= free {
            low = middle;
        } else {
            high = middle;
        }
    }
    low
}

/// Ledger mirror of one live snapshot net of the account's in-flight
/// deals: the goods a seller promised and the coins a buyer promised are
/// still physically held until the handover, but no longer free. Pure:
/// the match-time ledger move (`record_fill`) keeps the same meaning
/// between two syncs.
pub fn net_of_deals(account: &str, coins: i64, stock: &[(i64, i64)], deals: &[lifescape_store::Deal]) -> (i64, Vec<(i64, i64)>) {
    let mut purse = coins;
    let mut left: BTreeMap<i64, i64> = stock.iter().copied().collect();
    for deal in deals {
        if deal.buyer == account {
            purse -= crate::market_units::deal_coins(deal.item_id, deal.quantity, deal.price.max(0));
        } else if deal.seller == account {
            *left.entry(deal.item_id).or_insert(0) -= deal.quantity;
        }
    }
    (purse.max(0), left.into_iter().collect())
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
                    promised_coins += crate::market_units::deal_coins(deal.item_id, deal.quantity, deal.price);
                } else {
                    *promised.entry(deal.item_id).or_insert(0) += deal.quantity;
                }
            }
            let plan = audit_plan(&orders, coins, &stock, promised_coins, &promised);
            if !plan.cancel.is_empty() {
                cancelled += self.cancel_rows(&account, &plan.cancel, "not covered by its holdings");
            }
            self.lower_rows(&account, &plan.lower);
        }
        cancelled
    }

    /// What each live account's open orders and in-flight deals promise:
    /// coins for bids and buyer deals, base objects for sells and seller
    /// deals (a banknote order locks its base stack: the bank, where the
    /// lock bites, holds base objects). Pushed to the workers (`set_locks`) so no script spends them
    /// outside the trade procedure (user rule 2026-11-13).
    pub fn locks_for(&self, accounts: &[String]) -> BTreeMap<String, (i64, BTreeMap<i64, i64>)> {
        let mut out: BTreeMap<String, (i64, BTreeMap<i64, i64>)> = BTreeMap::new();
        for account in accounts {
            // The account's own resting rows through its index (the
            // whole-carnet scan read ~95k rows per call, many calls per
            // brain pass: CPU 2026-11-14). Same rows as `resting_orders`
            // (not dormant), open and the resting remainder of partly
            // filled ones.
            let open: Vec<Order> = self
                .lock_store()
                .account_orders(account)
                .unwrap_or_default()
                .into_iter()
                .filter(|order| !order.dormant && order.remaining > 0 && matches!(order.status, lifescape_store::OrderStatus::Open | lifescape_store::OrderStatus::Matched))
                .collect();
            let mut coins = 0i64;
            let mut items: BTreeMap<i64, i64> = BTreeMap::new();
            for order in &open {
                if order.side == OrderSide::Buy {
                    coins += crate::market_units::deal_coins(order.item_id, order.remaining.max(0), order.price.max(0));
                } else {
                    *items.entry(base_of(order.item_id)).or_insert(0) += order.remaining.max(0);
                }
            }
            for deal in self.lock_store().open_deals_for_account(account).unwrap_or_default() {
                if &deal.buyer == account {
                    coins += crate::market_units::deal_coins(deal.item_id, deal.quantity, deal.price);
                } else {
                    *items.entry(base_of(deal.item_id)).or_insert(0) += deal.quantity;
                }
            }
            out.insert(account.clone(), (coins, items));
        }
        out
    }

    /// Lowers each uncovered bid to the price its purse covers (C4): an
    /// amend (the lineage goes on), its walk capped there.
    fn lower_rows(&self, account: &str, lowered: &[(i64, i64)]) {
        let mut done = Vec::new();
        for (order, price) in lowered {
            let reply = self.handle(&lifescape_json::object(vec![
                ("op", lifescape_json::str_value("amend")),
                ("account", lifescape_json::str_value(account)),
                ("order", Value::Int(*order)),
                ("price", Value::Int(*price)),
                ("per_lot", Value::Bool(true)),
            ]));
            let Some(replacement) = reply.get("order").as_i64().filter(|_| reply.get("ok").as_bool() == Some(true)) else {
                crate::logln!("market-audit: {account}: bid {order} not lowered to {price}: {:?}", reply.get("error").as_str());
                continue;
            };
            if let Some(bounds) = self.lock_engine().quote_bounds.get_mut(&replacement) {
                bounds.ceiling = bounds.ceiling.min(*price);
                bounds.floor = bounds.floor.min(*price);
            }
            done.push((*order, replacement, *price));
        }
        if !done.is_empty() {
            crate::logln!("market-audit: {account}: {} bid(s) lowered to what its coins cover (order, replacement, price): {done:?}", done.len());
        }
    }

    fn cancel_rows(&self, account: &str, ids: &[i64], why: &str) -> usize {
        let mut done = 0usize;
        for id in ids {
            let reply = self.handle(&lifescape_json::object(vec![
                ("op", lifescape_json::str_value("cancel")),
                ("account", lifescape_json::str_value(account)),
                ("order", Value::Int(*id)),
                ("reason", lifescape_json::str_value("audit")),
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
    /// funding without a live witness, and a snapshot whose pack or bank
    /// has not streamed since its session opened is no witness either
    /// (`witnesses_holdings`, live 2026-11-19 thorn_vera). The mirror is net of the
    /// account's in-flight deals (`net_of_deals`): the SET used to hand
    /// promised goods and coins back as free every round (deals 1716,
    /// 1721, 1772: relisted stock, re-spent coins, `short` at the bank).
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
            if !witnesses_holdings(&live, now) {
                continue;
            }
            let (coins, stock) = ledger_holdings(&live, coins_id);
            self.sync_holdings(&job.account, coins, &stock);
        }
    }

    /// Mirrors one witnessed holding into the ledger, net of the
    /// account's in-flight deals.
    pub fn sync_holdings(&self, account: &str, coins: i64, stock: &[(i64, i64)]) {
        let deals = self.lock_store().open_deals_for_account(account).unwrap_or_default();
        let (coins, stock) = net_of_deals(account, coins, stock, &deals);
        self.lock_engine().ledger.sync_account(account, coins, &stock);
    }

    /// Free coverage of a new order: the ledger net of the account's
    /// resting orders of the same side (`uncommitted`).
    pub fn free_coverage(&self, account: &str, side: OrderSide, item: i64) -> (i64, i64) {
        let resting: Vec<Order> = self
            .lock_store()
            .account_orders(account)
            .unwrap_or_default()
            .into_iter()
            .filter(|order| order.remaining > 0 && matches!(order.status, lifescape_store::OrderStatus::Open | lifescape_store::OrderStatus::Matched))
            .collect();
        let engine = self.lock_engine();
        let stock = engine.ledger.stock_rows(account);
        let (units, coins) = uncommitted(side, item, engine.ledger.coins_of(account), &stock, &resting);
        // An open quest's objects are never offered (bh99tw2f, 2026-11-14:
        // every banked air talisman listed, Rune Mysteries dead-ended).
        let reserved = engine.quest_reserved.get(account).and_then(|kept| kept.get(&base_of(item))).copied().unwrap_or(0);
        let units = if side == OrderSide::Sell { net_of_reserve(units, reserved) } else { units };
        (units, coins)
    }

    /// Replaces what `account`'s open quests keep (base object id ->
    /// units); empty clears it.
    pub fn set_quest_reserved(&self, account: &str, reserved: BTreeMap<i64, i64>) {
        let mut engine = self.lock_engine();
        if reserved.is_empty() {
            engine.quest_reserved.remove(account);
        } else {
            engine.quest_reserved.insert(account.to_string(), reserved);
        }
    }
}

/// Free units a sale may claim once `reserved` units stay with the bot.
pub fn net_of_reserve(units: i64, reserved: i64) -> i64 {
    (units - reserved.max(0)).max(0)
}

/// Serializes "compute the account's locks, then send them": the market
/// round and the brain's release push never interleave, so a stale
/// snapshot never lands after the fresh one.
static LOCK_PUSH_GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// One `set_locks` request out of an account's locks.
fn set_locks_request(coins: i64, items: &BTreeMap<i64, i64>) -> Value {
    let rows: Vec<Value> = items.iter().map(|(id, qty)| Value::Arr(vec![Value::Int(*id), Value::Int(*qty)])).collect();
    lifescape_json::object(vec![("cmd", lifescape_json::str_value("set_locks")), ("coins", Value::Int(coins)), ("items", Value::Arr(rows))])
}

/// Pushes the locks of each account through `send`. The locks of an
/// account are read right before its own send, under the gate: the old
/// all-accounts snapshot taken up front reached the last of 60 slow
/// workers seconds later, after a release had pushed the fresh locks
/// (live 2026-11-22, kate_ellis `locked:iron_ore` seven seconds after
/// its listing was withdrawn, no order left in the store). A send that
/// fails comes back typed (account, error), never ignored.
pub fn push_locks_with(service: &MarketService, accounts: &[String], send: &mut dyn FnMut(&str, &Value) -> Result<(), String>) -> Vec<(String, String)> {
    let mut failures = Vec::new();
    for account in accounts {
        let _gate = LOCK_PUSH_GATE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some((coins, items)) = service.locks_for(std::slice::from_ref(account)).remove(account) else { continue };
        if let Err(error) = send(account, &set_locks_request(coins, &items)) {
            failures.push((account.clone(), error));
        }
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::{uncovered, COINS_ID};

    /// Lot 5: `base_of` is the planner's. The former body, kept here as
    /// the oracle, agrees on every object id of the dump (banknotes
    /// included) and on ids out of range or negative.
    #[test]
    fn base_of_is_the_planners_on_every_object_id() {
        fn former(id: i64) -> i64 {
            let Ok(index) = usize::try_from(id) else { return id };
            let name = lifescape_gamedata::obj_by_id(index).and_then(|entry| entry.name).unwrap_or_default();
            match name.strip_prefix("cert_") {
                Some(base) => lifescape_gamedata::objs_by_name(base).into_iter().next().map(|entry| entry.obj_id as i64).unwrap_or(id),
                None => id,
            }
        }
        let mut notes = 0;
        let top = lifescape_gamedata::all_objs().iter().map(|obj| obj.obj_id as i64).max().unwrap_or(0) + 50;
        for id in (-5..=top).chain([i64::MAX, i64::MIN]) {
            assert_eq!(super::base_of(id), former(id), "id {id}");
            assert_eq!(super::base_of(id), lifescape_planner::base_of(id), "id {id}");
            notes += usize::from(former(id) != id);
        }
        assert!(notes > 0, "no banknote met: the oracle proves nothing");
    }

    /// A banknote is its object (user 2026-11-19): the ledger keys noted
    /// stock by the base id, so an in-flight sale (deals sit on the base
    /// id) nets against it instead of a phantom negative row.
    #[test]
    fn the_ledger_keys_banknotes_by_their_object() {
        let live = lifescape_json::parse(r#"{"inv":[{"id":1516,"count":28},{"id":995,"count":40}],"bank":[{"id":1515,"count":100}],"worn":[]}"#).expect("json");
        let (coins, stock) = super::ledger_holdings(&live, COINS_ID);
        assert_eq!((coins, stock), (40, vec![(1515, 128)]));
    }

    /// Live 2026-11-19 (thorn_vera, redeploy at 08:40:04): a fresh worker
    /// had not seen its bank yet, the mirror set the ledger to 0 coins
    /// and 0 items, its bids ("buyer holds 0 coins, needs 6516" with
    /// 242 681 coins) and six surplus listings ("seller holds 0, needs
    /// 392") were refused until its first bank open reached the daemon.
    /// Only a fresh snapshot whose pack and bank both streamed witnesses
    /// the holdings; anything else keeps the last known ledger.
    #[test]
    fn a_snapshot_without_its_bank_never_funds_the_ledger() {
        use lifescape_json::{object, Value};
        let snapshot = |inv: bool, bank: bool, updated: f64| object(vec![("inv_streamed", Value::Bool(inv)), ("bank_streamed", Value::Bool(bank)), ("updated", Value::Float(updated))]);
        let now = 1000.0;
        assert!(super::witnesses_holdings(&snapshot(true, true, 990.0), now));
        assert!(!super::witnesses_holdings(&snapshot(true, false, 990.0), now), "bank not seen since the session opened");
        assert!(!super::witnesses_holdings(&snapshot(false, true, 990.0), now), "pack not streamed yet");
        assert!(!super::witnesses_holdings(&snapshot(true, true, 960.0), now), "stale (30 s)");
        assert!(!super::witnesses_holdings(&object(vec![("updated", Value::Float(990.0))]), now), "no stream flags: no witness");
    }
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
            ..Order::default()
        }
    }

    /// Plan marché C4: a bot that spent coins at a shop keeps its bids;
    /// the uncovered one (newest lineage) is lowered to what its purse
    /// still covers, a bid it cannot keep at 1 coin goes. Lineages rank
    /// by t0: a walked (amended) bid keeps its claim over a newer one.
    #[test]
    fn uncovered_bids_are_lowered_not_cancelled() {
        use super::{audit_plan, AuditPlan};
        let mut walked = order(9, OrderSide::Buy, 317, 10, 2, 50);
        walked.root_clock = 5;
        let newer = order(8, OrderSide::Buy, 1511, 5, 3, 20);
        let orders = vec![newer.clone(), walked];
        // 10 x 2 (t0 5) first, then 5 x 3 = 15 with 8 coins left: 1 each.
        assert_eq!(audit_plan(&orders, 28, &BTreeMap::new(), 0, &BTreeMap::new()), AuditPlan { cancel: vec![], lower: vec![(8, 1)] });
        assert_eq!(audit_plan(&orders, 24, &BTreeMap::new(), 0, &BTreeMap::new()), AuditPlan { cancel: vec![8], lower: vec![] }, "4 coins for 5 units: gone");
        assert_eq!(audit_plan(&[newer], 30, &BTreeMap::new(), 0, &BTreeMap::new()), AuditPlan::default());
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

    /// Deals 1716/1722 (bct5rxpg) and 1721 (fiona_park): the stock of a
    /// resting sell is claimed, a second listing only gets what is left;
    /// bids claim their coins the same way (bd23dqpq, 1643-1652).
    #[test]
    fn resting_orders_claim_their_share() {
        use super::uncommitted;
        let resting = vec![order(1, OrderSide::Sell, 882, 7, 2, 10), order(2, OrderSide::Buy, 558, 70, 1, 11), order(3, OrderSide::Buy, 1511, 5, 3, 12)];
        let stock = [(882i64, 12i64), (558, 3)];
        assert_eq!(uncommitted(OrderSide::Sell, 882, 100, &stock, &resting).0, 5, "12 arrows, 7 listed: 5 free");
        assert_eq!(uncommitted(OrderSide::Sell, 558, 100, &stock, &resting).0, 3, "bids never claim stock");
        assert_eq!(uncommitted(OrderSide::Buy, 558, 100, &stock, &resting).1, 15, "100 coins, 70 + 15 bid: 15 free");
        assert_eq!(uncommitted(OrderSide::Buy, 558, 50, &stock, &resting).1, 0, "over-committed purse is empty, never negative");
    }

    /// The ledger mirror is net of in-flight deals: the 25 arrows fiona
    /// sold (deal 1698, still in her bank until the handover) and the
    /// coins a buyer promised are not free.
    #[test]
    fn ledger_mirror_is_net_of_in_flight_deals() {
        use super::net_of_deals;
        let deal = |buyer: &str, seller: &str, item: i64, quantity: i64, price: i64| lifescape_store::Deal {
            deal_id: 1,
            buyer: buyer.to_string(),
            seller: seller.to_string(),
            item_id: item,
            quantity,
            price,
            meeting_x: 0,
            meeting_y: 0,
            meeting_level: 0,
            status: lifescape_store::DealStatus::Matched,
            fail_reason: String::new(),
            deadline_tick: 0,
            created_clock: 0,
            updated_clock: 0,
            taker_side: None,
        };
        let deals = vec![deal("b5xntyz6", "fiona_park", 882, 25, 1), deal("fiona_park", "bdr53jee", 558, 15, 2)];
        let (coins, stock) = net_of_deals("fiona_park", 100, &[(882, 127), (556, 1)], &deals);
        assert_eq!(coins, 70, "15 x 2 promised to bdr53jee");
        assert_eq!(stock, vec![(556, 1), (882, 102)], "25 arrows promised to b5xntyz6");
    }
}
