//! Permanent tool kit (user decision 2026-11-14, every bot, Iron
//! included): each bot owns at least one axe, one pickaxe, one hammer and
//! one tinderbox, whatever its current goal, and never sells them when
//! it changes goals.
//!
//! - Hammer, tinderbox: one copy (general stores, 1 coin).
//! - Pickaxe: the best tier its MINING level opens (`pickaxe_checker.rs2`:
//!   the server works only with a pickaxe the level allows).
//! - Axe: the best tier its WOODCUTTING level opens (`gear::usable_tools`,
//!   user rule). The server itself chops with any carried axe at any
//!   level (`woodcutting_axe_checker`, 2004 rules) and asks the attack
//!   level only to WIELD it (`levelrequire/tier*.rs2`): the kit carries
//!   it in the pack, so a held axe above the level still counts as the
//!   working one, and the attack level gates nothing here.
//!
//! Purchases (`kit_purchase`, pure): a tool whose tier has a shop route
//! (`gamedata::obtain::kit_shop_route`: Nurmof's pickaxes, Bob's axes up
//! to steel, the general stores; D-A exception for the kit's axes), at
//! the object's cost (`kit_price`: what Bob and Nurmof ask, and the
//! price the obtain's `shop:buy` leg checks the wallet against). Base kit first (missing
//! hammer, tinderbox, working pickaxe, axe): the best tier within
//! `TOOL_SHARE_PCT` (25 %) of the free wallet, else the cheapest tier
//! within `KIT_BASE_SHARE_PCT` (50 %). Then upgrades: the best tier
//! above the working one within 25 %. Nothing affordable: nothing (the
//! bot earns, the next pass looks again). Smithed-only tiers (mithril
//! axe and up) stay with the market wishlist.
//!
//! Keep (`kit_keep`): the best held member of each family (never sold,
//! even above the level: the bot grows into it), the best working one,
//! one hammer, one tinderbox. Lower tiers are surplus.

use lifescape_gamedata::gear::usable_tools;
use lifescape_gamedata::obtain::kit_shop_route;

use crate::gear_ladder::{wallet_ceiling, TOOL_SHARE_PCT};

/// Single tools of the kit (one copy each).
pub const KIT_SINGLES: [&str; 2] = ["hammer", "tinderbox"];
/// Tool families of the kit: gear family (`gear::usable_tools`) and its
/// canonical name (`tools::tool_family`).
pub const KIT_FAMILIES: [(&str, &str); 2] = [("pickaxe", "bronze_pickaxe"), ("axe", "bronze_axe")];
/// Canonical names of the whole kit (what the sales pass keeps).
pub const PERMANENT_KIT: [&str; 4] = ["hammer", "tinderbox", "bronze_pickaxe", "bronze_axe"];
/// Wallet share (percent) a missing kit tool may take at its cheapest
/// tier (bronze: 1 to 16 coins).
pub const KIT_BASE_SHARE_PCT: i64 = 50;

/// One kit purchase: the tool, its shop price, and whether it fills a
/// missing tool (base) or upgrades a working one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KitPurchase {
    pub item: String,
    pub price: i64,
    pub base: bool,
}

/// Shop price of a kit tool: its cost (Bob, Nurmof: multiplier 1000;
/// the general stores' 1300 rounds a 1-coin hammer down to 1), the price
/// the `shop:buy` leg checks the wallet against (`brain_plan::shop_floor`).
/// `None` without a shop route.
pub fn kit_price(item: &str) -> Option<i64> {
    if !kit_shop_route(item) {
        return None;
    }
    lifescape_gamedata::objs_by_name(item).into_iter().next().map(|obj| obj.price.max(1))
}

fn held(owned: &[String], name: &str) -> bool {
    owned.iter().any(|owned_name| owned_name == name)
}

/// Members of a kit family, best first (`tools::tool_family`).
fn members(canonical: &str) -> Vec<String> {
    crate::tools::tool_family(canonical)
}

/// Rank, in `usable` (best first), of the best held tool the server
/// works with: a pickaxe the mining level allows; any held axe (a held
/// axe above the woodcutting level ranks first). `None`: no working tool.
fn working_rank(family: &str, usable: &[String], owned: &[String], canonical: &str) -> Option<usize> {
    let axe_above_level = family == "axe" && members(canonical).iter().any(|name| held(owned, name) && !usable.contains(name));
    if axe_above_level {
        return Some(0);
    }
    usable.iter().position(|name| held(owned, name))
}

/// The kit purchase of one family: the best shop tier its level opens
/// above the working tool (module doc), within the wallet shares.
fn family_purchase(family: &str, canonical: &str, levels: &[(&str, i64)], owned: &[String], coins: i64) -> Option<KitPurchase> {
    let usable = usable_tools(family, levels);
    let rank = working_rank(family, &usable, owned, canonical);
    let above = &usable[..rank.unwrap_or(usable.len())];
    let priced: Vec<(String, i64)> = above
        .iter()
        .filter_map(|name| kit_price(name).map(|price| (name.clone(), price)))
        .collect();
    let base = rank.is_none();
    let upgrade_ceiling = wallet_ceiling(coins, TOOL_SHARE_PCT);
    if let Some((item, price)) = priced.iter().find(|(_, price)| *price <= upgrade_ceiling) {
        return Some(KitPurchase { item: item.clone(), price: *price, base });
    }
    if !base {
        return None;
    }
    let base_ceiling = wallet_ceiling(coins, KIT_BASE_SHARE_PCT);
    priced
        .iter()
        .filter(|(_, price)| *price <= base_ceiling)
        .min_by_key(|(_, price)| *price)
        .map(|(item, price)| KitPurchase { item: item.clone(), price: *price, base: true })
}

/// Every purchase the kit wants now, in order: base tools first
/// (hammer, tinderbox, pickaxe, axe), then upgrades. `owned`: pack, bank
/// and worn debug names; `coins`: the free wallet.
pub fn kit_purchases(levels: &[(&str, i64)], owned: &[String], coins: i64) -> Vec<KitPurchase> {
    let mut base = Vec::new();
    let base_ceiling = wallet_ceiling(coins, KIT_BASE_SHARE_PCT);
    for single in KIT_SINGLES {
        if held(owned, single) {
            continue;
        }
        if let Some(price) = kit_price(single).filter(|price| *price <= base_ceiling) {
            base.push(KitPurchase { item: single.to_string(), price, base: true });
        }
    }
    let mut upgrades = Vec::new();
    for (family, canonical) in KIT_FAMILIES {
        match family_purchase(family, canonical, levels, owned, coins) {
            Some(purchase) if purchase.base => base.push(purchase),
            Some(purchase) => upgrades.push(purchase),
            None => {}
        }
    }
    base.extend(upgrades);
    base
}

/// The next kit purchase (`kit_purchases`' first).
pub fn kit_purchase(levels: &[(&str, i64)], owned: &[String], coins: i64) -> Option<KitPurchase> {
    kit_purchases(levels, owned, coins).into_iter().next()
}

/// Held kit tools the bot keeps (module doc): per family the best held
/// member and the best working one, plus the held singles.
pub fn kit_keep(levels: &[(&str, i64)], owned: &[String]) -> Vec<String> {
    let mut keep: Vec<String> = KIT_SINGLES.iter().filter(|single| held(owned, single)).map(|single| single.to_string()).collect();
    for (family, canonical) in KIT_FAMILIES {
        let family_members = members(canonical);
        if let Some(best) = family_members.iter().find(|name| held(owned, name)) {
            keep.push(best.clone());
        }
        // Any held axe chops: the best held one is the working one.
        let usable = usable_tools(family, levels);
        if let Some(working) = usable.iter().find(|name| family == "pickaxe" && held(owned, name)) {
            keep.push(working.clone());
        }
    }
    keep.sort();
    keep.dedup();
    keep
}

#[cfg(test)]
#[path = "tool_kit_tests.rs"]
mod tests;
