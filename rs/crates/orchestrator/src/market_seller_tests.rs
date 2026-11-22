use super::*;
use crate::brain_reserve::CostSource;
use lifescape_market::Reference;
use lifescape_planner::market_fit::BookView;

fn reserve(coins: i64) -> Option<Reserve> {
    Some(Reserve { coins, source: CostSource::Measured })
}

const UNTRADED: Reference = Reference { price: 1, traded: false };

fn demand(volume_24h: i64, bids: Vec<(i64, i64)>) -> BookView {
    BookView { bids, asks: Vec::new(), volume_24h, ..BookView::default() }
}

/// Willow 2026-11-14: 33 364 open at 1 coin against 4 319 traded a day.
/// A seller with 30 000 logs lists what buyers take (bids + a day of
/// volume, `market_fit::sale_lot`) over its own cost (plan prix §2: one
/// coin above at the least, a course to concede), never the whole pile
/// at the 1-coin reference.
#[test]
fn a_sale_lists_a_demand_lot_at_the_sellers_own_cost() {
    let traded_at_one = Reference { price: 1, traded: true };
    assert_eq!(plan_sale(30_000, reserve(4), traded_at_one, None, None, FoundMarks::default(), &demand(4319, vec![(2, 81)])), SaleDecision::List { qty: 4400, price: 5, floor: 4 });
    assert_eq!(plan_sale(30_000, reserve(4), UNTRADED, None, None, FoundMarks::default(), &demand(0, Vec::new())), SaleDecision::List { qty: 28, price: 8, floor: 4 }, "untraded: 2 x reserve, one inventory");
    assert!(matches!(plan_sale(30_000, None, UNTRADED, None, None, FoundMarks::default(), &demand(0, Vec::new())), SaleDecision::Hold(_)), "no own cost: hold, never 1 coin");
    assert!(matches!(plan_sale(0, reserve(4), UNTRADED, None, None, FoundMarks::default(), &demand(99, Vec::new())), SaleDecision::Hold(_)));
}

/// No band (user rule 2026-11-15): a reserve over 5x a traded reference
/// still lists at its own floor; a fast fill opens above it.
#[test]
fn no_band_caps_the_opening_or_holds_over_the_reserve() {
    let traded = Reference { price: 2, traded: true };
    assert!(matches!(plan_sale(50, reserve(11), traded, None, None, FoundMarks::default(), &demand(100, Vec::new())), SaleDecision::List { floor: 11, .. }), "11 > 5 x 2 lists anyway");
    let fast = OwnFill { price: 12, rested_ticks: Some(10) };
    assert!(matches!(plan_sale(50, reserve(4), traded, None, Some(fast), FoundMarks::default(), &demand(30, Vec::new())), SaleDecision::List { qty: 30, floor: 4, price } if price > 10), "no 5x cap");
}

#[test]
fn stale_listings_are_raised_walked_or_withdrawn() {
    assert_eq!(adoption(1, 500, Some(4), 1079, None), Adoption::Raise { price: 4, floor: 4 }, "ryan_park's 1-coin asks come up to the reserve");
    assert_eq!(adoption(9, 500, Some(4), 1079, None), Adoption::Walk { floor: 4 });
    assert!(matches!(adoption(1, 33_364, Some(4), 1079, None), Adoption::Withdraw(_)), "a dump over the lot goes back to the bank");
    assert!(matches!(adoption(5, 10, None, 1079, None), Adoption::Withdraw(_)), "no own cost: held");
}

fn found() -> Option<Reserve> {
    Some(Reserve { coins: 1, source: CostSource::Found })
}

fn asks(asks: Vec<(i64, i64)>) -> BookView {
    BookView { asks, ..BookView::default() }
}

/// Live frank_lane 2026-11-14: 838 bronze arrows held (reserve 12 over
/// the 5x band of 2), 41 iron daggers (nobody traded them, asks at 18-28),
/// fur with an empty book. Found stock lists a whole demand lot (at
/// least an inventory, never one unit of a big stack) at the clearing
/// price, floor 1, never above the 5x band.
#[test]
fn found_loot_lists_a_whole_lot_at_a_clearing_price() {
    let arrows = Reference { price: 2, traded: true };
    let view = BookView { asks: vec![(1, 7), (2, 14), (2, 451)], volume_24h: 332, ..BookView::default() };
    let marks = |others_ask: Option<i64>, anchor: Option<i64>| FoundMarks { others_ask, anchor, ..FoundMarks::default() };
    assert_eq!(plan_sale(838, found(), arrows, None, None, marks(Some(1), Some(1)), &view), SaleDecision::List { qty: 332, price: 2, floor: 1 });
    let daggers = Reference { price: 18, traded: false };
    assert_eq!(plan_sale(41, found(), daggers, None, None, marks(Some(18), Some(21)), &asks(vec![(18, 2), (22, 4), (28, 9)])), SaleDecision::List { qty: 28, price: 18, floor: 1 }, "an inventory, AT the cheapest ask (wave 2: joins, never undercuts)");
    // Fur: only its own old ask rests (131): it never anchors the lot.
    assert_eq!(plan_sale(6, found(), Reference { price: 131, traded: false }, None, None, marks(None, Some(12)), &asks(vec![(131, 2)])), SaleDecision::List { qty: 6, price: 12, floor: 1 }, "the whole small stack at its alch value");
    // No band: a fast fill far above the reference is the opening's guide.
    let outlier = Reference { price: 30, traded: true };
    let fast = OwnFill { price: 600, rested_ticks: Some(5) };
    assert!(matches!(plan_sale(10, found(), outlier, None, Some(fast), FoundMarks::default(), &asks(Vec::new())), SaleDecision::List { qty: 10, floor: 1, price } if price > 150));
}

/// Iron dagger 1 @628 resting while 40 more sat in the bank: a listing
/// that lists a sliver of the free stack is regrouped (withdrawn, the next
/// pass lists a whole lot); partly sold or demand-sized lots stay.
#[test]
fn a_sliver_of_a_big_stack_is_regrouped() {
    assert!(undersized(1, 1, 40, 28), "1 of 41");
    assert!(undersized(2, 2, 5, 7), "staff: 2 of 7");
    assert!(!undersized(26, 26, 4_677, 28), "cow hides: a demand lot already");
    assert!(!undersized(2, 2, 0, 2), "nothing more held");
    assert!(!undersized(28, 10, 30, 40), "partly sold: walks on");
}

/// Re-adoption after a restart: an ask above today's opening comes down
/// to it (frank_lane's staff at 1644 against asks at 17), never under the
/// reserve.
#[test]
fn stale_asks_above_the_opening_come_down() {
    assert_eq!(adoption(1644, 2, Some(1), 28, Some(16)), Adoption::Lower { price: 16, floor: 1 });
    assert_eq!(adoption(12, 2, Some(1), 28, Some(16)), Adoption::Walk { floor: 1 }, "under the opening: walks");
    assert_eq!(adoption(1, 500, Some(4), 1079, Some(2)), Adoption::Raise { price: 4, floor: 4 }, "the reserve still wins");
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

/// Plan marché M7: a never-traded object lists on its public anchors.
/// A smith whose own bars cost 20 opens over the published production
/// cost (30 x (1 + margin)), its floor its own cost (no shop or alchemy
/// floor, user 2026-11-18); loot of a costed object anchors on that cost,
/// never over the shop's selling price; without anchors, the wave-2 rules.
#[test]
fn never_traded_objects_list_on_their_public_anchors() {
    let anchors = lifescape_market::Anchors { shop_sale: Some(24), alch_net: None, make_cost: Some(30), shop_buy: None };
    let marks = FoundMarks { discovery: anchors, margin: 0.10, ..FoundMarks::default() };
    let view = demand(0, Vec::new());
    assert_eq!(plan_sale(28, reserve(20), UNTRADED, None, None, marks, &view), SaleDecision::List { qty: 28, price: 33, floor: 20 });
    assert_eq!(plan_sale(28, reserve(20), UNTRADED, None, None, FoundMarks::default(), &view), SaleDecision::List { qty: 28, price: 40, floor: 20 }, "no anchor: twice the own cost");
    // A caster's own cast nets 35: no floor at it (user 2026-11-18), the
    // own cost stays the floor.
    let caster = FoundMarks { discovery: lifescape_market::Anchors { alch_net: Some(35), ..anchors }, ..marks };
    assert_eq!(plan_sale(28, reserve(20), UNTRADED, None, None, caster, &view), SaleDecision::List { qty: 28, price: 33, floor: 20 });
    // Traded: the current price, floor at the own cost.
    let traded = Reference { price: 50, traded: true };
    assert!(matches!(plan_sale(28, reserve(20), traded, None, None, marks, &view), SaleDecision::List { price: 50, floor: 20, .. }));
    // Loot (zero own cost) of a costed object, a shop selling it at 31.
    let loot = FoundMarks { discovery: lifescape_market::Anchors { shop_buy: Some(31), ..anchors }, anchor: Some(12), ..marks };
    assert_eq!(plan_sale(5, found(), UNTRADED, None, None, loot, &asks(Vec::new())), SaleDecision::List { qty: 5, price: 31, floor: 1 }, "loot walks down to 1");
}

/// No price floor (user 2026-11-18: "retire les floor de prix"): an
/// overstocked lot or one an NPC shop would buy keeps the seller's own
/// cost as its floor, never the shop's offer nor the alchemy outlet.
#[test]
fn neither_the_shop_nor_the_alchemy_is_a_floor() {
    let traded = Reference { price: 858, traded: true };
    match plan_sale(564, reserve(500), traded, Some(823), None, FoundMarks::default(), &demand(1, vec![(823, 1)])) {
        SaleDecision::List { floor, .. } => assert_eq!(floor, 500, "the own cost, under the shop's 858"),
        other => panic!("{other:?}"),
    }
}

/// User 2026-11-22 (the mirror of B1): the ask walk stops at the seller's
/// OWN value of the unit (its cost: measured ticks x its coins per tick,
/// plus inputs). Unsold horizons no longer lower that limit (the old
/// plan prix §1 decay, 10 % per horizon, is gone): an iron ore that costs
/// its miner 15 coins of time stops at 15, never at 1. The listing opens
/// over the limit by its margin, never on it.
#[test]
fn an_unsold_seller_keeps_its_own_value_as_the_walk_limit() {
    let traded = Reference { price: 700, traded: true };
    for unsold in [0, 2, 20] {
        let marks = FoundMarks { margin: 0.10, unsold, ..FoundMarks::default() };
        assert_eq!(plan_sale(564, reserve(858), traded, None, None, marks, &demand(1, Vec::new())), SaleDecision::List { qty: 28, price: 944, floor: 858 }, "{unsold} unsold horizons");
    }
    let ore = plan_sale(564, reserve(15), Reference { price: 1, traded: true }, None, None, FoundMarks { unsold: 9, ..FoundMarks::default() }, &demand(1, Vec::new()));
    assert!(matches!(ore, SaleDecision::List { floor: 15, .. }), "{ore:?}");
    // A byproduct (found stock, own value ~0): the limit is one coin.
    let found = Some(Reserve { coins: 1, source: CostSource::Found });
    let feathers = plan_sale(500, found, Reference { price: 3, traded: true }, None, None, FoundMarks { unsold: 9, ..FoundMarks::default() }, &demand(100, Vec::new()));
    assert!(matches!(feathers, SaleDecision::List { floor: 1, .. }), "{feathers:?}");
    let fresh = FoundMarks { margin: 0.10, ..FoundMarks::default() };
    assert_eq!(plan_sale(564, reserve(858), traded, None, None, fresh, &demand(1, Vec::new())), SaleDecision::List { qty: 28, price: 944, floor: 858 }, "no failure yet: its cost, opened over it");
    let rich = Reference { price: 1_200, traded: true };
    assert_eq!(plan_sale(564, reserve(858), rich, None, None, fresh, &demand(1, Vec::new())), SaleDecision::List { qty: 28, price: 1_200, floor: 858 }, "a richer market keeps its price");
}
