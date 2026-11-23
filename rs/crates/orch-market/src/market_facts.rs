#![forbid(unsafe_code)]
//! What the market engine asks of the bots' policy layer: whether an
//! account is a maker, its drawn price patience, and the public anchors
//! of a never-traded object. The engine cannot name the brain, so the
//! brain registers an `AccountFacts` at startup (`market_maker::
//! register_account_facts`, first thing in `main`, before any market
//! request). Unregistered, the answers are today's "no life" ones: not a
//! maker, no patience (the engine reads 0.5), no anchors (the caller's own
//! opening stands). Layering step A7, 2026-11-23.

use std::sync::{Arc, OnceLock};

use lifescape_json::Value;
use lifescape_market::discovery::Anchors;

use crate::market_service::MarketService;

pub trait AccountFacts: Send + Sync {
    /// True when `account`'s life is a market maker.
    fn is_maker(&self, account: &str) -> bool;
    /// The life's drawn `price_patience` (`None`: no life).
    fn price_patience(&self, account: &str) -> Option<f64>;
    /// Anchors a buyer reads for the never-traded `item` (debug name).
    fn public_anchors(&self, item: &str) -> Option<Anchors>;
    /// Reply to the `seller_quote` op (the seller's own listing for `qty`
    /// free units): the pricing policy is the brain's.
    fn seller_quote(&self, service: &MarketService, request: &Value) -> Value;
}

/// Today's answers for an account without a life: no maker, no patience,
/// no anchors.
pub struct NoLife;

impl AccountFacts for NoLife {
    fn is_maker(&self, _account: &str) -> bool {
        false
    }

    fn price_patience(&self, _account: &str) -> Option<f64> {
        None
    }

    fn public_anchors(&self, _item: &str) -> Option<Anchors> {
        None
    }

    fn seller_quote(&self, _service: &MarketService, _request: &Value) -> Value {
        MarketService::err("seller_quote: no seller policy registered".to_string())
    }
}

static FACTS: OnceLock<Arc<dyn AccountFacts>> = OnceLock::new();

/// Register the facts; the first registration wins.
pub fn register(facts: Arc<dyn AccountFacts>) {
    let _ = FACTS.set(facts);
}

pub fn is_registered() -> bool {
    FACTS.get().is_some()
}

/// The registered facts, else the no-life defaults.
fn facts() -> &'static dyn AccountFacts {
    static NONE: NoLife = NoLife;
    FACTS.get().map(|facts| facts.as_ref()).unwrap_or(&NONE)
}

pub fn is_maker(account: &str) -> bool {
    facts().is_maker(account)
}

pub fn price_patience(account: &str) -> Option<f64> {
    facts().price_patience(account)
}

pub fn public_anchors(item: &str) -> Option<Anchors> {
    facts().public_anchors(item)
}

pub fn seller_quote(service: &MarketService, request: &Value) -> Value {
    facts().seller_quote(service, request)
}

/// Test stub of the brain's facts (feature `test-support`, and this
/// crate's own tests): no maker, no patience, and the public anchors the
/// brain computes from gamedata and the planner while no market service
/// and no guide set are published (runes at 1 coin, no production cost).
/// Registered by the tests that price a never-traded object
/// (`test_support::register`); the same prices as the brain's facts gave
/// them before the inversion.
#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    use super::*;

    pub struct TestFacts;

    impl AccountFacts for TestFacts {
        fn is_maker(&self, _account: &str) -> bool {
            false
        }

        fn price_patience(&self, _account: &str) -> Option<f64> {
            None
        }

        fn public_anchors(&self, item: &str) -> Option<Anchors> {
            let alch_net = lifescape_planner::alchemy::caster(21, false, &|_: &str| 1.0).and_then(|caster| caster.net(item)).filter(|net| *net >= 1.0).map(|net| net.floor() as i64);
            let window = lifescape_planner::guide_model::WINDOW_TICKS as i64;
            let shop_buy = lifescape_gamedata::shop_buy::purchase_steps(item, window).into_iter().filter(|step| step.quest.is_none() && step.quest_points <= 0).map(|step| step.price).filter(|price| *price >= 1).min();
            Some(Anchors { shop_sale: lifescape_gamedata::shop_sale::npc_sale_value(item), alch_net, make_cost: None, shop_buy })
        }

        fn seller_quote(&self, _service: &MarketService, _request: &Value) -> Value {
            MarketService::err("seller_quote: no seller policy in this test".to_string())
        }
    }

    /// Registers the stub (the first registration wins, like the brain's).
    pub fn register() {
        super::register(Arc::new(TestFacts));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unregistered, the engine reads what it read for an account with no
    /// life: not a maker, no patience (0.5 downstream), no anchors.
    #[test]
    fn the_defaults_are_the_no_life_answers() {
        assert!(!NoLife.is_maker("anyone"));
        assert_eq!(NoLife.price_patience("anyone"), None);
        assert_eq!(NoLife.public_anchors("bronze_bar"), None);
    }
}
