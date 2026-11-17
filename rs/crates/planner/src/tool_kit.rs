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
//! bot earns, the next pass looks again).
//!
//! Since 2026-11-17 (user, live jim_ellis: an iron axe on yews for days
//! while mithril axes rested on the book): `kit_purchases_in` reads every
//! offer of a tier (`tool_upgrade::ToolOffer`: shop, best ask, own make),
//! so the target is the best tier ON OFFER the level opens, never the
//! absolute one (the patient wish bid on the tiers above stays, Q8).
//! An upgrade above the quarter of the wallet is still bought when it
//! pays for itself (`tool_upgrade::upgrade_pays`: ticks the planned work
//! saves > price in the bot's ticks): at once when the free wallet holds
//! the price, else `financed` (the step's obtain blocks on its coins, a
//! `have_coins` node earns them with the brain's money methods, then it
//! buys). Upgrades that do not pay are never funded.
//!
//! Keep (`kit_keep`): the best held member of each family (never sold,
//! even above the level: the bot grows into it), the best working one,
//! one hammer, one tinderbox. Lower tiers are surplus.

use lifescape_gamedata::gear::usable_tools;
use lifescape_gamedata::obtain::kit_shop_route;

use crate::gear_ladder::{wallet_ceiling, TOOL_SHARE_PCT};
use crate::model::{Life, NodeKind, NodeStatus};
use crate::tool_upgrade::{upgrade_pays, ToolOffer, ToolSource};

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

/// One kit purchase: the tool, its price and where (`source`), whether
/// it fills a missing tool (base) or upgrades a working one, whether a
/// money step funds it first (`financed`: price above the free wallet),
/// and the ticks the planned work saves with it (0: not counted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KitPurchase {
    pub item: String,
    pub price: i64,
    pub base: bool,
    pub source: ToolSource,
    pub financed: bool,
    pub saved_ticks: i64,
}

impl KitPurchase {
    fn bought(item: &str, offer: &ToolOffer, base: bool) -> KitPurchase {
        KitPurchase { item: item.to_string(), price: offer.price, base, source: offer.source.clone(), financed: false, saved_ticks: 0 }
    }
}

/// What the kit reads besides levels and holdings (module doc).
pub struct KitMarket<'a> {
    /// The cheapest offer of one tier now (`tool_upgrade::cheapest`).
    pub offer: &'a dyn Fn(&str) -> Option<ToolOffer>,
    /// Ticks the planned work saves going from the working tool (first)
    /// to a candidate (second) (`tool_upgrade::ticks_saved`).
    pub saved: &'a dyn Fn(&str, &str) -> f64,
    /// The bot's coins per tick.
    pub coin_rate: f64,
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

/// The kit purchase of one family: the best tier on offer its level
/// opens above the working tool (module doc), within the wallet shares,
/// or above them when it pays for itself.
fn family_purchase(family: &str, canonical: &str, levels: &[(&str, i64)], owned: &[String], coins: i64, market: &KitMarket) -> Option<KitPurchase> {
    let usable = usable_tools(family, levels);
    let rank = working_rank(family, &usable, owned, canonical);
    let above = &usable[..rank.unwrap_or(usable.len())];
    let priced: Vec<(String, ToolOffer)> = above.iter().filter_map(|name| (market.offer)(name).map(|offer| (name.clone(), offer))).collect();
    let base = rank.is_none();
    let upgrade_ceiling = wallet_ceiling(coins, TOOL_SHARE_PCT);
    if base {
        if let Some((item, offer)) = priced.iter().find(|(_, offer)| offer.price <= upgrade_ceiling) {
            return Some(KitPurchase::bought(item, offer, true));
        }
        let base_ceiling = wallet_ceiling(coins, KIT_BASE_SHARE_PCT);
        return priced
            .iter()
            .filter(|(_, offer)| offer.price <= base_ceiling)
            .min_by_key(|(_, offer)| offer.price)
            .map(|(item, offer)| KitPurchase::bought(item, offer, true));
    }
    // The working tool the gain is measured from: the best held member
    // (an axe above the level ranks first and leaves nothing above).
    let working = rank.and_then(|at| usable.get(at)).cloned().unwrap_or_default();
    for (item, offer) in &priced {
        if offer.price <= upgrade_ceiling {
            return Some(KitPurchase::bought(item, offer, false));
        }
        let saved = (market.saved)(&working, item);
        if upgrade_pays(saved, offer.price, market.coin_rate) {
            let mut purchase = KitPurchase::bought(item, offer, false);
            purchase.financed = offer.price > coins;
            purchase.saved_ticks = saved.round() as i64;
            return Some(purchase);
        }
    }
    None
}

/// Every purchase the kit wants now at the shops alone (no ask, no own
/// make, no gain counted): `kit_purchases_in` with shop offers.
pub fn kit_purchases(levels: &[(&str, i64)], owned: &[String], coins: i64) -> Vec<KitPurchase> {
    let offer = |item: &str| kit_price(item).map(|price| ToolOffer { price, source: ToolSource::Shop });
    let saved = |_: &str, _: &str| 0.0;
    kit_purchases_in(levels, owned, coins, &KitMarket { offer: &offer, saved: &saved, coin_rate: crate::market_fit::MIN_COIN_RATE })
}

/// Every purchase the kit wants now, in order: base tools first
/// (hammer, tinderbox, pickaxe, axe), then the upgrades the wallet pays,
/// then the financed ones. `owned`: pack, bank and worn debug names;
/// `coins`: the free wallet.
pub fn kit_purchases_in(levels: &[(&str, i64)], owned: &[String], coins: i64, market: &KitMarket) -> Vec<KitPurchase> {
    let mut base = Vec::new();
    let base_ceiling = wallet_ceiling(coins, KIT_BASE_SHARE_PCT);
    for single in KIT_SINGLES {
        if held(owned, single) {
            continue;
        }
        if let Some(offer) = (market.offer)(single).filter(|offer| offer.price <= base_ceiling) {
            base.push(KitPurchase::bought(single, &offer, true));
        }
    }
    let mut upgrades = Vec::new();
    let mut financed = Vec::new();
    for (family, canonical) in KIT_FAMILIES {
        match family_purchase(family, canonical, levels, owned, coins, market) {
            Some(purchase) if purchase.base => base.push(purchase),
            Some(purchase) if purchase.financed => financed.push(purchase),
            Some(purchase) => upgrades.push(purchase),
            None => {}
        }
    }
    base.extend(upgrades);
    base.extend(financed);
    base
}

/// True for a tool of the permanent kit (a hammer, a tinderbox, any
/// pickaxe or woodcutting axe).
pub fn kit_tool(item: &str) -> bool {
    KIT_SINGLES.contains(&item) || KIT_FAMILIES.iter().any(|(_, canonical)| members(canonical).iter().any(|name| name == item))
}

/// Wallet share (percent) the obtain of a gear step may spend: a kit
/// tool spends the free wallet the kit rule already checked (its funded
/// price is the coin target, never twice it), any other piece the gear
/// plan's half (`GEAR_SHARE_PCT`).
pub fn step_coin_share(item: &str) -> Option<i64> {
    (!kit_tool(item)).then_some(crate::gear_ladder::GEAR_SHARE_PCT)
}

/// The obtain of an open kit-tool step still waiting for its coins
/// (pending on its `have_coins`, or blocked), nothing of the step
/// running: `(index, item)`. A parked market buy is Ready, never here.
/// The brain withdraws it once the kit rule no longer names that tool
/// (its seller left, it no longer pays): never funded for nothing.
pub fn waiting_kit_step(life: &Life) -> Option<(usize, String)> {
    for (index, node) in life.nodes.iter().enumerate() {
        if !crate::gear_ladder::is_ladder_equip(node) || matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned) {
            continue;
        }
        let chain = crate::gear_ladder::step_chain(life, index);
        let members: Vec<&crate::model::Node> = life.nodes.iter().filter(|member| chain.contains(&member.id)).collect();
        if members.iter().any(|member| member.status == NodeStatus::Active) {
            continue;
        }
        let waiting = life.nodes.iter().enumerate().find(|(_, member)| {
            chain.contains(&member.id)
                && matches!(&member.kind, NodeKind::ObtainItems { item, .. } if kit_tool(item))
                && matches!(member.status, NodeStatus::Pending | NodeStatus::Blocked { .. })
        });
        if let Some((at, member)) = waiting {
            if let NodeKind::ObtainItems { item, .. } = &member.kind {
                return Some((at, item.clone()));
            }
        }
    }
    None
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
