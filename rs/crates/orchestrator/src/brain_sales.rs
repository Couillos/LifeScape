#![forbid(unsafe_code)]
//! Default selling (PRD3 lot 3). Every product a bot does not need goes
//! to the market, never to an NPC shop: the brain lists the bot's
//! unlisted tradeable surplus (pack + bank) directly on the daemon
//! market, priced at the market (10% over the reference, or the best bid
//! when richer) and walking down to a reserve floor (`market_reprice`).
//! The physical handover stays the convene loop's job.
//!
//! Protected (never listed): coins, food up to 100 units, the graph
//! keep-list (obtain items, quest checks), the inputs and products of the
//! skill being trained (raw fish for a cook, ore for a smelter), tools,
//! worn gear, wanted upgrades. Iron accounts never list.

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

/// What the unfinished graph still needs on hand, beyond the cleanup
/// keep-list: every skill node's product and the inputs of the recipes
/// making it (a cook keeps raw fish, a smelter keeps ore).
pub fn working_keep(life: &Life, wished: &[String]) -> Vec<String> {
    let mut keep = crate::brain::cleanup_keep(life);
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
            let Some(product) = method_product(&method) else { continue };
            keep.push(product.to_string());
            for way in lifescape_gamedata::obtain::ways(product) {
                if way.is_production() {
                    keep.extend(way.inputs.iter().map(|(input, _)| input.clone()));
                    keep.extend(way.tools.iter().cloned());
                }
            }
        }
    }
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
    let wished: Vec<String> = crate::brain_wishlist::wishlist(job, life).into_iter().map(|wish| wish.item).collect();
    let keep = crate::brain_plan::merge_keep(working_keep(life, &wished), worn.clone());
    let listed: HashSet<i64> = service
        .lock_store()
        .account_orders(&job.account)
        .unwrap_or_default()
        .iter()
        .filter(|row| row.side == lifescape_store::OrderSide::Sell && row.status == lifescape_store::OrderStatus::Open)
        .map(|row| row.item_id)
        .collect();
    let stacks = crate::brain_plan::live_stacks(job);
    let mut placed = 0;
    for (name, item, qty) in surplus(&stacks, &keep, &worn, &listed) {
        let reference = service.reference(item);
        let (bid, _) = service.lock_engine().books.get(&item).map(|book| book.best()).unwrap_or((None, None));
        let asking = reference.price + (reference.price / 10).max(if reference.price > 1 { 1 } else { 0 });
        let price = asking.max(bid.unwrap_or(0)).max(1);
        let floor = if reference.traded { (reference.price / 2).max(1) } else { 1 };
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
    fn surplus_skips_needs_tools_food_and_listed() {
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
        assert_eq!(names, vec!["bronze_bar".to_string(), "logs".to_string()], "coins, food under 100, keep, tools and listed stay");
    }

    #[test]
    fn method_products_resolve() {
        assert_eq!(method_product("cook/raw_shrimp"), Some("raw_shrimp"));
        assert_eq!(method_product("smelt:bronze_bar"), Some("bronze_bar"));
        assert_eq!(method_product("spin:ball_of_wool"), Some("ball_of_wool"));
    }
}
