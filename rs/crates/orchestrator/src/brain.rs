//! Brain: the daemon-side autopilot. Life files live in
//! `run/lives/<compte>.json` (schema v1, atomic write); the single
//! supervisor calls `on_supervise` every pass, which folds pushed task
//! reports into the graph (replanning) and launches the next ready node
//! when the worker idles. No-ops without a life file, so manual bots are
//! untouched (R1-R7 parity).
//!
//! Planning inputs are public data only (market references, published
//! yields, trend scores): never another bot's state (D22).

#![forbid(unsafe_code)]

use std::sync::Arc;

use lifescape_json::Value;
use lifescape_planner::{
    Archetype, Engagement, GoalKind, HistoryEntry, Life, Node, NodeKind, NodeStatus, Profile,
    draw_engagement_ticks, expand_goal, life_from_value, life_to_value, map_method, needs_replan,
    replan_node, resolve_ready,
};
use lifescape_planner::expand::ReadyCtx;

use crate::daemon::Orchestrator;
use crate::jobs::Job;

/// Merchant share of `get_rich` draws (D18, calibrated in L5).
pub const MERCHANT_SHARE_PCT: u64 = 10;
/// Iron cap (D8): at most 20 Iron accounts, enforced at creation.
pub const MAX_IRON: usize = 20;

/// Iron slots left at creation: `existing` counts current Iron lives.
pub fn iron_allowed(existing: usize) -> bool {
    existing < MAX_IRON
}
/// Fixed round-trip estimate to Varrock for cost comparisons.
// shortcut: fixed 800-tick round trip; replaced by measured convene
// ETAs once the brain reads them (L8).
pub const TRIP_TICKS: u64 = 800;

pub fn lives_dir() -> std::path::PathBuf {
    crate::paths::run_dir().join("lives")
}

pub fn life_path(account: &str) -> std::path::PathBuf {
    lives_dir().join(format!("{account}.json"))
}

/// Load one life file; `None` means no autopilot for this account (not
/// an error: manual bots have no file).
pub fn load_life(account: &str) -> Option<Life> {
    let text = std::fs::read_to_string(life_path(account)).ok()?;
    let value = lifescape_json::parse(&text).ok()?;
    life_from_value(&value)
}

pub fn save_life(life: &Life) -> Result<(), String> {
    crate::paths::write_json(&life_path(&life.account), &life_to_value(life))
        .map_err(|error| format!("brain save {}: {error}", life.account))
}

/// Accepted `--goal` shapes: `max:a,b:40`, `obtain:feather:2000`,
/// `money`, `quest:cook`, `equip:melee`, `area:karamja`, `rich[:merchant
/// |:producer]`. Bare `rich` draws the archetype (10% merchant).
pub fn parse_goal(text: &str) -> Result<GoalKind, String> {
    let text = text.trim();
    if text.is_empty() || text == "money" {
        return Ok(GoalKind::MakeMoney);
    }
    if let Some(rest) = text.strip_prefix("max:") {
        let (skills, level) = rest.rsplit_once(':').ok_or_else(|| "goal max:<skills>:<level>".to_string())?;
        let level: u8 = level.parse().map_err(|_| "goal level must be 2-99".to_string())?;
        if !(2..=99).contains(&level) {
            return Err("goal level must be 2-99".to_string());
        }
        let skills: Vec<String> = skills.split(',').map(str::trim).filter(|skill| !skill.is_empty()).map(str::to_string).collect();
        if skills.is_empty() {
            return Err("goal max: needs at least one skill".to_string());
        }
        return Ok(GoalKind::MaxSkills { skills, level });
    }
    if let Some(rest) = text.strip_prefix("obtain:") {
        let (item, count) = rest.rsplit_once(':').ok_or_else(|| "goal obtain:<item>:<count>".to_string())?;
        let count: i64 = count.parse().map_err(|_| "goal count must be a number".to_string())?;
        return Ok(GoalKind::Obtain { item: item.to_string(), count: count.max(1) });
    }
    if let Some(quest) = text.strip_prefix("quest:") {
        if quest.is_empty() {
            return Err("goal quest:<key>".to_string());
        }
        return Ok(GoalKind::CompleteQuest { quest: quest.to_string() });
    }
    if let Some(style) = text.strip_prefix("equip:") {
        return Ok(GoalKind::EquipBest { style: if style.is_empty() { "melee".to_string() } else { style.to_string() } });
    }
    if let Some(area) = text.strip_prefix("area:") {
        return Ok(GoalKind::AccessArea { area: area.to_string() });
    }
    if text == "rich" {
        return Ok(GoalKind::GetRich { merchant: None });
    }
    if let Some(arch) = text.strip_prefix("rich:") {
        return match arch {
            "merchant" => Ok(GoalKind::GetRich { merchant: Some(true) }),
            "producer" => Ok(GoalKind::GetRich { merchant: Some(false) }),
            _ => Err("goal rich[:merchant|:producer]".to_string()),
        };
    }
    Err(format!("unknown goal {text:?} (max:|obtain:|money|quest:|equip:|area:|rich)"))
}

fn xorshift(seed: u64, stream: u64) -> u64 {
    let mut word = seed.wrapping_add(stream.wrapping_mul(0x9E37_79B9_7F4A_7C15)).max(1);
    word ^= word << 13;
    word ^= word >> 7;
    word ^= word << 17;
    word
}

/// `Iron` + 8 chars (12 max, RS2 rule); others get a short `b`-prefixed
/// name. Charset avoids ambiguous glyphs.
pub fn generate_pseudo(iron: bool, seed: u64) -> String {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut name = if iron { "Iron".to_string() } else { "b".to_string() };
    let width = if iron { 8 } else { 7 };
    for slot in 0..width {
        name.push(ALPHABET[(xorshift(seed, slot + 1) % ALPHABET.len() as u64) as usize] as char);
    }
    name
}

/// Profile draw from the seed: affinity and patience uniform in [0,1],
/// merchant share 10% (never Iron, enforced by the caller too).
pub fn draw_profile(seed: u64, iron: bool) -> Profile {
    let affinity = (xorshift(seed, 101) % 1000) as f64 / 999.0;
    let patience = (xorshift(seed, 102) % 1000) as f64 / 999.0;
    let archetype = if !iron && xorshift(seed, 103) % 100 < MERCHANT_SHARE_PCT {
        Archetype::Merchant
    } else {
        Archetype::Producer
    };
    Profile::new(iron, affinity, patience, seed, archetype)
}

fn iron_count() -> usize {
    let dir = lives_dir();
    let entries = std::fs::read_dir(&dir).map(|entries| entries.filter_map(|entry| entry.ok()).count()).unwrap_or(0);
    let _ = entries;
    // Count files whose profile says ironman (cheap: only at creation).
    let mut count = 0;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(value) = lifescape_json::parse(&text) {
                    if value.get("profile").get("ironman").as_bool() == Some(true) {
                        count += 1;
                    }
                }
            }
        }
    }
    count
}

/// Create a life file (and its account name when omitted). Existing
/// files load untouched: creation never rewrites a running brain.
pub fn ensure_life(account: &str, iron: bool, goal_text: &str) -> Result<Life, String> {
    if let Some(life) = load_life(account) {
        return Ok(life);
    }
    if iron && !iron_allowed(iron_count()) {
        return Err(format!("iron cap reached ({MAX_IRON}): no new Ironman"));
    }
    let goal = parse_goal(goal_text)?;
    if iron {
        if let GoalKind::GetRich { merchant: Some(true) } = goal {
            return Err("iron never merchants".to_string());
        }
    }
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|span| span.as_nanos() as u64)
        .unwrap_or(1)
        ^ (std::process::id() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let profile = draw_profile(seed, iron);
    let nodes = expand_goal(&goal, &profile);
    let life = Life {
        schema: lifescape_planner::model::LIFE_SCHEMA,
        account: account.to_string(),
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|span| span.as_secs() as i64)
            .unwrap_or(0),
        seed,
        profile,
        goal,
        nodes,
        engagement: None,
        active: None,
        history: Vec::new(),
    };
    save_life(&life)?;
    Ok(life)
}

/// `create-user [pseudo] [--iron] [--goal kind]`: pseudo generated when
/// omitted, life file written, then the CLI opens the session and runs
/// the tutorial (tutorial first, brain takes over whenever idle after).
pub fn create_user(account_opt: Option<String>, iron: bool, goal: Option<String>) -> Result<String, String> {
    let account = match account_opt {
        Some(name) if !name.is_empty() => name,
        _ => {
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|span| span.as_nanos() as u64)
                .unwrap_or(1);
            generate_pseudo(iron, seed)
        }
    };
    ensure_life(&account, iron, goal.as_deref().unwrap_or("max:fishing:40"))?;
    Ok(account)
}

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
fn ready_ctx(life: &Life, node: &Node) -> ReadyCtx {
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

fn holdings_count(job: &Job, item_debug: &str) -> i64 {
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
fn bank_types(job: &Job) -> usize {
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

/// Keep-list for a cleanup order: unfinished graph obtain items plus the
/// item checks of unfinished quests, deduplicated and sorted. Coins, food
/// and best tools are added worker-side (planner triage, kit specs).
pub fn cleanup_keep(life: &Life) -> Vec<String> {
    let mut keep = Vec::new();
    for node in &life.nodes {
        if matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned) {
            continue;
        }
        match &node.kind {
            NodeKind::ObtainItems { item, .. } => keep.push(item.clone()),
            NodeKind::CompleteQuest { quest } => {
                if let Some(required) = lifescape_gamedata::quest_requirements::by_key(quest) {
                    for check in &required.items {
                        keep.push(check.item.clone());
                    }
                }
            }
            _ => {}
        }
    }
    keep.sort();
    keep.dedup();
    keep
}

/// Space-pressure insert (L8) for one life: terminal graphs included, a
/// full bank needs its janitor even when everything else is done.
/// Returns true when a node was inserted.
pub fn maybe_insert_cleanup(life: &mut Life, bank_type_count: usize) -> bool {
    let open = life.nodes.iter().any(|node| {
        matches!(node.kind, NodeKind::SpaceCleanup)
            && !matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned)
    });
    // Done breaks a failure streak; two abandoned janitors in a row mean
    // the pressure never drains, so inserts stop instead of looping.
    let stalled = lifescape_planner::cleanup_stalled(&life.nodes);
    if !lifescape_planner::cleanup_needed(open, stalled, bank_type_count) {
        return false;
    }
    let mut counter = life.nodes.len();
    for node in &life.nodes {
        if let Some(number) = node.id.strip_prefix('n').and_then(|raw| raw.parse::<usize>().ok()) {
            counter = counter.max(number);
        }
    }
    let mut node = Node::new(&format!("n{}", counter + 1), NodeKind::SpaceCleanup, Vec::new(), 0);
    node.status = NodeStatus::Ready;
    life.nodes.push(node);
    true
}

fn reach_level(report: &Value, skill: &str) -> Option<u8> {
    let stat = lifescape_sdk::provenance::skill_id(&skill.to_lowercase())? as i64;
    report
        .get("levels_after")
        .as_arr()?
        .iter()
        .find(|row| row.get("stat").as_i64() == Some(stat))
        .and_then(|row| row.get("level").as_i64())
        .map(|level| level.clamp(1, 99) as u8)
}

fn report_missing(report: &Value) -> Vec<String> {
    report
        .get("missing")
        .as_arr()
        .map(|rows| rows.iter().filter_map(|row| row.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn push_history(life: &mut Life, node: &str, outcome: &str) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0);
    life.history.push(HistoryEntry { node: node.to_string(), started: now, ended: now, outcome: outcome.to_string() });
}

fn fail_count(life: &Life, node: &str) -> usize {
    life.history.iter().filter(|entry| entry.node == node && entry.outcome == "failed").count()
}

fn done_ids(life: &Life) -> Vec<String> {
    life.nodes.iter().filter(|node| node.status == NodeStatus::Done).map(|node| node.id.clone()).collect()
}

/// Fold one pushed report into the graph; launches nothing (the launch
/// pass below runs right after, same tick).
fn handle_report(orch: &Arc<Orchestrator>, job: &Arc<Job>, report: &Value) {
    let account = job.account.clone();
    let Some(mut life) = load_life(&account) else { return };
    let node_id = report.get("node_id").as_str().unwrap_or("").to_string();
    if node_id.is_empty() {
        return;
    }
    let Some(index) = life.nodes.iter().position(|node| node.id == node_id) else { return };
    let stop = report.get("stop").as_str().unwrap_or("").to_string();
    let missing = report_missing(report);
    let ticks = report.get("ticks").as_i64().unwrap_or(0).max(0) as u64;
    let died = missing.iter().any(|need| need.contains("died") || need.contains("death"));
    let stock_lost = stop == "failed" && missing.iter().any(|need| need.contains("lost"));
    // Price drift: the market moved >25% on a locked buy since resolve.
    let price_drift = false;
    life.nodes[index].spent_ticks = life.nodes[index].spent_ticks.saturating_add(ticks);
    let kind = life.nodes[index].kind.clone();
    match stop.as_str() {
        "goal_reached" => {
            let done = match &kind {
                NodeKind::ReachLevel { skill, level } => {
                    if let Some(reached) = reach_level(report, skill) {
                        life.nodes[index].progress_current = i64::from(reached);
                        reached >= *level
                    } else {
                        life.nodes[index].progress_current = life.nodes[index].progress_target;
                        true
                    }
                }
                NodeKind::ObtainItems { item, count } => {
                    // Market buys settle later: a placed order is not a
                    // filled one. Shop buys settle now: a shortfall means
                    // the leg failed (broke or unstocked), never a wait.
                    let held = holdings_count(job, item);
                    life.nodes[index].progress_current = held;
                    held >= *count
                }
                _ => {
                    life.nodes[index].progress_current = life.nodes[index].progress_target;
                    true
                }
            };
            if done {
                life.nodes[index].status = NodeStatus::Done;
                if life.active.as_deref() == Some(life.nodes[index].id.as_str()) {
                    life.active = None;
                }
                if life.engagement.as_ref().is_some_and(|engaged| engaged.node == life.nodes[index].id) {
                    life.engagement = None;
                }
                let id = life.nodes[index].id.clone();
                push_history(&mut life, &id, "goal_reached");
            } else if matches!(&kind, NodeKind::ObtainItems { .. })
                && life.nodes[index].method.as_deref() != Some("market:buy")
            {
                // Instant leg, still short: the method failed, not the
                // market. Replan (bounded by the 3-strike rule) instead
                // of waiting for a fill that never comes on its own.
                let id = life.nodes[index].id.clone();
                life.nodes[index].status = NodeStatus::Ready;
                life.nodes[index].method = None;
                if life.active.as_deref() == Some(id.as_str()) {
                    life.active = None;
                }
                if fail_count(&life, &id) >= 2 {
                    life.nodes[index].status = NodeStatus::Abandoned;
                    push_history(&mut life, &id, "abandoned");
                } else {
                    push_history(&mut life, &id, "failed");
                }
            } else {
                // Placed but unfilled: wait for stock, keep the method.
                life.nodes[index].status = NodeStatus::Active;
            }
        }
        other => {
            // `blocked` without a typed `missing` is a script bug, not a
            // shopping list: count it as a failure so the 3-strike rule
            // bounds the replan-relaunch loop instead of spinning it.
            let effective = if other == "blocked" && missing.is_empty() { "failed" } else { other };
            if let Some(cause) = needs_replan(effective, &missing, died, stock_lost, price_drift) {
                if fail_count(&life, &life.nodes[index].id) >= 3 && effective == "failed" {
                    let id = life.nodes[index].id.clone();
                    life.nodes[index].status = NodeStatus::Abandoned;
                    life.active = None;
                    push_history(&mut life, &id, "abandoned");
                } else {
                    let id = life.nodes[index].id.clone();
                    let mut counter = life.nodes.len();
                    // Unique ids across growth: bump past every n<k>.
                    for node in &life.nodes {
                        if let Some(number) = node.id.strip_prefix('n').and_then(|raw| raw.parse::<usize>().ok()) {
                            counter = counter.max(number);
                        }
                    }
                    let fresh = {
                        let node = &mut life.nodes[index];
                        replan_node(node, &cause, &missing, &mut counter)
                    };
                    if effective == "failed" {
                        push_history(&mut life, &id, "failed");
                    }
                    if matches!(cause, lifescape_planner::ReplanCause::EngagementEnd)
                        && life.nodes[index].status == NodeStatus::Done
                    {
                        push_history(&mut life, &id, "engagement_end");
                    }
                    life.nodes.extend(fresh);
                    // A replanned node leaves Active: the launch pass may
                    // pick it up again, never strand on the old lock.
                    if life.nodes[index].status != NodeStatus::Done
                        && life.active.as_deref() == Some(id.as_str())
                    {
                        life.active = None;
                    }
                }
            }
        }
    }
    if save_life(&life).is_err() {
        println!("brain {}: life save failed, report dropped", job.account);
        return;
    }
    println!(
        "brain {}: folded report node={} stop={}",
        job.account,
        node_id,
        report.get("stop").as_str().unwrap_or("?")
    );
    let _ = orch;
}

fn job_idle(job: &Job) -> bool {
    let Ok(inner) = job.inner.try_lock() else {
        return false;
    };
    // Disconnected workers never take orders: the operator reopens them.
    // A stale script NAME never blocks: only `phase` tells whether a
    // script thread is alive (the worker itself refuses double `run`
    // with busy). Light heartbeats keep carrying the last script name
    // long after it finished; waiting on None would stall the brain.
    inner.phase == "idle" && !inner.held && !inner.paused && inner.status != "disconnected"
}

/// Launch the next ready node when the worker idles. Blocked nodes whose
/// deps landed flip back to ready here; price drift re-locks a cheaper
/// method before launch.
fn maybe_launch(orch: &Arc<Orchestrator>, job: &Arc<Job>) {
    if !job_idle(job) {
        return;
    }
    let Some(mut life) = load_life(&job.account) else { return };
    let mut dirty = false;
    // Space pressure (L8) precedes the terminal check: a full bank needs
    // its janitor even when the graph is done.
    if maybe_insert_cleanup(&mut life, bank_types(job)) {
        let inserted = life.nodes.last().map(|node| node.id.clone()).unwrap_or_default();
        println!("brain {}: bank past {} types, inserting {}", job.account, lifescape_planner::BANK_TYPE_LIMIT, inserted);
        dirty = true;
    }
    if life.nodes.iter().all(|node| node.status == NodeStatus::Done || node.status == NodeStatus::Abandoned) {
        return;
    }
    let done = done_ids(&life);
    let abandoned: Vec<String> = life
        .nodes
        .iter()
        .filter(|node| node.status == NodeStatus::Abandoned)
        .map(|node| node.id.clone())
        .collect();
    for node in &mut life.nodes {
        if matches!(node.status, NodeStatus::Pending | NodeStatus::Blocked { .. }) && node.ready(&done) {
            node.status = NodeStatus::Ready;
            if matches!(node.kind, NodeKind::ObtainItems { .. } | NodeKind::MakeMoney) {
                node.method = None;
            }
            dirty = true;
        }
        // An abandoned dependency strands its dependents: abandon them
        // loudly instead of pending forever. Re-planning an abandoned
        // branch (new sources, new funds) is future work.
        if matches!(node.status, NodeStatus::Pending | NodeStatus::Blocked { .. })
            && node.deps.iter().any(|dep| abandoned.contains(dep))
        {
            node.status = NodeStatus::Abandoned;
            dirty = true;
        }
    }
    // A market node placed but unfilled waits instead of reordering: the
    // holdings check below closes it once the deal settles.
    let mut await_fill = false;
    if let Some(active_id) = life.active.clone() {
        if let Some(position) = life.nodes.iter().position(|node| node.id == active_id) {            let done = match life.nodes[position].kind.clone() {
                NodeKind::ObtainItems { item, count } if life.nodes[position].status == NodeStatus::Active => {
                    let held = holdings_count(job, &item);
                    life.nodes[position].progress_current = held;
                    if held >= count {
                        life.nodes[position].status = NodeStatus::Done;
                        life.active = None;
                        let id = life.nodes[position].id.clone();
                        push_history(&mut life, &id, "goal_reached");
                        dirty = true;
                        true
                    } else {
                        await_fill = true;
                        false
                    }
                }
                _ => false,
            };
            let _ = done;
            if await_fill {
                let _ = save_life(&life);
                return;
            }
        }
    }
    // Crash recovery: an Active node with an idle worker and no
    // report coming (restart, lost push, refused launch) re-queues
    // instead of stranding. A launch grace period (30 s, memory only)
    // stops the reset while the worker is still picking the order up;
    // the worker takes the first `run` and refuses later doubles as
    // busy, so execution stays at-most-once.
    if !recent_launch(&job.account) {
        if let Some(active_id) = life.active.clone() {
            if let Some(position) = life.nodes.iter().position(|node| node.id == active_id) {
                let awaiting = matches!(&life.nodes[position].kind, NodeKind::ObtainItems { .. })
                    && life.nodes[position].status == NodeStatus::Active;
                if !awaiting && life.nodes[position].status == NodeStatus::Active {
                    life.nodes[position].status = NodeStatus::Ready;
                    life.nodes[position].method = None;
                    life.active = None;
                    dirty = true;
                }
            } else {
                life.active = None;
                dirty = true;
            }
        }
    }
    let ready = life.nodes.iter().position(|node| {
        node.status == NodeStatus::Ready && node.ready(&done_ids(&life)) && life.active.as_deref() != Some(node.id.as_str())
    });
    let Some(position) = ready else {
        if dirty {
            let _ = save_life(&life);
        }
        return;
    };
    let mut ctx = {
        let node = life.nodes[position].clone();
        ready_ctx(&life, &node)
    };
    // Own wallet gates the market leg (same coverage rule as the
    // carnet): a broke bot farms instead of placing doomed orders.
    ctx.coins = holdings_count(job, "coins");
    {
        let node = &mut life.nodes[position];
        resolve_ready(node, &ctx);
        println!("brain {}: resolve {} -> {:?} (ctx coins={})", job.account, node.id, node.method, ctx.coins);
    }
    let method = life.nodes[position].method.clone().unwrap_or_default();
    if method.is_empty() {
        println!("brain {}: no method for {} (unresolvable obtain?)", job.account, life.nodes[position].id);
        let _ = save_life(&life);
        return;
    }
    // Iron never trades (D15): a stale market lock from an old file resets
    // instead of launching; the planner never locks one on fresh resolves.
    if !lifescape_planner::launch_allowed(&method, life.profile.ironman) {
        println!("brain {}: refusing {method} for Ironman", job.account);
        let node_id = life.nodes[position].id.clone();
        life.nodes[position].status = NodeStatus::Ready;
        life.nodes[position].method = None;
        if life.active.as_deref() == Some(node_id.as_str()) {
            life.active = None;
        }
        let _ = save_life(&life);
        return;
    }
    // Engagement: money draws [6000, 18000] at the activity start;
    // reach boxes at one window and retries; farm obtains work one
    // window then recheck holdings; cleanup works one window per pass.
    let budget: Option<(u64, u64)> = match &life.nodes[position].kind {
        NodeKind::MakeMoney => {
            let chosen = draw_engagement_ticks(life.seed, life.history.len() as u64);
            life.engagement = Some(Engagement {
                node: life.nodes[position].id.clone(),
                min_ticks: 6000,
                max_ticks: chosen.max(6000),
                chosen: chosen.max(6000),
            });
            let max = life.engagement.as_ref().map(|engaged| engaged.max_ticks).unwrap_or(6000);
            Some((6000, max))
        }
        NodeKind::ReachLevel { .. } => Some((0, 18000)),
        NodeKind::ObtainItems { .. } if method.starts_with("farm:") => Some((6000, 6000)),
        NodeKind::SpaceCleanup => Some((0, 6000)),
        _ => None,
    };
    let node_snapshot = life.nodes[position].clone();
    let Some((script, args)) = map_method(&method, &node_snapshot, budget) else {
        return;
    };
    let mut args = args;
    // Cleanup carries its keep-list (graph obtains, quest checks) and the
    // Iron flag (D2 direct, no carnet) to the worker; coins, food and best
    // tools resolve worker-side.
    if method == "space:cleanup" {
        for kept in cleanup_keep(&life) {
            args.push("--keep".to_string());
            args.push(kept);
        }
        if life.profile.ironman {
            args.push("--iron".to_string());
        }
    }
    if !crate::registry::script_known(&script) {
        return;
    }
    let values: Vec<Value> = args.iter().map(|arg| lifescape_json::str_value(arg)).collect();
    println!("brain {}: launching {script} {}", job.account, args.join(" "));
    // Queued, not run: the launcher thread owns the blocking worker RPC.
    // Mark Active now so the next pass does not double-queue; a refused
    // launch re-queues via crash recovery (no report ever arrives).
    life.nodes[position].status = NodeStatus::Active;
    life.active = Some(life.nodes[position].id.clone());
    note_launch(&job.account);
    // Blocking worker RPC is fine here: this is the brain thread, not
    // the supervisor. A refused launch re-queues via crash recovery.
    let reply = orch.run_job(&job.account, &script, &values);
    if reply.get("ok").as_bool() != Some(true) {
        println!("brain {}: launch refused: {}", job.account, reply.get("error").as_str().unwrap_or("?"));
    }
    dirty = true;
    if dirty {
        let _ = save_life(&life);
    }
}

/// Launch grace table (30 s per account, memory only): stops crash
/// recovery from re-queueing while the worker is still picking up.
/// A `HashMap` in a `OnceLock`, never persisted, never shared.
fn launch_marks() -> &'static std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>> {
    static LAUNCHED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>> =
        std::sync::OnceLock::new();
    LAUNCHED.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn recent_launch(account: &str) -> bool {
    launch_marks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(account)
        .is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(30))
}

fn note_launch(account: &str) {
    launch_marks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(account.to_string(), std::time::Instant::now());
}

/// Brain inbox: the supervisor never waits on files, the store or a
/// session socket. It only try-sends here; one brain thread owns every
/// blocking brain step (life files, market/store reads, worker RPC).
/// A wedged worker or store stalls the brain, never the supervision or
/// the CLI dispatch. A full queue drops loudly (the next pass re-queues).
fn inbox() -> &'static std::sync::Mutex<
    Option<std::sync::mpsc::SyncSender<(String, Option<Value>)>>,
> {
    static INBOX: std::sync::OnceLock<
        std::sync::Mutex<Option<std::sync::mpsc::SyncSender<(String, Option<Value>)>>>,
    > = std::sync::OnceLock::new();
    INBOX.get_or_init(|| std::sync::Mutex::new(None))
}

fn ensure_brain(orch: &Arc<Orchestrator>) {
    {
        let slot = inbox().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_some() {
            return;
        }
    }
    let (sender, receiver) = std::sync::mpsc::sync_channel::<(String, Option<Value>)>(16);
    {
        let mut slot = inbox().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = Some(sender);
    }
    let orch = orch.clone();
    std::thread::Builder::new()
        .name("brain-tick".to_string())
        .spawn(move || {
            while let Ok((account, report)) = receiver.recv() {
                let job = orch.jobs.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&account).cloned();
                let Some(job) = job else {
                    continue;
                };
                if let Some(report) = report {
                    // Rates first (L4 store, raw rows + snapshot buckets),
                    // then the life graph. Both are bounded file/store
                    // writes on the brain thread, never the supervisor.
                    if let Err(error) = crate::jobs::ingest_report_persisted(&report) {
                        println!("job {account}: rates ingest failed: {error}");
                    }
                    println!("brain {}: folded report stop={}", account, report.get("stop").as_str().unwrap_or("?"));
                    handle_report(&orch, &job, &report);
                }
                maybe_launch(&orch, &job);
            }
        })
        .expect("brain thread spawns");
}

/// Supervisor entry: enqueue only, never block. The brain thread folds
/// reports and launches; `had_report` mirrors the flag the loop already
/// drained, so the brain never misses a push.
pub fn on_supervise(orch: &Arc<Orchestrator>, job: &Arc<Job>, report: Option<Value>) {
    ensure_brain(orch);
    let queued = inbox()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .map(|sender| sender.try_send((job.account.clone(), report)))
        .is_some_and(|sent| sent.is_ok());
    if !queued {
        println!("brain {}: inbox full, tick deferred to next pass", job.account);
    }
}

#[cfg(test)]
mod tests {
    use super::{choose_money, cleanup_keep, draw_profile, generate_pseudo, iron_allowed, maybe_insert_cleanup, parse_goal, MAX_IRON};
    use lifescape_planner::{GoalKind, Life, Node, NodeKind};

    #[test]
    fn goals_parse() {
        assert!(matches!(parse_goal("money"), Ok(GoalKind::MakeMoney)));
        assert!(matches!(parse_goal(""), Ok(GoalKind::MakeMoney)));
        let max = parse_goal("max:fishing,cooking:40").expect("max parses");
        assert!(matches!(max, GoalKind::MaxSkills { level: 40, .. }));
        assert!(parse_goal("max:fishing").is_err(), "level required");
        assert!(matches!(parse_goal("quest:cook"), Ok(GoalKind::CompleteQuest { .. })));
        assert!(matches!(parse_goal("rich:merchant"), Ok(GoalKind::GetRich { merchant: Some(true) })));
        assert!(parse_goal("rich:ever").is_err(), "archetype closed");
        assert!(parse_goal("nope").is_err(), "unknown goals fail loud");
    }

    #[test]
    fn pseudos_fit_rs2_and_replay() {
        let iron = generate_pseudo(true, 42);
        assert!(iron.starts_with("Iron") && iron.len() == 12, "Iron + 8 = 12 max, got {iron}");
        assert_eq!(iron, generate_pseudo(true, 42), "seeded pseudos replay");
        assert_ne!(generate_pseudo(false, 1), generate_pseudo(false, 2), "names vary");
    }

    #[test]
    fn merchant_share_and_iron_rule() {
        let mut merchants = 0;
        for seed in 0..100 {
            if draw_profile(seed, false).archetype == lifescape_planner::Archetype::Merchant {
                merchants += 1;
            }
            assert_eq!(draw_profile(seed, true).archetype, lifescape_planner::Archetype::Producer, "iron never merchants");
        }
        assert!((5..=20).contains(&merchants), "merchant share ~10%, got {merchants}/100");
    }

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

    #[test]
    fn profile_draw_replays_from_the_seed() {
        let first = draw_profile(918273645, false);
        let second = draw_profile(918273645, false);
        assert_eq!(first, second, "same seed replays the whole profile");
        assert!((0.0..=1.0).contains(&first.market_affinity), "affinity in [0,1]");
        assert!((0.0..=1.0).contains(&first.price_patience), "patience in [0,1]");
        assert!((first.margin - (0.05 + first.price_patience * 0.15)).abs() < 1e-12, "margin derives from patience");
        assert_eq!(first.seed, 918273645, "seed persists for the replay");
        assert_ne!(draw_profile(1, false).market_affinity, draw_profile(2, false).market_affinity, "seeds vary");
    }

    #[test]
    fn iron_cap_holds_at_twenty() {
        assert_eq!(MAX_IRON, 20, "D8 cap pinned");
        assert!(iron_allowed(19), "19 Iron leave one slot");
        assert!(!iron_allowed(20), "20 Iron close creation");
        assert!(!iron_allowed(99), "over cap stays closed");
    }

    #[test]
    fn cleanup_keep_covers_graph_and_quests() {
        let profile = draw_profile(7, false);
        let mut nodes = lifescape_planner::expand_goal(
            &GoalKind::Obtain { item: "feather".to_string(), count: 100 },
            &profile,
        );
        nodes.push(Node::new("n9", NodeKind::CompleteQuest { quest: "cook".to_string() }, Vec::new(), 1));
        let life = Life {
            schema: lifescape_planner::model::LIFE_SCHEMA,
            account: "t99".to_string(),
            created_at: 0,
            seed: 7,
            profile,
            goal: GoalKind::Obtain { item: "feather".to_string(), count: 100 },
            nodes,
            engagement: None,
            active: None,
            history: Vec::new(),
        };
        let keep = cleanup_keep(&life);
        assert!(keep.contains(&"feather".to_string()), "graph obtain survives, got {keep:?}");
        assert!(keep.contains(&"bucket_milk".to_string()), "quest check survives, got {keep:?}");
        assert!(keep.windows(2).all(|pair| pair[0] <= pair[1]), "sorted for stable orders");
    }

    fn terminal_life() -> Life {
        let profile = draw_profile(7, false);
        let mut done = Node::new("n1", NodeKind::MakeMoney, Vec::new(), 0);
        done.status = lifescape_planner::NodeStatus::Done;
        Life {
            schema: lifescape_planner::model::LIFE_SCHEMA,
            account: "t99".to_string(),
            created_at: 0,
            seed: 7,
            profile,
            goal: GoalKind::MakeMoney,
            nodes: vec![done],
            engagement: None,
            active: None,
            history: Vec::new(),
        }
    }

    #[test]
    fn space_pressure_inserts_even_on_terminal_graphs() {
        let mut life = terminal_life();
        assert!(maybe_insert_cleanup(&mut life, 55), "full bank on a done graph still inserts");
        assert!(matches!(life.nodes.last().map(|node| (&node.kind, &node.status)),
            Some((NodeKind::SpaceCleanup, lifescape_planner::NodeStatus::Ready))), "janitor ready at once");
        let mut roomy = terminal_life();
        assert!(!maybe_insert_cleanup(&mut roomy, 54), "room left inserts nothing");
        assert_eq!(roomy.nodes.len(), 1, "life untouched");
        let mut second = terminal_life();
        second.nodes.push(Node::new("n2", NodeKind::SpaceCleanup, Vec::new(), 0));
        assert!(!maybe_insert_cleanup(&mut second, 59), "an open node blocks doubles");
        let mut stalled = terminal_life();
        for id in ["n2", "n3"] {
            let mut abandoned = Node::new(id, NodeKind::SpaceCleanup, Vec::new(), 0);
            abandoned.status = lifescape_planner::NodeStatus::Abandoned;
            stalled.nodes.push(abandoned);
        }
        assert!(!maybe_insert_cleanup(&mut stalled, 60), "two abandoned in a row stop the loop");
    }
}
