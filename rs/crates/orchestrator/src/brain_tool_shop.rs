#![forbid(unsafe_code)]
//! Permanent tool kit in the brain (user decision 2026-11-14: every bot,
//! Iron included, owns an axe, a pickaxe, a hammer and a tinderbox, the
//! best tier its level opens, and keeps them across goals). The rule is
//! pure (`lifescape_planner::tool_kit`); this pass files its purchases.
//!
//! Once per `TOOL_SHOP_EVERY_SECS`, with the bank streamed (a banked tool
//! is never bought twice) and no gear step open, the next kit purchase
//! (`tool_kit::kit_purchase`: base kit first, cheapest tools first, then
//! upgrades; wallet shares of the FREE wallet `brain_gear::free_coins`)
//! becomes a gear step (`brain_gear::file_step`): an `obtain_items`
//! resolved `shop:buy` (Nurmof's pickaxes, the general stores' hammer and
//! tinderbox) or, for an axe, `market:buy` on a covered ask else Bob's
//! shop (D-A exception, `gamedata::obtain::kit_shop_route`), plus the
//! `equip_best` that closes the step. Launched before the goal's next
//! node, never blocking it. History: 2026-11-14 the pass bought only
//! pickaxe upgrades for bots whose work needed the family.
//!
//! Since 2026-11-17 (user, live jim_ellis): every tier's offer is read
//! (`KitView`: shop, best ask unless Iron, own make), the planned work of
//! each family and the bot's coin rate (`OwnCosts`); the rule
//! (`tool_kit::kit_purchases_in`) targets the best tier on offer and
//! funds an upgrade that pays for itself (`financed`: the step's obtain
//! blocks on its price, a `have_coins` node earns it). A kit step still
//! waiting for its coins that the rule no longer names is withdrawn
//! (`withdraw_unfunded`).
//!
//! Loops: one gear step at a time (`gear_ladder::step_open`); a dead
//! step cools its piece down (`WITHDRAWN_RETRY_SECS`, `withdraw_dead`);
//! its failure never bans a skill (`brain_succession`: gear subtree); a
//! shop piece is never withdrawn on an empty book (`brain_launch`). The
//! kit tools are kept by the sales pass (`brain_tools::kept_tools`), the
//! replaced lower tier sells, and the rule only buys above the best
//! working tier: no buy/sell loop. Resting market buys on shop-sold
//! tools (old wishes) are cancelled.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use lifescape_json::Value;
use lifescape_planner::gear_ladder;
use lifescape_planner::tool_kit::{KitMarket, KitPurchase};
use lifescape_planner::tool_upgrade::{PlannedWork, ToolOffer, ToolSource};
use lifescape_planner::{Life, NodeKind, NodeStatus};

use crate::jobs::Job;

/// Wall seconds between two kit passes for one account.
pub const TOOL_SHOP_EVERY_SECS: f64 = 60.0;

/// True for a gathering tool (pickaxe, woodcutting axe) the kit buys
/// through a step (an NPC shop sells it, `kit_shop_route`): never a
/// market wish bid.
pub fn merchant_tool(name: &str) -> bool {
    let tool = name.ends_with("_pickaxe") || (name.ends_with("_axe") && !name.contains("battleaxe"));
    tool && crate::brain_gear::shop_only(name)
}

/// True while an obtain of `item` runs (Active): the kit never buys a
/// second one beside it.
pub fn obtain_in_flight(life: &Life, item: &str) -> bool {
    life.nodes.iter().any(|node| node.status == NodeStatus::Active && matches!(&node.kind, NodeKind::ObtainItems { item: wanted, .. } if wanted == item))
}

fn wall_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs_f64()).unwrap_or(0.0)
}

fn last_pass() -> &'static Mutex<HashMap<String, f64>> {
    static TABLE: OnceLock<Mutex<HashMap<String, f64>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Cancels the account's resting buys on shop-sold tools (old wishlist
/// bids nobody fills), except what the graph's own obtains want.
fn cancel_merchant_tool_bids(job: &Job, life: &Life) {
    let Some(service) = crate::market_service::shared() else { return };
    let spared = crate::brain_wishlist::wanted_names(life);
    let rows = service.lock_store().account_orders(&job.account).unwrap_or_default();
    let mut names: Vec<String> = rows
        .iter()
        .filter(|row| crate::brain_gear::cancellable(row))
        .filter_map(|row| crate::brain_gear::name_of(row.item_id))
        .filter(|name| merchant_tool(name) && !spared.contains(name))
        .collect();
    names.sort();
    names.dedup();
    for name in &names {
        let closed = crate::brain_market::cancel_open_buys(&job.account, name);
        crate::logln!("brain {}: wish bid on {name} cancelled ({closed}): the kit buys it through a step", job.account);
    }
}

/// Best ask of one unit of `item` on the book (`None`: no seller).
fn best_ask(service: &crate::market_service::MarketService, item: &str) -> Option<i64> {
    let id = crate::brain_gear::obj_id(item)?;
    service.lock_engine().books.get(&id).and_then(|book| book.best().1)
}

/// Market price of an input the bot would make a tool from (`None`: a
/// guess, never a priced make).
fn input_price(store: &lifescape_store::Store, item: &str) -> Option<i64> {
    let (price, guessed) = crate::rates_price::price_estimate(store, &crate::brain_reserve::display_of(item));
    (!guessed).then_some(price.max(1.0).round() as i64)
}

/// Everything the kit rule reads this pass besides levels and holdings
/// (`tool_kit::KitMarket`): every tier's offer (shop, best ask unless
/// Iron, own make), the planned work of each family, the bot's rate.
struct KitView {
    offers: HashMap<String, Option<ToolOffer>>,
    axe_work: Vec<PlannedWork>,
    pickaxe_work: Vec<PlannedWork>,
    coin_rate: f64,
}

impl KitView {
    fn read(job: &Job, life: &Life, levels: &[(&str, i64)]) -> KitView {
        let level_of = |skill: &str| levels.iter().find(|(name, _)| *name == skill).map(|(_, level)| *level).unwrap_or(1);
        let level_u8 = |skill: &str| u8::try_from(level_of(skill).clamp(1, 99)).unwrap_or(1);
        let service = crate::market_service::shared();
        let mut offers = HashMap::new();
        let mut tiers: Vec<String> = lifescape_planner::tool_kit::KIT_SINGLES.iter().map(|single| single.to_string()).collect();
        for (_, canonical) in lifescape_planner::tool_kit::KIT_FAMILIES {
            tiers.extend(lifescape_planner::tools::tool_family(canonical));
        }
        for tier in tiers {
            let ask = service.as_ref().filter(|_| !life.profile.ironman).and_then(|service| best_ask(service, &tier));
            let make = service.as_ref().and_then(|service| {
                let store = service.lock_store();
                lifescape_planner::tool_upgrade::own_make(&tier, &level_of, &|input| input_price(&store, input))
            });
            let offer = lifescape_planner::tool_upgrade::cheapest(lifescape_planner::tool_kit::kit_price(&tier), ask, make);
            offers.insert(tier, offer);
        }
        let coin_rate = service
            .as_ref()
            .map(|service| crate::brain_reserve::OwnCosts::load(&service.lock_store(), &job.account).coins_per_tick)
            .unwrap_or(lifescape_planner::market_fit::MIN_COIN_RATE);
        KitView {
            offers,
            axe_work: lifescape_planner::tool_upgrade::planned_work(life, "bronze_axe", &level_u8),
            pickaxe_work: lifescape_planner::tool_upgrade::planned_work(life, "bronze_pickaxe", &level_u8),
            coin_rate,
        }
    }

    fn purchases(&self, levels: &[(&str, i64)], owned: &[String], coins: i64) -> Vec<KitPurchase> {
        let offer = |item: &str| self.offers.get(item).cloned().flatten();
        let saved = |from: &str, to: &str| {
            let work = if to.ends_with("_pickaxe") { &self.pickaxe_work } else { &self.axe_work };
            lifescape_planner::tool_upgrade::ticks_saved(work, from, to)
        };
        lifescape_planner::tool_kit::kit_purchases_in(levels, owned, coins, &KitMarket { offer: &offer, saved: &saved, coin_rate: self.coin_rate })
    }
}

/// A kit step still waiting for its coins whose tool the rule no longer
/// names (its seller left, it no longer pays) leaves the graph with its
/// money step (user 2026-11-17: never funded for nothing). Returns true
/// when the life changed.
fn withdraw_unfunded(job: &Job, life: &mut Life, purchases: &[KitPurchase]) -> bool {
    let Some((index, item)) = lifescape_planner::tool_kit::waiting_kit_step(life) else { return false };
    if purchases.iter().any(|purchase| purchase.item == item) {
        return false;
    }
    let removed = lifescape_planner::withdraw_gear_step(life, index);
    if removed.is_empty() {
        return false;
    }
    crate::logln!("brain {}: kit step {item} withdrawn ({}): no longer on offer or no longer paying", job.account, removed.join(", "));
    for id in &removed {
        crate::brain::push_history(life, id, "gear_withdrawn");
    }
    lifescape_planner::normalize(life);
    true
}

/// Price of the kit purchase the rule names next at this free wallet
/// (0: none, or the bank not streamed): the part of a donor's reserve
/// kept for its own tools (`funding_plan`).
pub(crate) fn next_kit_price(job: &Job, life: &Life, live: &Value) -> i64 {
    let levels_owned = crate::brain_tools::live_levels(live);
    if live.get("bank_streamed").as_bool() != Some(true) || levels_owned.is_empty() {
        return 0;
    }
    let levels = crate::brain_tools::level_refs(&levels_owned);
    let (owned, _) = crate::brain_gear::holdings(job);
    let coins = crate::brain_gear::free_coins(job, None);
    KitView::read(job, life, &levels).purchases(&levels, &owned, coins).first().map(|purchase| purchase.price.max(0)).unwrap_or(0)
}

/// One kit pass. Returns true when the life changed (save it).
pub(crate) fn tool_shop_pass(job: &Job, life: &mut Life, live: &Value) -> bool {
    let now = wall_now();
    {
        let mut table = last_pass().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if table.get(&job.account).is_some_and(|last| now - last < TOOL_SHOP_EVERY_SECS) {
            return false;
        }
        table.insert(job.account.clone(), now);
    }
    if !life.profile.ironman {
        cancel_merchant_tool_bids(job, life);
    }
    // A bank not streamed yet hides banked tools: never buy a second one.
    if live.get("bank_streamed").as_bool() != Some(true) {
        return false;
    }
    let levels_owned = crate::brain_tools::live_levels(live);
    if levels_owned.is_empty() {
        return false;
    }
    let levels = crate::brain_tools::level_refs(&levels_owned);
    let waiting = lifescape_planner::tool_kit::waiting_kit_step(life).is_some();
    if gear_ladder::step_open(life) && !waiting {
        return false;
    }
    let (owned, _) = crate::brain_gear::holdings(job);
    let coins = crate::brain_gear::free_coins(job, None);
    let view = KitView::read(job, life, &levels);
    let purchases = view.purchases(&levels, &owned, coins);
    if gear_ladder::step_open(life) {
        return withdraw_unfunded(job, life, &purchases);
    }
    let Some(purchase) = purchases
        .into_iter()
        .find(|purchase| !obtain_in_flight(life, &purchase.item) && !crate::brain_gear::recently_withdrawn(&job.account, &purchase.item, now))
    else {
        return false;
    };
    let kind = match (purchase.base, purchase.financed) {
        (true, _) => "missing kit tool",
        (false, false) => "kit upgrade",
        (false, true) => "kit upgrade, funded by a money step",
    };
    let why = format!(
        "{kind}, {:?} price {}, free wallet {coins}, saves {} ticks of planned work at {:.2} coins/tick",
        purchase.source, purchase.price, purchase.saved_ticks, view.coin_rate
    );
    let ids = crate::brain_gear::file_step(job, life, Some(&purchase.item), crate::brain_gear::goal_style(life), &why);
    // Own make: the step's obtain lists the bot's production leg too.
    if let (ToolSource::Make(method), Some(obtain)) = (&purchase.source, ids.first()) {
        if let Some(node) = life.nodes.iter_mut().find(|node| &node.id == obtain) {
            node.alternatives.push(lifescape_planner::Alternative { method: method.clone(), est_ticks: 0, est_coins: 0 });
        }
    }
    true
}

#[cfg(test)]
#[path = "brain_tool_shop_tests.rs"]
mod tests;
