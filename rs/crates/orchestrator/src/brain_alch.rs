#![forbid(unsafe_code)]
//! Alchemy in the brain (user 2026-11-14: "pourquoi personne n'a l'idée
//! d'alch des objets ?"). Pure rules in `lifescape_planner::alchemy`;
//! this file reads the bot's own state (magic level, staff, pack and
//! bank) and the PUBLIC market (rune prices, best bids, asks):
//! - money method: the bot's best alchemy (`alch:<object>`, profit per
//!   6000 ticks) is published in the node's `method_profit` beside the
//!   gathering and coin spots, only when the cast beats selling
//!   (`publish`), its runes gated by `tools::required_tool`;
//! - draw: a stock worth alching (net ≥ `BID_MIN_PROFIT`, rate ≥
//!   `BID_PREEMPT` × the bot's own coin rate, runes held) draws a
//!   `MakeMoney` goal of origin `alch:<object>` (`alch_goal_for`);
//! - sales: a caster never lists what alchs for more than it sells
//!   (`alch_keep`), never sells under the cast's net (`alch_floor`: the
//!   natural price floor of the market);
//! - market (`market_pass`, every `ALCH_EVERY_SECS`): nature runes for
//!   the stock to alch (market only: a combat spot drops them, D-A), and
//!   an ask under the cast's net less the bot's time is bought to alch.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use lifescape_json::Value;
use lifescape_planner::alchemy::{alch_bid_ceiling, best_alch, caster, AlchPick, Caster, FIRE_STAFF, NATURE_RUNE};
use lifescape_planner::{GoalKind, Life};

use crate::jobs::Job;

/// Prefix of an alchemy goal's origin.
pub const ALCH_PREFIX: &str = "alch:";
/// Wall seconds between two alchemy market passes for one account.
pub const ALCH_EVERY_SECS: f64 = 600.0;
/// Most units one alchemy purchase or rune bid asks for.
pub const ALCH_LOT: i64 = 100;

fn obj_id(item: &str) -> Option<i64> {
    lifescape_gamedata::objs_by_name(item).into_iter().next().map(|obj| obj.obj_id as i64)
}

/// Current price of one object (`MarketService::reference`), 1 while the
/// market is down.
fn current_price(item: &str) -> f64 {
    let (Some(service), Some(id)) = (crate::market_service::shared(), obj_id(item)) else { return 1.0 };
    service.reference(id).price.max(1) as f64
}

/// What selling one unit brings now: the best bid, else the current
/// price of a traded object, else nothing (no buyer, no trade).
fn sale_price(item: &str) -> f64 {
    let (Some(service), Some(id)) = (crate::market_service::shared(), obj_id(item)) else { return 0.0 };
    let bid = service.lock_engine().books.get(&id).and_then(|book| book.best().0).unwrap_or(0);
    let reference = service.reference(id);
    let traded = if reference.traded { reference.price } else { 0 };
    bid.max(traded).max(0) as f64
}

/// The bot as a caster (magic level, staff of fire held, runes at their
/// current market price), `None` under magic 21.
pub(crate) fn caster_for(live: &Value) -> Option<Caster> {
    let magic = crate::brain_succession::live_level(live, "magic");
    let staff = crate::brain_ambition::live_count(live, FIRE_STAFF) > 0;
    caster(magic, staff, &current_price)
}

/// Free stacks of the pack and bank by debug name (coins and runes out).
fn stacks_of(live: &Value) -> Vec<(String, i64)> {
    let mut totals: HashMap<String, i64> = HashMap::new();
    for key in ["inv", "bank"] {
        for entry in live.get(key).as_arr().cloned().unwrap_or_default() {
            let Some(name) = crate::brain_plan::live_debug_name(&entry) else { continue };
            let qty = entry.get("count").as_i64().unwrap_or(1);
            if qty > 0 && name != "coins" && !name.ends_with("rune") {
                *totals.entry(name).or_insert(0) += qty;
            }
        }
    }
    let mut out: Vec<(String, i64)> = totals.into_iter().collect();
    out.sort();
    out
}

/// Stock the bot could alch: `(object, free units, what one sells for)`,
/// minus what it keeps (`keep`: its plan's inputs, its gear, its tools).
fn alch_stock(live: &Value, account: &str, keep: &[String]) -> Vec<(String, i64, f64)> {
    let locks = crate::brain_locks::locked_items(account);
    stacks_of(live)
        .into_iter()
        .filter(|(name, _)| !keep.contains(name))
        .map(|(name, qty)| {
            let free = crate::brain_locks::free_count(qty, crate::brain_locks::locked_in(&locks, &name));
            (name, free)
        })
        .filter(|(name, free)| *free > 0 && lifescape_planner::alchemy::alch_value(name, true).is_some())
        .map(|(name, free)| {
            let price = sale_price(&name);
            (name, free, price)
        })
        .collect()
}

/// What the bot keeps out of alchemy: its plan's inputs and products,
/// its gear (best per slot and better), its graph's obtains.
fn kept(life: &Life, live: &Value) -> Vec<String> {
    let mut keep = crate::brain_sales::working_keep(life, &[]);
    let levels_owned = crate::brain_market::live_levels(live);
    let levels: Vec<(&str, i64)> = levels_owned.iter().map(|(name, level)| (name.as_str(), i64::from(*level))).collect();
    let quests = crate::brain_rescue::quests_done(life);
    let quests: Vec<&str> = quests.iter().map(String::as_str).collect();
    let owned: Vec<String> = stacks_of(live).into_iter().map(|(name, _)| name).collect();
    keep.extend(lifescape_planner::gear_plan::kept_gear(&levels, &quests, &owned));
    keep
}

/// The bot's best alchemy now (`None`: no caster, nothing beats selling).
pub(crate) fn best_for(life: &Life, live: &Value) -> Option<(Caster, AlchPick)> {
    let mage = caster_for(live)?;
    let keep = kept(life, live);
    let pick = best_alch(&mage, &alch_stock(live, &life.account, &keep))?;
    Some((mage, pick))
}

/// Publishes the bot's best alchemy as a money method of `ctx` (a money
/// or coin node): `alch:<object>` at its profit per 6000 ticks.
pub(crate) fn publish(ctx: &mut lifescape_planner::expand::ReadyCtx, node: &lifescape_planner::Node, life: &Life, live: &Value) {
    if !matches!(node.kind, lifescape_planner::NodeKind::MakeMoney | lifescape_planner::NodeKind::HaveCoins { .. }) {
        return;
    }
    if let Some((_, pick)) = best_for(life, live) {
        ctx.method_profit.push((format!("{ALCH_PREFIX}{}", pick.item), pick.profit_per_6000));
    }
}

/// Nature runes the bot holds (pack and bank).
fn natures_held(live: &Value) -> i64 {
    crate::brain_ambition::live_count(live, NATURE_RUNE)
}

/// The alchemy goal of a draw: the best stock to alch, worth the bot's
/// time like a bid (`bid_response::preempts` numbers), its nature runes
/// held for at least `MIN_ALCH_CASTS`. Origin `alch:<object>`: no
/// milestone, the ambition untouched.
pub(crate) fn alch_goal_for(life: &Life, live: &Value) -> Option<(GoalKind, String)> {
    let _phase = crate::cpu_profile::phase("alch.goal_for");
    let (_, pick) = best_for(life, live)?;
    let casts = pick.casts.min(natures_held(live));
    if casts < lifescape_planner::alchemy::MIN_ALCH_CASTS {
        return None;
    }
    let level_of = |skill: &str| crate::brain_succession::live_level(live, skill);
    let own_rate = lifescape_planner::market_fit::coin_rate(&crate::brain_bids::rate_rows(), &level_of);
    let rate = pick.profit_each / lifescape_planner::alchemy::CAST_TICKS;
    let profit = casts as f64 * (pick.profit_each - lifescape_planner::alchemy::CAST_TICKS * own_rate);
    let bid = lifescape_planner::bid_response::BID_PREEMPT;
    if rate < bid * own_rate.max(lifescape_planner::market_fit::MIN_COIN_RATE) || profit < lifescape_planner::bid_response::BID_MIN_PROFIT {
        return None;
    }
    crate::logln!(
        "brain {}: alchemy pays: {casts} {} at {:.0} net each ({:.1} coins/tick vs own {own_rate:.2}, {:.0} magic XP per 6000)",
        life.account,
        pick.item,
        pick.profit_each,
        rate,
        pick.xp_per_6000
    );
    Some((GoalKind::MakeMoney, format!("{ALCH_PREFIX}{}", pick.item)))
}

/// Objects a caster keeps for alchemy instead of listing them: their
/// cast nets more than selling brings now.
pub(crate) fn alch_keep(life: &Life, live: &Value) -> Vec<String> {
    let Some(mage) = caster_for(live) else { return Vec::new() };
    let keep = kept(life, live);
    alch_stock(live, &life.account, &keep)
        .into_iter()
        .filter(|(item, _, price)| mage.profit(item, *price).is_some_and(|profit| profit > 0.0))
        .map(|(item, _, _)| item)
        .collect()
}

/// The natural price floor of a caster's listing: the cast's net,
/// rounded up (`None`: no caster, or the cast loses).
pub(crate) fn alch_floor(live: &Value, item: &str) -> Option<i64> {
    let net = caster_for(live)?.net(item)?;
    (net >= 1.0).then(|| net.ceil() as i64)
}

fn last_pass() -> &'static Mutex<HashMap<String, f64>> {
    static TABLE: OnceLock<Mutex<HashMap<String, f64>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn wall_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs_f64()).unwrap_or(0.0)
}

/// The resting ask (another account's) furthest under the cast's net
/// less the bot's time: `(object, book id, ask, units resting,
/// ceiling)` (a banknote book bids on its own id).
fn alch_ask(mage: &Caster, account: &str, own_rate: f64) -> Option<(String, i64, i64, i64, i64)> {
    let service = crate::market_service::shared()?;
    let asks: Vec<(i64, i64, i64)> = {
        let engine = service.lock_engine();
        engine
            .books
            .iter()
            .filter_map(|(item_id, book)| {
                book.orders()
                    .into_iter()
                    .filter(|order| order.side == lifescape_market::Side::Sell && order.remaining > 0 && !order.account.eq_ignore_ascii_case(account))
                    .min_by_key(|order| order.price)
                    .map(|order| (*item_id, order.price, order.remaining))
            })
            .collect()
    };
    let mut best: Option<(String, i64, i64, i64, i64)> = None;
    for (item_id, ask, qty) in asks {
        let Some(item) = lifescape_gamedata::obj_by_id(usize::try_from(item_id).ok()?).and_then(|obj| obj.name).map(|name| name.strip_prefix("cert_").unwrap_or(name).to_string()) else { continue };
        let Some(ceiling) = alch_bid_ceiling(mage, &item, own_rate) else { continue };
        if ask > ceiling {
            continue;
        }
        if best.as_ref().is_none_or(|(_, _, known_ask, _, known_ceiling)| ceiling - ask > known_ceiling - known_ask) {
            best = Some((item, item_id, ask, qty, ceiling));
        }
    }
    best
}

/// One alchemy market pass for an idle, non-Iron caster: nature runes
/// for the stock it alchs, and an ask under the alch floor bought to
/// alch it. Returns the bids placed or kept.
pub(crate) fn market_pass(job: &Job, life: &Life, live: &Value) -> usize {
    if life.profile.ironman {
        return 0;
    }
    let now = wall_now();
    {
        let mut table = last_pass().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if table.get(&job.account).is_some_and(|last| now - last < ALCH_EVERY_SECS) {
            return 0;
        }
        table.insert(job.account.clone(), now);
    }
    let Some(service) = crate::market_service::shared() else { return 0 };
    let Some(mage) = caster_for(live) else { return 0 };
    let level_of = |skill: &str| crate::brain_succession::live_level(live, skill);
    let own_rate = lifescape_planner::market_fit::coin_rate(&crate::brain_bids::rate_rows(), &level_of);
    let coins = crate::brain_gear::free_coins(job, None);
    let natures = natures_held(live);
    let mut placed = 0;
    // Runes for the stock: a nature rune is worth its price plus half the
    // margin of the cast it enables.
    if let (Some((_, pick)), Some(id)) = (best_for(life, live), obj_id(NATURE_RUNE)) {
        let missing = (pick.casts - natures).clamp(0, ALCH_LOT);
        let ceiling = (current_price(NATURE_RUNE) + pick.profit_each / 2.0).floor() as i64;
        if missing > 0 && ceiling >= 1 && coins >= ceiling {
            let qty = missing.min(coins / ceiling.max(1));
            if let Ok(crate::market_reprice::StandingBid::Placed { price, .. }) = crate::market_reprice::keep_bid(&service, &job.account, id, qty, ceiling) {
                crate::logln!("brain {}: alchemy needs {qty} nature runes, bid @ {price} (ceiling {ceiling}) for {}", job.account, pick.item);
            }
            placed += 1;
        }
    }
    // Buy to alch: an ask under the floor, runes in hand.
    if natures >= lifescape_planner::alchemy::MIN_ALCH_CASTS {
        if let Some((item, id, ask, resting, ceiling)) = alch_ask(&mage, &job.account, own_rate) {
            let qty = resting.min(natures).min(ALCH_LOT).min(coins / ask.max(1));
            if qty >= 1 {
                if let Ok(crate::market_reprice::StandingBid::Placed { price, .. }) = crate::market_reprice::keep_bid(&service, &job.account, id, qty, ceiling) {
                    crate::logln!("brain {}: buys {qty} {item} @ {price} to alch (ask {ask}, alch floor {ceiling})", job.account);
                }
                placed += 1;
            }
        }
    }
    placed
}
