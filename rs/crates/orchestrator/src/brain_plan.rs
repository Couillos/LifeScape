#![forbid(unsafe_code)]
//! Brain planning context (split from brain.rs, same behavior):
//! public-data inputs for node resolution (money-method pick, price and
//! yield context, wallet and bank gauges). Never another bot's state.

use crate::jobs::Job;
use lifescape_json::Value;
use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::{Alternative, Life, Node, NodeKind, NodeStatus};

/// Fixed round-trip estimate to Varrock for cost comparisons.
// shortcut: fixed 800-tick round trip; replaced by measured convene
// ETAs once the brain reads them (L8).
pub const TRIP_TICKS: u64 = 800;

/// Pure money-method pick over published profits: the trend bonus
/// (score × revenue × weight) can flip a close call toward a shortage
/// item without ever reading another bot.
pub fn choose_money(profits: &[(String, f64, f64, f64)], trend_weight: f64) -> Option<String> {
    let mut best: Option<(String, f64)> = None;
    for (method, profit, revenue, trend) in profits {
        let score = lifescape_planner::rank_money_method(*profit, trend.clamp(-1.0, 1.0), *revenue, trend_weight);
        if best.as_ref().is_none_or(|(_, top)| score > *top) {
            best = Some((method.clone(), score));
        }
    }
    best.map(|(method, _)| method)
}

fn display_of(item_debug: &str) -> String {
    lifescape_gamedata::objs_by_name(item_debug)
        .into_iter()
        .next()
        .and_then(|entry| entry.display.or(entry.name))
        .unwrap_or(item_debug)
        .to_string()
}

fn shop_floor(item_debug: &str) -> i64 {
    let mut floor = i64::MAX;
    for shop in lifescape_gamedata::f2p::f2p_shops() {
        for stock in &shop.stocks {
            if stock.obj == item_debug {
                // Static shop price lookup by debug name.
                if let Some(entry) = lifescape_gamedata::objs_by_name(item_debug).into_iter().next() {
                    floor = floor.min(entry.price.max(1));
                }
            }
        }
    }
    if floor == i64::MAX { 1 } else { floor }
}

/// Public-data planning context for one node: market reference (or shop
/// fallback), published yields (or cold-start priors), trend score.
pub(crate) fn ready_ctx(life: &Life, node: &Node) -> ReadyCtx {
    let mut ctx = ReadyCtx::blind(life.profile.market_affinity);
    ctx.ironman = life.profile.ironman;
    ctx.trip_ticks = TRIP_TICKS;
    let (item_debug, band) = match &node.kind {
        NodeKind::ObtainItems { item, .. } => (item.clone(), 1),
        NodeKind::ReachLevel { level, .. } => (String::new(), crate::rates_service::level_band(i64::from(*level))),
        _ => (String::new(), 1),
    };
    let snapshot = crate::rates_service::live_snapshot();
    if let Ok(store) = lifescape_store::Store::open(&crate::market_service::db_path()) {
        // Best published coin rate prices the grind (fallback 0.5).
        let profits = crate::rates_service::profit_rows(&store, &snapshot);
        if let Some(top) = profits.iter().map(|row| row.profit).max_by(|left, right| left.total_cmp(right)) {
            if top > 0.0 {
                ctx.coins_per_tick = (top / crate::rates_service::NORMALIZE_TICKS).max(0.01);
            }
        }
        if !item_debug.is_empty() {
            let display = display_of(&item_debug);
            let (price, _) = crate::rates_service::price_estimate(&store, &display);
            ctx.price = price.max(1.0) as i64;
            ctx.shop_price = shop_floor(&item_debug);
            let farm_method = node.alternatives.iter().find(|alt| alt.method.starts_with("farm:")).map(|alt| alt.method.clone()).unwrap_or_default();
            let key_item = if farm_method.starts_with("farm:") { display.clone() } else { display.clone() };
            ctx.yield_per_6000 = crate::rates_service::measured_yield(&snapshot, &farm_method, &key_item, band)
                .or_else(|| crate::rates_service::prior_for(&farm_method, &key_item.to_lowercase()).map(|(yield_per, _)| yield_per))
                .unwrap_or(200.0);
        } else if matches!(&node.kind, NodeKind::MakeMoney) {
            // Money leaves price the best published profit; the flip leg
            // stays a flat engagement (measured once merchants report).
            let _ = &snapshot;
        }
    }
    ctx
}

pub(crate) fn holdings_count(job: &Job, item_debug: &str) -> i64 {
    let display = display_of(item_debug).to_lowercase();
    let debug = item_debug.to_lowercase();
    let live = job.inner.try_lock().map(|inner| inner.live.clone()).unwrap_or(None).unwrap_or(Value::Null);
    let mut total = 0i64;
    for key in ["inv", "bank", "worn"] {
        for entry in live.get(key).as_arr().cloned().unwrap_or_default() {
            match &entry {
                Value::Obj(_) => {
                    let name = entry.get("name").as_str().unwrap_or("").to_lowercase();
                    if name == display || name == debug {
                        total += entry.get("count").as_i64().unwrap_or(1).max(0);
                    }
                }
                Value::Str(name) if name.to_lowercase() == display || name.to_lowercase() == debug => {
                    total += 1;
                }
                _ => {}
            }
        }
    }
    total
}

/// Distinct banked types with a positive count (the space-pressure gauge,
/// L8: cleanup at 55 of 60 F2P slots). Name or id keys the type, so both
/// live snapshot shapes count.
pub(crate) fn bank_types(job: &Job) -> usize {
    let live = job.inner.try_lock().map(|inner| inner.live.clone()).unwrap_or(None).unwrap_or(Value::Null);
    let mut types = std::collections::HashSet::new();
    for entry in live.get("bank").as_arr().cloned().unwrap_or_default() {
        match &entry {
            Value::Obj(_) => {
                if entry.get("count").as_i64().unwrap_or(1) <= 0 {
                    continue;
                }
                if let Some(name) = entry.get("name").as_str() {
                    types.insert(name.to_lowercase());
                } else if let Some(id) = entry.get("id").as_i64() {
                    types.insert(format!("id{id}"));
                }
            }
            Value::Str(name) => {
                types.insert(name.to_lowercase());
            }
            _ => {}
        }
    }
    types.len()
}

/* P1.6 D52: sale gate (worn, protected tools, graph items never list). */
/// D52 protected tool debug names (kit orders + tinderbox): never sold by flip or
/// cleanup, whether carried, worn or banked. Debug names, as the keep
/// list and the live snapshots carry them.
pub const PROTECTED_TOOLS: &[&str] = &[
    "rune_axe", "adamant_axe", "mithril_axe", "black_axe", "steel_axe", "iron_axe", "bronze_axe",
    "rune_pickaxe", "adamant_pickaxe", "mithril_pickaxe", "steel_pickaxe", "iron_pickaxe", "bronze_pickaxe",
    "lobster_pot", "harpoon", "fishing_rod", "fly_fishing_rod", "net",
    "tinderbox",
];

/// D52 tool gate (case-insensitive): the flip auto-pick sold its own
/// net because it only excluded worn ids, never carried tools.
pub fn is_protected_tool(name: &str) -> bool {
    PROTECTED_TOOLS.iter().any(|tool| tool.eq_ignore_ascii_case(name))
}

/// D52 sale gate: true when `name` must never list (worn, tools, graph keep, coins).
pub fn sale_blocked(name: &str, keep: &[String], worn: &[String]) -> bool {
    if name.eq_ignore_ascii_case("coins") {
        return true;
    }
    if is_protected_tool(name) {
        return true;
    }
    if worn.iter().any(|held| held.eq_ignore_ascii_case(name)) {
        return true;
    }
    keep.iter().any(|kept| kept.eq_ignore_ascii_case(name))
}

/// Merge the graph keep-list with the live worn names (D52): sorted, deduplicated.
pub fn merge_keep(graph: Vec<String>, worn: Vec<String>) -> Vec<String> {
    let mut keep = graph;
    keep.extend(worn);
    keep.sort_by_key(|name| name.to_lowercase());
    keep.dedup_by(|second, first| second.eq_ignore_ascii_case(first));
    keep
}

/// D52 flip pick: richest F2P tradeable stack outside the sale gate.
/// `None` means engagement sans objet (the clock ends it, no doomed order).
pub fn pick_flip_target(
    stacks: &[(String, i64)],
    keep: &[String],
    worn: &[String],
) -> Option<(String, i64)> {
    let mut best: Option<(String, i64, i64)> = None;
    for (name, qty) in stacks {
        if *qty <= 0 || sale_blocked(name, keep, worn) {
            continue;
        }
        let Some(entry) = lifescape_gamedata::objs_by_name(name).into_iter().next() else {
            continue;
        };
        if !entry.tradeable || entry.members == Some(true) {
            continue;
        }
        let value = entry.price.max(1).saturating_mul(*qty);
        if best.as_ref().is_none_or(|(_, _, top)| value > *top) {
            best = Some((name.clone(), *qty, value));
        }
    }
    best.map(|(name, qty, _)| (name, qty))
}

fn live_names(job: &Job, key: &str) -> Vec<String> {
    let live = job.inner.try_lock().map(|inner| inner.live.clone()).unwrap_or(None).unwrap_or(Value::Null);
    let mut names = Vec::new();
    for entry in live.get(key).as_arr().cloned().unwrap_or_default() {
        if let Value::Obj(_) = &entry {
            if let Some(name) = entry.get("name").as_str() {
                if entry.get("count").as_i64().unwrap_or(1) > 0 {
                    names.push(name.to_string());
                }
            }
        } else if let Value::Str(name) = &entry {
            names.push(name.clone());
        }
    }
    names
}

/// Live worn debug names (D52) for the `--keep` merge.
pub(crate) fn worn_names(job: &Job) -> Vec<String> {
    live_names(job, "worn")
}

/// Live carried + banked stacks (debug name, summed quantity) for the flip pick.
pub(crate) fn live_stacks(job: &Job) -> Vec<(String, i64)> {
    let live = job.inner.try_lock().map(|inner| inner.live.clone()).unwrap_or(None).unwrap_or(Value::Null);
    let mut totals: std::collections::HashMap<String, (String, i64)> = std::collections::HashMap::new();
    for key in ["inv", "bank"] {
        for entry in live.get(key).as_arr().cloned().unwrap_or_default() {
            let (name, qty) = match &entry {
                Value::Obj(_) => match entry.get("name").as_str() {
                    Some(name) => (name.to_string(), entry.get("count").as_i64().unwrap_or(1)),
                    None => continue,
                },
                Value::Str(name) => (name.clone(), 1),
                _ => continue,
            };
            if qty <= 0 {
                continue;
            }
            totals
                .entry(name.to_lowercase())
                .and_modify(|held| held.1 = held.1.saturating_add(qty))
                .or_insert((name, qty));
        }
    }
    totals.into_values().collect()
}

// P1.6 + P2.5 chaînes/choix: an expired rescue (typed `RescueBudget`
// in `last_error`, dossier on disk) replans like a failure, even when
// no task report arrives. Pure bridge: `on_supervise` folds it into
// the `needs_replan` path, which stays non-null for it (tested below).
pub fn rescue_replan_cause(last_error: &Value) -> Option<lifescape_planner::ReplanCause> {
    if last_error.get("type").as_str() == Some("RescueBudget") {
        Some(lifescape_planner::ReplanCause::Failed)
    } else {
        None
    }
}

/// Money diversity (P2.5 §4.4): `rank_money_method` plus a ±2% seed
/// jitter on revenue, so near-ties vary by profile instead of herding
/// every bot onto one chain. The seed is the bot's own (D22-safe).
pub fn rank_money_diverse(profit: f64, trend_score: f64, revenue: f64, trend_weight: f64, seed: u64, slot: u64) -> f64 {
    let mut word = seed.wrapping_add(slot.wrapping_mul(0x9E37_79B9_7F4A_7C15)).max(1);
    word ^= word << 13;
    word ^= word >> 7;
    word ^= word << 17;
    let jitter = (word % 200) as f64 / 100.0 - 1.0;
    lifescape_planner::rank_money_method(profit, trend_score, revenue, trend_weight) + jitter * 0.02 * revenue.max(0.0)
}

/// Bot combat level for the gate (cerveau): the live heartbeat value
/// (server XP-max formula), else recomputed from the skill XP rows
/// with the same formula, else 3 (fresh account).
pub(crate) fn live_combat(live: &Value) -> u8 {
    if let Some(level) = live.get("combat").as_i64() {
        return level.clamp(1, 126) as u8;
    }
    let stats: std::collections::HashMap<u8, (u8, i32)> = live
        .get("skills")
        .as_arr()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| {
            let id = row.get("id").as_i64()?;
            let xp = row.get("xp").as_i64()?;
            let level = row.get("level").as_i64().unwrap_or(1).clamp(1, 99) as u8;
            Some((id as u8, (level, xp as i32)))
        })
        .collect();
    lifescape_sdk::snapshot::combat_level(&stats).unwrap_or(3)
}

/// One gated launch step: the ready node dispatches, a priority node
/// (safe access or combat training) launches instead, the brain waits
/// on a node already running, or the style fails typed (no fallback).
pub(crate) enum CombatStep {
    Direct,
    Redirect(usize),
    Wait,
    Fail(String),
}

fn next_node_id(life: &Life) -> String {
    let mut counter = life.nodes.len();
    for node in &life.nodes {
        if let Some(number) = node.id.strip_prefix('n').and_then(|raw| raw.parse::<usize>().ok()) {
            counter = counter.max(number);
        }
    }
    format!("n{}", counter + 1)
}

/// Priority safe-area access (cerveau relocate): reuses the open one,
/// never doubles it.
fn ensure_access_node(life: &mut Life, area: &str) -> usize {
    if let Some(at) = life.nodes.iter().position(|node| {
        matches!(&node.kind, NodeKind::AccessArea { area: reached } if reached == area)
            && !matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned)
    }) {
        // Crash recovery re-queues Active as Ready with method=None: the
        // redirect would launch empty (fail_no_script vide). The gate
        // carries the method itself, so backfill it on reuse.
        if life.nodes[at].method.as_deref().is_none_or(str::is_empty) {
            life.nodes[at].method = Some(format!("access:{area}"));
        }
        if life.nodes[at].alternatives.is_empty() {
            life.nodes[at].alternatives.push(Alternative { method: format!("access:{area}"), est_ticks: 6000, est_coins: 0 });
        }
        return at;
    }
    let mut node = Node::new(&next_node_id(life), NodeKind::AccessArea { area: area.to_string() }, Vec::new(), 1);
    node.status = NodeStatus::Ready;
    node.method = Some(format!("access:{area}"));
    node.alternatives.push(Alternative { method: format!("access:{area}"), est_ticks: 0, est_coins: 0 });
    life.nodes.push(node);
    life.nodes.len() - 1
}

/// Priority combat training (cerveau train-first): reuses the open
/// one, locks the goal-style method on chickens at creation.
fn ensure_training_node(life: &mut Life, required: u8, method: &str) -> usize {
    if let Some(at) = life.nodes.iter().position(|node| {
        matches!(&node.kind, NodeKind::ReachCombat { .. })
            && !matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned)
    }) {
        // Same crash-recovery hole as the access gate: a reused open
        // training node without a method would redirect empty.
        if life.nodes[at].method.as_deref().is_none_or(str::is_empty) {
            life.nodes[at].method = Some(method.to_string());
        }
        if life.nodes[at].alternatives.is_empty() {
            life.nodes[at].alternatives.push(Alternative { method: method.to_string(), est_ticks: 6000, est_coins: 0 });
        }
        return at;
    }
    let mut node = Node::new(&next_node_id(life), NodeKind::ReachCombat { level: required }, Vec::new(), i64::from(required));
    node.status = NodeStatus::Ready;
    node.method = Some(method.to_string());
    node.alternatives.push(Alternative { method: method.to_string(), est_ticks: 0, est_coins: 0 });
    life.nodes.push(node);
    life.nodes.len() - 1
}

/// Gate one locked method on the bot's combat level: dispatch it, or
/// point at the priority node to launch instead (`position` keeps its
/// method; the access/training node runs first).
pub(crate) fn combat_step(life: &mut Life, position: usize, combat: u8) -> CombatStep {
    let method = life.nodes.get(position).and_then(|node| node.method.clone()).unwrap_or_default();
    match lifescape_planner::combat_launch(combat, &method, &life.nodes) {
        lifescape_planner::CombatLaunch::Direct => CombatStep::Direct,
        lifescape_planner::CombatLaunch::Access { area } => {
            let at = ensure_access_node(life, area);
            if matches!(life.nodes[at].status, NodeStatus::Ready) {
                CombatStep::Redirect(at)
            } else {
                CombatStep::Wait
            }
        }
        lifescape_planner::CombatLaunch::Train { required } => {
            let style = lifescape_planner::goal_combat_style(&life.goal).to_string();
            let training = match lifescape_planner::training_method(&style) {
                Ok(training) => training.to_string(),
                Err(_) => return CombatStep::Fail(format!("fight:{style}/chickens")),
            };
            let at = ensure_training_node(life, required, &training);
            if matches!(life.nodes[at].status, NodeStatus::Ready) {
                CombatStep::Redirect(at)
            } else {
                CombatStep::Wait
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{choose_money, combat_step, is_protected_tool, live_combat, merge_keep, pick_flip_target, rank_money_diverse, rescue_replan_cause, sale_blocked, CombatStep};
    use lifescape_json::{object, str_value};
    use lifescape_planner::{Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};

    #[test]
    fn trend_bonus_flips_a_close_money_call() {
        let profits = vec![
            ("gather:fishing/raw_lobster".to_string(), 500.0, 1000.0, 0.0),
            ("gather:woodcut/yew_logs".to_string(), 510.0, 1000.0, -0.5),
        ];
        assert_eq!(choose_money(&profits, 0.1).as_deref(), Some("gather:fishing/raw_lobster"), "flat profits pick the top");
        let shortage = vec![
            ("gather:fishing/raw_lobster".to_string(), 500.0, 1000.0, 0.6),
            ("gather:woodcut/yew_logs".to_string(), 510.0, 1000.0, -0.5),
        ];
        assert_eq!(choose_money(&shortage, 0.1).as_deref(), Some("gather:fishing/raw_lobster"), "shortage bonus keeps lobster");
        let flip = vec![
            ("gather:fishing/raw_lobster".to_string(), 500.0, 1000.0, -0.5),
            ("gather:woodcut/yew_logs".to_string(), 480.0, 1000.0, 0.8),
        ];
        assert_eq!(choose_money(&flip, 0.1).as_deref(), Some("gather:woodcut/yew_logs"), "trend flips the money call");
    }

    /// D52: worn, tools and graph items never sell. The L11 flip sold
    /// its own net (only worn ids were excluded, never carried tools).
    #[test]
    fn sale_gate_blocks_worn_tools_and_graph() {
        let keep = vec!["feather".to_string()];
        let worn = vec!["iron_sword".to_string()];
        assert!(sale_blocked("net", &[], &[]), "carried tool never lists");
        assert!(sale_blocked("bronze_axe", &[], &[]), "axe never lists");
        assert!(sale_blocked("tinderbox", &[], &[]), "tinderbox never lists");
        assert!(sale_blocked("iron_sword", &[], &worn), "worn never lists");
        assert!(sale_blocked("feather", &keep, &[]), "graph item never lists");
        assert!(sale_blocked("coins", &[], &[]), "wallet never lists");
        assert!(!sale_blocked("yew_logs", &keep, &worn), "plain surplus lists");
        assert!(is_protected_tool("NET"), "gate is case-insensitive");
    }

    #[test]
    fn keep_merge_dedups_worn_into_graph() {
        let merged = merge_keep(vec!["feather".to_string()], vec!["Iron_Sword".to_string(), "feather".to_string()]);
        assert_eq!(merged.len(), 2, "dedup is case-insensitive, got {merged:?}");
        assert!(merged.contains(&"feather".to_string()));
        assert!(merged.contains(&"Iron_Sword".to_string()));
    }

    #[test]
    fn flip_pick_skips_the_gate_and_takes_the_richest() {
        let stacks = vec![
            ("net".to_string(), 1),
            ("coins".to_string(), 5000),
            ("feather".to_string(), 2000),
            ("yew_logs".to_string(), 400),
        ];
        let keep = vec!["feather".to_string()];
        let picked = pick_flip_target(&stacks, &keep, &[]).expect("surplus lists");
        assert_eq!(picked.0, "yew_logs", "tools, coins and graph items never win, got {picked:?}");
        assert!(pick_flip_target(&[("net".to_string(), 1)], &[], &[]).is_none(), "tools alone mean no target");
        assert!(pick_flip_target(&[], &[], &[]).is_none(), "empty stock means engagement sans objet");
    }

    /// P1.6 reprise: `on_supervise` folds a typed `RescueBudget`
    /// `last_error` into the replan path, so `needs_replan` stays
    /// non-null for it (the node retries with a fresh method).
    #[test]
    fn rescue_budget_last_error_replans() {
        let budget = object(vec![
            ("where", str_value("maze")),
            ("type", str_value("RescueBudget")),
            ("message", str_value("rescue maze over budget")),
        ]);
        let cause = rescue_replan_cause(&budget).expect("RescueBudget replans");
        assert_eq!(lifescape_planner::needs_replan("rescue_budget", &[], false, false, false), Some(cause), "bridge feeds the replan path");
        assert!(rescue_replan_cause(&lifescape_json::Value::Null).is_none(), "no error never replans");
        let other = object(vec![("type", str_value("Locked"))]);
        assert!(rescue_replan_cause(&other).is_none(), "other errors never rescue-replan");
    }

    #[test]
    fn money_diversity_flips_only_close_calls() {
        // Same seed replays.
        assert_eq!(rank_money_diverse(500.0, 0.0, 1000.0, 0.1, 7, 3), rank_money_diverse(500.0, 0.0, 1000.0, 0.1, 7, 3));
        // A 10x profit gap never flips on a ±2% jitter.
        let poor: Vec<f64> = (0..8).map(|slot| rank_money_diverse(100.0, 0.0, 1000.0, 0.1, 7, slot)).collect();
        let rich = rank_money_diverse(1000.0, -1.0, 1000.0, 0.1, 7, 0);
        assert!(poor.iter().all(|score| *score < rich), "10x gap stays put");
        // A tie breaks by seed: two bots need not pick the same slot.
        let left = rank_money_diverse(500.0, 0.0, 1000.0, 0.1, 7, 0);
        let varied = (0..8).any(|slot| rank_money_diverse(500.0, 0.0, 1000.0, 0.1, slot + 100, 0) != left);
        assert!(varied, "seeds vary close calls");
    }

    fn gate_life(goal: GoalKind, method: &str) -> Life {
        let mut node = Node::new(
            "n1",
            NodeKind::ReachLevel { skill: "woodcutting".to_string(), level: 30 },
            Vec::new(),
            30,
        );
        node.status = NodeStatus::Ready;
        node.method = Some(method.to_string());
        Life {
            schema: lifescape_planner::model::LIFE_SCHEMA,
            account: "t99".to_string(),
            created_at: 0,
            seed: 7,
            profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer),
            goal,
            nodes: vec![node],
            engagement: None,
            active: None,
            history: Vec::new(),
        }
    }

    #[test]
    fn gate_reads_xp_max_combat_then_skills_then_fresh() {
        let heartbeat = object(vec![("combat", lifescape_json::Value::Int(43))]);
        assert_eq!(live_combat(&heartbeat), 43, "heartbeat combat wins");
        let skills = lifescape_json::Value::Arr(
            [0u8, 1, 2, 3, 4, 5, 6]
                .iter()
                .map(|id| {
                    let (level, xp) = if *id == 3 { (10, 1154) } else { (1, 0) };
                    object(vec![
                        ("id", lifescape_json::Value::Int(i64::from(*id))),
                        ("level", lifescape_json::Value::Int(level)),
                        ("xp", lifescape_json::Value::Int(xp)),
                    ])
                })
                .collect(),
        );
        let fallback = object(vec![("skills", skills)]);
        let stats: std::collections::HashMap<u8, (u8, i32)> =
            [(0u8, (1u8, 0i32)), (1, (1, 0)), (2, (1, 0)), (3, (10, 1154)), (4, (1, 0)), (5, (1, 0)), (6, (1, 0))]
                .into_iter()
                .collect();
        assert_eq!(
            live_combat(&fallback),
            lifescape_sdk::snapshot::combat_level(&stats).unwrap_or(3),
            "skills recompute the XP-max formula"
        );
        assert_eq!(live_combat(&lifescape_json::Value::Null), 3, "no snapshot reads fresh");
    }

    /// Cerveau gate: CB3 aux saules -> Lumbridge d'abord (la même
    /// méthode après) ; CB3 au charbon -> combat d'abord (chickens,
    /// style du but) ; jamais d'envoi vers la mort.
    #[test]
    fn under_level_redirects_instead_of_dispatching() {
        let mut willows = gate_life(GoalKind::MakeMoney, "gather:woodcut/willow_logs");
        let at = match combat_step(&mut willows, 0, 3) {
            CombatStep::Redirect(at) => at,
            other => panic!("CB3 willows redirect, got {}", matches!(other, CombatStep::Direct)),
        };
        assert!(matches!(&willows.nodes[at].kind, NodeKind::AccessArea { area } if area == "lumbridge"), "safe area first, got {:?}", willows.nodes[at].kind);
        assert_eq!(willows.nodes[at].method.as_deref(), Some("access:lumbridge"), "fallback method, pas la dangereuse");
        assert_eq!(willows.nodes[0].method.as_deref(), Some("gather:woodcut/willow_logs"), "original keeps its method");
        // An open access reuses (no double), a running one waits.
        let again = match combat_step(&mut willows, 0, 3) {
            CombatStep::Redirect(at) => at,
            _ => panic!("open access reuses"),
        };
        assert_eq!(again, at, "no double insert");
        willows.nodes[at].status = NodeStatus::Active;
        assert!(matches!(combat_step(&mut willows, 0, 3), CombatStep::Wait), "running access waits");
        // Done access clears: the same method dispatches.
        willows.nodes[at].status = NodeStatus::Done;
        assert!(matches!(combat_step(&mut willows, 0, 3), CombatStep::Direct), "relocated bots dispatch");

        let mut coal = gate_life(GoalKind::MaxSkills { skills: vec!["ranged".to_string()], level: 40 }, "gather:mining/coal");
        let train = match combat_step(&mut coal, 0, 3) {
            CombatStep::Redirect(at) => at,
            _ => panic!("CB3 coal trains first"),
        };
        assert!(matches!(&coal.nodes[train].kind, NodeKind::ReachCombat { level } if *level == 43), "priority reach_combat, got {:?}", coal.nodes[train].kind);
        assert_eq!(coal.nodes[train].method.as_deref(), Some("fight:ranged/chickens"), "goal style on the clean camp");
        let (script, _) = lifescape_planner::map_method("fight:ranged/chickens", &coal.nodes[train], None).expect("training executes");
        assert_eq!(script, "ranged");
        // At level the dangerous method dispatches unchanged.
        assert!(matches!(combat_step(&mut coal, 0, 43), CombatStep::Direct), "CB43 coal clears");
    }

    /// Crash-recovery hole (fail_no_script vide): an Active access or
    /// training node re-queued Ready loses its lock (method=None, brain
    /// crash recovery); the gate reuses the open node, so the redirect
    /// carries the method itself — never an empty launch.
    #[test]
    fn redirect_recarries_a_crash_cleared_method() {
        // Access: Ready Lumbridge node with a cleared lock (alternatives kept).
        let mut willows = gate_life(GoalKind::MakeMoney, "gather:woodcut/willow_logs");
        let mut access = Node::new("n2", NodeKind::AccessArea { area: "lumbridge".to_string() }, Vec::new(), 1);
        access.status = NodeStatus::Ready;
        access.method = None;
        access.alternatives.push(lifescape_planner::Alternative { method: "access:lumbridge".to_string(), est_ticks: 6000, est_coins: 0 });
        willows.nodes.push(access);
        let at = match combat_step(&mut willows, 0, 3) {
            CombatStep::Redirect(at) => at,
            _ => panic!("cleared access still redirects"),
        };
        assert_eq!(willows.nodes[at].method.as_deref(), Some("access:lumbridge"), "gate re-carries, got {:?}", willows.nodes[at].method);
        let (script, _) = lifescape_planner::map_method("access:lumbridge", &willows.nodes[at], None).expect("redirect executes");
        assert_eq!(script, "walkto", "restored method names a script, never fail_no_script vide");

        // Training: same hole on a node that never resolved (no alternatives at all).
        let mut coal = gate_life(
            GoalKind::MaxSkills { skills: vec!["ranged".to_string()], level: 40 },
            "gather:mining/coal",
        );
        let mut training = Node::new("n2", NodeKind::ReachCombat { level: 43 }, Vec::new(), 43);
        training.status = NodeStatus::Ready;
        training.method = None;
        coal.nodes.push(training);
        let reache = match combat_step(&mut coal, 0, 3) {
            CombatStep::Redirect(at) => at,
            _ => panic!("cleared training still redirects"),
        };
        assert_eq!(coal.nodes[reache].method.as_deref(), Some("fight:ranged/chickens"), "goal style re-carried, got {:?}", coal.nodes[reache].method);
        assert!(!coal.nodes[reache].alternatives.is_empty(), "alternative backfilled with the method");
        let (script, _) = lifescape_planner::map_method("fight:ranged/chickens", &coal.nodes[reache], None).expect("training executes");
        assert_eq!(script, "ranged");
    }
}
