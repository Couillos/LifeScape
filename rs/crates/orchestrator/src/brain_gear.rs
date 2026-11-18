#![forbid(unsafe_code)]
//! Equipment in the brain (user 2026-11-14: "les bots qui font de la
//! mêlée devraient vouloir le meilleur matériel pour leur combat lvl";
//! pure rules in `lifescape_planner::gear_plan`, the ladder module keeps
//! the step mechanics). User 2026-11-14: every bot owns at least one
//! complete combat set, the best it affords, never sold (`ladder_keep`).
//! Once per `GEAR_EVERY_SECS` for an idle bot, whatever its goal's style
//! (melee, ranged, magic; non-combat goals fight melee):
//! - a dead gear step (abandoned) leaves the graph on EVERY pass, any
//!   goal style (`withdraw_dead`, before `graph_state`: never a Stuck goal);
//! - an owned piece better than the worn one → an `equip_best` step;
//! - every slot (weapon, body, legs, helm, shield, boots, gloves, amulet,
//!   cape) wants the best piece the bot's levels open that its gear
//!   budget affords (`gear_plan::plan_gear`): budget = `GEAR_SHARE_PCT`
//!   of the wallet left free by its non-gear buys; expected price = the
//!   median of the last five fills, else the high alchemy value; each
//!   want bids its budget share (`keep_bid`: opens at the current price,
//!   climbs with patience), ALL slots at once (live 2026-11-14: one
//!   bronze scimitar at a time for level 50 fighters, nothing else);
//! - a want with a resting ask under its ceiling, or one only an NPC
//!   shop sells (D-A), becomes an `obtain_items` + `equip_best` step
//!   (one open step at a time), launched before the goal's next node;
//! - bids on wearables the plan no longer wants are cancelled (a slot
//!   upgraded past it, a tier the budget dropped);
//! - the plan is HELD between passes (plan marché M2, C2:
//!   `brain_gear_hold`): smoothed budget, a slot keeps its piece down to
//!   70 % of its initial ceiling, a full replan at most once per 36 000
//!   ticks or on a tier gain: a wallet that swings no longer cancels and
//!   re-places bids every 60 s;
//! - a want whose ceiling is under the object's public floor bids
//!   nothing (§3.6 credibility, C5);
//! - an Iron (no market) plans only the pieces an NPC shop sells (D-A,
//!   `shop_only`), priced at the shop, and files them as steps;
//! - a market bot plans only pieces obtainable NOW (user 2026-11-18,
//!   `obtainable_now`: a resting ask, an open shop, a way a fleet bot has
//!   the level for), never a tier nobody sells or can make.
//!
//! Tools are the permanent kit's (`brain_tool_shop`, steps through
//! `file_step`: same cooldown, same no-ban rule); smithed-only axe tiers
//! stay with the wishlist (`brain_wishlist`).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use lifescape_json::Value;
use lifescape_gamedata::gear::Style;
use lifescape_planner::{gear_ladder, gear_plan};
use lifescape_planner::Life;

use crate::brain::push_history;
use crate::jobs::Job;

/// Wall seconds between two ladder passes for one account.
pub const GEAR_EVERY_SECS: f64 = 60.0;
/// Wall seconds before an `equip_best` step for the same piece is filed
/// again (a wear that did not take never loops).
pub const EQUIP_RETRY_SECS: f64 = 3600.0;

fn wall_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs_f64()).unwrap_or(0.0)
}

fn last_pass() -> &'static Mutex<HashMap<String, f64>> {
    static TABLE: OnceLock<Mutex<HashMap<String, f64>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn last_equip() -> &'static Mutex<HashMap<(String, String), f64>> {
    static TABLE: OnceLock<Mutex<HashMap<(String, String), f64>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A step withdrawn for lack of a seller is not filed again for this long:
/// the standing bid waits for a producer and the bot works its goal
/// meanwhile (live 2026-11-14 08:05: a step filed then withdrawn on every
/// plain tick held 25 bots idle).
pub const WITHDRAWN_RETRY_SECS: f64 = 1800.0;

fn last_withdrawn() -> &'static Mutex<HashMap<(String, String), f64>> {
    static TABLE: OnceLock<Mutex<HashMap<(String, String), f64>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Records a gear step withdrawn for lack of a seller (see `WITHDRAWN_RETRY_SECS`).
pub(crate) fn note_withdrawn(account: &str, item: &str) {
    let mut table = last_withdrawn().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    table.insert((account.to_string(), item.to_string()), wall_now());
}

pub(crate) fn recently_withdrawn(account: &str, item: &str, now: f64) -> bool {
    let table = last_withdrawn().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    table.get(&(account.to_string(), item.to_string())).is_some_and(|last| now - last < WITHDRAWN_RETRY_SECS)
}

/// Combat style of the life's goal (non-combat goals fight melee).
pub fn goal_style(life: &Life) -> Style {
    Style::parse(lifescape_planner::goal_combat_style(&life.goal))
}

/// Still true for melee and non-combat goals (callers that only speak of
/// the melee ladder: the wishlist keeps tools only either way now).
pub fn ladder_goal(life: &Life) -> bool {
    goal_style(life) == Style::Melee
}

pub(crate) fn obj_id(item: &str) -> Option<i64> {
    lifescape_gamedata::objs_by_name(item).into_iter().next().map(|obj| obj.obj_id as i64)
}

/// Owned debug names (pack, bank, worn) and worn names.
pub(crate) fn holdings(job: &Job) -> (Vec<String>, Vec<String>) {
    let worn = crate::brain_plan::worn_names(job);
    let mut owned: Vec<String> = crate::brain_plan::live_stacks(job).into_iter().map(|(name, _)| name).collect();
    owned.extend(worn.iter().cloned());
    (owned, worn)
}

/// Gear the sales pass must never list: per style and slot, the best
/// piece the bot may wear and any better one (`gear_plan::kept_gear`).
/// Replaced pieces are surplus: sold (or alched) by the default sales.
pub fn ladder_keep(job: &Job, life: &Life, live: &Value) -> Vec<String> {
    let levels_owned = crate::brain_market::live_levels(live);
    let levels: Vec<(&str, i64)> = levels_owned.iter().map(|(name, level)| (name.as_str(), i64::from(*level))).collect();
    let quests = crate::brain_rescue::quests_done(life);
    let quests: Vec<&str> = quests.iter().map(String::as_str).collect();
    let (owned, _) = holdings(job);
    gear_plan::kept_gear(&levels, &quests, &owned)
}

/// A wearable bid the gear plan owns (tools stay with the wishlist).
fn gear_wearable(name: &str) -> bool {
    let tool = name.ends_with("_pickaxe") || (name.ends_with("_axe") && !name.contains("battleaxe"));
    !tool && lifescape_gamedata::equipment::by_name(name).is_some()
}

pub(crate) fn name_of(item_id: i64) -> Option<String> {
    lifescape_gamedata::obj_by_id(usize::try_from(item_id).ok()?).and_then(|obj| obj.name).map(str::to_string)
}

/// A buy still resting on the book: open, or partly matched with a
/// remainder (review 2026-11-13: `Matched` rows escaped the cancels).
pub(crate) fn cancellable(row: &lifescape_store::Order) -> bool {
    row.side == lifescape_store::OrderSide::Buy
        && matches!(row.status, lifescape_store::OrderStatus::Open | lifescape_store::OrderStatus::Matched)
        && row.remaining > 0
}

/// Coins the resting buys commit (price × remainder), except those on
/// `except` (a bid never counts against its own ceiling).
pub(crate) fn committed_from(rows: &[lifescape_store::Order], except: Option<i64>) -> i64 {
    rows.iter()
        .filter(|row| cancellable(row) && Some(row.item_id) != except)
        .map(|row| crate::market_units::deal_coins(row.item_id, row.remaining, row.price.max(0)))
        .sum()
}

/// Coins the account's in-flight buyer deals promise: matched, not yet
/// handed over, still in its bank (live 2026-11-13, bd23dqpq 1643-1652,
/// bct5rxpg 1772: a bid swept the book, the next one sized itself on the
/// same purse, the handover came `short`). Every item counts, `except`
/// included: those coins are spent whatever the next bid.
pub(crate) fn deal_coins_from(account: &str, deals: &[lifescape_store::Deal]) -> i64 {
    deals.iter().filter(|deal| deal.buyer == account).map(|deal| crate::market_units::deal_coins(deal.item_id, deal.quantity.max(0), deal.price.max(0))).sum()
}

/// Coins this account's resting buys commit, except on `except`, plus
/// what its in-flight buyer deals promise (0 while the market service is
/// down: nothing rests then).
pub(crate) fn committed_coins(account: &str, except: Option<i64>) -> i64 {
    let Some(service) = crate::market_service::shared() else { return 0 };
    let (rows, deals) = {
        let store = service.lock_store();
        (store.account_orders(account).unwrap_or_default(), store.open_deals_for_account(account).unwrap_or_default())
    };
    // The coins a donor owes the makers (`fund-makers`) are spent too.
    committed_from(&rows, except) + deal_coins_from(account, &deals) + service.gifts_owed(account)
}

/// The wallet a new ceiling may share (review 2026-11-13, finding 6):
/// coins held minus what this bot's other resting buys already commit, so
/// gear (50 %), tools (25 % each) and the graph's buys never promise more
/// than the wallet holds.
pub(crate) fn free_coins(job: &Job, except: Option<i64>) -> i64 {
    (crate::brain_plan::holdings_count(job, "coins") - committed_coins(&job.account, except)).max(0)
}

/// The wallet the gear budget shares: coins minus what the bot's
/// NON-gear resting buys and in-flight deals commit (the gear bids are
/// re-shared by the plan itself, never counted against it).
fn gear_free_coins(job: &Job) -> i64 {
    let coins = crate::brain_plan::holdings_count(job, "coins");
    let Some(service) = crate::market_service::shared() else { return coins.max(0) };
    let store = service.lock_store();
    let rows = store.account_orders(&job.account).unwrap_or_default();
    let deals = store.open_deals_for_account(&job.account).unwrap_or_default();
    drop(store);
    let other: Vec<lifescape_store::Order> = rows.into_iter().filter(|row| !name_of(row.item_id).is_some_and(|name| gear_wearable(&name))).collect();
    (coins - committed_from(&other, None) - deal_coins_from(&job.account, &deals)).max(0)
}

/// Expected price of a piece: the median of its last five fills (the
/// bot's own resting bid never moves it), else its high alchemy value
/// (the natural floor), at least 1.
fn expected_price(service: Option<&std::sync::Arc<crate::market_service::MarketService>>, item: &str) -> i64 {
    let traded = service.zip(obj_id(item)).and_then(|(service, id)| lifescape_market::median_price(&service.last_fill_prices(id)));
    traded.unwrap_or_else(|| lifescape_gamedata::objs_by_name(item).into_iter().next().map(|obj| obj.alch_high).unwrap_or(1)).max(1)
}

/// A piece the bot can obtain now (`gear_plan::obtainable_now`): a
/// resting ask, a shop route its quest points open, or a way a fleet bot
/// has the level for (`brain_gear_fleet`).
fn obtainable_now(service: Option<&std::sync::Arc<crate::market_service::MarketService>>, item: &str, quests: &[&str], now: f64) -> bool {
    let has_ask = service.zip(obj_id(item)).is_some_and(|(service, id)| service.lock_engine().books.get(&id).and_then(|book| book.best().1).is_some());
    gear_plan::obtainable_now(item, has_ask, gear_plan::shop_route_open(item, quests), &|skill| crate::brain_gear_fleet::best(skill, now))
}

/// What the gear plan would buy next at this wallet (its wants at their
/// expected prices, same budget as `gear_pass`): the part of a donor's
/// reserve kept for its own gear (`funding_plan`). An Iron buys none.
pub(crate) fn next_gear_cost(job: &Job, life: &Life, live: &Value) -> i64 {
    let levels_owned = crate::brain_market::live_levels(live);
    if life.profile.ironman || levels_owned.is_empty() {
        return 0;
    }
    let levels: Vec<(&str, i64)> = levels_owned.iter().map(|(name, level)| (name.as_str(), i64::from(*level))).collect();
    let quests_owned = crate::brain_rescue::quests_done(life);
    let quests: Vec<&str> = quests_owned.iter().map(String::as_str).collect();
    let (owned, _) = holdings(job);
    let service = crate::market_service::shared();
    let budget = gear_ladder::wallet_ceiling(gear_free_coins(job), gear_ladder::GEAR_SHARE_PCT);
    let now = wall_now();
    let allowed = |item: &str| obtainable_now(service.as_ref(), item, &quests, now);
    let wants = gear_plan::plan_gear_from(goal_style(life), &levels, &quests, &owned, budget, &|item| expected_price(service.as_ref(), item), &allowed);
    wants.iter().map(|want| want.expected.max(0)).sum()
}

fn cancel_order(service: &crate::market_service::MarketService, account: &str, order: i64) -> bool {
    let reply = service.handle(&lifescape_json::object(vec![
        ("op", lifescape_json::str_value("cancel")),
        ("account", lifescape_json::str_value(account)),
        ("order", Value::Int(order)),
        ("reason", lifescape_json::str_value(lifescape_store::market_criteria::GEAR_PLAN_REASON)),
    ]));
    reply.get("ok").as_bool() == Some(true)
}

/// Cancels the account's resting buys on wearables the gear plan does
/// not want (`wanted`): an upgraded slot, a tier the budget dropped, an
/// old ladder step or wish. Spared: what the graph's own obtains want,
/// and tools (the wishlist keeps those).
fn cancel_stale_bids(job: &Job, life: &Life, wanted: &[String]) {
    let Some(service) = crate::market_service::shared() else { return };
    let spared = crate::brain_wishlist::wanted_names(life);
    let rows = service.lock_store().account_orders(&job.account).unwrap_or_default();
    for row in rows.iter().filter(|row| cancellable(row) && row.motive != crate::market_lineage::NEED) {
        let Some(name) = name_of(row.item_id) else { continue };
        if gear_wearable(&name) && !wanted.contains(&name) && !spared.contains(&name) {
            let closed = cancel_order(&service, &job.account, row.order_id);
            crate::logln!("brain {}: gear bid {} on {name} cancelled ({closed}): not wanted by the gear plan", job.account, row.order_id);
        }
    }
}

/// Withdraw every dead ladder step (review 2026-11-13, finding 2): run on
/// EVERY launch pass, before `graph_state`, whatever the goal's style, so
/// an abandoned step never leaves the goal Stuck (closed stuck, its skill
/// banned for a gear cause). Returns true when the life changed.
pub(crate) fn withdraw_dead(job: &Job, life: &mut Life) -> bool {
    // Old melee equip goals carried the level-1 set as market obtains
    // (live 2026-11-13: amulet_of_power, iron_platebody on 1-coin bids):
    // the ladder owns melee pieces now, those obtains and bids go.
    let set = lifescape_planner::withdraw_melee_set(life);
    for (node_id, item) in &set {
        let closed = crate::brain_market::cancel_open_buys(&job.account, item);
        crate::logln!("brain {}: {node_id} ({item}) left the melee equip goal: the ladder buys melee pieces ({closed} bid cancelled)", job.account);
        push_history(life, node_id, "gear_withdrawn");
    }
    if !set.is_empty() {
        lifescape_planner::normalize(life);
    }
    withdraw_dead_steps_logged(job, life) || !set.is_empty()
}

fn withdraw_dead_steps_logged(job: &Job, life: &mut Life) -> bool {
    let causes: Vec<String> = life
        .nodes
        .iter()
        .filter(|node| node.status == lifescape_planner::NodeStatus::Abandoned && !node.last_error.is_empty())
        .map(|node| format!("{}={}", node.id, node.last_error))
        .collect();
    let items: Vec<(String, String)> = life
        .nodes
        .iter()
        .filter_map(|node| match &node.kind {
            lifescape_planner::NodeKind::ObtainItems { item, .. } => Some((node.id.clone(), item.clone())),
            _ => None,
        })
        .collect();
    let removed = lifescape_planner::withdraw_dead_steps(life);
    if removed.is_empty() {
        return false;
    }
    // A dead step's piece cools down like a withdrawn one: never refiled
    // on the next pass to die again.
    for (node_id, item) in &items {
        if removed.contains(node_id) {
            note_withdrawn(&job.account, item);
        }
    }
    let mine: Vec<&String> = causes.iter().filter(|cause| removed.iter().any(|node_id| cause.starts_with(&format!("{node_id}=")))).collect();
    crate::logln!("brain {}: gear step {} died ({}), withdrawn", job.account, removed.join(", "), mine.iter().map(|cause| cause.as_str()).collect::<Vec<_>>().join(" | "));
    for node_id in &removed {
        push_history(life, node_id, "gear_withdrawn");
    }
    true
}

/// Files a gear step (`insert_gear_step`); returns its ids (the obtain
/// first when `item` is named).
pub(crate) fn file_step(job: &Job, life: &mut Life, item: Option<&str>, style: Style, why: &str) -> Vec<String> {
    let ids = lifescape_planner::insert_gear_step(life, item, style_name(style));
    crate::logln!("brain {}: gear step {} ({why}): {}", job.account, item.unwrap_or("equip"), ids.join(", "));
    for id in &ids {
        push_history(life, id, "gear_step");
    }
    lifescape_planner::normalize(life);
    ids
}

fn style_name(style: Style) -> &'static str {
    match style {
        Style::Melee => "melee",
        Style::Ranged => "ranged",
        Style::Magic => "magic",
    }
}

/// True for a piece only an NPC shop sells (D-A), or a kit axe Bob sells
/// (`kit_shop_route`): never left to a bid nobody fills, the bot walks
/// to the shop through an obtain step (a kit axe takes a covered ask
/// first).
pub(crate) fn shop_only(item: &str) -> bool {
    lifescape_gamedata::obtain::kit_shop_route(item)
}

/// Iron gear (user 2026-11-14, no market): the plan over the pieces an
/// NPC shop sells, priced at the shop, the first want filed as a step
/// when none is open. Returns true when the life changed.
fn iron_gear(job: &Job, life: &mut Life, plan: &IronPlan) -> bool {
    if gear_ladder::step_open(life) {
        return false;
    }
    let budget = gear_ladder::wallet_ceiling(crate::brain_plan::holdings_count(job, "coins"), gear_ladder::GEAR_SHARE_PCT);
    let price_of = |item: &str| lifescape_gamedata::obtain::merchant_price(item).unwrap_or(i64::MAX / 64);
    // A guild keeper opens with its quest points (Scavvo, Valaine: 32).
    let quests = plan.quests();
    let allowed = |item: &str| gear_plan::shop_route_open(item, &quests);
    let wants = gear_plan::plan_gear_from(plan.style, &plan.levels(), &quests, &plan.owned, budget, &price_of, &allowed);
    let now = wall_now();
    let Some(want) = wants.iter().find(|want| !recently_withdrawn(&job.account, &want.item, now)) else { return false };
    let why = format!("Iron, shop price {}, budget {budget}", want.expected);
    file_step(job, life, Some(&want.item), plan.style, &why);
    true
}

/// What the Iron gear plan reads (owned copies of the pass's inputs).
struct IronPlan {
    style: Style,
    levels: Vec<(String, i64)>,
    quests: Vec<String>,
    owned: Vec<String>,
}

impl IronPlan {
    fn levels(&self) -> Vec<(&str, i64)> {
        self.levels.iter().map(|(name, level)| (name.as_str(), *level)).collect()
    }

    fn quests(&self) -> Vec<&str> {
        self.quests.iter().map(String::as_str).collect()
    }
}

/// One gear pass. Returns true when the life changed (save it).
pub(crate) fn gear_pass(job: &Job, life: &mut Life, live: &Value) -> bool {
    let mut changed = withdraw_dead(job, life);
    let now = wall_now();
    {
        let mut table = last_pass().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if table.get(&job.account).is_some_and(|last| now - last < GEAR_EVERY_SECS) {
            return changed;
        }
        table.insert(job.account.clone(), now);
    }
    let levels_owned = crate::brain_market::live_levels(live);
    if levels_owned.is_empty() {
        return changed;
    }
    let levels: Vec<(&str, i64)> = levels_owned.iter().map(|(name, level)| (name.as_str(), i64::from(*level))).collect();
    crate::brain_gear_fleet::record(&job.account, now, &levels);
    let quests_owned = crate::brain_rescue::quests_done(life);
    let quests: Vec<&str> = quests_owned.iter().map(String::as_str).collect();
    let style = goal_style(life);
    let (owned, worn) = holdings(job);
    let mut step_open = gear_ladder::step_open(life);
    if !step_open {
        if let Some(item) = gear_plan::unworn_upgrade(style, &levels, &quests, &owned, &worn) {
            let key = (job.account.clone(), item.clone());
            let mut table = last_equip().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if table.get(&key).is_none_or(|last| now - last >= EQUIP_RETRY_SECS) {
                table.insert(key, now);
                drop(table);
                file_step(job, life, None, style, &format!("{item} owned, not worn"));
                step_open = true;
                changed = true;
            }
        }
    }
    if life.profile.ironman {
        if !step_open {
            let plan = IronPlan {
                style,
                levels: levels.iter().map(|(name, level)| (name.to_string(), *level)).collect(),
                quests: quests_owned.clone(),
                owned: owned.clone(),
            };
            changed |= iron_gear(job, life, &plan);
        }
        return changed;
    }
    // "Obtainable now" reads the fleet's levels: never plan (and hold)
    // from a table a daemon start left partial.
    if !crate::brain_gear_fleet::warm(now) {
        return changed;
    }
    let service = crate::market_service::shared();
    let budget = gear_ladder::wallet_ceiling(gear_free_coins(job), gear_ladder::GEAR_SHARE_PCT);
    let owned_wearables: Vec<String> = owned.iter().filter(|name| gear_wearable(name)).cloned().collect();
    let signature = crate::brain_gear_hold::gear_signature(style_name(style), &levels, &owned_wearables);
    let now_tick = crate::brain_stuck::seconds_to_ticks(now as i64);
    let allowed = |item: &str| obtainable_now(service.as_ref(), item, &quests, now);
    let plan = |budget: i64| gear_plan::plan_gear_from(style, &levels, &quests, &owned, budget, &|item| expected_price(service.as_ref(), item), &allowed);
    let wants = crate::brain_gear_hold::held_wants(&job.account, budget, now_tick, &signature, &plan);
    let wanted: Vec<String> = wants.iter().map(|want| want.item.clone()).collect();
    cancel_stale_bids(job, life, &wanted);
    let stepping = crate::brain_wishlist::wanted_names(life);
    for want in &wants {
        if stepping.contains(&want.item) {
            continue;
        }
        let id = obj_id(&want.item);
        let best_ask = service.as_ref().zip(id).and_then(|(service, id)| service.lock_engine().books.get(&id).and_then(|book| book.best().1).map(|price| crate::market_units::to_unit(id, price)));
        let covered = best_ask.is_some_and(|ask| ask <= want.ceiling);
        // A shop the bot's quest points open (Champions' Guild at 32:
        // Scavvo, Valaine) sells it: the step walks there (wave 2 (a)).
        if covered || gear_plan::shop_route_open(&want.item, &quests) {
            if !step_open && !recently_withdrawn(&job.account, &want.item, now) {
                let why = format!("ask {best_ask:?}, ceiling {}, budget {budget}", want.ceiling);
                file_step(job, life, Some(&want.item), style, &why);
                step_open = true;
                changed = true;
            }
            continue;
        }
        let (Some(service), Some(id)) = (service.as_ref(), id) else { continue };
        match crate::market_reprice::keep_bid_from(service, &job.account, id, 1, want.expected, want.ceiling, crate::market_lineage::GEAR) {
            Ok(crate::market_reprice::StandingBid::Placed { price, .. }) => {
                crate::logln!(
                    "brain {}: gear {} bid @ {price} (expected {}, ceiling {}, budget {budget}), waiting for a producer",
                    job.account,
                    want.item,
                    want.expected,
                    want.ceiling
                );
            }
            Ok(crate::market_reprice::StandingBid::Kept { .. }) => {}
            Err(error) => crate::logln!("brain {}: gear {} bid refused: {error}", job.account, want.item),
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    #[test]
    fn withdrawn_piece_cools_down_per_account() {
        let now = super::wall_now();
        assert!(!super::recently_withdrawn("cooldown_probe", "black_med_helm", now));
        super::note_withdrawn("cooldown_probe", "black_med_helm");
        assert!(super::recently_withdrawn("cooldown_probe", "black_med_helm", super::wall_now()));
        assert!(!super::recently_withdrawn("cooldown_other", "black_med_helm", super::wall_now()));
        assert!(!super::recently_withdrawn("cooldown_probe", "black_med_helm", now + super::WITHDRAWN_RETRY_SECS + 1.0));
    }

    use super::{cancellable, committed_from};
    use lifescape_store::{Order, OrderSide, OrderStatus};

    fn order(item: i64, side: OrderSide, price: i64, remaining: i64, status: OrderStatus) -> Order {
        Order {
            order_id: item,
            account: "b0".to_string(),
            side,
            item_id: item,
            quantity: remaining.max(1),
            remaining,
            price,
            status,
            dormant: false,
            created_clock: 0,
            updated_clock: 0,
            ..Order::default()
        }
    }

    /// Review 2026-11-13 (finding 6): standing bids (gear 50 %, two tools
    /// 25 % each) and the graph's buys all took shares of the WHOLE wallet.
    /// Each ceiling now reads the wallet minus the coins this bot's other
    /// resting buys commit; a partly matched buy still rests (cancellable).
    #[test]
    fn resting_buys_commit_their_coins() {
        let rows = vec![
            order(1, OrderSide::Buy, 10, 3, OrderStatus::Open),
            order(2, OrderSide::Buy, 5, 4, OrderStatus::Matched),
            order(3, OrderSide::Buy, 7, 9, OrderStatus::Settled),
            order(4, OrderSide::Sell, 100, 9, OrderStatus::Open),
            order(5, OrderSide::Buy, 100, 9, OrderStatus::Cancelled),
            order(6, OrderSide::Buy, 100, 0, OrderStatus::Matched),
        ];
        assert_eq!(committed_from(&rows, None), 50);
        assert_eq!(committed_from(&rows, Some(1)), 20, "a bid never counts against its own ceiling");
        let resting: Vec<i64> = rows.iter().filter(|row| cancellable(row)).map(|row| row.item_id).collect();
        assert_eq!(resting, vec![1, 2], "open and partly matched buys");
    }

    /// bd23dqpq 2026-11-13: 98 runes matched at 1 coin are spent until
    /// the handover, whatever item the next bid is on; its sales are not.
    #[test]
    fn in_flight_buyer_deals_commit_their_coins() {
        use super::deal_coins_from;
        let deal = |buyer: &str, seller: &str, quantity: i64, price: i64| lifescape_store::Deal {
            deal_id: 1,
            buyer: buyer.to_string(),
            seller: seller.to_string(),
            item_id: 556,
            quantity,
            price,
            meeting_x: 0,
            meeting_y: 0,
            meeting_level: 0,
            status: lifescape_store::DealStatus::Convened,
            fail_reason: String::new(),
            deadline_tick: 0,
            created_clock: 0,
            updated_clock: 0,
            taker_side: None,
        };
        let deals = vec![deal("bd23dqpq", "george_97", 24, 1), deal("bd23dqpq", "lara_lane", 24, 2), deal("t01", "bd23dqpq", 6, 5)];
        assert_eq!(deal_coins_from("bd23dqpq", &deals), 72, "24 + 48 promised, its own sale adds nothing");
    }
}
