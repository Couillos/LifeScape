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
//! each bought as it comes back (stock 1). Shops: every F2P shop a
//! non-member reaches (`obtain::shop_reach`, as the sale side, Q3),
//! each at its cheapest F2P opener (keeper param or scripted `~openshop`
//! multiplier, haggle from `shop_haggle.json`). A shop behind a
//! condition (an F2P quest: Oziach after Dragon Slayer; quest points:
//! the Champions' Guild door, 32) sells too, its condition on each of
//! its steps (user 2026-11-16, Q7: open to every bot that fulfils it);
//! a shop no F2P bot reaches never does (Magic Guild, members; Lundail,
//! Mage Arena off the F2P map).

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

/// One step of a purchase: `units` at `price` each from `shop`, and
/// what a bot needs to reach it (`obtain::ShopReach`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShopStep {
    pub shop: String,
    pub price: i64,
    pub units: i64,
    /// F2P quest directory the shop's opener asks (`quest_dragon`).
    pub quest: Option<String>,
    /// Quest points the keeper's door asks (Champions' Guild: 32).
    pub quest_points: i64,
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
            _ => out.push(ShopStep { shop: shop.to_string(), price, units, ..ShopStep::default() }),
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
/// reachable shop's ladder (`shop_ladder`), each step with its shop's
/// condition, merged cheapest first. Empty: no F2P shop a non-member
/// reaches sells it.
pub fn purchase_steps(item: &str, window_ticks: i64) -> Vec<ShopStep> {
    let Some(obj) = crate::objs_by_name(item).into_iter().next() else { return Vec::new() };
    let mut out: Vec<ShopStep> = Vec::new();
    for shop in crate::f2p::f2p_shop_list() {
        if shop.members == Some(true) {
            continue;
        }
        let Some(stock) = crate::f2p::f2p_stock(&shop).into_iter().find(|stock| stock.obj == item && stock.count > 0) else { continue };
        let (Some(access), Some(reach)) = (crate::f2p::f2p_shop_access(&shop.name), crate::obtain::shop_reach(&shop.name)) else { continue };
        let Some((multiplier, haggle)) = terms(&shop, &access.npcs) else { continue };
        let ladder = shop_ladder(&shop.name, obj.price, haggle, multiplier, stock.count, stock.rate, shop.restock, window_ticks);
        out.extend(ladder.into_iter().map(|step| ShopStep { quest: reach.quest.clone(), quest_points: reach.quest_points, ..step }));
    }
    out.sort_by(|left, right| left.price.cmp(&right.price).then_with(|| left.shop.cmp(&right.shop)));
    out
}

/// Most one unit of `item` can cost at `shop` (`None`: every F2P shop
/// that sells it): its price at stock 1, the dearest unit `buy_item`
/// ever asks, whatever the live stock other buyers left. What a bot
/// carries to the counter (live 2026-11-17: jane_95 carried the dump
/// value of an adamant chainbody, 4 800, and the White Knights' asked
/// more: "You don't have enough coins", three runs). `None` when no
/// such shop sells it.
pub fn unit_price_ceiling(item: &str, shop: Option<&str>) -> Option<i64> {
    let steps = purchase_steps(item, 0);
    let at_shop: Vec<&ShopStep> = steps.iter().filter(|step| shop.is_none_or(|key| step.shop == key)).collect();
    at_shop.iter().map(|step| step.price).max()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ceiling is the dearest base-stock unit (stock 1) of the shop,
    /// never under the dump value.
    #[test]
    fn the_ceiling_is_the_last_base_unit() {
        let steps = purchase_steps("adamant_chainbody", 0);
        let shop = steps.first().map(|step| step.shop.clone()).expect("an F2P shop sells adamant chainbodies");
        let dearest = steps.iter().filter(|step| step.shop == shop).map(|step| step.price).max();
        assert_eq!(unit_price_ceiling("adamant_chainbody", Some(&shop)), dearest);
        let value = crate::objs_by_name("adamant_chainbody").first().map(|obj| obj.price).unwrap_or(0);
        assert!(dearest.unwrap_or(0) >= value, "never under the dump value at stock 1: {dearest:?} vs {value}");
        // A keeper selling over the value (multiplier above 1000) asks more
        // than the dump value at its last unit: the ceiling follows it.
        let dear = crate::shops::all_shops().into_iter().flat_map(|shop| shop.keepers).any(|keeper| keeper.sell_multiplier.is_some_and(|multiplier| multiplier > 1000));
        assert!(dear, "some F2P keeper sells over the value");
        assert_eq!(unit_price_ceiling("adamant_chainbody", Some("no_such_shop")), None);
        assert_eq!(unit_price_ceiling("no_such_item", None), None);
    }

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
        assert!(purchase_steps("rune_full_helm", 6000).is_empty(), "no F2P shop sells it");
        assert!(steps.iter().all(|step| step.quest.is_none() && step.quest_points == 0), "feathers sell behind no condition");
        assert_eq!(keeper_haggle("gerrant"), haggles().keepers.get("gerrant").copied());
        assert!(keeper_haggle("no_such_keeper").is_some(), "the param default");
    }

    /// User 2026-11-16 (Q7): a shop behind a condition is in the window,
    /// its condition on each of its steps: Oziach's rune platebody after
    /// Dragon Slayer (`runeplateshop`), Scavvo's rune longsword behind the
    /// Champions' Guild door (32 quest points). A shop no F2P bot reaches
    /// stays out: Lundail (Mage Arena cellar, off the F2P map) and the
    /// Magic Guild (members) never sell nature runes here.
    #[test]
    fn conditioned_shops_sell_with_their_condition() {
        let plate = purchase_steps("rune_platebody", 6000);
        assert!(!plate.is_empty(), "Oziach sells it after Dragon Slayer");
        assert!(plate.iter().all(|step| step.shop == "runeplateshop" && step.quest.as_deref() == Some("quest_dragon") && step.quest_points == 0), "{plate:?}");
        let sword = purchase_steps("rune_longsword", 6000);
        assert!(!sword.is_empty(), "Scavvo sells it upstairs");
        assert!(sword.iter().all(|step| step.shop == "runiteshop" && step.quest.is_none() && step.quest_points == 32), "{sword:?}");
        assert!(purchase_steps("naturerune", 6000).iter().all(|step| step.shop != "magearena_runeshop" && step.shop != "magicguildshop"));
    }
}
