//! NPC shop SALES (plan marché Q3, user decision 2026-11-16: an object
//! the market pays less for than a merchant is sold to the merchant,
//! like a player emptying his loot at the general store; an exception
//! to D-A, `docs/decisions.md`). Server truth (`shop.rs2`):
//! - `can_sell_obj`: a tradeable, non-members object, never coins, to a
//!   general store (`inv_allstock`) or a shop whose base stock lists it;
//! - `adjusted_item_cost_selling`: `calc_shop_value(cost, shop_delta,
//!   shop_buy_multiplier, stock - base)`, the value falling with every
//!   unit the shop already holds (`shop_delta` per unit, not in our
//!   dump: the first unit at base stock is the best case, the script
//!   checks every coin it receives against its floor).

#![forbid(unsafe_code)]

/// Server `calc_shop_value(base_cost, haggle, multiplier, diff)`:
/// `scale(max(100, multiplier - clamp(diff * haggle, -5000, 1000)), 1000,
/// base_cost)`.
pub fn calc_shop_value(base_cost: i64, haggle: i64, multiplier: i64, diff: i64) -> i64 {
    let shift = diff.saturating_mul(haggle).clamp(-5000, 1000);
    let rate = (multiplier - shift).max(100);
    rate.saturating_mul(base_cost.max(0)) / 1000
}

/// One F2P shop that buys an object: its key, what it pays for the
/// first unit at base stock, its keepers' reachable F2P sites, and what
/// a bot needs to reach them (`obtain::ShopReach`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SaleShop {
    pub shop: String,
    pub value: i64,
    pub sites: Vec<(i64, i64, i64)>,
    /// F2P quest directory the shop's opener asks (`quest_dragon`).
    pub quest: Option<String>,
    /// Quest points the keeper's door asks (Champions' Guild: 32).
    pub quest_points: i64,
}

impl SaleShop {
    /// True when every bot reaches it (no quest, no quest points).
    pub fn open(&self) -> bool {
        self.quest.is_none() && self.quest_points == 0
    }
}

/// True when the server lets an F2P player sell `item` to `shop`
/// (`can_sell_obj`, F2P world).
fn buys(shop: &crate::shops::Shop, item: &str, obj: &crate::Obj) -> bool {
    item != "coins" && obj.tradeable && obj.members != Some(true) && (shop.allstock || shop.stocks.iter().any(|stock| stock.obj == item))
}

/// Every F2P shop a non-member reaches that buys `item`, its condition
/// with it (an F2P quest, the Champions' Guild's quest points; user
/// 2026-11-16, Q7, as the purchase side `shop_buy`), best value first;
/// never a guessed price (a keeper without its buy multiplier is
/// skipped), never a shop no F2P bot reaches (Magic Guild, Mage Arena).
pub fn npc_sale_shops_reachable(item: &str) -> Vec<SaleShop> {
    let Some(obj) = crate::objs_by_name(item).into_iter().next() else { return Vec::new() };
    let mut out = Vec::new();
    for shop in crate::f2p::f2p_shop_list() {
        if shop.members == Some(true) || !buys(&shop, item, obj) {
            continue;
        }
        let Some(multiplier) = shop.keepers.iter().filter_map(|keeper| keeper.buy_multiplier).max() else { continue };
        let value = calc_shop_value(obj.price, 0, multiplier, 0);
        if value < 1 {
            continue;
        }
        let Some(reach) = crate::obtain::shop_reach(&shop.name) else { continue };
        let mut sites = reach.sites;
        sites.sort_unstable();
        sites.dedup();
        if sites.is_empty() {
            continue;
        }
        out.push(SaleShop { shop: shop.name.clone(), value, sites, quest: reach.quest, quest_points: reach.quest_points });
    }
    out.sort_by(|left, right| right.value.cmp(&left.value).then_with(|| left.shop.cmp(&right.shop)));
    out
}

/// F2P shops that buy `item` and that EVERY bot reaches (no quest, no
/// quest points: the brain's shop trip, Q3), best value first.
pub fn npc_sale_shops(item: &str) -> Vec<SaleShop> {
    npc_sale_shops_reachable(item).into_iter().filter(SaleShop::open).collect()
}

/// Best unit value any F2P shop open to every bot pays for `item` (first
/// unit, base stock); `None` when none buys it or pays at least a coin.
pub fn npc_sale_value(item: &str) -> Option<i64> {
    npc_sale_shops(item).first().map(|shop| shop.value)
}

/// Largest `shop_delta` param of the server's shop keepers (content npc
/// configs: 10, 20 or 30); the dump does not carry it per keeper.
pub const MAX_SHOP_DELTA: i64 = 30;

/// What `shop` pays AT LEAST for the next unit of `item` while it holds
/// `held` units of it (`adjusted_item_cost_selling`): the steepest delta
/// when the shop holds its base stock or more, none under it (the lower
/// bound either way). `None`: the shop does not buy it or its buy
/// multiplier is unknown. The selling script checks every unit against
/// the market with this bound: never a unit sold under it.
pub fn unit_value_floor(shop: &str, item: &str, held: i64) -> Option<i64> {
    let obj = crate::objs_by_name(item).into_iter().next()?;
    let entry = crate::shops::shop_list().iter().find(|entry| entry.name.eq_ignore_ascii_case(shop))?;
    if !buys(entry, item, obj) {
        return None;
    }
    let multiplier = entry.keepers.iter().filter_map(|keeper| keeper.buy_multiplier).max()?;
    // `price_mod`: a stocked object counts from its base, another from 0.
    let base = entry.stocks.iter().find(|stock| stock.obj == item).map(|stock| stock.count).unwrap_or(0);
    let diff = held - base;
    let delta = if diff >= 0 { MAX_SHOP_DELTA } else { 0 };
    Some(calc_shop_value(obj.price, delta, multiplier, diff))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `shop.rs2` `calc_shop_value`: a general store (buy multiplier
    /// 400) pays 40 % of the object's cost for its first unit, less per
    /// unit it already holds, never under 10 %.
    #[test]
    fn shop_values_follow_the_server_formula() {
        assert_eq!(calc_shop_value(100, 30, 400, 0), 40);
        assert_eq!(calc_shop_value(100, 30, 400, 5), 25, "five held: 400 - 150");
        assert_eq!(calc_shop_value(100, 30, 400, 100), 10, "never under 10 %");
        assert_eq!(calc_shop_value(2, 30, 400, 0), 0, "a feather pays nothing");
    }

    /// Iron daggers (cost 35, never traded live 2026-11-16, 60 sellers):
    /// the general stores buy them; coins sell nowhere, a feather for 1.
    #[test]
    fn general_stores_buy_loot_never_coins_or_feathers() {
        let daggers = npc_sale_shops("iron_dagger");
        assert!(!daggers.is_empty(), "a general store buys iron daggers");
        let cost = crate::objs_by_name("iron_dagger")[0].price;
        assert!(daggers.iter().all(|shop| shop.value >= 1 && shop.value <= cost), "{daggers:?}");
        assert!(daggers.iter().all(|shop| !shop.sites.is_empty()));
        assert_eq!(npc_sale_value("coins"), None);
        let general = &daggers.iter().find(|shop| shop.shop.starts_with("generalshop")).expect("a general store").shop;
        let first = unit_value_floor(general, "iron_dagger", 0).expect("buys daggers");
        let tenth = unit_value_floor(general, "iron_dagger", 9).expect("buys daggers");
        assert!(first >= tenth && tenth >= 1, "the value falls with the shop's stock: {first} {tenth}");
        assert_eq!(unit_value_floor(general, "coins", 0), None);
        assert!(npc_sale_value("feather").is_none_or(|value| value <= 1), "a feather (cost 2) pays a coin at most");
    }

    /// User 2026-11-16 (Q7), the sale side aligned on the purchase side:
    /// a shop behind a condition buys too, its condition with it (Oziach
    /// after Dragon Slayer, Scavvo behind the Champions' Guild's 32 quest
    /// points); the open list (the brain's shop trip) keeps only the
    /// shops every bot reaches.
    #[test]
    fn conditioned_shops_buy_with_their_condition() {
        let plate = npc_sale_shops_reachable("rune_platebody");
        let oziach = plate.iter().find(|shop| shop.shop == "runeplateshop").expect("Oziach buys rune platebodies");
        assert_eq!((oziach.quest.as_deref(), oziach.quest_points), (Some("quest_dragon"), 0));
        let sword = npc_sale_shops_reachable("rune_longsword");
        let scavvo = sword.iter().find(|shop| shop.shop == "runiteshop").expect("Scavvo buys rune longswords");
        assert_eq!((scavvo.quest.as_deref(), scavvo.quest_points), (None, 32));
        assert!(npc_sale_shops("rune_longsword").iter().all(SaleShop::open), "the open list: every bot reaches it");
        assert!(!npc_sale_shops("rune_longsword").iter().any(|shop| shop.shop == "runiteshop" || shop.shop == "runeplateshop"));
        assert!(npc_sale_shops_reachable("naturerune").iter().all(|shop| shop.shop != "magearena_runeshop" && shop.shop != "magicguildshop"));
    }
}
