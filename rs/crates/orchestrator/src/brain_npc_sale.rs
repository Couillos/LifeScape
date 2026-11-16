#![forbid(unsafe_code)]
//! NPC shop sales of the unsellable (plan marché Q3, user decision
//! 2026-11-16, an exception to D-A: `docs/decisions.md`). The selling
//! pass (`brain_sales`) keeps off the market every surplus lot an F2P
//! shop pays MORE for than the market (`lifescape_market::outlets`: the
//! best bid, the traded price; nothing for a never-traded object
//! without a bid) and hands the lots here. When the trip pays (the
//! gain over the market beats `SHOP_TRIP_TICKS` at the bot's own coin
//! rate) a sale plan waits for the janitor node (`SpaceCleanup`), whose
//! `market cleanup` run walks to the shop that pays the most in total
//! (its keeper site nearest the bot) and sells unit by unit, never a unit
//! at or under what the market pays (`--shopsell ITEM:QTY:MIN`).
//!
//! The coins the shops pay are new money: the coin ledger tags them
//! `shop_sold`, `orchestrator economy` prints them (Q3 tracking).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use lifescape_planner::{Life, Node, NodeKind, NodeStatus};

/// Ticks a shop trip costs (bank, keeper, back), an estimate in ticks:
/// the sale must gain more than this many ticks of the bot's own rate.
pub const SHOP_TRIP_TICKS: f64 = 1500.0;

/// One lot to sell: the object, its units, the least coins one unit may
/// fetch (market pays + 1) and what the shop pays for the first unit.
#[derive(Clone, Debug, PartialEq)]
pub struct SaleLine {
    pub item: String,
    pub qty: i64,
    pub min_unit: i64,
    pub value: i64,
}

/// One shop trip: the shop key, its keeper site, the lots.
#[derive(Clone, Debug, PartialEq)]
pub struct ShopSalePlan {
    pub shop: String,
    pub site: (i64, i64, i64),
    pub lines: Vec<SaleLine>,
}

impl ShopSalePlan {
    /// Coins gained over the market (first-unit value, an upper bound:
    /// the script stops at each line's minimum).
    pub fn gain(&self) -> i64 {
        self.lines.iter().map(|line| (line.value - line.min_unit + 1).max(0) * line.qty).sum()
    }

    /// `market cleanup` arguments of the trip.
    pub fn args(&self) -> Vec<String> {
        let mut args = vec!["--sale-shop".to_string(), self.shop.clone(), "--sale-site".to_string(), format!("{},{},{}", self.site.0, self.site.1, self.site.2)];
        for line in &self.lines {
            args.push("--shopsell".to_string());
            args.push(format!("{}:{}:{}", line.item, line.qty, line.min_unit));
        }
        args
    }
}

/// A candidate lot: object, free units, what the market pays per unit.
pub type Candidate = (String, i64, i64);

/// The trip (pure): per shop, the lots it pays more for than the market;
/// the shop with the largest gain, its keeper site nearest `position`
/// (x, z, level). `None` when nothing beats the market or the gain is
/// under `min_gain` coins.
pub fn plan_shop_sale(candidates: &[Candidate], position: Option<(i64, i64, i64)>, min_gain: i64, shops_of: &dyn Fn(&str) -> Vec<lifescape_gamedata::shop_sale::SaleShop>) -> Option<ShopSalePlan> {
    let mut by_shop: HashMap<String, (Vec<(i64, i64, i64)>, Vec<SaleLine>)> = HashMap::new();
    for (item, qty, pays) in candidates.iter().filter(|(_, qty, _)| *qty > 0) {
        for shop in shops_of(item).into_iter().filter(|shop| shop.value > *pays) {
            let entry = by_shop.entry(shop.shop.clone()).or_insert_with(|| (shop.sites.clone(), Vec::new()));
            entry.1.push(SaleLine { item: item.clone(), qty: *qty, min_unit: pays + 1, value: shop.value });
        }
    }
    let distance = |site: &(i64, i64, i64)| match position {
        Some((x, z, level)) => (site.0 - x).abs() + (site.1 - z).abs() + if site.2 == level { 0 } else { 1000 },
        None => 0,
    };
    let mut plans: Vec<ShopSalePlan> = by_shop
        .into_iter()
        .filter_map(|(shop, (sites, lines))| {
            let site = sites.iter().min_by_key(|site| distance(site)).copied()?;
            Some(ShopSalePlan { shop, site, lines })
        })
        .collect();
    plans.sort_by(|left, right| right.gain().cmp(&left.gain()).then_with(|| distance(&left.site).cmp(&distance(&right.site))).then_with(|| left.shop.cmp(&right.shop)));
    plans.into_iter().next().filter(|plan| plan.gain() >= min_gain.max(1))
}

fn pending() -> &'static Mutex<HashMap<String, ShopSalePlan>> {
    static TABLE: OnceLock<Mutex<HashMap<String, ShopSalePlan>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Files (or replaces) the account's pending trip.
pub fn note(account: &str, plan: ShopSalePlan) {
    pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(account.to_string(), plan);
}

/// Takes the account's pending trip (the janitor launch).
pub fn take(account: &str) -> Option<ShopSalePlan> {
    pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(account)
}

/// Drops a trip the last selling pass no longer backs.
pub fn forget(account: &str) {
    pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(account);
}

fn waiting(account: &str) -> bool {
    pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).contains_key(account)
}

/// Inserts a janitor node for a pending trip (none open, the janitor
/// not cooling down after a short run). Returns true when inserted.
pub fn maybe_insert(life: &mut Life, now: i64) -> bool {
    if !waiting(&life.account) || lifescape_planner::cleanup_cooling(&life.history, now) {
        return false;
    }
    let open = life.nodes.iter().any(|node| matches!(node.kind, NodeKind::SpaceCleanup) && !matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned));
    if open || lifescape_planner::janitor_stalled(life) {
        return false;
    }
    let id = lifescape_planner::next_id(life);
    lifescape_planner::insert_ready(life, Node::new(&id, NodeKind::SpaceCleanup, Vec::new(), 0));
    true
}

/// The bot's position from its live snapshot (`[level, x, z]`) as
/// (x, z, level).
pub fn live_position(live: &lifescape_json::Value) -> Option<(i64, i64, i64)> {
    crate::verify_assert::position_of(live).map(|(level, x, z)| (x, z, level))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lifescape_gamedata::shop_sale::SaleShop;

    fn shops(item: &str) -> Vec<SaleShop> {
        match item {
            "iron_dagger" => vec![
                SaleShop { shop: "generalshop1".to_string(), value: 14, sites: vec![(3212, 3246, 0)] },
                SaleShop { shop: "generalshop4".to_string(), value: 14, sites: vec![(2958, 3390, 0)] },
            ],
            "bronze_med_helm" => vec![SaleShop { shop: "generalshop1".to_string(), value: 9, sites: vec![(3212, 3246, 0)] }],
            _ => Vec::new(),
        }
    }

    /// Q3: only the lots the shop pays more for than the market go; the
    /// trip goes to the shop paying the most, nearest keeper first.
    #[test]
    fn the_trip_carries_only_what_the_market_pays_less_for() {
        let candidates = vec![("iron_dagger".to_string(), 40, 0), ("bronze_med_helm".to_string(), 80, 12), ("feather".to_string(), 5000, 1)];
        let plan = plan_shop_sale(&candidates, Some((3220, 3218, 0)), 100, &shops).expect("daggers pay the trip");
        assert_eq!(plan.shop, "generalshop1", "Lumbridge is nearer than Falador");
        assert_eq!(plan.lines, vec![SaleLine { item: "iron_dagger".to_string(), qty: 40, min_unit: 1, value: 14 }], "helms fetch 12 on the market: never sold for 9");
        assert_eq!(plan.args()[..4], ["--sale-shop", "generalshop1", "--sale-site", "3212,3246,0"]);
        assert!(plan.args().contains(&"iron_dagger:40:1".to_string()));
        assert_eq!(plan_shop_sale(&candidates, None, 10_000, &shops), None, "a trip that does not pay waits");
        assert_eq!(plan_shop_sale(&[("bronze_med_helm".to_string(), 80, 12)], None, 1, &shops), None);
    }
}
