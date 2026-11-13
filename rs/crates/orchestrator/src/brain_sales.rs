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

/// What the unfinished graph still needs on hand, beyond the cleanup
/// keep-list: the inputs (and tools) of every transformation it trains
/// (a cook keeps raw fish, a smelter keeps ore). A transformation's
/// product is SOLD (2026-11-13: producers sell what they make, bars,
/// smithed and crafted goods) unless another open method consumes it.
pub fn working_keep(life: &Life, wished: &[String]) -> Vec<String> {
    let mut keep = crate::brain::cleanup_keep(life);
    let mut products: Vec<String> = Vec::new();
    for node in &life.nodes {
        if matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned) {
            continue;
        }
        if !matches!(node.kind, NodeKind::ReachLevel { .. } | NodeKind::MakeMoney) {
            continue;
        }
        let methods = node.method.iter().cloned().chain(node.alternatives.iter().map(|alt| alt.method.clone()));
        for method in methods {
            // Gatherers sell their catch: only transformers keep inputs.
            if method.starts_with("gather:") || method.starts_with("fight:") || method.starts_with("market:") {
                continue;
            }
            let Some(named) = method_product(&method) else { continue };
            if INPUT_NAMED.iter().any(|prefix| method.starts_with(prefix)) {
                keep.push(named.to_string());
            } else {
                products.push(named.to_string());
            }
            for way in lifescape_gamedata::obtain::ways(named) {
                if way.is_production() {
                    keep.extend(way.inputs.iter().map(|(input, _)| input.clone()));
                    keep.extend(way.tools.iter().cloned());
                }
            }
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

/// Opening price and reserve floor of a listing (2026-11-13): it opens
/// at the reference (or the best bid when a buyer pays more) and walks
/// with demand (`market_reprice`: up while it sells, down while it
/// rests). The reserve is half a traded reference (one listing never
/// dumps under half the market), 1 coin for an untraded object; never
/// an input cost.
pub fn listing_quote(reference: lifescape_market::Reference, best_bid: Option<i64>) -> (i64, i64) {
    let price = lifescape_market::opening_ask(reference.price, best_bid);
    let floor = if reference.traded { (reference.price / 2).max(1) } else { 1 };
    (price, floor.min(price))
}

/// Re-adopts the account's resting listings that walk no more (placed
/// before a daemon restart, or before demand pricing): from now on they
/// follow demand like a fresh listing, their reserve from today's
/// reference.
fn adopt_listings(service: &crate::market_service::MarketService, account: &str, selling: &[lifescape_store::Order]) {
    for row in selling {
        if crate::market_reprice::bounds_of(service, row.order_id).is_some() {
            continue;
        }
        let (_, floor) = listing_quote(service.reference(row.item_id), None);
        let bounds = crate::market_reprice::QuoteBounds {
            account: account.to_string(),
            sell: true,
            floor: floor.min(row.price),
            ceiling: crate::market_reprice::SELL_CAP,
            patience_ticks: crate::market_reprice::PATIENCE_TICKS,
            last_change_wall: wall_now(),
            last_remaining: row.remaining,
        };
        if crate::market_reprice::adopt(service, row.order_id, bounds) {
            crate::logln!("brain {account}: listing {} ({} left @ {}) adopted by demand pricing", row.order_id, row.remaining, row.price);
        }
    }
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
            crate::logln!("brain {account}: listing {order} of {name} cancelled (its last copy of a tool it works with)");
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
    wished.extend(crate::brain_gear::ladder_keep(job, life, &live));
    let stacks = crate::brain_plan::live_stacks(job);
    let tools = crate::brain_tools::kept_tools(life, &stacks);
    wished.extend(tools.iter().cloned());
    let keep = crate::brain_plan::merge_keep(working_keep(life, &wished), worn.clone());
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
    let listed: HashSet<i64> = selling.iter().map(|row| row.item_id).collect();
    // Merchants price their flips themselves (fixed asks over their buy).
    if life.profile.archetype != lifescape_planner::Archetype::Merchant {
        adopt_listings(&service, &job.account, &selling);
    }
    let mut placed = 0;
    let mut offers = surplus(&stacks, &keep, &worn, &listed);
    offers.extend(spare_tools(life, &stacks, &tools, &listed));
    for (name, item, qty) in offers {
        let reference = service.reference(item);
        let (bid, _) = service.lock_engine().books.get(&item).map(|book| book.best()).unwrap_or((None, None));
        let (price, floor) = listing_quote(reference, bid);
        let reply = service.handle(&object(vec![
            ("op", str_value("place")),
            ("account", str_value(&job.account)),
            ("side", str_value("sell")),
            ("item", Value::Int(item)),
            ("qty", Value::Int(qty)),
            ("price", Value::Int(price)),
            ("floor", Value::Int(floor.min(price))),
        ]));
        if reply.get("ok").as_bool() == Some(true) {
            placed += 1;
            crate::logln!("brain {}: listed {qty} {name} @ {price} (floor {floor})", job.account);
        } else {
            crate::logln!("brain {}: listing {qty} {name} refused: {:?}", job.account, reply.get("error").as_str());
        }
    }
    placed
}

#[cfg(test)]
mod tests {
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

    fn life_training(methods: &[(&str, &str)]) -> lifescape_planner::Life {
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

    #[test]
    fn listings_open_at_the_reference() {
        use lifescape_market::Reference;
        assert_eq!(super::listing_quote(Reference { price: 1, traded: false }, None), (1, 1), "bootstrap: 1 coin");
        assert_eq!(super::listing_quote(Reference { price: 40, traded: true }, None), (40, 20), "no margin, reserve half the market");
        assert_eq!(super::listing_quote(Reference { price: 40, traded: true }, Some(55)), (55, 20), "a richer bid is taken");
        assert_eq!(super::listing_quote(Reference { price: 9, traded: false }, None), (9, 1), "untraded: reserve 1 coin");
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

    #[test]
    fn method_products_resolve() {
        assert_eq!(method_product("cook/raw_shrimp"), Some("raw_shrimp"));
        assert_eq!(method_product("smelt:bronze_bar"), Some("bronze_bar"));
        assert_eq!(method_product("spin:ball_of_wool"), Some("ball_of_wool"));
    }
}
