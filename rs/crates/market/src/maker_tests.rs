//! Market makers (plan marché Q6, wave 3): slices, capital target and
//! stock-dependent quotes.

use super::*;

fn traded(price: i64) -> Reference {
    Reference { price, traded: true }
}

/// A liquid object at 100: others bid and buy, the maker holds one lot.
fn liquid(stock: i64) -> MakerInput {
    MakerInput {
        reference: traded(100),
        best_bid: Some(90),
        best_ask: Some(115),
        bid_depth: 40,
        ask_depth: 40,
        recent_buys: 50,
        anchor: 80,
        outside: Some(30),
        outlet: None,
        make_cost: None,
        stock,
        stock_cost: stock * 100,
        capital: 100_000,
        own_bid: None,
        own_ask: None,
    }
}

#[test]
fn slice_capacity_follows_the_f2p_bank_and_pack() {
    assert_eq!(BANK_SLOTS, 60, "bank.constant ^bank_free_slots");
    assert_eq!(slice_capacity(), 54, "(60 + 28 - 20) * 80 %");
    assert_eq!(maker_count(575), 11);
    assert_eq!(maker_count(54), 1);
    assert_eq!(maker_count(55), 2);
}

/// A synthetic universe shaped like the F2P one: 575 objects, three in
/// four cheap (a lot is 1000 coins of them), one in four worth more than
/// a lot per unit (2 000 to 65 000 coins), heavy ids clustered like the
/// rune and adamant sets.
fn shaped_universe() -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    for index in 0..575_i64 {
        let id = 1 + index * 3;
        let anchor = if (300..440).contains(&index) { 2_000 + (index * 7919 % 63) * 1_000 } else { 1 + index * 37 % 900 };
        out.push((id, anchor));
    }
    out
}

fn needs(slices: &[Vec<i64>], universe: &[(i64, i64)], target: i64) -> Vec<i64> {
    let anchor_of = |id: &i64| universe.iter().find(|(item, _)| item == id).map(|(_, anchor)| *anchor).unwrap_or(1);
    slices.iter().map(|slice| slice_need(&MAKER_CAPS, &slice.iter().map(anchor_of).collect::<Vec<i64>>(), target)).collect()
}

fn assigned(slices: &[Vec<i64>]) -> std::collections::HashMap<i64, usize> {
    slices.iter().enumerate().flat_map(|(index, slice)| slice.iter().map(move |id| (*id, index))).collect()
}

#[test]
fn partition_covers_everything_once_within_the_bank() {
    let universe = shaped_universe();
    let slices = partition(&MAKER_CAPS, &universe, maker_count(universe.len()));
    assert_eq!(slices.len(), 11);
    let mut seen: Vec<i64> = slices.iter().flatten().copied().collect();
    seen.sort_unstable();
    let want: Vec<i64> = universe.iter().map(|(id, _)| *id).collect();
    assert_eq!(seen, want, "every object in exactly one slice");
    assert!(slices.iter().all(|slice| slice.len() <= slice_capacity()), "{:?}", slices.iter().map(Vec::len).collect::<Vec<_>>());
    // Deterministic, whatever the input order (duplicates folded).
    let mut shuffled = universe.clone();
    shuffled.reverse();
    shuffled.push(universe[4]);
    assert_eq!(partition(&MAKER_CAPS, &shuffled, 11), slices);
}

/// The slices ask the same capital: every slice's need at the common
/// target is within 10 % of that target of every other one, and the
/// common target covers the largest.
#[test]
fn partition_balances_the_capital_and_one_target_covers_every_slice() {
    let universe = shaped_universe();
    let slices = partition(&MAKER_CAPS, &universe, 11);
    let anchors: Vec<Vec<i64>> = slices.iter().map(|slice| slice.iter().map(|id| universe.iter().find(|(item, _)| item == id).map_or(1, |(_, anchor)| *anchor)).collect()).collect();
    let all: Vec<i64> = universe.iter().map(|(_, anchor)| *anchor).collect();
    let balanced = balanced_capital(&MAKER_CAPS, &all, 11);
    let target = common_capital_target(&MAKER_CAPS, &anchors, balanced);
    let per_slice = needs(&slices, &universe, target);
    let (low, high) = (*per_slice.iter().min().expect("slices"), *per_slice.iter().max().expect("slices"));
    assert!(high <= target, "the common target covers every slice: {per_slice:?} vs {target}");
    assert!((high - low) * 10 <= target, "balanced within 10 %: {per_slice:?} vs {target}");
    assert!(target <= balanced * 11 / 10, "the target stays near the balanced need: {target} vs {balanced}");
    assert_eq!(target % 1000, 0);
}

/// Stability: a cheap object added past the others moves nothing (the
/// rounded caps hold). The spread of the moves for an object added
/// anywhere is measured on the real universe
/// (`orchestrator::market_maker` tests).
#[test]
fn a_cheap_object_added_past_the_others_moves_nothing() {
    let universe = shaped_universe();
    let before = assigned(&partition(&MAKER_CAPS, &universe, 11));
    for anchor in [5, 120, 900] {
        let mut grown = universe.clone();
        grown.push((10_000, anchor));
        let after = assigned(&partition(&MAKER_CAPS, &grown, 11));
        assert!(before.iter().all(|(id, slice)| after[id] == *slice), "a cheap object at {anchor} past the others moves nothing");
    }
}

#[test]
fn the_balanced_capital_quotes_one_lot_of_every_object_within_its_share() {
    // 50 objects worth 2 coins: lots of 500 units (1000 coins) each.
    let cheap = vec![2; 50];
    assert_eq!(balanced_capital(&MAKER_CAPS, &cheap, 1), 50_000);
    assert_eq!(balanced_capital(&MAKER_CAPS, &cheap, 2), 25_000, "split over two accounts");
    // One rune platebody (65 000) next to them cannot take the capital:
    // its lot is capped at 10 % of the target.
    let mut rich = cheap.clone();
    rich.push(65_000);
    let target = balanced_capital(&MAKER_CAPS, &rich, 1);
    assert!((55_000..=56_000).contains(&target), "{target}");
    assert!(balanced_capital(&MAKER_CAPS, &[], 1) >= 1000);
    assert_eq!(slice_need(&MAKER_CAPS, &rich, 56_000), 50_000 + 5_600, "the platebody at 10 % of 56 000");
    assert_eq!(common_capital_target(&MAKER_CAPS, &[rich.clone(), cheap.clone()], 1_000), 56_000, "the largest slice need, a fixed point");
}

#[test]
fn quotes_bracket_the_current_price_with_a_paying_spread() {
    let at_lot = quote(&MAKER_CAPS, &liquid(10));
    assert_eq!(at_lot.regime, Regime::Liquid);
    let (bid, ask) = (at_lot.bid.expect("bid"), at_lot.ask.expect("ask"));
    assert!(bid.price < 100 && ask.price > 100, "{at_lot:?}");
    assert!(ask.price as f64 / bid.price as f64 <= 1.25, "spread under x1.25: {at_lot:?}");
    assert!(ask.price - bid.price >= 2, "the round trip pays: {at_lot:?}");
}

#[test]
fn stock_moves_both_sides() {
    let empty = quote(&MAKER_CAPS, &liquid(0));
    let full = quote(&MAKER_CAPS, &liquid(30));
    assert!(empty.ask.is_none(), "nothing to sell");
    let high = full.bid.expect("bid").price;
    let low = empty.bid.expect("bid").price;
    assert!(high < low, "stock high lowers the bid: {high} vs {low}");
    let normal = quote(&MAKER_CAPS, &liquid(10)).ask.expect("ask").price;
    assert!(full.ask.expect("ask").price < normal, "stock high lowers the ask");
    let thin = quote(&MAKER_CAPS, &liquid(2)).ask.expect("ask").price;
    assert!(thin > normal, "stock low raises the ask: {thin} vs {normal}");
    for stock in [0, 2, 10, 30, 400] {
        let seen = quote(&MAKER_CAPS, &liquid(stock));
        if let Some(bid) = seen.bid {
            assert!(bid.price < 100, "bid always under the current price: {seen:?}");
        }
        if let Some(ask) = seen.ask {
            assert!(ask.price > 100, "ask always over it: {seen:?}");
        }
    }
}

/// The 10 % rule for every bid (user 2026-11-20, « il faut les faire
/// monter les offres d'achat pour inciter les gens à vendre »; live: 351
/// maker bids at 1 coin frozen for a day): nobody sells to it, its bid
/// climbs 5 % a pass (at least a coin), whatever the regime, the
/// quantity shrinking with the capital cap; past one lot the stock still
/// goes to the shop (a real buyer). A fill restarts from a fresh quote
/// (the caller passes no `own_bid`).
#[test]
fn an_unserved_bid_climbs_five_percent_a_pass_and_dumps_to_the_shop() {
    let idle = MakerInput { best_bid: None, recent_buys: 0, stock: 300, capital: 1_000_000, own_bid: Some(90), ..liquid(0) };
    let first = quote(&MAKER_CAPS, &idle);
    assert_eq!(first.regime, Regime::NoDemand);
    assert_eq!(first.bid.map(|leg| leg.price), Some(95), "5 % up a pass");
    let mut own = Some(1);
    for _ in 0..30 {
        let next = quote(&MAKER_CAPS, &MakerInput { own_bid: own, stock: 0, ..idle }).bid.map(|leg| leg.price);
        assert!(next > own, "always up from an unserved bid: {next:?} after {own:?}");
        own = next;
    }
    let fresh = quote(&MAKER_CAPS, &MakerInput { own_bid: None, ..idle }).bid.map(|leg| leg.price);
    assert!(fresh < Some(90), "served: a fresh quote under the fair value: {fresh:?}");
    assert_eq!(first.npc_dump, 300 - lot_units(&MAKER_CAPS, 100), "past one lot goes to the shop");
    let no_shop = quote(&MAKER_CAPS, &MakerInput { outside: None, ..idle });
    assert_eq!(no_shop.npc_dump, 0, "no shop pays: nothing dumped");
}

/// No price floor: the raw shop value (`outside`, first unit at base
/// stock, no trip netted) is no price; without a net outlet (D2: the
/// trip costs the maker more than the shop pays over a lot) the ask
/// follows its fair value, even under it.
#[test]
fn the_ask_may_go_under_the_shop_offer() {
    let cheap = quote(&MAKER_CAPS, &MakerInput { reference: traded(10), outside: Some(30), stock: 50, ..liquid(0) });
    assert!(cheap.ask.expect("ask").price < 30, "{cheap:?}");
}

/// Plan équilibre D2 (user 2026-11-19): yew logs traded at 1 coin, asks
/// at 1, a general store paying 16 at its floor without limit, 15 net of
/// the trip over a lot. The outlet is a real buyer: it enters the
/// maker's reference value for BUYING. The maker buys the asks under it
/// (its bid crosses the 1-coin asks), keeps bidding with no other bidder
/// (the shop IS demand: no walk to 1) and sends the stock past one lot to
/// the shop; its unsold ask walks down 10 % a pass like everyone's (no
/// floor).
#[test]
fn a_shop_outlet_enters_the_reference_value() {
    let yew = MakerInput {
        reference: traded(1),
        best_bid: None,
        best_ask: Some(1),
        bid_depth: 0,
        ask_depth: 5000,
        recent_buys: 0,
        anchor: 160,
        outside: Some(64),
        outlet: Some(15),
        make_cost: None,
        stock: 0,
        stock_cost: 0,
        capital: 103_000,
        own_bid: Some(1),
        own_ask: None,
    };
    let seen = quote(&MAKER_CAPS, &yew);
    assert_eq!(seen.fair, 15, "the outlet, not the 1-coin trade: {seen:?}");
    assert_ne!(seen.regime, Regime::NoDemand, "the shop buys: {seen:?}");
    let bid = seen.bid.expect("a bid under the outlet");
    assert_eq!(bid.price, 14, "one coin under the outlet: it takes every ask under it: {seen:?}");
    assert!(bid.qty >= lot_units(&MAKER_CAPS, 15) / 2, "{seen:?}");
    let lot = lot_units(&MAKER_CAPS, 15);
    let full = quote(&MAKER_CAPS, &MakerInput { stock: 400, stock_cost: 400, ..yew });
    assert!(full.ask.expect("an ask").price > 15, "a fresh ask quotes over its fair value: {full:?}");
    assert_eq!(full.npc_dump, 400 - lot, "past one lot, the shop: {full:?}");
    // Review lot 5 (user: no floor of any kind, shop and alchemy
    // included; the walk is everyone's, 5 %): an ask still unsold drops
    // 5 % a pass, under the outlet too.
    let walked = quote(&MAKER_CAPS, &MakerInput { stock: 400, stock_cost: 400, own_ask: Some(15), ..yew });
    assert_eq!(walked.ask.expect("ask").price, 14, "15 unsold: 5 % down, under the outlet's 15: {walked:?}");
    // The same object without the outlet: the 1-coin trade rules.
    let plain = quote(&MAKER_CAPS, &MakerInput { outlet: None, ..yew });
    assert_eq!((plain.fair, plain.regime), (1, Regime::NoDemand), "{plain:?}");
}

#[test]
fn a_never_traded_object_is_discovered_prudently() {
    let fresh = MakerInput {
        reference: Reference { price: 1, traded: false },
        best_bid: None,
        best_ask: Some(400),
        bid_depth: 0,
        ask_depth: 5,
        recent_buys: 0,
        anchor: 200,
        outside: Some(60),
        outlet: None,
        make_cost: None,
        stock: 0,
        stock_cost: 0,
        capital: 100_000,
        own_bid: None,
        own_ask: None,
    };
    let seen = quote(&MAKER_CAPS, &fresh);
    assert_eq!(seen.regime, Regime::Discovery);
    let bid = seen.bid.expect("a discovery bid");
    assert!(bid.qty <= (lot_units(&MAKER_CAPS, 200) / 4).max(1), "a quarter lot: {seen:?}");
    let held = quote(&MAKER_CAPS, &MakerInput { stock: 3, ..fresh });
    let ask = held.ask.expect("ask");
    assert!(ask.price <= 200, "never over the shop value a buyer faces: {held:?}");
    assert!(ask.price as f64 / held.bid.expect("bid").price as f64 <= 1.25, "{held:?}");
    // A cheaper public listing is the ceiling then.
    let listed = quote(&MAKER_CAPS, &MakerInput { best_ask: Some(120), stock: 3, ..fresh });
    assert!(listed.ask.expect("ask").price <= 120 && listed.bid.expect("bid").price < 120, "{listed:?}");
    // Prudence: an empty stock never raises a discovery bid.
    let lot = lot_units(&MAKER_CAPS, seen.fair);
    assert_eq!(seen.bid.map(|leg| leg.price), quote(&MAKER_CAPS, &MakerInput { stock: lot, ..fresh }).bid.map(|leg| leg.price));
    // No floor at what a shop pays (user 2026-11-18): a public listing at
    // 30 sets the ceiling, the ask follows it under the shop's 60.
    let cheap = quote(&MAKER_CAPS, &MakerInput { best_ask: Some(30), stock: 3, ..fresh });
    assert!(cheap.ask.expect("ask").price <= 30, "{cheap:?}");
}

#[test]
fn risk_caps_bound_capital_stock_and_book_share() {
    // Averaging down: stock bought at 100, now valued 50, cost counts.
    let fallen = quote(&MAKER_CAPS, &MakerInput { reference: traded(50), stock: 100, stock_cost: 10_000, ..liquid(0) });
    assert!(fallen.bid.is_none(), "the cost already fills its share: {fallen:?}");
    // Capital: 10 % of 5000 = 500 coins per object, 4 units held at 100.
    let poor = quote(&MAKER_CAPS, &MakerInput { capital: 5000, stock: 4, ..liquid(0) });
    let bid = poor.bid.expect("bid");
    assert!(4 * 100 + bid.qty * bid.price <= 500, "{poor:?}");
    let broke = quote(&MAKER_CAPS, &MakerInput { capital: 5000, stock: 5, ..liquid(0) });
    assert!(broke.bid.is_none(), "share used up: no bid {broke:?}");
    // Stock cap: 500 units of a 1-coin object, no more bid, excess dumped.
    let feathers = MakerInput { reference: traded(2), stock: 520, outside: Some(1), ..liquid(0) };
    let seen = quote(&MAKER_CAPS, &feathers);
    assert!(seen.bid.is_none(), "{seen:?}");
    assert_eq!(seen.npc_dump, 20);
    // Book share: others rest 6 units, the maker posts at most 2.
    let shallow = quote(&MAKER_CAPS, &MakerInput { bid_depth: 6, ask_depth: 6, stock: 10, ..liquid(0) });
    assert_eq!(shallow.bid.map(|leg| leg.qty), Some(2));
    assert_eq!(shallow.ask.map(|leg| leg.qty), Some(2));
    // Alone on a side: one lot (the maker is the liquidity).
    let alone = quote(&MAKER_CAPS, &MakerInput { bid_depth: 0, stock: 0, ..liquid(0) });
    assert_eq!(alone.bid.map(|leg| leg.qty), Some(lot_units(&MAKER_CAPS, 100)));
}

/// M7: a never-traded object with a published production cost is quoted
/// around that cost (no sellers' floor, user 2026-11-18) and its ask under
/// the buyers' ceiling.
#[test]
fn discovery_centres_on_the_published_cost_inside_the_public_bounds() {
    let fresh = MakerInput {
        reference: Reference { price: 1, traded: false },
        best_bid: None,
        best_ask: None,
        bid_depth: 0,
        ask_depth: 0,
        recent_buys: 0,
        anchor: 200,
        outside: Some(24),
        outlet: None,
        make_cost: Some(30),
        stock: 3,
        stock_cost: 90,
        capital: 100_000,
        own_bid: None,
        own_ask: None,
    };
    let seen = quote(&MAKER_CAPS, &fresh);
    assert_eq!((seen.regime, seen.fair), (Regime::Discovery, 30), "the published cost, not 200 / 1.1: {seen:?}");
    let (bid, ask) = (seen.bid.expect("bid").price, seen.ask.expect("ask").price);
    assert!(bid < 30 && ask > 30 && ask <= 33, "around the cost: {seen:?}");
    // A cheap public listing caps the fair value whatever the cost.
    let listed = quote(&MAKER_CAPS, &MakerInput { best_ask: Some(28), ..fresh });
    assert!(listed.ask.expect("ask").price <= 28, "{listed:?}");
    // No cost published: the ceiling rule of wave 3.
    let unknown = quote(&MAKER_CAPS, &MakerInput { make_cost: None, ..fresh });
    assert_eq!(unknown.fair, (200.0_f64 / (1.0 + MAKER_CAPS.half_spread_discovery)).floor() as i64);
}

/// Every order works the same (user 2026-11-18: "tous les items doivent
/// fonctionner pareil bid et ask"): a maker's ask still resting unsold at
/// the next pass drops 5 % (the sellers' rate), whatever the last trade
/// says (wooden shield: the maker held 55 over a last trade at 53 while
/// nobody bought), down to one coin over its own bid; a fresh quote under
/// it is taken at once.
#[test]
fn an_unsold_maker_ask_walks_down_like_any_seller() {
    let held = MakerInput { stock: 50, ..liquid(50) };
    let fresh = quote(&MAKER_CAPS, &held).ask.expect("ask").price;
    let bid = quote(&MAKER_CAPS, &held).bid.expect("bid").price;
    let walked = quote(&MAKER_CAPS, &MakerInput { own_ask: Some(fresh), ..held });
    assert_eq!(walked.ask.expect("ask").price, ((fresh as f64 * 0.95).floor() as i64).max(bid + 1), "5 % down a pass: {walked:?}");
    assert!(walked.ask.expect("ask").price < fresh);
    let mut own = Some(fresh);
    for _ in 0..200 {
        own = quote(&MAKER_CAPS, &MakerInput { own_ask: own, ..held }).ask.map(|leg| leg.price);
    }
    assert_eq!(own, Some(bid + 1), "down to one coin over its bid");
    let under = quote(&MAKER_CAPS, &MakerInput { own_ask: Some(fresh * 3), ..held });
    assert_eq!(under.ask.expect("ask").price, fresh, "a lower fresh quote is taken at once");
}
