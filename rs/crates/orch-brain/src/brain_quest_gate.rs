#![forbid(unsafe_code)]
//! Quest gate of the brain (user rule 2026-11-14: "il faut les items
//! prérequis avant de lancer une quête", and never a silent loop).
//!
//! Live 2026-11-14 (bh99tw2f, a merchant selling air talismans): the
//! quest's own talisman sat in the bank with its nine siblings, all ten
//! locked by the bot's listings; the Duke never re-issues one while any
//! is held, so Sedridor refused round after round, 13 000 log lines.
//!
//! Every brain pass:
//! - `publish_reserved`: the open quests' objects go to the market
//!   service, which never lets a listing claim them (`free_coverage`);
//! - `hold_pass`, for each ready quest node: a cooled quest waits; units
//!   held but locked by the bot's own listings are released (listing cut,
//!   the rest re-listed at its price) and the node waits one pass; a
//!   brought item missing is obtained first (`record_block_from`, graph
//!   nodes like any dependency). Waiting nodes are skipped like parked
//!   market buys (the filler covers the wait, no idle bot).
//!
//! The pure rules live in `lifescape_planner::quest_hold`.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};

use lifescape_json::{object, str_value, Value};
use lifescape_planner::{Life, NodeKind, NodeStatus, QuestShortfall};
use lifescape_store::OrderSide;

use crate::brain::push_history;
use crate::jobs::Job;
use crate::market_service::MarketService;

/// Base object id of a debug name.
fn base_id(name: &str) -> Option<i64> {
    let obj = lifescape_gamedata::objs_by_name(name).into_iter().next()?;
    Some(crate::market_audit::base_of(obj.obj_id as i64))
}

/// Reservation by base object id (unknown names dropped).
pub fn reserved_ids(reserved: &[(String, i64)]) -> BTreeMap<i64, i64> {
    let mut out: BTreeMap<i64, i64> = BTreeMap::new();
    for (name, units) in reserved {
        if let Some(id) = base_id(name) {
            *out.entry(id).or_insert(0) += (*units).max(0);
        }
    }
    out
}

/// Hands the open quests' objects to the market service (every pass).
pub(crate) fn publish_reserved(account: &str, life: &Life, deps: &crate::brain_deps::BrainDeps) {
    let Some(service) = deps.market.clone() else { return };
    service.set_quest_reserved(account, reserved_ids(&lifescape_planner::reserved_quest_objects(life)));
}

/// Listings `(order, remaining)` to cut so `need` units come free when
/// `free` are: `(order, cut)`, newest listing first. A cut short of the
/// remainder re-lists the rest at the same price.
pub fn release_plan(listings: &[(i64, i64)], free: i64, need: i64) -> Vec<(i64, i64)> {
    let mut short = need - free.max(0);
    let mut sorted: Vec<(i64, i64)> = listings.to_vec();
    sorted.sort_by(|left, right| right.0.cmp(&left.0));
    let mut out = Vec::new();
    for (order, remaining) in sorted {
        if short <= 0 {
            break;
        }
        if remaining <= 0 {
            continue;
        }
        let cut = remaining.min(short);
        out.push((order, cut));
        short -= cut;
    }
    out
}

/// Cuts `account`'s resting listings of `item` so `need` units come
/// free (`free` now). Returns one log fragment per touched listing.
pub(crate) fn release_listings(service: &MarketService, account: &str, item: &str, free: i64, need: i64) -> Vec<String> {
    let Some(base) = base_id(item) else { return Vec::new() };
    let rows: Vec<lifescape_store::Order> = service
        .read_store()
        .account_orders(account)
        .unwrap_or_default()
        .into_iter()
        .filter(|row| {
            row.side == OrderSide::Sell
                && row.remaining > 0
                && crate::market_audit::base_of(row.item_id) == base
                && matches!(row.status, lifescape_store::OrderStatus::Open | lifescape_store::OrderStatus::Matched)
        })
        .collect();
    let listings: Vec<(i64, i64)> = rows.iter().map(|row| (row.order_id, row.remaining)).collect();
    let mut notes = Vec::new();
    for (order, cut) in release_plan(&listings, free, need) {
        let Some(row) = rows.iter().find(|row| row.order_id == order) else { continue };
        let floor = service.lock_engine().quote_bounds.get(&order).map(|bounds| bounds.floor).unwrap_or(row.price);
        let reply = service.handle(&object(vec![("op", str_value("cancel")), ("account", str_value(account)), ("order", Value::Int(order))]));
        if reply.get("ok").as_bool() != Some(true) {
            notes.push(format!("listing {order} not released ({})", reply.get("error").as_str().unwrap_or("?")));
            continue;
        }
        let rest = row.remaining - cut;
        if rest <= 0 {
            notes.push(format!("listing {order} ({} {item}) cancelled", row.remaining));
            continue;
        }
        let placed = service.handle(&object(vec![
            ("op", str_value("place")),
            ("account", str_value(account)),
            ("side", str_value("sell")),
            ("item", Value::Int(row.item_id)),
            ("qty", Value::Int(rest)),
            ("price", Value::Int(row.price)),
            ("floor", Value::Int(floor.min(row.price))),
            ("motive", str_value(&row.motive)),
            // The carnet's price and the walk's floor: per lot already.
            ("per_lot", Value::Bool(true)),
        ]));
        match placed.get("order").as_i64() {
            Some(new_order) => notes.push(format!("listing {order} cut by {cut} {item} (rest {rest} re-listed as {new_order} @ {})", row.price)),
            None => notes.push(format!("listing {order} ({} {item}) cancelled, rest not re-listed ({})", row.remaining, placed.get("error").as_str().unwrap_or("?"))),
        }
    }
    notes
}

/// Sales-pass side of the reservation: every reserved object the bot
/// holds (`stacks`: pack + bank) but whose free part (held minus what
/// its listings lock) falls short is released up to what it holds.
/// Returns the number of listings touched.
pub(crate) fn release_for_quests(service: &MarketService, account: &str, stacks: &[(String, i64)], reserved: &[(String, i64)], deps: &crate::brain_deps::BrainDeps) -> usize {
    if reserved.is_empty() {
        return 0;
    }
    let locks = crate::brain_locks::locked_items_in(deps.market.as_deref(), account);
    let mut touched = 0;
    for (name, units) in reserved {
        let held: i64 = stacks.iter().filter(|(stack, _)| stack == name).map(|(_, qty)| *qty).sum();
        let need = (*units).min(held);
        let free = crate::brain_locks::free_count(held, crate::brain_locks::locked_in(&locks, name));
        if need <= 0 || free >= need {
            continue;
        }
        let notes = release_listings(service, account, name, free, need);
        if !notes.is_empty() {
            crate::logln!("brain {account}: open quest keeps {need} {name} ({held} held, {free} free): {}", notes.join(", "));
            touched += notes.len();
        }
    }
    touched
}

/// Last wait note per account and node (one log line per change).
fn last_note() -> &'static Mutex<HashMap<String, String>> {
    static TABLE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn note_once(account: &str, node: &str, text: &str) {
    let key = format!("{account}/{node}");
    let mut table = last_note().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if table.get(&key).map(String::as_str) != Some(text) {
        crate::logln!("brain {account}: {text}");
        table.insert(key, text.to_string());
    }
}

/// What the gate does with one ready quest node.
#[derive(Debug, Clone, PartialEq)]
pub enum GateMove {
    /// Every object at hand: launch.
    Launch,
    /// Units held but listed: release those listings, wait a pass.
    Release(Vec<(String, i64, i64)>),
    /// Brought items short: obtain them first (graph nodes).
    Obtain(Vec<String>),
    /// Holdings unknown (bank never streamed): wait.
    Unknown,
}

/// Pure gate on the shortfalls of one quest. A missing brought item is
/// obtained first; locked units are released (both when both apply:
/// the release rides along the block).
pub fn gate_move(gaps: &[QuestShortfall], bank_known: bool) -> GateMove {
    if gaps.is_empty() {
        return GateMove::Launch;
    }
    let missing: Vec<String> = gaps
        .iter()
        .filter_map(|gap| match gap {
            QuestShortfall::Missing { item, .. } => Some(item.clone()),
            QuestShortfall::Locked { .. } => None,
        })
        .collect();
    if !missing.is_empty() {
        return if bank_known { GateMove::Obtain(missing) } else { GateMove::Unknown };
    }
    GateMove::Release(
        gaps.iter()
            .filter_map(|gap| match gap {
                QuestShortfall::Locked { item, need, free, .. } => Some((item.clone(), *need, *free)),
                QuestShortfall::Missing { .. } => None,
            })
            .collect(),
    )
}

/// The quest gate over every ready quest node. Returns whether the graph
/// changed (an obtain filed) and the node ids that wait this pass.
pub(crate) fn hold_pass(job: &Job, life: &mut Life, live: &Value, deps: &crate::brain_deps::BrainDeps) -> (bool, Vec<String>) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0);
    let cooled = lifescape_planner::cooled_quests(&life.history, &|stamp: i64| crate::brain_stuck::seconds_to_ticks(now - stamp));
    let bank_known = live.get("bank_streamed").as_bool() == Some(true);
    let ready: Vec<(usize, String, String)> = life
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.status == NodeStatus::Ready)
        .filter_map(|(position, node)| match &node.kind {
            NodeKind::CompleteQuest { quest } if quest != "tutorial" => Some((position, node.id.clone(), quest.clone())),
            _ => None,
        })
        .collect();
    let mut changed = false;
    let mut waits = Vec::new();
    for (position, id, quest) in ready {
        // A block filed above may have moved this one (normalize).
        if life.nodes[position].status != NodeStatus::Ready {
            continue;
        }
        if cooled.contains(&quest) {
            note_once(&job.account, &id, &format!("quest {quest} ({id}) cooling after a failure, waiting it out"));
            waits.push(id);
            continue;
        }
        let count_of = |name: &str| (crate::brain_plan::raw_holdings_count(job, name), crate::brain_plan::holdings_count(job, name, deps));
        let gaps = lifescape_planner::quest_shortfalls(&quest, &count_of);
        let reasons: Vec<String> = gaps.iter().map(QuestShortfall::describe).collect();
        match gate_move(&gaps, bank_known) {
            GateMove::Launch => {}
            GateMove::Release(locked) => {
                let mut notes = Vec::new();
                if let Some(service) = deps.market.clone() {
                    for (item, need, free) in &locked {
                        notes.extend(release_listings(&service, &job.account, item, *free, *need));
                    }
                }
                // The book answers at once: freed now, the quest launches
                // this pass (the worker's locks follow within a market
                // round, long before its first bank stop).
                let freed = lifescape_planner::quest_shortfalls(&quest, &count_of).is_empty();
                let released = if notes.is_empty() { "no listing to release yet".to_string() } else { notes.join(", ") };
                if freed {
                    crate::logln!("brain {}: quest {quest} ({id}) objects freed for launch: {}; {released}", job.account, reasons.join("; "));
                } else {
                    note_once(&job.account, &id, &format!("quest {quest} ({id}) waits: {}; {released}", reasons.join("; ")));
                    waits.push(id);
                }
            }
            GateMove::Unknown => {
                note_once(&job.account, &id, &format!("quest {quest} ({id}) waits: bank not streamed yet ({})", reasons.join("; ")));
                waits.push(id);
            }
            GateMove::Obtain(missing) => {
                let level_of = |skill: &str| crate::brain_succession::live_level(live, skill);
                let view = crate::brain_plan_read::BotView::of(life);
                let fold = crate::brain_plan_read::with_plan_read(&view, live, |read| lifescape_planner::record_block_with(life, position, &missing, &level_of, read), deps);
                push_history(life, &id, crate::brain_launch::block_outcome(&fold));
                crate::logln!("brain {}: quest {quest} ({id}) not launched: {} -> obtained first", job.account, reasons.join("; "));
                lifescape_planner::normalize(life);
                changed = true;
                if let lifescape_planner::FailureFold::CycleBroken(line) = &fold {
                    // Nodes may have left the graph: the positions below
                    // are stale, the next pass sees the rest.
                    crate::logln!("brain {}: {line}", job.account);
                    break;
                }
            }
        }
    }
    (changed, waits)
}

#[cfg(test)]
#[path = "brain_quest_gate_tests.rs"]
mod tests;
