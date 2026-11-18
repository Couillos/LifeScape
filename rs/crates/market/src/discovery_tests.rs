//! Plan marché M7 : découverte du prix d'un objet jamais échangé sur
//! des ancrages publics (boutique PNJ, alchimie, coût publié du guide,
//! prix de vente d'une boutique), jamais un prix inventé.

use super::*;
use crate::price::Reference;

const UNTRADED: Reference = Reference { price: 1, traded: false };

fn anchors(shop_sale: Option<i64>, alch_net: Option<i64>, make_cost: Option<i64>, shop_buy: Option<i64>) -> Anchors {
    Anchors { shop_sale, alch_net, make_cost, shop_buy }
}

#[test]
fn the_floor_is_the_best_outside_option() {
    assert_eq!(anchors(Some(24), Some(30), None, None).floor(), 30, "a caster's net beats the shop");
    assert_eq!(anchors(Some(24), Some(-5), None, None).floor(), 24, "a losing cast is no option");
    assert_eq!(Anchors::default().floor(), 0);
    assert!(!Anchors::default().known());
    assert!(anchors(None, None, Some(30), None).known());
    assert_eq!(seller_floor(20, &anchors(Some(24), None, None, None)), 24, "never under the shop's offer");
    assert_eq!(seller_floor(40, &anchors(Some(24), None, None, None)), 40, "its own keep value when higher");
    assert_eq!(seller_floor(0, &Anchors::default()), 1);
}

#[test]
fn margins_follow_the_profile() {
    assert!((margin_of(0.0) - 0.05).abs() < 1e-12);
    assert!((margin_of(1.0) - 0.20).abs() < 1e-12);
    assert!((margin_of(7.0) - 0.20).abs() < 1e-12, "clamped");
}

#[test]
fn a_seller_opens_over_the_published_cost_under_the_shop() {
    let cost = anchors(Some(24), None, Some(30), None);
    assert_eq!(ask_open(&cost, 24, 0.10, 999), 33, "30 x 1.10");
    assert_eq!(ask_open(&cost, 40, 0.10, 999), 40, "never under its floor");
    let shop = anchors(Some(24), None, Some(30), Some(31));
    assert_eq!(ask_open(&shop, 24, 0.10, 999), 31, "no buyer pays over the shop");
    assert_eq!(ask_open(&shop, 35, 0.10, 999), 35, "the floor still holds");
    let no_cost = anchors(Some(24), None, None, Some(50));
    assert_eq!(ask_open(&no_cost, 24, 0.10, 48), 48, "no cost: the seller's own rule");
    assert_eq!(ask_open(&no_cost, 24, 0.10, 80), 50, "capped by the shop");
}

#[test]
fn a_buyer_opens_under_its_value_over_the_public_floor() {
    let cost = anchors(Some(24), None, Some(30), None);
    assert_eq!(bid_open(&cost, Some(90), 200, 0.10), Some(30), "never over the published cost");
    assert_eq!(bid_open(&cost, None, 200, 0.10), Some(27), "no own value: the published cost less the margin");
    assert_eq!(bid_open(&cost, Some(25), 200, 0.10), Some(24), "never under the sellers' floor");
    assert_eq!(bid_open(&cost, Some(90), 28, 0.10), Some(28), "never over its ceiling");
    assert_eq!(bid_open(&cost, Some(20), 20, 0.10), Some(20), "a ceiling under the floor rests at the ceiling");
    let shop = anchors(None, None, None, Some(40));
    assert_eq!(bid_open(&shop, Some(90), 200, 0.10), Some(40), "the shop sells at 40");
    assert_eq!(bid_open(&shop, None, 200, 0.10), Some(36), "the shop's price as the value");
    let floor_only = anchors(Some(12), None, None, None);
    assert_eq!(bid_open(&floor_only, Some(90), 200, 0.10), Some(81), "its use value less the margin");
    assert_eq!(bid_open(&floor_only, None, 200, 0.10), Some(12), "only the floor: bid it");
    assert_eq!(bid_open(&Anchors::default(), Some(90), 200, 0.10), None, "no anchor: the buyer's own rule");
}

#[test]
fn untraded_listings_open_on_the_anchors_traded_ones_on_the_price() {
    let cost = anchors(Some(24), None, Some(30), None);
    assert_eq!(listing_open(UNTRADED, None, 20, None, &cost, 0.10), (33, 24), "over the cost, floor at the shop");
    assert_eq!(listing_open(UNTRADED, Some(45), 20, None, &cost, 0.10), (45, 24), "a richer bid is taken");
    assert_eq!(listing_open(UNTRADED, None, 20, None, &Anchors::default(), 0.10), (40, 20), "no anchor: twice the keep value");
    assert_eq!(listing_open(UNTRADED, None, 20, None, &anchors(Some(24), None, None, None), 0.10), (48, 24), "floor only: twice the floor");
    let traded = Reference { price: 60, traded: true };
    assert_eq!(listing_open(traded, None, 20, None, &cost, 0.10), (60, 24), "traded: the current price rules");
    assert_eq!(listing_open(Reference { price: 10, traded: true }, None, 20, None, &cost, 0.10), (24, 24), "never under the shop's offer");
}

#[test]
fn untraded_loot_anchors_on_the_cost_else_its_alch_value() {
    let cost = anchors(Some(5), None, Some(30), Some(31));
    assert_eq!(loot_anchor(UNTRADED, Some(12), 5, &cost, 0.10), Some(31), "the cost, capped by the shop");
    let no_cost = anchors(Some(5), None, None, None);
    assert_eq!(loot_anchor(UNTRADED, Some(12), 5, &no_cost, 0.10), Some(12), "its high alchemy value");
    assert_eq!(loot_anchor(UNTRADED, None, 5, &no_cost, 0.10), Some(10), "twice the floor");
    assert_eq!(loot_anchor(Reference { price: 9, traded: true }, Some(12), 5, &cost, 0.10), Some(12), "traded: untouched");
    assert_eq!(loot_anchor(UNTRADED, Some(12), 1, &Anchors::default(), 0.10), Some(12), "no anchor: untouched");
}

/// User 2026-11-18 (point 4, universal): an overstocked seller's floor
/// is the alchemy outlet's buy price, the cast's net less the caster's
/// 10 %: mithril full helm net 753 (858 less a nature at 105) → 677.
#[test]
fn the_alch_outlet_leaves_the_caster_a_tenth() {
    assert_eq!(alch_outlet_price(753), Some(677));
    assert_eq!(alch_outlet_price(1221), Some(1098));
    assert_eq!(alch_outlet_price(1), None, "nothing left for the seller");
    assert_eq!(alch_outlet_price(-40), None, "a losing cast is no outlet");
}

/// Plan prix §2 (user 2026-11-18): 93 % of the listings opened AT their
/// floor, a concession curve with no course. A listing opens at least
/// its floor times the profile margin, one coin above at the least; a
/// standing bid symmetric under its ceiling.
#[test]
fn an_order_opens_with_room_to_concede() {
    assert_eq!(open_over_floor(858, 0.10), 944);
    assert_eq!(open_over_floor(5, 0.05), 6, "at least one coin above");
    assert_eq!(open_over_floor(1, 0.20), 2);
    assert_eq!(bid_under_ceiling(1_000, 0.10), 900);
    assert_eq!(bid_under_ceiling(5, 0.05), 4, "at least one coin under");
    assert_eq!(bid_under_ceiling(1, 0.20), 1, "never under one coin");
}

/// Plan prix §1 (user 2026-11-18, Q1 10 %, Q2 under the making cost): a
/// seller's floor drops 10 % per horizon that ended unsold (net of its
/// sales), never under its outside bound nor over its keep value.
#[test]
fn the_floor_learns_from_unsold_horizons() {
    assert_eq!(learned_floor(858, 300, 0), 858);
    assert_eq!(learned_floor(858, 300, 1), 772);
    assert_eq!(learned_floor(858, 300, 3), 625);
    assert_eq!(learned_floor(858, 300, 20), 300, "the outside bound holds");
    assert_eq!(learned_floor(858, 1, 64), 1);
    assert_eq!(learned_floor(858, 900, 2), 858, "a bound over the keep value never raises it");
    assert_eq!(unsold_horizons(3, 1), 2, "each sale takes one failure back");
    assert_eq!(unsold_horizons(1, 4), 0);
}
