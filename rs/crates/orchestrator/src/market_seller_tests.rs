use super::*;
use crate::brain_reserve::{CostSource, OwnLine, OwnReport};
use lifescape_market::Reference;
use lifescape_planner::market_fit::BookView;

fn reserve(coins: i64) -> Option<Reserve> {
    Some(Reserve { coins, source: CostSource::Measured })
}

const UNTRADED: Reference = Reference { price: 1, traded: false };

fn demand(volume_24h: i64, bids: Vec<(i64, i64)>) -> BookView {
    BookView { bids, asks: Vec::new(), volume_24h }
}

/// Willow 2026-11-14: 33 364 open at 1 coin against 4 319 traded a day.
/// A seller with 30 000 logs lists what buyers take (bids + a day of
/// volume, `market_fit::sale_lot`) at its own cost, never the whole pile
/// at the 1-coin reference.
#[test]
fn a_sale_lists_a_demand_lot_at_the_sellers_own_cost() {
    let traded_at_one = Reference { price: 1, traded: true };
    assert_eq!(plan_sale(30_000, reserve(4), traded_at_one, None, None, &demand(4319, vec![(2, 81)])), SaleDecision::List { qty: 4400, price: 4, floor: 4 });
    assert_eq!(plan_sale(30_000, reserve(4), UNTRADED, None, None, &demand(0, Vec::new())), SaleDecision::List { qty: 28, price: 8, floor: 4 }, "untraded: 2 x reserve, one inventory");
    assert!(matches!(plan_sale(30_000, None, UNTRADED, None, None, &demand(0, Vec::new())), SaleDecision::Hold(_)), "no own cost: hold, never 1 coin");
    assert!(matches!(plan_sale(0, reserve(4), UNTRADED, None, None, &demand(99, Vec::new())), SaleDecision::Hold(_)));
}

/// The anti-bug band stays (reported, unchanged): a reserve over 5x a
/// traded reference holds; an opening over it comes down to it.
#[test]
fn the_band_caps_the_opening_and_holds_over_the_reserve() {
    let traded = Reference { price: 2, traded: true };
    assert!(matches!(plan_sale(50, reserve(11), traded, None, None, &demand(100, Vec::new())), SaleDecision::Hold(_)), "11 > 5 x 2");
    let fast = OwnFill { price: 12, rested_ticks: Some(10) };
    assert_eq!(plan_sale(50, reserve(4), traded, None, Some(fast), &demand(30, Vec::new())), SaleDecision::List { qty: 30, price: 10, floor: 4 }, "13 capped at 10");
}

#[test]
fn stale_listings_are_raised_walked_or_withdrawn() {
    assert_eq!(adoption(1, 500, Some(4), 1079), Adoption::Raise { price: 4, floor: 4 }, "ryan_park's 1-coin asks come up to the reserve");
    assert_eq!(adoption(9, 500, Some(4), 1079), Adoption::Walk { floor: 4 });
    assert!(matches!(adoption(1, 33_364, Some(4), 1079), Adoption::Withdraw(_)), "a dump over the lot goes back to the bank");
    assert!(matches!(adoption(5, 10, None, 1079), Adoption::Withdraw(_)), "no own cost: held");
}

#[test]
fn flip_margins_pay_the_merchants_time() {
    let reports = vec![
        OwnReport { method: "market".to_string(), ticks: 6000, lines: vec![OwnLine { item: "Feather".to_string(), provenance: "traded".to_string(), quantity: -300 }, OwnLine { item: "Coins".to_string(), provenance: "traded".to_string(), quantity: 600 }] },
        OwnReport { method: "melee".to_string(), ticks: 1500, lines: vec![] },
    ];
    assert_eq!(flip_ticks_per_unit(&reports), Some(20.0));
    assert_eq!(flip_margin(Some(20.0), 0.1), 2);
    assert_eq!(flip_margin(None, 0.1), 6, "unmeasured: 6000 ticks over 100 units");
    assert_eq!(flip_margin(Some(0.5), 0.01), 1, "at least one coin");
}

/// Live service: the seller's own fill is remembered with its speed, and
/// the next quote opens over a fast fill.
#[test]
fn own_fills_are_remembered_with_their_speed() {
    let path = std::env::temp_dir().join(format!("lifescape-seller-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let catalogue = vec![
        crate::market_seed::CatalogueItem { id: 314, name: "Feather".to_string(), shop_price: 2, alch_value: 1 },
        crate::market_seed::CatalogueItem { id: 995, name: "Coins".to_string(), shop_price: 1, alch_value: 0 },
    ];
    let service = MarketService::open(&path, catalogue).expect("opens");
    for account in ["seller_a", "buyer_b"] {
        let reply = service.handle(&object(vec![
            ("op", str_value("fund")),
            ("account", str_value(account)),
            ("coins", Value::Int(100_000)),
            ("item", Value::Int(314)),
            ("qty", Value::Int(1_000)),
        ]));
        assert_eq!(reply.get("ok").as_bool(), Some(true));
    }
    let place = |account: &str, side: &str, price: i64| {
        service.handle(&object(vec![
            ("op", str_value("place")),
            ("account", str_value(account)),
            ("side", str_value(side)),
            ("item", Value::Int(314)),
            ("qty", Value::Int(10)),
            ("price", Value::Int(price)),
            ("floor", Value::Int(1)),
        ]))
    };
    assert_eq!(place("seller_a", "sell", 7).get("ok").as_bool(), Some(true));
    assert_eq!(place("buyer_b", "buy", 7).get("ok").as_bool(), Some(true));
    let fill = service.own_sell_fill("seller_a", 314).expect("remembered");
    assert_eq!(fill.price, 7);
    assert!(fill.fast(), "sold within its first window: {fill:?}");
    assert_eq!(service.own_sell_fill("buyer_b", 314), None, "a purchase is no sale");
    assert_eq!(service.own_buy_price("buyer_b", 314), Some(7));
    let _ = std::fs::remove_file(&path);
}
