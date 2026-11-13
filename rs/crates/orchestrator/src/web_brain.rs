#![forbid(unsafe_code)]
//! Brain view of one bot (Lives page, "Cerveau" tab): `GET /api/brain/<account>`.
//!
//! One JSON per poll: the ambition (agent D contract, passed through
//! verbatim when the life carries one, `null` otherwise), the current goal
//! and the milestone it came from, every graph node with its deps, the
//! sub-tasks a blocked node filed, its method and costed alternatives,
//! the wishlist with the bid that serves each wish, open orders and
//! deals, the live levels / coins / worn set, the goal history and the
//! skill bans. Read-only: nothing here writes a life or touches a bot.
//!
//! `version` = life file mtime (ms) + a digest of the rendered content,
//! volatile countdowns left out (`ticks_left`, `until_tick`): the front
//! redraws the graph only when it moves. The snapshot `updated` stamp is
//! deliberately not part of it — pushes refresh it every second, which
//! would redraw on every poll.

use lifescape_json::{self as json, object, str_value, Value};
use lifescape_planner::{ban_states, life_from_value, BanState, HistoryEntry, Life, Node, NodeKind, NodeStatus};

use crate::web::{err_json, Dispatch};

/// Every source besides the life file, gathered by `brain_response_in`
/// (tests pass fixtures). Rows keep the daemon command shapes.
#[derive(Debug, Clone)]
pub struct BrainSources {
    /// `live` command state: `skills` (id/xp rows), `combat`, `inv`,
    /// `bank`, `worn`, `ticks`.
    pub live: Value,
    /// `market my_orders` rows (`order`, `side`, `item` id, `qty`,
    /// `left`, `price`, `status`).
    pub orders: Vec<Value>,
    /// `market my_deals` rows (`deal`, `buyer`, `seller`, `item` id,
    /// `qty`, `price`, `status`).
    pub deals: Vec<Value>,
    /// `brain_wishes` rows (`item` debug name, `slot`, `gain`).
    pub wishes: Vec<Value>,
    /// Repricing ceiling per resting order id (in-memory bounds).
    pub ceilings: Vec<(i64, i64)>,
    /// Source failures shown to the operator (never silently empty).
    pub errors: Vec<String>,
    /// Life file modification stamp, milliseconds.
    pub life_mtime_ms: i64,
}

fn arr(rows: Vec<Value>) -> Value {
    Value::Arr(rows)
}

fn node_kind_name(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::ReachLevel { .. } => "reach_level",
        NodeKind::ReachCombat { .. } => "reach_combat",
        NodeKind::ObtainItems { .. } => "obtain_items",
        NodeKind::HaveCoins { .. } => "have_coins",
        NodeKind::MakeMoney => "make_money",
        NodeKind::CompleteQuest { .. } => "complete_quest",
        NodeKind::EquipBest { .. } => "equip_best",
        NodeKind::AccessArea { .. } => "access_area",
        NodeKind::SpaceCleanup => "space_cleanup",
    }
}

/// Short node caption for the graph box: `fishing 20`, `20 × ball_of_wool`.
pub fn node_label(kind: &NodeKind) -> String {
    match kind {
        NodeKind::ReachLevel { skill, level } => format!("{skill} {level}"),
        NodeKind::ReachCombat { level } => format!("combat {level}"),
        NodeKind::ObtainItems { item, count } => format!("{count} × {item}"),
        NodeKind::HaveCoins { amount } => format!("{amount} coins"),
        NodeKind::MakeMoney => "make money".to_string(),
        NodeKind::CompleteQuest { quest } => format!("quest {quest}"),
        NodeKind::EquipBest { style } => format!("equip {style}"),
        NodeKind::AccessArea { area } => format!("area {area}"),
        NodeKind::SpaceCleanup => "bank cleanup".to_string(),
    }
}

fn id_number(id: &str) -> Option<u64> {
    id.strip_prefix('n').and_then(|digits| digits.parse::<u64>().ok())
}

/// Sub-tasks a blocked node filed: the deps created after it (higher
/// `n<k>` id) once it has been blocked at least once. The skeleton only
/// ever points at older ids; `record_block_from` appends fresh ones.
pub fn filed_subtasks(node: &Node) -> Vec<String> {
    if node.blocks == 0 && !node.last_error.starts_with("Blocked") {
        return Vec::new();
    }
    let Some(own) = id_number(&node.id) else { return Vec::new() };
    node.deps.iter().filter(|dep| id_number(dep).is_some_and(|number| number > own)).cloned().collect()
}

fn node_value(node: &Node) -> Value {
    let chosen = node.method.clone().unwrap_or_default();
    let alternatives = node
        .alternatives
        .iter()
        .map(|alt| {
            object(vec![
                ("method", str_value(&alt.method)),
                ("est_ticks", Value::Int(alt.est_ticks as i64)),
                ("est_coins", Value::Int(alt.est_coins)),
                ("chosen", Value::Bool(!chosen.is_empty() && alt.method == chosen)),
            ])
        })
        .collect();
    let item = match &node.kind {
        NodeKind::ObtainItems { item, .. } => str_value(item),
        _ => Value::Null,
    };
    let block = match &node.status {
        NodeStatus::Blocked { reason } => str_value(reason),
        _ => Value::Null,
    };
    let detail = node.status.detail();
    object(vec![
        ("id", str_value(&node.id)),
        ("kind", str_value(node_kind_name(&node.kind))),
        ("label", str_value(&node_label(&node.kind))),
        ("item", item),
        ("status", str_value(&node.status.as_str())),
        ("detail", str_value(&detail)),
        ("progress", arr(vec![Value::Int(node.progress_current), Value::Int(node.progress_target)])),
        ("deps", arr(node.deps.iter().map(|dep| str_value(dep)).collect())),
        ("filed", arr(filed_subtasks(node).iter().map(|dep| str_value(dep)).collect())),
        ("method", node.method.as_deref().map(str_value).unwrap_or(Value::Null)),
        ("alternatives", arr(alternatives)),
        ("block", block),
        ("last_error", str_value(&node.last_error)),
        ("failures", Value::Int(i64::from(node.failures))),
        ("blocks", Value::Int(i64::from(node.blocks))),
        ("revives", Value::Int(i64::from(node.revives))),
        ("spent_ticks", Value::Int(node.spent_ticks as i64)),
    ])
}

/// (debug name, display name) of an object id; empty when unknown.
fn item_names(item_id: i64) -> (String, String) {
    let obj = usize::try_from(item_id).ok().and_then(lifescape_gamedata::obj_by_id);
    let debug = obj.and_then(|obj| obj.name).unwrap_or("").to_string();
    let display = obj.and_then(|obj| obj.display).unwrap_or("").to_string();
    (debug, display)
}

/// Live summary: base levels by skill, combat level, coins carried and
/// banked, worn debug names. Empty without a snapshot (fresh daemon).
pub fn live_view(live: &Value) -> Value {
    let levels = crate::brain_market::live_levels(live);
    let mut level_map = std::collections::BTreeMap::new();
    for (skill, level) in &levels {
        level_map.insert(skill.clone(), Value::Int(i64::from(*level)));
    }
    let combat = if levels.is_empty() && live.get("combat").is_null() {
        Value::Null
    } else {
        Value::Int(i64::from(crate::brain_plan::live_combat(live)))
    };
    let mut coins = 0i64;
    let mut seen_stacks = false;
    for key in ["inv", "bank"] {
        for entry in live.get(key).as_arr().cloned().unwrap_or_default() {
            seen_stacks = true;
            if crate::brain_plan::live_debug_name(&entry).as_deref() == Some("coins") {
                coins = coins.saturating_add(entry.get("count").as_i64().unwrap_or(1).max(0));
            }
        }
    }
    let worn: Vec<Value> = live
        .get("worn")
        .as_arr()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|entry| entry.get("count").as_i64().unwrap_or(1) > 0)
        .filter_map(crate::brain_plan::live_debug_name)
        .map(|name| str_value(&name))
        .collect();
    object(vec![
        ("levels", Value::Obj(level_map)),
        ("combat", combat),
        ("coins", if seen_stacks || !levels.is_empty() { Value::Int(coins) } else { Value::Null }),
        ("worn", arr(worn)),
    ])
}

fn order_value(row: &Value, ceilings: &[(i64, i64)]) -> Value {
    let order_id = row.get("order").as_i64().unwrap_or(-1);
    let item_id = row.get("item").as_i64().unwrap_or(-1);
    let (debug, display) = item_names(item_id);
    let ceiling = ceilings.iter().find(|(order, _)| *order == order_id).map(|(_, ceiling)| Value::Int(*ceiling));
    object(vec![
        ("order_id", Value::Int(order_id)),
        ("side", row.get("side").clone()),
        ("item", str_value(&debug)),
        ("item_id", Value::Int(item_id)),
        ("name", str_value(&display)),
        ("qty", row.get("qty").clone()),
        ("remaining", row.get("left").clone()),
        ("price", row.get("price").clone()),
        ("ceiling", ceiling.unwrap_or(Value::Null)),
        ("status", row.get("status").clone()),
    ])
}

fn deal_value(row: &Value, account: &str) -> Value {
    let buyer = row.get("buyer").as_str().unwrap_or("");
    let seller = row.get("seller").as_str().unwrap_or("");
    let (role, partner) = if buyer == account { ("buyer", seller) } else { ("seller", buyer) };
    let item_id = row.get("item").as_i64().unwrap_or(-1);
    let (debug, display) = item_names(item_id);
    object(vec![
        ("deal_id", row.get("deal").clone()),
        ("role", str_value(role)),
        ("partner", str_value(partner)),
        ("item", str_value(&debug)),
        ("item_id", Value::Int(item_id)),
        ("name", str_value(&display)),
        ("qty", row.get("qty").clone()),
        ("price", row.get("price").clone()),
        ("status", row.get("status").clone()),
    ])
}

/// One wish with the open buy order that serves it (same object), if any.
fn wish_value(wish: &Value, orders: &[Value]) -> Value {
    let item = wish.get("item").as_str().unwrap_or("");
    let bid = orders
        .iter()
        .find(|order| {
            order.get("side").as_str() == Some("buy")
                && order.get("status").as_str() == Some("open")
                && order.get("item").as_str() == Some(item)
        })
        .map(|order| {
            object(vec![
                ("order_id", order.get("order_id").clone()),
                ("price", order.get("price").clone()),
                ("ceiling", order.get("ceiling").clone()),
            ])
        })
        .unwrap_or(Value::Null);
    object(vec![
        ("item", str_value(item)),
        ("slot", wish.get("slot").clone()),
        ("gain", wish.get("gain").clone()),
        ("bid", bid),
    ])
}

/// Kind of a `node: "goal"` history outcome.
pub fn goal_entry_kind(outcome: &str) -> &'static str {
    if outcome.starts_with("next:") {
        "next"
    } else if outcome.starts_with(lifescape_planner::DONE_TAG) {
        "done"
    } else if outcome.starts_with(lifescape_planner::STUCK_TAG) {
        "stuck"
    } else if outcome.starts_with(lifescape_planner::UNBAN_TAG) {
        "unban"
    } else {
        "other"
    }
}

fn goal_history(history: &[HistoryEntry]) -> Value {
    let rows: Vec<Value> = history
        .iter()
        .filter(|entry| entry.node == "goal")
        .map(|entry| {
            object(vec![
                ("at", Value::Int(entry.ended)),
                ("kind", str_value(goal_entry_kind(&entry.outcome))),
                ("outcome", str_value(&entry.outcome)),
            ])
        })
        .collect();
    let skip = rows.len().saturating_sub(60);
    arr(rows.into_iter().skip(skip).collect())
}

/// Every skill whose latest ban is not lifted: active bans with the
/// ticks left (and the bot tick it ends at when the snapshot has one),
/// expired-but-unnoted ones flagged inactive.
fn banned_view(history: &[HistoryEntry], ticks_since: &dyn Fn(i64) -> u64, live_ticks: Option<i64>) -> Value {
    let rows = ban_states(history, ticks_since)
        .into_iter()
        .map(|(skill, state)| match state {
            BanState::Active { ticks_left, cause } => object(vec![
                ("skill", str_value(&skill)),
                ("active", Value::Bool(true)),
                ("ticks_left", Value::Int(ticks_left as i64)),
                (
                    "until_tick",
                    live_ticks.map(|now| Value::Int(now.saturating_add(ticks_left as i64))).unwrap_or(Value::Null),
                ),
                ("cause", str_value(&cause)),
            ]),
            BanState::Expired { cause, .. } => object(vec![
                ("skill", str_value(&skill)),
                ("active", Value::Bool(false)),
                ("ticks_left", Value::Int(0)),
                ("until_tick", Value::Null),
                ("cause", str_value(&cause)),
            ]),
        })
        .collect();
    arr(rows)
}

fn fnv64(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// Version of a payload: life mtime + digest of everything shown, the
/// ban countdowns excepted (they move every poll, the graph does not).
pub fn brain_version(life_mtime_ms: i64, payload: &Value) -> String {
    let mut stable = payload.clone();
    if let Value::Obj(map) = &mut stable {
        map.remove("version");
        if let Some(Value::Arr(bans)) = map.get_mut("banned_skills") {
            for ban in bans.iter_mut() {
                if let Value::Obj(fields) = ban {
                    fields.remove("ticks_left");
                    fields.remove("until_tick");
                }
            }
        }
    }
    format!("{life_mtime_ms}-{:016x}", fnv64(&json::stringify(&stable)))
}

/// The whole view from a parsed life file and its sources. A life the
/// planner cannot read is a loud error (never a half-drawn brain).
pub fn brain_payload(account: &str, life_value: &Value, sources: &BrainSources, ticks_since: &dyn Fn(i64) -> u64) -> Value {
    let Some(life) = life_from_value(life_value) else {
        return err_json(format!("life {account} unreadable (schema {:?})", life_value.get("schema").as_i64()));
    };
    let orders: Vec<Value> = sources.orders.iter().map(|row| order_value(row, &sources.ceilings)).collect();
    let deals: Vec<Value> = sources.deals.iter().map(|row| deal_value(row, account)).collect();
    let wishes: Vec<Value> = sources.wishes.iter().map(|wish| wish_value(wish, &orders)).collect();
    let mut payload = object(vec![
        ("ok", Value::Bool(true)),
        ("account", str_value(account)),
        ("ambition", life_value.get("ambition").clone()),
        ("goal", goal_value(&life, life_value)),
        ("active", life.active.as_deref().map(str_value).unwrap_or(Value::Null)),
        ("nodes", arr(life.nodes.iter().map(node_value).collect())),
        ("wishes", arr(wishes)),
        ("orders", arr(orders)),
        ("deals", arr(deals)),
        ("live", live_view(&sources.live)),
        ("goal_history", goal_history(&life.history)),
        ("banned_skills", banned_view(&life.history, ticks_since, sources.live.get("ticks").as_i64())),
        ("errors", arr(sources.errors.iter().map(|error| str_value(error)).collect())),
    ]);
    let version = brain_version(sources.life_mtime_ms, &payload);
    if let Value::Obj(map) = &mut payload {
        map.insert("version".to_string(), str_value(&version));
    }
    payload
}

fn goal_value(life: &Life, life_value: &Value) -> Value {
    object(vec![
        ("label", str_value(&crate::web_job_lives::goal_text(&life.goal))),
        ("origin_milestone", life_value.get("goal_origin").clone()),
        ("raw", life_value.get("goal").clone()),
    ])
}

fn rows_of(reply: &Value, key: &str, source: &str, errors: &mut Vec<String>) -> Vec<Value> {
    if reply.get("ok").as_bool() == Some(false) {
        errors.push(format!("{source}: {}", reply.get("error").as_str().unwrap_or("failed")));
        return Vec::new();
    }
    reply.get(key).as_arr().cloned().unwrap_or_default()
}

/// Ticks since a history stamp with the daemon's measured tick length.
fn live_ticks_since(stamp: i64) -> u64 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0);
    crate::brain_stuck::seconds_to_ticks(now - stamp)
}

/// `GET /api/brain/<account>` on the daemon's run root.
pub fn brain_response(account: &str, dispatch: &Dispatch) -> Value {
    brain_response_in(&crate::paths::run_dir(), account, &**dispatch, &live_ticks_since)
}

/// Brain view over an explicit run root and dispatcher (tests).
pub fn brain_response_in(
    run_dir: &std::path::Path,
    account: &str,
    dispatch: &(dyn Fn(Value) -> Value + Send + Sync),
    ticks_since: &dyn Fn(i64) -> u64,
) -> Value {
    if account.is_empty() || account == "." || account == ".." || account.contains('/') {
        return err_json(format!("bad account '{account}'"));
    }
    let path = run_dir.join("lives").join(format!("{account}.json"));
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return err_json(format!("no life for {account}")),
    };
    let life_value = match json::parse(&text) {
        Ok(value) => value,
        Err(error) => return err_json(format!("life {account} corrupt: {error}")),
    };
    let life_mtime_ms = std::fs::metadata(&path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|span| span.as_millis() as i64)
        .unwrap_or(0);
    let mut errors = Vec::new();
    let request = |pairs: Vec<(&str, Value)>| dispatch(object(pairs));
    let live = request(vec![("cmd", str_value("live")), ("account", str_value(account))]);
    if live.get("ok").as_bool() == Some(false) {
        errors.push(format!("live: {}", live.get("error").as_str().unwrap_or("failed")));
    }
    let market = |op: &str| request(vec![("cmd", str_value("market")), ("op", str_value(op)), ("account", str_value(account))]);
    let orders = rows_of(&market("my_orders"), "orders", "orders", &mut errors);
    let deals = rows_of(&market("my_deals"), "deals", "deals", &mut errors);
    let wishes = rows_of(&request(vec![("cmd", str_value("brain_wishes")), ("account", str_value(account))]), "wishes", "wishes", &mut errors);
    let ceilings = match crate::market_service::shared() {
        Some(service) => orders
            .iter()
            .filter_map(|row| row.get("order").as_i64())
            .filter_map(|order| crate::market_reprice::bounds_of(&service, order).filter(|bounds| !bounds.sell).map(|bounds| (order, bounds.ceiling)))
            .collect(),
        None => Vec::new(),
    };
    let sources = BrainSources { live: live.get("state").clone(), orders, deals, wishes, ceilings, errors, life_mtime_ms };
    brain_payload(account, &life_value, &sources, ticks_since)
}

/// `brain_wishes` daemon command: the brain's own wishlist for a job
/// (same computation as the bid pass, read-only). No job or no life =
/// no wishes (the wishlist needs a live snapshot either way).
pub(crate) fn wishes_reply(job: Option<&crate::jobs::Job>, account: &str) -> Value {
    let rows = match (job, crate::brain::load_life(account)) {
        (Some(job), Some(life)) => crate::brain_wishlist::wishlist(job, &life)
            .into_iter()
            .map(|wish| {
                object(vec![
                    ("item", str_value(&wish.item)),
                    ("slot", str_value(&wish.slot)),
                    ("gain", Value::Int(wish.gain)),
                ])
            })
            .collect(),
        _ => Vec::new(),
    };
    object(vec![("ok", Value::Bool(true)), ("wishes", Value::Arr(rows))])
}

#[cfg(test)]
#[path = "web_brain_tests.rs"]
mod tests;
