//! Merchant arbitrage (plan équilibre step 5, D2): yew logs at 1 coin on
//! the market, a general store paying 16 at its floor.

use super::*;
use lifescape_gamedata::shop_sale::{likely_held, npc_sale_shops, unit_value_floor};

/// Ticks of the janitor's shop trip (`orchestrator::brain_npc_sale::
/// SHOP_TRIP_TICKS`).
const SHOP_TRIP: f64 = 1500.0;
/// The fleet's measured time value on the 2026-11-19 copy (D1).
const FLEET_RATE: f64 = 0.28;

/// The best open shop that buys yew logs and the units it likely holds
/// after the fleet's `sold` units landed just now.
fn yew_shop(sold: i64) -> (String, i64) {
    let shop = npc_sale_shops("yew_logs").into_iter().next().expect("a general store buys yew logs").shop;
    let sales = if sold > 0 { vec![(sold, 0)] } else { Vec::new() };
    let held = likely_held(&shop, "yew_logs", &sales);
    (shop, held)
}

/// The shop's price for the unit of rank `k` of the lot: the server's
/// stock-based drop from the units it holds.
fn ladder(shop: &str, held: i64) -> impl Fn(i64) -> f64 + '_ {
    move |rank: i64| unit_value_floor(shop, "yew_logs", held + rank).unwrap_or(0) as f64
}

fn shop_outlet<'a>(shop: &str, value: &'a dyn Fn(i64) -> f64) -> Outlet<'a> {
    Outlet { kind: OutletKind::Shop(shop.to_string()), unit_value: value, unit_ticks: SHOP_SELL_TICKS_PER_UNIT, trip_ticks: SHOP_TRIP }
}

/// The constat: the shop already holds what the fleet sold (its floor,
/// 16 a log, without limit); 100 logs asked at 1 are bought and sold
/// there: 15 coins a log, the trip paid at the bot's rate.
#[test]
fn yew_logs_at_one_coin_go_to_the_merchant() {
    let (shop, held) = yew_shop(40);
    let value = ladder(&shop, held);
    assert_eq!(value(0), 16.0, "the floor: 10 % of a yew log's value");
    assert_eq!(value(500), 16.0, "without limit");
    let lot = best_lot("yew_logs", &[(1, 100)], &[shop_outlet(&shop, &value)], FLEET_RATE, 10_000).expect("1 coin under a 16-coin shop: a lot");
    assert_eq!(lot.outlet, OutletKind::Shop(shop.clone()));
    assert_eq!((lot.count, lot.open, lot.spent), (100, 1, 100), "the bid opens at the dearest ask taken");
    // Review lot 5: the ceiling is what the shop pays for the last unit,
    // net of its handling and its share of the trip: an unfilled bid
    // climbs 10 % a pass (the market's walk) up to it, toward the outlet.
    assert_eq!(Some(lot.ceiling), net_outlet(16.0, SHOP_SELL_TICKS_PER_UNIT, SHOP_TRIP, FLEET_RATE, 100), "{lot:?}");
    assert_eq!(lot.ceiling, 11, "16 - 0.07 - 1600 x 0.28 / 100: {lot:?}");
    assert!((lot.revenue - 1600.0).abs() < 1e-9, "{lot:?}");
    let ticks = SHOP_TRIP + HANDOVER_TICKS + 100.0 * SHOP_SELL_TICKS_PER_UNIT;
    assert!((lot.ticks - ticks).abs() < 1e-9, "{lot:?}");
    assert!((lot.profit - (1600.0 - 100.0 - ticks * FLEET_RATE)).abs() < 1e-6, "{lot:?}");
    assert!(lot.rate() > FLEET_RATE, "beats the bot's own rate: {lot:?}");
}

/// Asked at 15, one coin under the shop: the trip at the bot's rate eats
/// the margin, nothing is bought.
#[test]
fn yew_logs_at_fifteen_stay_on_the_market() {
    let (shop, held) = yew_shop(40);
    let value = ladder(&shop, held);
    assert_eq!(best_lot("yew_logs", &[(15, 100)], &[shop_outlet(&shop, &value)], FLEET_RATE, 10_000), None);
    // Asks at 1 then 15: the units at 15 still pay their own handling
    // once the trip is paid by the cheap ones; 17 never does.
    let lot = best_lot("yew_logs", &[(1, 60), (15, 40), (17, 100)], &[shop_outlet(&shop, &value)], FLEET_RATE, 10_000).expect("a lot");
    assert_eq!((lot.count, lot.open, lot.spent), (100, 15, 60 + 40 * 15), "{lot:?}");
    assert_eq!(lot.ceiling, 15, "never under its opening: {lot:?}");
}

/// A shop at its base stock pays more for its first units, less for each
/// one it already holds (`shop_haggle.json` delta): the lot stops at the
/// first unit the shop pays no more than its ask.
#[test]
fn the_lot_stops_where_the_shop_pays_no_more_than_the_ask() {
    let (shop, held) = yew_shop(0);
    let value = ladder(&shop, held);
    assert!(value(0) > value(1) && value(1) > value(2), "the price falls with the stock: {} {} {}", value(0), value(1), value(2));
    let paying = (0..MAX_ARBITRAGE_LOT).take_while(|rank| value(*rank) > 20.0 + SHOP_SELL_TICKS_PER_UNIT * 0.01).count() as i64;
    assert!(paying > 0 && paying < 100, "{paying}");
    let lot = best_lot("yew_logs", &[(20, 100)], &[shop_outlet(&shop, &value)], 0.01, 10_000).expect("the first units pay the trip");
    assert_eq!(lot.count, paying, "{lot:?}");
    let revenue: f64 = (0..paying).map(&value).sum();
    assert!((lot.revenue - revenue).abs() < 1e-9, "{lot:?}");
}

/// The wallet bounds the lot; no coin, no lot.
#[test]
fn the_wallet_bounds_the_lot() {
    let (shop, held) = yew_shop(40);
    let value = ladder(&shop, held);
    let lot = best_lot("yew_logs", &[(1, 100)], &[shop_outlet(&shop, &value)], FLEET_RATE, 70).expect("70 coins buy 70 logs");
    assert_eq!(lot.count, 70);
    assert_eq!(best_lot("yew_logs", &[(1, 100)], &[shop_outlet(&shop, &value)], FLEET_RATE, 0), None);
    assert_eq!(best_lot("yew_logs", &[(1, 5000)], &[shop_outlet(&shop, &value)], FLEET_RATE, 1_000_000).map(|lot| lot.count), Some(MAX_ARBITRAGE_LOT), "one trip at most");
}

/// High alchemy is an outlet too (a caster's own cast, no trip): the
/// outlet that nets the most wins.
#[test]
fn the_best_outlet_wins() {
    let (shop, held) = yew_shop(40);
    let value = ladder(&shop, held);
    let cast = |_: i64| 40.0;
    let alchemy = Outlet { kind: OutletKind::Alchemy, unit_value: &cast, unit_ticks: crate::alchemy::CAST_TICKS, trip_ticks: 0.0 };
    let lot = best_lot("yew_logs", &[(1, 100)], &[shop_outlet(&shop, &value), alchemy], FLEET_RATE, 10_000).expect("a lot");
    assert_eq!(lot.outlet, OutletKind::Alchemy, "{lot:?}");
    let poor = |_: i64| 3.0;
    let weak = Outlet { kind: OutletKind::Alchemy, unit_value: &poor, unit_ticks: crate::alchemy::CAST_TICKS, trip_ticks: 0.0 };
    assert_eq!(best_lot("yew_logs", &[(1, 100)], &[weak, shop_outlet(&shop, &value)], FLEET_RATE, 10_000).map(|lot| lot.outlet), Some(OutletKind::Shop(shop.clone())));
}

/// A maker's net outlet: 500 logs to the shop at 16, the trip shared
/// over them at its rate: 15 coins a log.
#[test]
fn a_whole_lot_nets_the_last_unit_less_its_trip_share() {
    assert_eq!(net_outlet(16.0, SHOP_SELL_TICKS_PER_UNIT, SHOP_TRIP, FLEET_RATE, 500), Some(15));
    assert_eq!(net_outlet(16.0, SHOP_SELL_TICKS_PER_UNIT, SHOP_TRIP, 10.0, 500), None, "a dear trip eats a cheap outlet");
    assert_eq!(net_outlet(1.0, SHOP_SELL_TICKS_PER_UNIT, SHOP_TRIP, FLEET_RATE, 500), None);
}
