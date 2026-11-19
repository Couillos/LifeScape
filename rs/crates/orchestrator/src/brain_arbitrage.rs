#![forbid(unsafe_code)]
//! Merchant arbitrage, brain side (plan équilibre step 5, user decision
//! D2, 2026-11-19): every bot (not Iron; a quoting market maker carries
//! the outlet in its own quotes, `market_maker::quote_pass`) may buy on
//! the market under what a merchant pays it, net of the trip at its real
//! time value (D1, `brain_d17::coin_rate`), and resell to the merchant:
//! the market price climbs toward the outlet through play, no floor rule.
//! The pure rules live in `lifescape_planner::merchant_arbitrage`.
//!
//! One pass every `ARBITRAGE_EVERY_SECS` for an idle bot:
//! 1. the time value first (its refresh takes the store lock: never read
//!    under it);
//! 2. one store read (orders, deals; an error skips the pass: never a
//!    second bid beside one unseen): each lot holds what its bid REALLY
//!    bought (its deals, `brain_npc_sale::bought_since`), never the units
//!    it asked for; one `arbitrage` bid at a time, climbing 10 % a window
//!    toward its ceiling (the market's walk), withdrawn past its motive's
//!    horizon or for a lot of another object; withdrawn with nothing
//!    bought, its lot is dropped (no phantom units at the shop);
//! 3. the public asks of others on every object an open F2P shop buys,
//!    except the objects another motive of the bot already bids on (one
//!    bid per object);
//! 4. each shop valued unit by unit at its likely stock: the fleet's
//!    recent sales, the trips in flight and the OTHER bots' arbitrage
//!    lots (`brain_npc_sale::fleet_sales_for`), the bot's own lot of the
//!    object landing first; high alchemy for a caster (an object whose
//!    best outlet is alchemy is the alchemy pass's, `brain_alch`: the best
//!    shop lot of another object is taken);
//! 5. the most profitable shop lot: a bid opening at its dearest ask (it
//!    crosses every ask it was priced on, each filled at its own price),
//!    its ceiling what the shop pays for the last unit net of the trip;
//!    filed for its shop with the units it bought at once
//!    (`brain_npc_sale::note_arbitrage`).
//!
//! The units then sell through the selling pass's shop trip
//! (`brain_sales`: an arbitrage object goes to the shop whatever its
//! stock, its trip judged on the shop's whole proceeds, the janitor
//! carries it), never under what the market pays plus one.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Mutex, OnceLock};

use lifescape_json::{object, str_value, Value};
use lifescape_planner::merchant_arbitrage::{best_lot, ArbitrageLot, Outlet, OutletKind, SHOP_SELL_TICKS_PER_UNIT};
use lifescape_planner::Life;

use crate::jobs::Job;

/// Wall seconds between two arbitrage passes for one account.
pub const ARBITRAGE_EVERY_SECS: f64 = 600.0;
/// Reason of a withdrawn arbitrage bid.
const STALE_REASON: &str = "arbitrage_resized";

fn last_pass() -> &'static Mutex<HashMap<String, f64>> {
    static TABLE: OnceLock<Mutex<HashMap<String, f64>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn wall_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs_f64()).unwrap_or(0.0)
}

/// Others' resting asks per object (debug name, banknotes folded into
/// their object): `(unit price, units)` cheapest first.
pub type PublicAsks = Vec<(String, Vec<(i64, i64)>)>;

/// Pure: others' resting sells `(object, account, unit price, units)`
/// grouped per object, `account`'s own left out, cheapest first, objects
/// by name.
pub fn asks_by_object(sells: &[(String, String, i64, i64)], account: &str) -> PublicAsks {
    let mut grouped: std::collections::BTreeMap<String, Vec<(i64, i64)>> = std::collections::BTreeMap::new();
    for (item, owner, price, units) in sells {
        if owner.eq_ignore_ascii_case(account) || *units <= 0 || *price < 1 {
            continue;
        }
        grouped.entry(item.clone()).or_default().push((*price, *units));
    }
    grouped
        .into_iter()
        .map(|(item, mut asks)| {
            asks.sort_by_key(|(price, _)| *price);
            (item, asks)
        })
        .collect()
}

/// Pure: the most profitable lot over `books` (module doc): every open
/// shop that buys the object, valued unit by unit at its likely stock
/// (`sales`, `ahead(shop, item)` units of the bot's own landing first),
/// and the bot's alchemy net (`alch`, runes paid) when it casts; at the
/// bot's `coin_rate`, within `wallet`.
pub fn best_arbitrage(books: &PublicAsks, sales: &crate::brain_npc_sale::ShopSales, ahead: &dyn Fn(&str, &str) -> i64, alch: &dyn Fn(&str) -> Option<f64>, coin_rate: f64, wallet: i64) -> Option<ArbitrageLot> {
    let mut best: Option<ArbitrageLot> = None;
    for (item, asks) in books {
        let shops = lifescape_gamedata::shop_sale::npc_sale_shops(item);
        if shops.is_empty() {
            continue;
        }
        // A shop whose next unit pays no more than the cheapest ask is
        // out at once (one game-data read): most objects stop here.
        let cheapest = asks.first().map(|(price, _)| *price).unwrap_or(i64::MAX);
        // The ladder goes as deep as the units on sale (no lot cap,
        // user 2026-11-19); it stops earlier at the shop's floor.
        let on_sale: i64 = asks.iter().map(|(_, units)| (*units).max(0)).sum();
        let ladders: Vec<(String, Vec<i64>)> = shops
            .iter()
            .filter_map(|shop| {
                let before = ahead(&shop.shop, item);
                let first = crate::brain_npc_sale::shop_unit_value(&shop.shop, item, sales, before, 0)?;
                if first <= cheapest {
                    return None;
                }
                Some((shop.shop.clone(), crate::brain_npc_sale::shop_ladder(&shop.shop, item, sales, before, on_sale)?))
            })
            .collect();
        if ladders.is_empty() {
            // No shop pays over the asks: alchemy alone is the alchemy
            // pass's (`brain_alch::market_pass`).
            continue;
        }
        let values: Vec<Box<dyn Fn(i64) -> f64 + '_>> = ladders
            .iter()
            .map(|(_, ladder)| {
                let value = move |rank: i64| crate::brain_npc_sale::ladder_value(ladder, rank) as f64;
                Box::new(value) as Box<dyn Fn(i64) -> f64 + '_>
            })
            .collect();
        let mut outlets: Vec<Outlet> = ladders
            .iter()
            .zip(values.iter())
            .map(|((shop, _), value)| Outlet { kind: OutletKind::Shop(shop.clone()), unit_value: value.as_ref(), unit_ticks: SHOP_SELL_TICKS_PER_UNIT, trip_ticks: crate::brain_npc_sale::SHOP_TRIP_TICKS })
            .collect();
        let cast = alch(item);
        let cast_value = move |_: i64| cast.unwrap_or(0.0);
        if cast.is_some() {
            outlets.push(Outlet { kind: OutletKind::Alchemy, unit_value: &cast_value, unit_ticks: lifescape_planner::alchemy::CAST_TICKS, trip_ticks: 0.0 });
        }
        let Some(lot) = best_lot(item, asks, &outlets, coin_rate, wallet) else { continue };
        // Alchemy pays more: the alchemy pass's object; the best shop lot
        // of another object still goes.
        if lot.outlet == OutletKind::Alchemy {
            continue;
        }
        if best.as_ref().is_none_or(|known| lot.profit > known.profit) {
            best = Some(lot);
        }
    }
    best
}

/// Others' resting sells on the live book as `(object, account, unit
/// price, units)` (banknote books read as their object).
fn live_sells() -> Vec<(String, String, i64, i64)> {
    let Some(service) = crate::market_service::shared() else { return Vec::new() };
    let engine = service.lock_engine();
    let mut out = Vec::new();
    for (item_id, book) in engine.books.iter() {
        let Some(name) = usize::try_from(*item_id).ok().and_then(lifescape_gamedata::obj_by_id).and_then(|obj| obj.name) else { continue };
        let name = lifescape_gamedata::base_obj_name(name).to_string();
        for order in book.orders().into_iter().filter(|order| order.side == lifescape_market::Side::Sell && order.remaining > 0) {
            out.push((name.clone(), order.account.clone(), crate::market_units::to_unit(*item_id, order.price), order.remaining));
        }
    }
    out
}

fn base_id(item: &str) -> Option<i64> {
    lifescape_gamedata::objs_by_name(item).into_iter().next().map(|obj| obj.obj_id as i64)
}

/// One arbitrage pass for an idle, non-Iron bot (module doc). Returns the
/// bids placed (0 or 1).
pub(crate) fn arbitrage_pass(job: &Job, life: &Life, live: &Value) -> usize {
    if life.profile.ironman {
        return 0;
    }
    let now = wall_now();
    {
        let mut table = last_pass().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if table.get(&job.account).is_some_and(|last| now - last < ARBITRAGE_EVERY_SECS) {
            return 0;
        }
        table.insert(job.account.clone(), now);
    }
    // The time value first: its refresh takes the store lock.
    let rate = crate::brain_d17::coin_rate(&job.account);
    let Some(service) = crate::market_service::shared() else { return 0 };
    let account = job.account.as_str();
    let clock = crate::market_service::MarketService::now_clock();
    let own = crate::brain_npc_sale::own_arbitrage(account);
    let since = own.iter().map(|lot| lot.since).min().unwrap_or(clock);
    // A store error skips the pass: never a second bid beside one unseen.
    let read = {
        let store = service.lock_store();
        store.account_orders(account).and_then(|rows| Ok((rows, store.own_deals_since(account, since)?)))
    };
    let (rows, deals) = match read {
        Ok(read) => read,
        Err(error) => {
            crate::logln!("brain {account}: arbitrage pass skipped, store refused its orders or deals: {error}");
            return 0;
        }
    };
    let resting: Vec<&lifescape_store::Order> = rows.iter().filter(|row| crate::brain_gear::cancellable(row) && row.motive == crate::market_lineage::ARBITRAGE).collect();
    // Each lot holds what its bid really bought (its deals), never the
    // units it asked for.
    for lot in &own {
        let still = resting.iter().any(|row| crate::market_audit::book_item(row.item_id) == lot.item_id);
        crate::brain_npc_sale::settle_arbitrage(account, &lot.item, crate::brain_npc_sale::bought_since(&deals, account, lot.item_id, lot.since), still);
    }
    let withdraw = |row: &lifescape_store::Order| {
        let reply = service.handle(&object(vec![("op", str_value("cancel")), ("account", str_value(account)), ("order", Value::Int(row.order_id)), ("reason", str_value(STALE_REASON))]));
        if reply.get("ok").as_bool() != Some(true) {
            crate::logln!("brain {account}: arbitrage bid {} not withdrawn: {:?}", row.order_id, reply.get("error").as_str());
            return;
        }
        let book = crate::market_audit::book_item(row.item_id);
        if let Some(lot) = own.iter().find(|lot| lot.item_id == book) {
            crate::brain_npc_sale::settle_arbitrage(account, &lot.item, crate::brain_npc_sale::bought_since(&deals, account, lot.item_id, lot.since), false);
        }
    };
    // One arbitrage bid at a time, until its motive's horizon: it climbs
    // 10 % a window toward its ceiling meanwhile (the market's walk).
    let horizon_secs = i64::try_from(crate::market_lineage::horizon_ticks(crate::market_lineage::ARBITRAGE, false).saturating_mul(crate::market_eta::live_tick_ms()) / 1000).unwrap_or(i64::MAX);
    let mut walking: Option<&lifescape_store::Order> = None;
    for row in &resting {
        if walking.is_some() || clock - row.lineage_clock() > horizon_secs {
            withdraw(row);
        } else {
            walking = Some(row);
        }
    }
    // One bid per object: another motive's bid owns its object.
    let foreign: BTreeSet<i64> = rows.iter().filter(|row| crate::brain_gear::cancellable(row) && row.motive != crate::market_lineage::ARBITRAGE).map(|row| crate::market_audit::book_item(row.item_id)).collect();
    let books: PublicAsks = asks_by_object(&live_sells(), account)
        .into_iter()
        .filter(|(item, _)| base_id(item).is_some_and(|id| !foreign.contains(&id)))
        .filter(|(item, _)| !lifescape_gamedata::shop_sale::npc_sale_shops(item).is_empty())
        .collect();
    if books.is_empty() {
        return 0;
    }
    let sales = crate::brain_npc_sale::fleet_sales_for(&service, account);
    // The units of the bot's own lot of an object land first.
    let carried: HashMap<String, i64> = crate::brain_npc_sale::own_arbitrage(account).into_iter().map(|lot| (lot.item.clone(), lot.units())).collect();
    let ahead = |_: &str, item: &str| carried.get(item).copied().unwrap_or(0);
    let mage = crate::brain_alch::caster_for(live);
    let alch = |item: &str| mage.as_ref().and_then(|caster| caster.net(item));
    let walking_book = walking.map(|row| crate::market_audit::book_item(row.item_id));
    let wallet = crate::brain_gear::free_coins(job, walking_book);
    let Some(lot) = best_arbitrage(&books, &sales, &ahead, &alch, rate, wallet) else { return 0 };
    let OutletKind::Shop(shop) = &lot.outlet else { return 0 };
    let Some(id) = base_id(&lot.item) else { return 0 };
    match walking {
        // The resting bid is this lot's: it keeps climbing.
        Some(row) if crate::market_audit::book_item(row.item_id) == id => return 0,
        Some(row) => withdraw(row),
        None => {}
    }
    let quote_lot = crate::market_units::lot_of(id);
    let qty = lifescape_market::lots::whole_lots_down(lot.count, quote_lot);
    if qty <= 0 {
        return 0;
    }
    // Opens at the dearest ask of the lot (it crosses each ask at its own
    // price), climbs 10 % a window up to what the shop pays for the last
    // unit net of the trip (the market's walk): the price climbs toward
    // the outlet through play.
    let reply = service.handle(&object(vec![
        ("op", str_value("place")),
        ("account", str_value(account)),
        ("side", str_value("buy")),
        ("item", Value::Int(id)),
        ("qty", Value::Int(qty)),
        ("price", Value::Int(lot.open)),
        ("ceiling", Value::Int(lot.ceiling)),
        ("motive", str_value(crate::market_lineage::ARBITRAGE)),
    ]));
    if reply.get("ok").as_bool() != Some(true) {
        crate::logln!("brain {account}: arbitrage bid {qty} {} @ {} refused: {:?}", lot.item, lot.open, reply.get("error").as_str());
        return 0;
    }
    crate::brain_npc_sale::note_arbitrage(crate::brain_npc_sale::ArbitrageInFlight {
        account: account.to_string(),
        shop: shop.clone(),
        item: lot.item.clone(),
        item_id: id,
        carried: ahead(shop, &lot.item),
        bought: 0,
        since: clock,
        resting: true,
    });
    // The asks it crossed are bought now: the lot counts them at once.
    match service.lock_store().own_deals_since(account, clock) {
        Ok(fresh) => crate::brain_npc_sale::settle_arbitrage(account, &lot.item, crate::brain_npc_sale::bought_since(&fresh, account, id, clock), true),
        Err(error) => crate::logln!("brain {account}: arbitrage deals of {} unread ({error}): counted at the next pass", lot.item),
    }
    crate::logln!(
        "brain {account}: arbitrage: bids {qty} {} @ {} up to {} (asks {} coins, {shop} pays {:.0}; lot {:.0} ticks, {:.2} coins/tick vs own {rate:.2}, net {:.0})",
        lot.item,
        lot.open,
        lot.ceiling,
        lot.spent,
        lot.revenue,
        lot.ticks,
        lot.rate(),
        lot.profit
    );
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plan équilibre D2 (the constat): yew logs asked at 1 coin by
    /// woodcutters, a general store paying 16 at its floor (the fleet's
    /// sales there), a bot at the fleet's measured rate: it buys the lot
    /// for that shop; asked at 15, nothing (the trip eats the margin);
    /// its own asks are never bought back.
    #[test]
    fn yew_logs_at_one_coin_are_bought_for_the_merchant() {
        // Every shop that buys yew logs holds what the fleet just sold it:
        // each pays its floor, 16 (a shop at its base pays more for its
        // first units and would win).
        let shops = lifescape_gamedata::shop_sale::npc_sale_shops("yew_logs");
        let shop = shops.first().expect("a shop buys yew logs").shop.clone();
        let mut sales = crate::brain_npc_sale::ShopSales::new();
        for each in &shops {
            sales.insert((each.shop.clone(), "yew_logs".to_string()), vec![(40, 0)]);
        }
        let none = |_: &str, _: &str| 0;
        let no_cast = |_: &str| None;
        let sells = vec![("yew_logs".to_string(), "carl_miller".to_string(), 1, 200), ("yew_logs".to_string(), "buyer".to_string(), 1, 5000), ("feather".to_string(), "carl_miller".to_string(), 2, 100)];
        let books = asks_by_object(&sells, "buyer");
        assert_eq!(books.iter().find(|(item, _)| item == "yew_logs").map(|(_, asks)| asks.clone()), Some(vec![(1, 200)]), "its own asks left out");
        let lot = best_arbitrage(&books, &sales, &none, &no_cast, 0.28, 10_000).expect("a lot for the merchant");
        assert_eq!((lot.item.as_str(), lot.outlet.clone(), lot.count, lot.open), ("yew_logs", OutletKind::Shop(shop.clone()), 200, 1), "{lot:?}");
        assert!((lot.revenue - 200.0 * 16.0).abs() < 1e-9, "{lot:?}");
        let dear = asks_by_object(&[("yew_logs".to_string(), "carl_miller".to_string(), 15, 200)], "buyer");
        assert_eq!(best_arbitrage(&dear, &sales, &none, &no_cast, 0.28, 10_000), None, "15 coins: the trip eats the margin");
        // A caster whose cast nets more: the alchemy pass's lot, never
        // this pass's.
        let cast = |item: &str| (item == "yew_logs").then_some(40.0);
        assert_eq!(best_arbitrage(&books, &sales, &none, &cast, 0.28, 10_000), None);
    }

    /// Review lot 5: when the best lot of one object goes to alchemy (the
    /// alchemy pass's), the best SHOP lot on another object is still
    /// bought.
    #[test]
    fn an_alchemy_lot_leaves_room_for_a_shop_lot_on_another_object() {
        let mut sales = crate::brain_npc_sale::ShopSales::new();
        for item in ["yew_logs", "willow_logs"] {
            for each in lifescape_gamedata::shop_sale::npc_sale_shops(item) {
                sales.insert((each.shop.clone(), item.to_string()), vec![(40, 0)]);
            }
        }
        let none = |_: &str, _: &str| 0;
        let cast = |item: &str| (item == "yew_logs").then_some(40.0);
        let sells = vec![("yew_logs".to_string(), "carl_miller".to_string(), 1, 1000), ("willow_logs".to_string(), "carl_miller".to_string(), 1, 1000)];
        let books = asks_by_object(&sells, "buyer");
        let lot = best_arbitrage(&books, &sales, &none, &cast, 0.28, 10_000).expect("the willow lot");
        assert_eq!(lot.item, "willow_logs", "{lot:?}");
        assert!(matches!(lot.outlet, OutletKind::Shop(_)), "{lot:?}");
        assert_eq!((lot.open, lot.count), (1, 1000), "{lot:?}");
    }
}
