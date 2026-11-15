#![forbid(unsafe_code)]
//! Priced training, brain side (2026-11-14: "smelt bars then smith what
//! pays, alch for magic like a player"). The pure ranking lives in the
//! planner (`training`); this file reads the bot's own state (live
//! levels, free holdings, wallet, staff, stock) and the PUBLIC market
//! (book views, current prices, other accounts' asks), then hands the
//! resolver its pick (`ReadyCtx::training`).
//!
//! Called once per launch of a smithing or magic (from 21) climb, right
//! before `resolve_ready` (never on a brain tick without a launch: the
//! book is read once, every acquisition cost memoized for the pass).
//! No market service: no pick, the static tranche method runs.
//!
//! Prices handed to the planner:
//! - held units sell for the best bid or the current price (Iron: 0);
//! - a unit to acquire costs `bid_response::unit_cost` (cheapest asks
//!   that hold the units, or making it, time at the bot's coin rate),
//!   else the current price (a standing bid fills there), else an NPC
//!   shop when D-A allows it (a staff of fire), else nothing;
//! - a product is worth its current price times the share of the batch
//!   the book absorbs (`market_fit::saturation_factor`), or the alchemy
//!   net when the bot casts and that pays more;
//! - alchemy supply: the bot's own free stock (plan inputs and kept gear
//!   out) and other accounts' asks at or under the alchemy value.

use std::cell::RefCell;
use std::collections::HashMap;

use lifescape_json::Value;
use lifescape_planner::alchemy::Caster;
use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::market_fit::{coin_rate, saturation_factor};
use lifescape_planner::training::{best_pick, priced_at_resolve, Acquired, AlchSupply, TrainPick, TrainPrices};
use lifescape_planner::{Life, Node, NodeKind};

use crate::brain_bids::{BotState, MarketReader};

/// Objects of other accounts' asks a magic pick weighs at most.
pub const MAX_ASK_OBJECTS: usize = 20;

/// What the pick reads beyond the bot's state: the book, current prices
/// of traded objects, the bot as a caster, the alchemy supply, its free
/// wallet.
pub(crate) struct TrainMarket<'a> {
    pub reader: &'a MarketReader,
    /// Current price of an object that traded (`None`: never traded).
    pub traded: &'a dyn Fn(&str) -> Option<f64>,
    pub mage: Option<Caster>,
    pub supply: Vec<AlchSupply>,
    pub coins: i64,
}

/// Pure choice for one climb (`skill` at the live `level` toward
/// `target`): the planner's best priced method for this bot.
pub(crate) fn choose_pick(skill: &str, level: u8, target: u8, life: &Life, state: &BotState, rows: &[(String, i64, f64)], market: &TrainMarket) -> Option<TrainPick> {
    let rate = coin_rate(rows, state.level_of);
    let ironman = life.profile.ironman;
    let view = |item: &str| market.reader.view(item);
    let best_bid = |item: &str| view(item).and_then(|book| book.best_bid()).map(|price| price as f64);
    let sale_value = |item: &str| {
        if ironman {
            return 0.0;
        }
        best_bid(item).unwrap_or(0.0).max((market.traded)(item).unwrap_or(0.0))
    };
    let memo: RefCell<HashMap<(String, i64), Option<Acquired>>> = RefCell::new(HashMap::new());
    let acquire = |item: &str, units: i64| {
        let key = (item.to_string(), units);
        if let Some(known) = memo.borrow().get(&key) {
            return known.clone();
        }
        let found = state
            .with_producer(life, rate, |producer| lifescape_planner::bid_response::unit_cost(item, units, &view, producer))
            .map(|unit| Acquired { coins_each: unit.total(rate), made_by: unit.plan.first().and_then(|step| step.strip_prefix("make ")).map(str::to_string) })
            .or_else(|| (!ironman).then(|| (market.traded)(item)).flatten().map(|price| Acquired { coins_each: price, made_by: None }))
            .or_else(|| {
                lifescape_gamedata::obtain::merchant_buy_allowed(item)
                    .then(|| lifescape_gamedata::objs_by_name(item).into_iter().next().map(|obj| Acquired { coins_each: obj.price.max(1) as f64, made_by: None }))
                    .flatten()
            });
        memo.borrow_mut().insert(key, found.clone());
        found
    };
    let product_value = |item: &str, units: i64| {
        let alch = market.mage.as_ref().and_then(|mage| mage.net(item)).unwrap_or(0.0);
        if ironman {
            return alch.max(0.0);
        }
        let book = view(item);
        let sale = (market.traded)(item).or_else(|| best_bid(item)).unwrap_or(0.0) * saturation_factor(book.as_ref(), units as f64);
        sale.max(alch).max(0.0)
    };
    let prices = TrainPrices { coin_rate: rate, coins: market.coins, held: state.held, sale_value: &sale_value, acquire: &acquire, product_value: &product_value };
    best_pick(skill, level, target, &prices, &market.supply)
}

/// Current price of an object that traded (median of its last fills).
fn traded_price(item: &str) -> Option<f64> {
    let service = crate::market_service::shared()?;
    let id = crate::brain_alch::obj_id(item)?;
    let reference = service.reference(id);
    reference.traded.then_some(reference.price.max(1) as f64)
}

/// Alchemy supply of a caster: its own free stock, then others' asks at
/// or under the alchemy value (never for Iron).
fn alch_supply(life: &Life, live: &Value, mage: Option<&Caster>) -> Vec<AlchSupply> {
    let Some(mage) = mage else { return Vec::new() };
    let keep = crate::brain_alch::kept(life, live);
    let mut out: Vec<AlchSupply> = crate::brain_alch::alch_stock(live, &life.account, &keep)
        .into_iter()
        .map(|(item, units, price)| AlchSupply { item, units, unit_cost: if life.profile.ironman { 0.0 } else { price }, own: true })
        .collect();
    if !life.profile.ironman {
        // The widest margins under the alchemy value first, a bounded few
        // (one pricing per object at each launch).
        let mut asks: Vec<AlchSupply> = crate::brain_alch::cheap_asks(mage, &life.account)
            .into_iter()
            .filter(|(item, units, _)| *units > 0 && !out.iter().any(|known| known.item == *item))
            .map(|(item, units, coins)| AlchSupply { item, units, unit_cost: coins / units as f64, own: false })
            .collect();
        let margin = |offer: &AlchSupply| lifescape_planner::alchemy::alch_value(&offer.item, mage.spell.high).unwrap_or(0) as f64 - offer.unit_cost;
        asks.sort_by(|left, right| margin(right).total_cmp(&margin(left)).then_with(|| left.item.cmp(&right.item)));
        asks.truncate(MAX_ASK_OBJECTS);
        out.extend(asks);
    }
    out
}

/// Fills `ctx.training` for a smithing or magic climb about to resolve
/// (module doc), logging the pick with its numbers.
pub(crate) fn fill_ctx(ctx: &mut ReadyCtx, node: &Node, life: &Life, live: &Value) {
    let NodeKind::ReachLevel { skill, level: target } = &node.kind else { return };
    if node.method.is_some() || crate::market_service::shared().is_none() {
        return;
    }
    let level = crate::brain_succession::live_level(live, skill);
    if !priced_at_resolve(skill, level) || level >= *target {
        return;
    }
    let reader = MarketReader::live(&life.account);
    let mage = if skill == "magic" { crate::brain_alch::caster_for(live) } else { None };
    let supply = alch_supply(life, live, mage.as_ref());
    // Products are valued as alchemy only by a caster (any skill).
    let mage = mage.or_else(|| crate::brain_alch::caster_for(live));
    let market = TrainMarket { reader: &reader, traded: &traded_price, mage, supply, coins: ctx.coins };
    let rows = crate::brain_bids::rate_rows();
    let pick = crate::brain_bids::with_bot_state(life, live, |state| choose_pick(skill, level, *target, life, state, &rows, &market));
    match &pick {
        Some(found) => crate::logln!(
            "brain {}: {} {skill} {level}->{target} trains by {} ({:+.2} coins/tick, {:.2} coins/XP, {:.1} XP/tick{}{})",
            life.account,
            node.id,
            found.method,
            found.profit_per_tick,
            found.cost_per_xp,
            found.xp_per_tick,
            if found.batch > 0 { format!(", batch {}", found.batch) } else { String::new() },
            if found.missing.is_empty() { String::new() } else { format!(", obtains {}", found.missing.join(",")) }
        ),
        None => crate::logln!("brain {}: {} {skill} {level}->{target} has no priced method, static tranche", life.account, node.id),
    }
    ctx.training = pick;
}

#[cfg(test)]
#[path = "brain_train_tests.rs"]
mod tests;
