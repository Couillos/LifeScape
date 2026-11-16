#![forbid(unsafe_code)]
//! Default selling (PRD3 lot 3). Every product a bot does not need goes
//! to the market, never to an NPC shop: the brain lists the bot's
//! unlisted tradeable surplus (pack + bank) directly on the daemon
//! market, opening at the reference (or the best bid when richer) and
//! walking with demand (`market_reprice`: up while it sells, down to a
//! reserve while it rests). The physical handover stays the convene
//! loop's job; locked stock (`client/locks.rs`) and the audit
//! (`market_audit.rs`) keep every listing covered.
//!
//! Protected (never listed): coins, food up to 100 units, the graph
//! keep-list (obtain items, quest checks), the inputs of the skill being
//! trained (raw fish for a cook, ore for a smelter) and any product
//! another open method consumes, tools, worn gear, the equipment ladder's
//! pieces and next step, wanted upgrades. Produced goods (bars, smithed,
//! crafted, cooked) are sold. Iron accounts never list.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use lifescape_json::{object, str_value, Value};
use lifescape_planner::{Life, NodeKind, NodeStatus};

use crate::jobs::Job;

/// Wall seconds between two listing passes for one account.
pub const SALE_EVERY_SECS: f64 = 600.0;

fn last_pass() -> &'static Mutex<HashMap<String, f64>> {
    static TABLE: OnceLock<Mutex<HashMap<String, f64>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn wall_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs_f64()).unwrap_or(0.0)
}

/// Product named by a skill method (`cook/raw_shrimp` -> raw_shrimp,
/// `smelt:bronze_bar` -> bronze_bar, `spin:ball_of_wool` -> ball_of_wool).
fn method_product(method: &str) -> Option<&str> {
    let (_, tail) = method.split_once(['/', ':'])?;
    Some(tail.rsplit('/').next().unwrap_or(tail))
}

/// Methods whose tail names the object worked ON (the input): a cook's
/// raw fish, a firemaker's logs, an unstrung amulet, a filled container.
const INPUT_NAMED: [&str; 7] = ["cook/", "cook:", "fire:", "string:", "enchant:", "fill:", "tan:"];

/// Every method the bot's open graph will work (2026-11-14, anti-dumping
/// (c): what it will burn, cook, smith or cast is not surplus): the
/// chosen method and alternatives of each open level or money node, and
/// the tranche method of an open level node not resolved yet. The
/// ambition's open milestones are kept by `brain::cleanup_keep`
/// (`market_fit::own_use_items`), never twice here.
pub fn planned_methods(life: &Life) -> Vec<String> {
    let mut methods: Vec<String> = Vec::new();
    for node in &life.nodes {
        if matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned) {
            continue;
        }
        if !matches!(node.kind, NodeKind::ReachLevel { .. } | NodeKind::MakeMoney) {
            continue;
        }
        methods.extend(node.method.iter().cloned().chain(node.alternatives.iter().map(|alt| alt.method.clone())));
        if let NodeKind::ReachLevel { skill, level } = &node.kind {
            if node.method.is_none() && node.alternatives.is_empty() {
                methods.push(lifescape_planner::tranches::reach_method(skill, level.saturating_sub(1).max(1)));
            }
        }
    }
    methods.sort();
    methods.dedup();
    methods
}

/// Pushes into `keep` what one planned method consumes (inputs and
/// tools of a transformation, the runes of a combat spell); returns the
/// product a transformation makes (sold unless another method eats it).
/// Gatherers and melee/ranged fighters consume nothing kept.
fn method_inputs(method: &str, keep: &mut Vec<String>) -> Option<String> {
    // Alchemy works ON its object (never a product to sell) with nature
    // and fire runes, a staff of fire standing for the fire.
    if let Some(item) = method.strip_prefix("alch:") {
        keep.extend(lifescape_planner::training::kept_inputs("magic", 99));
        if !item.is_empty() {
            keep.push(item.to_string());
        }
        return None;
    }
    // Every per-cast spell (combat spell, curse, teleport, bananas: D18)
    // burns its runes, bananas their bones too; bananas are the product.
    if let Some(spell) = lifescape_planner::spell_methods::cast_spell(method) {
        keep.extend(lifescape_planner::method_way::spell_runes(&spell).into_iter().map(|(rune, _)| rune));
        let (product, way) = lifescape_planner::method_way::method_way(method)?;
        keep.extend(way.inputs.into_iter().map(|(input, _)| input));
        return (!product.is_empty()).then_some(product);
    }
    // A ranged fight fires the tranche's ammunition (`sizing`): kept.
    if method.starts_with("fight:ranged/") {
        keep.extend(lifescape_planner::sizing::tranche_consumption(method, 1, 2).into_iter().map(|(ammo, _)| ammo));
        return None;
    }
    if method.starts_with("gather:") || method.starts_with("fight:") || method.starts_with("market:") {
        return None;
    }
    let named = method_product(method)?;
    let product = if INPUT_NAMED.iter().any(|prefix| method.starts_with(prefix)) {
        keep.push(named.to_string());
        None
    } else {
        Some(named.to_string())
    };
    for way in lifescape_gamedata::obtain::ways(named) {
        if way.is_production() {
            keep.extend(way.inputs.iter().map(|(input, _)| input.clone()));
            keep.extend(way.tools.iter().cloned());
        }
    }
    product
}

/// What the unfinished graph still needs on hand, beyond the cleanup
/// keep-list: the inputs (and tools) of every transformation it trains
/// (a cook keeps raw fish, a smelter keeps ore). A transformation's
/// product is SOLD (2026-11-13: producers sell what they make, bars,
/// smithed and crafted goods) unless another open method consumes it.
pub fn working_keep(life: &Life, wished: &[String]) -> Vec<String> {
    let mut keep = crate::brain::cleanup_keep(life);
    let mut products: Vec<String> = Vec::new();
    for method in planned_methods(life) {
        if let Some(product) = method_inputs(&method, &mut keep) {
            products.push(product);
        }
    }
    // An open magic climb past 21 keeps its alchemy runes between
    // launches (priced at resolve: the next batch casts them).
    for node in life.nodes.iter().filter(|node| !matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned)) {
        if let NodeKind::ReachLevel { skill, level } = &node.kind {
            keep.extend(lifescape_planner::training::kept_inputs(skill, *level));
        }
    }
    let consumed: Vec<String> = keep.clone();
    keep.extend(products.into_iter().filter(|product| consumed.contains(product)));
    keep.extend(crate::brain_wishlist::wanted_names(life));
    keep.extend(wished.iter().cloned());
    keep.sort();
    keep.dedup();
    keep
}

/// True when `item` is an F2P object the market trades.
pub fn tradeable(item: &str) -> bool {
    lifescape_gamedata::objs_by_name(item).into_iter().next().is_some_and(|obj| obj.tradeable && obj.members == Some(false))
}

/// Unlisted tradeable surplus of one bot: `(debug name, obj id, qty)`.
pub fn surplus(stacks: &[(String, i64)], keep: &[String], worn: &[String], listed: &HashSet<i64>) -> Vec<(String, i64, i64)> {
    let (_, spare) = lifescape_planner::triage_bank(stacks, keep);
    let mut out = Vec::new();
    for (name, qty) in spare {
        if qty <= 0 || crate::brain_plan::sale_blocked(&name, keep, worn) {
            continue;
        }
        let Some(obj) = lifescape_gamedata::objs_by_name(&name).into_iter().next() else { continue };
        if !obj.tradeable || obj.members != Some(false) {
            continue;
        }
        let id = obj.obj_id as i64;
        if listed.contains(&id) {
            continue;
        }
        out.push((name, id, qty));
    }
    out.sort();
    out
}

/// Live stacks net of what the bot's resting sells and in-flight seller
/// deals lock (`brain_locks`). Live 2026-11-13: fiona_park sold 25
/// arrows (deal 1698, still banked until the handover, no resting row
/// left), the next pass listed all 102 held again and the relist matched
/// at once (deal 1721, `short` by 25); same for her one air rune (1692,
/// 1720) and bct5rxpg's 7 arrows (1716, 1722).
pub fn free_stacks(stacks: &[(String, i64)], locked: &dyn Fn(&str) -> i64) -> Vec<(String, i64)> {
    stacks
        .iter()
        .map(|(name, qty)| (name.clone(), crate::brain_locks::free_count(*qty, locked(name))))
        .filter(|(_, qty)| *qty > 0)
        .collect()
}

/// Spare copies of kept work tools (one unit stays): `(name, id, qty)`.
fn spare_tools(life: &Life, stacks: &[(String, i64)], kept: &[String], listed: &HashSet<i64>) -> Vec<(String, i64, i64)> {
    crate::brain_tools::spare_tool_units(life, stacks, kept)
        .into_iter()
        .filter(|(name, _)| tradeable(name))
        .filter_map(|(name, qty)| {
            let id = lifescape_gamedata::objs_by_name(&name).into_iter().next()?.obj_id as i64;
            (!listed.contains(&id)).then_some((name, id, qty))
        })
        .collect()
}

/// Re-adopts the account's resting listings that walk no more (placed
/// before a daemon restart, or before these rules), merchants included
/// (2026-11-14: no dumping flip stock either): a listing under the
/// bot's own reserve is amended up to it, one over the demand lot or
/// without a known own cost is withdrawn (its stock back in the bank,
/// the next pass lists a lot), the rest walks down to the reserve.
fn adopt_listings(
    service: &crate::market_service::MarketService,
    account: &str,
    selling: &[lifescape_store::Order],
    costs: &crate::brain_reserve::OwnCosts,
    life: &Life,
    make_costs: &HashMap<String, f64>,
    live: &Value,
) {
    use crate::market_seller::{Adoption, SaleDecision};
    let (mut walked, mut raised, mut lowered, mut withdrawn) = (0usize, 0usize, 0usize, 0usize);
    for row in selling {
        // Idempotent: an order that walks or was adopted once is skipped
        // before any reserve or book work (2026-11-14, daemon CPU).
        if crate::market_reprice::handled(service, row.order_id) {
            continue;
        }
        let Some(name) = lifescape_gamedata::obj_by_id(usize::try_from(crate::market_audit::base_of(row.item_id)).unwrap_or(0)).and_then(|obj| obj.name) else { continue };
        let make_cost = make_costs.get(name).copied();
        // Today's opening for this stock (2026-11-14: an ask anchored on
        // an old reserve or an outlier comes down to it).
        let (today, reserve) = service.sale_decision(account, name, row.item_id, row.remaining, costs, Some(life), make_cost);
        let reserve = reserve.map(|reserve| reserve.coins);
        let alch = crate::brain_alch::alch_floor(live, name).unwrap_or(0);
        let opening = match today {
            SaleDecision::List { price, .. } => Some(price.max(alch)),
            SaleDecision::Hold(_) => None,
        };
        let lot = lifescape_planner::market_fit::sale_lot(row.remaining, Some(&service.public_view(account, row.item_id)));
        let decision = crate::market_seller::adoption(row.price, row.remaining, reserve, lot, opening);
        if !matches!(decision, Adoption::Walk { .. }) {
            // Decided once: never retried nor re-logged every pass.
            service.lock_engine().adopted.insert(row.order_id);
        }
        let (order, floor) = match decision {
            Adoption::Withdraw(why) => {
                let reply = service.handle(&object(vec![("op", str_value("cancel")), ("account", str_value(account)), ("order", Value::Int(row.order_id))]));
                if reply.get("ok").as_bool() == Some(true) {
                    withdrawn += 1;
                } else {
                    crate::logln!("brain {account}: listing {} of {name} not withdrawn ({why}): {:?}", row.order_id, reply.get("error").as_str());
                }
                continue;
            }
            Adoption::Raise { price, floor } | Adoption::Lower { price, floor } => {
                let up = matches!(decision, Adoption::Raise { .. });
                let reply = service.handle(&object(vec![("op", str_value("amend")), ("account", str_value(account)), ("order", Value::Int(row.order_id)), ("price", Value::Int(price))]));
                let Some(order) = reply.get("order").as_i64().filter(|_| reply.get("ok").as_bool() == Some(true)) else {
                    let target = if up { "raised to its reserve" } else { "lowered to today's opening" };
                    crate::logln!("brain {account}: listing {} of {name} not {target} {price}: {:?}", row.order_id, reply.get("error").as_str());
                    continue;
                };
                if up {
                    raised += 1;
                } else {
                    lowered += 1;
                }
                (order, floor)
            }
            Adoption::Walk { floor } => (row.order_id, floor),
        };
        let bounds = crate::market_reprice::QuoteBounds {
            account: account.to_string(),
            sell: true,
            floor,
            ceiling: crate::market_reprice::SELL_CAP,
            patience_ticks: crate::market_reprice::PATIENCE_TICKS,
            last_change_wall: wall_now(),
            last_remaining: row.remaining,
            listed_wall: wall_now(),
        };
        if crate::market_reprice::adopt(service, order, bounds) {
            walked += 1;
        }
    }
    if walked + raised + lowered + withdrawn > 0 {
        crate::logln!("brain {account}: {walked} listings adopted by demand pricing ({raised} raised to the reserve, {lowered} lowered to today's opening), {withdrawn} withdrawn");
    }
}

/// Spare copies of kept or worn wearables (one unworn copy stays,
/// `gear_plan::spare_gear_units`): `(name, id, qty)`.
fn spare_gear(stacks: &[(String, i64)], gear: &[String], whole: &[String], listed: &HashSet<i64>) -> Vec<(String, i64, i64)> {
    lifescape_planner::gear_plan::spare_gear_units(stacks, gear, whole)
        .into_iter()
        .filter(|(name, _)| tradeable(name))
        .filter_map(|(name, qty)| {
            let id = lifescape_gamedata::objs_by_name(&name).into_iter().next()?.obj_id as i64;
            (!listed.contains(&id)).then_some((name, id, qty))
        })
        .collect()
}

/// Resting listings that offer a sliver of a big free stack
/// (`market_seller::undersized`; live frank_lane 2026-11-14: 1 iron
/// dagger listed at 628, 40 banked and never listed): withdrawn so the
/// stack lists as one lot. Only surplus items (not kept), open rows,
/// never a merchant's flip stock. Returns the order ids withdrawn.
fn regroup_slivers(
    service: &crate::market_service::MarketService,
    account: &str,
    selling: &[lifescape_store::Order],
    free: &[(String, i64)],
    keep: &[String],
    flip_stock: &HashSet<i64>,
) -> Vec<i64> {
    let mut withdrawn = Vec::new();
    for row in selling.iter().filter(|row| row.status == lifescape_store::OrderStatus::Open && !flip_stock.contains(&row.item_id)) {
        let Some(name) = lifescape_gamedata::obj_by_id(usize::try_from(crate::market_audit::base_of(row.item_id)).unwrap_or(0)).and_then(|obj| obj.name) else { continue };
        if crate::brain_plan::sale_blocked(name, keep, &[]) {
            continue;
        }
        let spare: i64 = free.iter().filter(|(held, _)| held == name).map(|(_, qty)| *qty).sum();
        // Cheap gate before the book read: the whole stack must be at
        // least twice the listing.
        if spare <= 0 || row.quantity.saturating_mul(2) > row.remaining + spare {
            continue;
        }
        let lot = lifescape_planner::market_fit::sale_lot(row.remaining + spare, Some(&service.public_view(account, row.item_id)));
        if !crate::market_seller::undersized(row.quantity, row.remaining, spare, lot) {
            continue;
        }
        let reply = service.handle(&object(vec![("op", str_value("cancel")), ("account", str_value(account)), ("order", Value::Int(row.order_id))]));
        if reply.get("ok").as_bool() == Some(true) {
            crate::logln!("brain {account}: listing {} of {} {name} withdrawn ({spare} more free, lot {lot}): the stack lists as one lot", row.order_id, row.remaining);
            withdrawn.push(row.order_id);
        } else {
            crate::logln!("brain {account}: listing {} of {name} not regrouped: {:?}", row.order_id, reply.get("error").as_str());
        }
    }
    withdrawn
}

/// Resting sells that lock the last free copy of a tool the bot works
/// with (`brain_tools::kept_tools`): `(order id, debug name)`. Live
/// 2026-11-13 (b3gybcev): an old listing of its only hammer locked it away
/// from the smith script. Any other tool listing stays (tools sell).
pub fn tool_listings(selling: &[lifescape_store::Order], stacks: &[(String, i64)], kept: &[String]) -> Vec<(i64, String)> {
    selling
        .iter()
        .filter(|row| row.side == lifescape_store::OrderSide::Sell && row.remaining > 0)
        .filter_map(|row| {
            let name = lifescape_gamedata::obj_by_id(usize::try_from(crate::market_audit::base_of(row.item_id)).ok()?)?.name?.to_string();
            crate::brain_tools::listing_locks_needed_tool(&name, row.remaining, stacks, kept).then_some((row.order_id, name))
        })
        .collect()
}

/// Cancels those listings; returns the order ids still resting.
fn withdraw_tool_listings(service: &crate::market_service::MarketService, account: &str, listings: &[(i64, String)]) -> Vec<i64> {
    let mut kept = Vec::new();
    for (order, name) in listings {
        let reply = service.handle(&object(vec![("op", str_value("cancel")), ("account", str_value(account)), ("order", Value::Int(*order))]));
        if reply.get("ok").as_bool() == Some(true) {
            crate::logln!("brain {account}: listing {order} of {name} cancelled (its last copy of a kit or work tool)");
        } else {
            crate::logln!("brain {account}: listing {order} of {name} not cancelled: {:?}", reply.get("error").as_str());
            kept.push(*order);
        }
    }
    kept
}

/// One listing pass for an idle, non-Iron bot (rate-limited per account).
/// Returns the number of sell orders placed.
pub fn maybe_list_surplus(job: &Job, life: &Life) -> usize {
    if life.profile.ironman {
        return 0;
    }
    let now = wall_now();
    {
        let table = last_pass().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if table.get(&job.account).is_some_and(|last| now - last < SALE_EVERY_SECS) {
            return 0;
        }
    }
    let Some(service) = crate::market_service::shared() else { return 0 };
    last_pass().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(job.account.clone(), now);
    let worn = crate::brain_plan::worn_names(job);
    let mut wished: Vec<String> = crate::brain_wishlist::wishlist(job, life).into_iter().map(|wish| wish.item).collect();
    let live = job.inner.try_lock().ok().and_then(|inner| inner.live.clone()).unwrap_or(Value::Null);
    // Kept and worn gear keep ONE spare copy, never the whole stack
    // (2026-11-14: a worn bronze med helm protected 88 looted copies).
    let mut gear = crate::brain_gear::ladder_keep(job, life, &live);
    gear.extend(worn.iter().cloned());
    // A caster keeps what alchs for more than it sells (2026-11-14).
    wished.extend(crate::brain_alch::alch_keep(life, &live));
    let stacks = crate::brain_plan::live_stacks(job);
    let levels = crate::brain_tools::live_levels(&live);
    let tools = crate::brain_tools::kept_tools(life, &stacks, &crate::brain_tools::level_refs(&levels));
    wished.extend(tools.iter().cloned());
    let whole = working_keep(life, &wished);
    let keep = crate::brain_plan::merge_keep(whole.clone(), gear.clone());
    // An open quest keeps its objects (2026-11-14, bh99tw2f: its own
    // listings locked every banked air talisman): a listing that locks
    // them is cut first, and they are never offered below.
    let reserved = lifescape_planner::reserved_quest_objects(life);
    crate::brain_quest_gate::release_for_quests(&service, &job.account, &stacks, &reserved);
    let mut selling: Vec<lifescape_store::Order> = service
        .lock_store()
        .account_orders(&job.account)
        .unwrap_or_default()
        .into_iter()
        .filter(|row| {
            row.side == lifescape_store::OrderSide::Sell
                && matches!(row.status, lifescape_store::OrderStatus::Open | lifescape_store::OrderStatus::Matched)
                && row.remaining > 0
        })
        .collect();
    // A listing of the last free copy of a work tool is withdrawn; every
    // other tool listing rests and reprices like any listing.
    let locking = tool_listings(&selling, &stacks, &tools);
    if !locking.is_empty() {
        let still = withdraw_tool_listings(&service, &job.account, &locking);
        selling.retain(|row| !locking.iter().any(|(order, _)| *order == row.order_id) || still.contains(&row.order_id));
    }
    // A merchant's flip stock (what it bought) is priced by its flips
    // only (`flip_quote`), never relisted here (ryan_park held 81 % of
    // the sell side at 1 coin, 2026-11-14).
    let flip_stock: HashSet<i64> = if life.profile.archetype == lifescape_planner::Archetype::Merchant {
        service.lock_store().items_bought_by(&job.account).unwrap_or_default().into_iter().collect()
    } else {
        HashSet::new()
    };
    let mut locks = crate::brain_locks::locked_items(&job.account);
    let mut free = lifescape_planner::withhold_reserved(&free_stacks(&stacks, &|name| crate::brain_locks::locked_in(&locks, name)), &reserved);
    // A sliver listed out of a big stack is withdrawn and the whole
    // stack lists as one lot in this same pass.
    let regrouped = regroup_slivers(&service, &job.account, &selling, &free, &keep, &flip_stock);
    if !regrouped.is_empty() {
        selling.retain(|row| !regrouped.contains(&row.order_id));
        locks = crate::brain_locks::locked_items(&job.account);
        free = lifescape_planner::withhold_reserved(&free_stacks(&stacks, &|name| crate::brain_locks::locked_in(&locks, name)), &reserved);
    }
    // Withdrawn lots stay out this pass (still in `selling`): their stock
    // is banked again and the next pass lists a demand lot.
    let listed: HashSet<i64> = selling.iter().map(|row| row.item_id).collect();
    let costs = crate::cpu_profile::timed("sales.own_costs", || crate::brain_reserve::OwnCosts::load(&service.lock_store(), &job.account));
    // The `bid_response` make cost caps every reserve (one book and rates
    // read for the pass): what a bid quota was priced at always sells.
    let mut priced: Vec<(String, i64)> = free.clone();
    priced.extend(selling.iter().filter_map(|row| {
        let name = lifescape_gamedata::obj_by_id(usize::try_from(crate::market_audit::base_of(row.item_id)).ok()?)?.name?;
        Some((name.to_string(), row.remaining))
    }));
    let make_costs = crate::brain_bids::production_costs_for(life, &live, &priced);
    adopt_listings(&service, &job.account, &selling, &costs, life, &make_costs, &live);
    let mut placed = 0;
    // Only what no listing or deal already promises (the market refuses
    // the rest anyway, `market_audit::uncommitted`): `free` above.
    let mut offers = surplus(&free, &keep, &worn, &listed);
    offers.extend(spare_tools(life, &free, &tools, &listed));
    offers.extend(spare_gear(&free, &gear, &whole, &listed));
    for (name, item, qty) in offers {
        if flip_stock.contains(&item) {
            continue;
        }
        let (decision, reserve) = service.sale_decision(&job.account, &name, item, qty, &costs, Some(life), make_costs.get(&name).copied());
        let (lot, price, floor) = match decision {
            // The alch floor (2026-11-14): a caster never sells under what
            // the cast nets it.
            crate::market_seller::SaleDecision::List { qty: lot, price, floor } => match crate::brain_alch::alch_floor(&live, &name) {
                Some(alch) if alch > floor => (lot, price.max(alch), alch),
                _ => (lot, price, floor),
            },
            crate::market_seller::SaleDecision::Hold(why) => {
                crate::logln!("brain {}: holding {qty} {name} ({why})", job.account);
                continue;
            }
        };
        let reply = service.handle(&object(vec![
            ("op", str_value("place")),
            ("account", str_value(&job.account)),
            ("side", str_value("sell")),
            ("item", Value::Int(item)),
            ("qty", Value::Int(lot)),
            ("price", Value::Int(price)),
            ("floor", Value::Int(floor.min(price))),
        ]));
        let source = reserve.map(|known| format!("{:?}", known.source).to_lowercase()).unwrap_or_default();
        if reply.get("ok").as_bool() == Some(true) {
            placed += 1;
            crate::logln!("brain {}: listed {lot}/{qty} {name} @ {price} (reserve {floor}, {source})", job.account);
        } else {
            crate::logln!("brain {}: listing {lot} {name} refused, holding: {:?}", job.account, reply.get("error").as_str());
        }
    }
    placed
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{method_product, surplus};
    use std::collections::HashSet;

    #[test]
    fn surplus_skips_needs_food_kept_tools_and_listed() {
        let stacks = vec![
            ("coins".to_string(), 500),
            ("logs".to_string(), 300),
            ("raw_shrimp".to_string(), 50),
            ("shrimp".to_string(), 40),
            ("net".to_string(), 1),
            ("bronze_bar".to_string(), 10),
            ("copper_ore".to_string(), 12),
        ];
        let keep = vec!["raw_shrimp".to_string()];
        let worn = vec!["bronze_sword".to_string()];
        let copper = lifescape_gamedata::objs_by_name("copper_ore")[0].obj_id as i64;
        let listed: HashSet<i64> = [copper].into_iter().collect();
        let names: Vec<String> = surplus(&stacks, &keep, &worn, &listed).into_iter().map(|(name, _, _)| name).collect();
        assert_eq!(names, vec!["bronze_bar".to_string(), "logs".to_string(), "net".to_string()], "coins, food under 100, keep and listed stay; an unused tool sells");
        let with_net = vec!["raw_shrimp".to_string(), "net".to_string()];
        let names: Vec<String> = surplus(&stacks, &with_net, &worn, &listed).into_iter().map(|(name, _, _)| name).collect();
        assert_eq!(names, vec!["bronze_bar".to_string(), "logs".to_string()], "a kept work tool stays");
    }

    /// Deal 1721: 127 arrows held, 25 promised to an in-flight deal with
    /// no resting row: only 102 are free; a stack fully promised (her
    /// one air rune, deal 1692) is not offered at all.
    #[test]
    fn promised_stock_is_not_surplus() {
        use super::free_stacks;
        let stacks = vec![("bronze_arrow".to_string(), 127), ("airrune".to_string(), 1), ("logs".to_string(), 5)];
        let locked = |name: &str| match name {
            "bronze_arrow" => 25,
            "airrune" => 1,
            _ => 0,
        };
        let free = free_stacks(&stacks, &locked);
        assert_eq!(free, vec![("bronze_arrow".to_string(), 102), ("logs".to_string(), 5)]);
        let offered: Vec<(String, i64)> = surplus(&free, &[], &[], &HashSet::new()).into_iter().map(|(name, _, qty)| (name, qty)).collect();
        assert!(offered.contains(&("bronze_arrow".to_string(), 102)), "{offered:?}");
        assert!(!offered.iter().any(|(name, _)| name == "airrune"), "{offered:?}");
    }

    pub(crate) fn life_training(methods: &[(&str, &str)]) -> lifescape_planner::Life {
        use lifescape_planner::{Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};
        let nodes = methods
            .iter()
            .enumerate()
            .map(|(index, (skill, method))| {
                let mut node = Node::new(&format!("n{}", index + 1), NodeKind::ReachLevel { skill: skill.to_string(), level: 20 }, Vec::new(), 20);
                node.status = NodeStatus::Ready;
                node.method = Some(method.to_string());
                node
            })
            .collect();
        Life {
            schema: 1,
            account: "t_sales".to_string(),
            created_at: 0,
            seed: 1,
            profile: Profile::new(false, 0.5, 0.5, 1, Archetype::Producer),
            goal: GoalKind::MaxSkills { skills: vec!["smithing".to_string()], level: 20 },
            nodes,
            engagement: None,
            active: None,
            history: Vec::new(),
            ambition: None,
            goal_origin: None,
            memory: Default::default(),
        }
    }

    /// Producers sell what they make (2026-11-13); inputs stay, and a
    /// product another open method consumes stays too.
    #[test]
    fn producers_sell_products_keep_inputs() {
        let smelter = super::working_keep(&life_training(&[("smithing", "smelt:bronze_bar")]), &[]);
        assert!(smelter.contains(&"copper_ore".to_string()) && smelter.contains(&"tin_ore".to_string()), "{smelter:?}");
        assert!(!smelter.contains(&"bronze_bar".to_string()), "bars are sold: {smelter:?}");
        let cook = super::working_keep(&life_training(&[("cooking", "cook/raw_shrimp")]), &[]);
        assert!(cook.contains(&"raw_shrimp".to_string()), "a cook keeps its raw fish: {cook:?}");
        let chain = super::working_keep(&life_training(&[("crafting", "jewelry:gold_ring"), ("smithing", "smelt:gold_bar")]), &[]);
        assert!(chain.contains(&"gold_bar".to_string()), "bars a crafter consumes stay: {chain:?}");
        assert!(!chain.contains(&"gold_ring".to_string()), "rings are sold: {chain:?}");
    }

    /// Anti-dumping (c), 2026-11-14: what the bot's own plan will burn,
    /// cook or cast is not surplus, even before the node is resolved: a
    /// pending firemaking tranche keeps its logs, a magic tranche its
    /// runes, an unfinished cooking milestone its raw fish (the latter
    /// through `market_fit::own_use_items` in `cleanup_keep`).
    #[test]
    fn planned_inputs_are_not_surplus() {
        use lifescape_planner::ambition::{Ambition, Milestone, MilestoneStatus, Target};
        use lifescape_planner::{Node, NodeKind, NodeStatus};
        let mut life = life_training(&[]);
        let mut fire = Node::new("n1", NodeKind::ReachLevel { skill: "firemaking".to_string(), level: 10 }, Vec::new(), 10);
        fire.status = NodeStatus::Pending;
        life.nodes.push(fire);
        let mut magic = Node::new("n2", NodeKind::ReachLevel { skill: "magic".to_string(), level: 10 }, Vec::new(), 10);
        magic.status = NodeStatus::Ready;
        magic.method = Some("fight:magic/wind_strike/chickens".to_string());
        life.nodes.push(magic);
        life.ambition = Some(Ambition {
            kind: "skiller".to_string(),
            label: String::new(),
            groups: Vec::new(),
            why: "random".to_string(),
            milestones: vec![Milestone {
                id: "m1".to_string(),
                label: String::new(),
                target: Target::Level { skill: "cooking".to_string(), level: 5 },
                status: MilestoneStatus::Pending,
                progress: (1, 5),
                blocked_count: 0,
                origin: "ladder".to_string(),
            }],
            drawn_at_tick: 0,
            drawn_at: 0,
            rung: 0,
            revisions: Vec::new(),
            pin: None,
        });
        let keep = super::working_keep(&life, &[]);
        assert!(keep.contains(&"logs".to_string()), "a firemaker keeps its logs: {keep:?}");
        assert!(keep.contains(&"airrune".to_string()) && keep.contains(&"mindrune".to_string()), "a caster keeps its runes: {keep:?}");
        assert!(keep.contains(&"raw_shrimp".to_string()), "a future cook keeps raw fish: {keep:?}");
        let stacks = vec![("logs".to_string(), 300), ("airrune".to_string(), 50), ("willow_logs".to_string(), 20)];
        let names: Vec<String> = surplus(&stacks, &keep, &[], &HashSet::new()).into_iter().map(|(name, _, _)| name).collect();
        assert_eq!(names, vec!["willow_logs".to_string()], "only what no plan burns is sold");
    }

    /// Live 2026-11-13 (b3gybcev): an old listing of its only hammer
    /// (order 56648, 1 @1) locked it away from the smith script. Only the
    /// listing of the last free copy of a tool it works with is withdrawn;
    /// the chisel and pickaxe it does not use keep selling (tools sell).
    #[test]
    fn only_the_needed_tool_listing_is_withdrawn() {
        let order = |order_id: i64, item: &str| lifescape_store::Order {
            order_id,
            account: "b3gybcev".to_string(),
            side: lifescape_store::OrderSide::Sell,
            item_id: lifescape_gamedata::objs_by_name(item)[0].obj_id as i64,
            quantity: 1,
            remaining: 1,
            price: 1,
            status: lifescape_store::OrderStatus::Open,
            dormant: false,
            created_clock: 0,
            updated_clock: 0,
        };
        let rows = vec![order(56648, "hammer"), order(2, "logs"), order(3, "chisel"), order(4, "bronze_pickaxe")];
        let held: Vec<(String, i64)> = ["hammer", "logs", "chisel", "bronze_pickaxe"].iter().map(|name| (name.to_string(), 1)).collect();
        let kept = vec!["hammer".to_string()];
        assert_eq!(super::tool_listings(&rows, &held, &kept), vec![(56648, "hammer".to_string())]);
        for tool in ["chisel", "needle", "shears", "knife", "fishing_bait", "net", "tinderbox"] {
            assert!(!crate::brain_plan::sale_blocked(tool, &kept, &[]), "{tool} sells when unused");
        }
    }

    /// Loot is surplus, ammunition a ranged plan fires is not (2026-11-14):
    /// an archer keeps its bronze arrows, a melee fighter sells them.
    #[test]
    fn a_ranged_plan_keeps_its_arrows_a_melee_one_sells_them() {
        let archer = super::working_keep(&life_training(&[("ranged", "fight:ranged/cows")]), &[]);
        assert!(archer.contains(&"bronze_arrow".to_string()), "{archer:?}");
        let fighter = super::working_keep(&life_training(&[("attack", "fight:melee/al_kharid_warriors")]), &[]);
        assert!(!fighter.contains(&"bronze_arrow".to_string()), "{fighter:?}");
        let stacks = vec![("bronze_arrow".to_string(), 838), ("iron_dagger".to_string(), 41)];
        let names: Vec<String> = surplus(&stacks, &fighter, &[], &HashSet::new()).into_iter().map(|(name, _, _)| name).collect();
        assert_eq!(names, vec!["bronze_arrow".to_string(), "iron_dagger".to_string()]);
    }

    #[test]
    fn method_products_resolve() {
        assert_eq!(method_product("cook/raw_shrimp"), Some("raw_shrimp"));
        assert_eq!(method_product("smelt:bronze_bar"), Some("bronze_bar"));
        assert_eq!(method_product("spin:ball_of_wool"), Some("ball_of_wool"));
    }
}
