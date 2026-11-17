#![forbid(unsafe_code)]
//! Public discovery anchors of an object as the daemon reads them (plan
//! marché M7, `lifescape_market::discovery`): only public data (D22/D11),
//! never another bot's state:
//! - `shop_sale`: the best unit an F2P shop open to every bot pays (Q3,
//!   server `calc_shop_value`, `shop_sale::npc_sale_value`);
//! - `alch_net`: a SELLER's own cast (its spell, high or low by its
//!   magic level, runes at their current price: `brain_alch::alch_floor`);
//!   for a buyer or a market maker, who cannot know which seller casts,
//!   the low alchemy (magic 21, the alchemy every caster reaches), runes
//!   at their current price;
//! - `make_cost`: the cheapest production cost the last published guide
//!   set prices (`planner::guide_cost`, cached per build);
//! - `shop_buy`: the cheapest first unit an F2P shop open to every bot
//!   sells (no quest, no quest points: `shop_buy::purchase_steps`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use lifescape_market::Anchors;

/// Magic level of the low alchemy (`alchemy::alch_spell`).
pub const LOW_ALCH_LEVEL: u8 = 21;

/// Published production costs of the guide set built at `built_unix_ms`.
struct CostCache {
    built_unix_ms: i64,
    costs: Arc<HashMap<String, f64>>,
}

fn cost_cache() -> &'static Mutex<Option<CostCache>> {
    static CACHE: OnceLock<Mutex<Option<CostCache>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Production costs of the last published guide set (empty before the
/// first build), recomputed once per build.
fn make_costs() -> Arc<HashMap<String, f64>> {
    let Some(set) = crate::guides_store::current() else { return Arc::new(HashMap::new()) };
    let mut cache = cost_cache().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(known) = cache.as_ref().filter(|known| known.built_unix_ms == set.built_unix_ms) {
        return known.costs.clone();
    }
    let costs = Arc::new(lifescape_planner::guide_cost::make_costs(&set.money_catalog.rows));
    *cache = Some(CostCache { built_unix_ms: set.built_unix_ms, costs: costs.clone() });
    costs
}

/// The published production cost of one unit of `item` (debug name),
/// rounded up to a coin.
pub fn make_cost(item: &str) -> Option<i64> {
    make_costs().get(item).map(|cost| cost.ceil() as i64).filter(|cost| *cost >= 1)
}

/// Cheapest first unit an F2P shop open to every bot sells `item` at
/// (cached: the dumps never change in a daemon's life).
pub fn shop_buy(item: &str) -> Option<i64> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<i64>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(known) = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(item) {
        return *known;
    }
    let window = lifescape_planner::guide_model::WINDOW_TICKS as i64;
    let found = lifescape_gamedata::shop_buy::purchase_steps(item, window).into_iter().filter(|step| step.quest.is_none() && step.quest_points <= 0).map(|step| step.price).filter(|price| *price >= 1).min();
    cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(item.to_string(), found);
    found
}

/// Current price of a rune (`MarketService::reference`), 1 while the
/// market is down.
fn rune_price(item: &str) -> f64 {
    let (Some(service), Some(id)) = (crate::market_service::shared(), crate::brain_alch::obj_id(item)) else { return 1.0 };
    service.reference(id).price.max(1) as f64
}

/// Net of one LOW alchemy cast on `item`, runes at their current price
/// (`None`: not alchable, or the cast loses).
pub fn public_alch_net(item: &str) -> Option<i64> {
    let caster = lifescape_planner::alchemy::caster(LOW_ALCH_LEVEL, false, &rune_price)?;
    let net = caster.net(item)?;
    (net >= 1.0).then(|| net.floor() as i64)
}

/// Anchors a buyer or a market maker reads for `item`.
pub fn public_anchors(item: &str) -> Anchors {
    Anchors {
        shop_sale: lifescape_gamedata::shop_sale::npc_sale_value(item),
        alch_net: public_alch_net(item),
        make_cost: make_cost(item),
        shop_buy: shop_buy(item),
    }
}

/// Anchors a seller reads for `item`: its own cast's net (`own_alch`,
/// `None` when it casts no alchemy) in place of the public one.
pub fn seller_anchors(item: &str, own_alch: Option<i64>) -> Anchors {
    Anchors {
        shop_sale: lifescape_gamedata::shop_sale::npc_sale_value(item),
        alch_net: own_alch,
        make_cost: make_cost(item),
        shop_buy: shop_buy(item),
    }
}

/// Profile margin of `account` (`discovery::margin_of` of its drawn
/// price patience, the planner's `Profile::margin`).
pub fn account_margin(service: &crate::market_service::MarketService, account: &str) -> f64 {
    lifescape_market::discovery::margin_of(crate::market_reprice::account_patience(service, account))
}

/// Opening bid of `account` on the never-traded `item` (debug name):
/// `discovery::bid_open` on the public anchors (`None`: no anchor, the
/// caller's own opening stands).
pub fn buyer_open(service: &crate::market_service::MarketService, account: &str, item: &str, value: Option<i64>, ceiling: i64) -> Option<i64> {
    lifescape_market::discovery::bid_open(&public_anchors(item), value, ceiling, account_margin(service, account))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The margin the market reads is the planner's profile margin.
    #[test]
    fn the_margin_is_the_profiles() {
        for patience in [0.0, 0.3, 0.5, 0.99] {
            let profile = lifescape_planner::Profile::new(false, 0.5, patience, 7, lifescape_planner::Archetype::Producer);
            assert!((lifescape_market::discovery::margin_of(patience) - profile.margin).abs() < 1e-12, "{patience}");
        }
    }

    /// Public anchors come from the dumps (shops) and the alchemy table.
    #[test]
    fn anchors_read_the_game_data() {
        // Bronze bars: the general store buys them, no F2P shop sells them.
        let bar = public_anchors("bronze_bar");
        assert!(bar.shop_sale.is_some_and(|coins| coins >= 1), "{bar:?}");
        // A tinderbox: general stores sell it to everyone.
        assert!(shop_buy("tinderbox").is_some_and(|coins| coins >= 1));
        // No guide set built in tests: no published cost, never a guess.
        assert_eq!(make_cost("bronze_bar"), None);
        // A seller that casts no alchemy has no cast in its floor.
        assert_eq!(seller_anchors("bronze_bar", None).alch_net, None);
    }
}
