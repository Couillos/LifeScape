//! NPC shop PURCHASES with a finite stock (decision utilisateur
//! 2026-11-16 §2). Server truth:
//! - `shop.rs2` `buy_item`: each unit bought costs
//!   `adjusted_item_cost_buying(obj, stock)` =
//!   `calc_shop_value(cost, shop_delta, shop_sell_multiplier,
//!   stock - base)`, at least 1 coin: the price rises with every unit the
//!   stock falls under its base; an empty stock sells nothing;
//! - engine `World.ts` (inv cleanup each tick): a restocking inventory
//!   gains one unit of an object under its base every `stockrate` ticks
//!   (`tick % stockrate == 0`).
//!
//! So one window of `window_ticks` buys at most the base stock (unit by
//! unit, the price rising) plus `window_ticks / rate` restocked units,
//! each bought as it comes back (stock 1). Shops: the F2P ones a
//! non-member reaches without a quest (`f2p::f2p_shop_access`, as the
//! sale side, Q3), each at its cheapest F2P opener (keeper param or
//! scripted `~openshop` multiplier, haggle from `shop_haggle.json`).

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::OnceLock;

const RAW: &str = include_str!("../../../../gamedata/data/shop_haggle.json");

struct Haggles {
    default: Option<i64>,
    keepers: HashMap<String, i64>,
    opens: HashMap<String, i64>,
}

fn haggles() -> &'static Haggles {
    static TABLE: OnceLock<Haggles> = OnceLock::new();
    TABLE.get_or_init(|| {
        let doc = lifescape_json::parse(RAW).unwrap_or(lifescape_json::Value::Null);
        let map = |key: &str| -> HashMap<String, i64> { doc.get(key).as_obj().cloned().unwrap_or_default().into_iter().filter_map(|(name, value)| value.as_i64().map(|delta| (name, delta))).collect() };
        Haggles { default: doc.get("default").as_i64(), keepers: map("keepers"), opens: map("opens") }
    })
}

/// Haggle (`shop_delta`) of a keeper NPC: its param, else the param's
/// default; `None` when the dump carries neither.
pub fn keeper_haggle(npc: &str) -> Option<i64> {
    haggles().keepers.get(npc).copied().or(haggles().default)
}

/// Haggle a scripted `~openshop` call passes for `inv`.
pub fn open_haggle(inv: &str) -> Option<i64> {
    haggles().opens.get(inv).copied()
}

/// Price of the next unit bought from a shop holding `stock` units of an
/// object it stocks `base` of (`adjusted_item_cost_buying`).
pub fn unit_buy_price(cost: i64, haggle: i64, sell_multiplier: i64, stock: i64, base: i64) -> i64 {
    crate::shop_sale::calc_shop_value(cost, haggle, sell_multiplier, stock - base).max(1)
}

/// One step of a purchase: `units` at `price` each from `shop`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShopStep {
    pub shop: String,
    pub price: i64,
    pub units: i64,
}

/// One shop's window, cheapest first: the base stock unit by unit
/// (equal prices merged), then the restocked units (one every `rate`
/// ticks, each bought at stock 1). Pure server arithmetic.
#[allow(clippy::too_many_arguments)]
pub fn shop_ladder(shop: &str, cost: i64, haggle: i64, sell_multiplier: i64, base: i64, rate: i64, restock: bool, window_ticks: i64) -> Vec<ShopStep> {
    let mut out: Vec<ShopStep> = Vec::new();
    let mut push = |price: i64, units: i64| {
        if units <= 0 {
            return;
        }
        match out.last_mut() {
            Some(last) if last.price == price => last.units += units,
            _ => out.push(ShopStep { shop: shop.to_string(), price, units }),
        }
    };
    for stock in (1..=base.max(0)).rev() {
        push(unit_buy_price(cost, haggle, sell_multiplier, stock, base), 1);
    }
    if restock && rate > 0 && base > 0 {
        push(unit_buy_price(cost, haggle, sell_multiplier, 1, base), window_ticks.max(0) / rate);
    }
    out
}

/// Cheapest F2P opener terms of `shop` (sell multiplier, haggle).
fn terms(shop: &crate::shops::Shop, npcs: &[usize]) -> Option<(i64, i64)> {
    let mut found: Vec<(i64, i64)> = Vec::new();
    for (npc, opening) in crate::shop_access::openers(&shop.name) {
        if !npcs.contains(&npc.npc_id) {
            continue;
        }
        match opening.sell_multiplier {
            Some(multiplier) => found.extend(open_haggle(&opening.inv).map(|haggle| (multiplier, haggle))),
            None => {
                for keeper in shop.keepers.iter().filter(|keeper| keeper.npc_id as usize == npc.npc_id) {
                    if let (Some(multiplier), Some(haggle)) = (keeper.sell_multiplier, keeper_haggle(&keeper.npc)) {
                        found.push((multiplier, haggle));
                    }
                }
            }
        }
    }
    found.into_iter().min()
}

/// What F2P shops sell of `item` in a window of `window_ticks`: every
/// shop's ladder (`shop_ladder`), merged cheapest first. Empty: no F2P
/// shop a non-member reaches without a quest sells it.
pub fn purchase_steps(item: &str, window_ticks: i64) -> Vec<ShopStep> {
    let Some(obj) = crate::objs_by_name(item).into_iter().next() else { return Vec::new() };
    let mut out: Vec<ShopStep> = Vec::new();
    for shop in crate::f2p::f2p_shops() {
        if shop.members == Some(true) {
            continue;
        }
        let Some(stock) = crate::f2p::f2p_stock(&shop).into_iter().find(|stock| stock.obj == item && stock.count > 0) else { continue };
        let Some(access) = crate::f2p::f2p_shop_access(&shop.name) else { continue };
        if access.quest.is_some() {
            continue;
        }
        let Some((multiplier, haggle)) = terms(&shop, &access.npcs) else { continue };
        out.extend(shop_ladder(&shop.name, obj.price, haggle, multiplier, stock.count, stock.rate, shop.restock, window_ticks));
    }
    out.sort_by(|left, right| left.price.cmp(&right.price).then_with(|| left.shop.cmp(&right.shop)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `buy_item` walks the stock down: 5 in stock of base 5, multiplier
    /// 1000, haggle 30, cost 100: 100, 103, 106, 109, 112; the restock (one
    /// unit every 100 ticks) sells 60 units a window at stock 1 (112).
    #[test]
    fn a_window_buys_the_stock_unit_by_unit_then_the_restock() {
        let ladder = shop_ladder("test", 100, 30, 1000, 5, 100, true, 6000);
        let prices: Vec<(i64, i64)> = ladder.iter().map(|step| (step.price, step.units)).collect();
        assert_eq!(prices, vec![(100, 1), (103, 1), (106, 1), (109, 1), (112, 61)], "the last base unit and the restock share stock 1");
        assert_eq!(shop_ladder("test", 100, 30, 1000, 5, 100, false, 6000).iter().map(|step| step.units).sum::<i64>(), 5, "no restock: the base stock only");
        assert_eq!(unit_buy_price(2, 30, 1000, 1000, 1000), 2, "a feather at full stock");
        assert_eq!(unit_buy_price(1, 10, 100, 5, 5), 1, "never under one coin");
    }

    /// Gerrant's fishing shop (Port Sarim, F2P): feathers at base 1000,
    /// one back every tick: 7000 a window, the price rising from 2 coins
    /// as the stock falls (haggle from the keeper's config).
    #[test]
    fn feathers_come_from_the_fishing_shop_with_their_restock() {
        let steps = purchase_steps("feather", 6000);
        let gerrant: Vec<&ShopStep> = steps.iter().filter(|step| step.shop == "fishingshop").collect();
        assert_eq!(gerrant.iter().map(|step| step.units).sum::<i64>(), 1000 + 6000, "{gerrant:?}");
        assert_eq!(gerrant.first().map(|step| step.price), Some(2));
        assert!(gerrant.windows(2).all(|pair| pair[0].price < pair[1].price), "the price rises as the stock falls");
        assert!(steps.windows(2).all(|pair| pair[0].price <= pair[1].price), "cheapest first");
        assert!(purchase_steps("rune_platebody", 6000).is_empty(), "no F2P shop sells it");
        assert_eq!(keeper_haggle("gerrant"), haggles().keepers.get("gerrant").copied());
        assert!(keeper_haggle("no_such_keeper").is_some(), "the param default");
    }
}
