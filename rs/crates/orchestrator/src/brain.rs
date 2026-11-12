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
    draw_engagement_ticks, expand_goal, life_from_value, life_to_value, map_method, resolve_ready,
};

use crate::brain_fold::handle_report;
use crate::brain_plan::{bank_types, carried_tools, holdings_count, live_combat, live_stacks, merge_keep, pick_flip_target, ready_ctx, worn_names};
use crate::brain_tutorial::{tutorial_gate, TutorialGate};
use lifescape_planner::GraphState;

use crate::daemon::Orchestrator;
use crate::jobs::Job;
pub use crate::brain_plan::{TRIP_TICKS, choose_money};

/// Merchant share of `get_rich` draws (D18, calibrated in L5).
pub const MERCHANT_SHARE_PCT: u64 = 10;
/// Iron cap (D8): at most 20 Iron accounts, enforced at creation.
pub const MAX_IRON: usize = 20;

/// Iron slots left at creation: `existing` counts current Iron lives.
pub fn iron_allowed(existing: usize) -> bool {
    existing < MAX_IRON
}
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

/// Natural player-like name (`olivia_king`, `IronOlivia`; 12 max, RS2
/// rule), see `names`.
pub fn generate_pseudo(iron: bool, seed: u64) -> String {
    crate::names::natural_name(iron, seed)
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
            // A name already used by a known account draws again.
            let taken = |name: &str| lives_dir().join(format!("{name}.json")).exists();
            (0..64u64).map(|attempt| generate_pseudo(iron, seed.wrapping_add(attempt))).find(|name| !taken(name)).ok_or("no free account name after 64 draws")?
        }
    };
    ensure_life(&account, iron, goal.as_deref().unwrap_or("max:fishing:40"))?;
    Ok(account)
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

pub(crate) fn push_history(life: &mut Life, node: &str, outcome: &str) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0);
    life.history.push(HistoryEntry { node: node.to_string(), started: now, ended: now, outcome: outcome.to_string() });
}

/// Typed launch failure (P3.6): the locked method names no executable
/// (unmapped prefix, unregistered script) or resolves to nothing. A
/// contract error: abandoned at once with its cause, never retried.
fn fail_no_script(life: &mut Life, position: usize, method: &str) {
    let id = life.nodes[position].id.clone();
    crate::logln!("brain {}: no script for {method}, abandoning {id}", life.account);
    lifescape_planner::record_contract_error(life, position, &format!("no-script: {method}"));
    push_history(life, &id, "abandoned");
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

fn seconds_since_history(life: &Life, node: &str, outcome: &str) -> Option<i64> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0);
    life.history.iter().rev().find(|entry| entry.node == node && entry.outcome == outcome).map(|entry| now - entry.ended)
}

fn save_or_log(life: &Life) {
    if let Err(error) = save_life(life) {
        crate::logln!("{error}");
    }
}

/// One stuck line per stuck spell (history `stuck`), never a revive:
/// the operator decides what an abandoned graph does next (rule 7).
fn note_stuck(life: &mut Life, abandoned: &[String]) -> bool {
    if life.history.last().is_some_and(|entry| entry.node == "graph" && entry.outcome == "stuck") {
        return false;
    }
    let causes: Vec<String> = life
        .nodes
        .iter()
        .filter(|node| abandoned.contains(&node.id))
        .map(|node| format!("{}={}", node.id, if node.last_error.is_empty() { "?" } else { node.last_error.as_str() }))
        .collect();
    crate::logln!("brain {}: graph stuck, nothing left to run; abandoned: {} — operator decides", life.account, causes.join(" | "));
    push_history(life, "graph", "stuck");
    true
}

/// Launch pass for one idle worker. Order: live snapshot (no decision
/// without it), invariants (`normalize`), the running node (market fill
/// wait, lost report), the tutorial gate, then the next ready node.
fn maybe_launch(orch: &Arc<Orchestrator>, job: &Arc<Job>) {
    if !job_idle(job) {
        return;
    }
    let Some(mut life) = load_life(&job.account) else { return };
    // Never decide on a missing snapshot (bt7ktb8c: an empty live read as
    // "not on the island" launched fishing there). A contended lock is
    // the same: next pass.
    let live = match job.inner.try_lock() {
        Ok(inner) => inner.live.clone(),
        Err(_) => return,
    };
    let Some(live) = live else { return };
    let mut dirty = false;
    // Space pressure (L8) precedes the terminal check: a full bank needs
    // its janitor even when the graph is done.
    if maybe_insert_cleanup(&mut life, bank_types(job)) {
        let inserted = life.nodes.last().map(|node| node.id.clone()).unwrap_or_default();
        crate::logln!("brain {}: bank past {} types, inserting {}", job.account, lifescape_planner::BANK_TYPE_LIMIT, inserted);
        dirty = true;
    }
    for change in lifescape_planner::normalize(&mut life) {
        crate::logln!("brain {}: normalize {change}", job.account);
        dirty = true;
    }
    // The running node with an idle worker: either a market order
    // awaiting its fill (bounded), or a run whose report never came
    // (worker crash, daemon restart): counted as a failure, so a script
    // that kills its worker cannot relaunch forever.
    if let Some(active_id) = life.active.clone() {
        if recent_launch(&job.account) {
            if dirty {
                save_or_log(&life);
            }
            return;
        }
        if let Some(position) = life.nodes.iter().position(|node| node.id == active_id) {
            let awaiting = seconds_since_history(&life, &active_id, "awaiting_fill");
            match (&life.nodes[position].kind.clone(), awaiting) {
                (NodeKind::ObtainItems { item, count }, Some(waited)) => {
                    if crate::brain_await::settle_fill(job, &mut life, position, item, *count, waited) {
                        if dirty {
                            save_or_log(&life);
                        }
                        return;
                    }
                }
                // Right after a daemon restart the restored workers read
                // idle before their scripts report: an operator restart,
                // never a crash (live 2026-11-12: every restart cost each
                // active node one failure). Retry uncounted, like `stopped`.
                _ if crate::brain_grace::take_operator_stop(&job.account) || crate::brain_grace::first_idle_since_start(&job.account) => {
                    crate::logln!("brain {}: {active_id} idle after a daemon restart or an operator stop, retrying uncounted", job.account);
                    life.nodes[position].status = NodeStatus::Ready;
                    life.nodes[position].method = None;
                    life.active = None;
                    push_history(&mut life, &active_id, "stopped");
                }
                _ => {
                    let text = "no report: worker went idle without reporting (crash or restart)";
                    crate::logln!("brain {}: {active_id} {text}", job.account);
                    let fold = lifescape_planner::record_failure(&mut life, position, text);
                    push_history(&mut life, &active_id, if fold == lifescape_planner::FailureFold::Abandoned { "abandoned" } else { "failed" });
                }
            }
            lifescape_planner::normalize(&mut life);
            dirty = true;
        }
    }
    // Tutorial gate: on the island only the tutorial runs; no position
    // yet means no launch at all.
    match tutorial_gate(&mut life, &live) {
        TutorialGate::Launch(at) => {
            if !crate::registry::script_known("tutorial") {
                fail_no_script(&mut life, at, "tutorial");
                save_or_log(&life);
                return;
            }
            let id = life.nodes[at].id.clone();
            crate::logln!("brain {}: launching tutorial --node {id}", job.account);
            lifescape_planner::mark_launched(&mut life, at);
            push_history(&mut life, &id, "launched");
            note_launch(&job.account);
            save_or_log(&life);
            let args = vec![lifescape_json::str_value("--node"), lifescape_json::str_value(&id)];
            let reply = orch.run_job(&job.account, "tutorial", &args);
            if reply.get("ok").as_bool() != Some(true) {
                crate::logln!("brain {}: launch refused: {}", job.account, reply.get("error").as_str().unwrap_or("?"));
            }
            return;
        }
        TutorialGate::Hold(reason) => {
            if life.history.last().is_none_or(|entry| entry.outcome != "hold") {
                crate::logln!("brain {}: holding launches: {reason}", job.account);
            }
            if dirty {
                save_or_log(&life);
            }
            return;
        }
        TutorialGate::Closed(id) => {
            crate::logln!("brain {}: seen off the island, closing tutorial {id}", job.account);
            push_history(&mut life, &id, "tutorial_done");
            lifescape_planner::normalize(&mut life);
            dirty = true;
        }
        TutorialGate::Clear => {}
    }
    // Graph tasks read the pack (tools, coins): none before it streamed.
    // The tutorial above needs only the position (the island hides the
    // pack until its own tab opens).
    if live.get("inv_streamed").as_bool() != Some(true) {
        if dirty {
            save_or_log(&life);
        }
        return;
    }
    // PRD3 lot 3: idle bots list their tradeable surplus on the market
    // (rate-limited, never an NPC sale).
    crate::brain_sales::maybe_list_surplus(job, &life);
    // PRD3 lot 5: upgrade wishes bid on the market, never blocking.
    crate::brain_wishlist::maybe_bid_wishes(job, &life);
    match lifescape_planner::graph_state(&life) {
        GraphState::Launchable => {}
        GraphState::Running(_) => {
            if dirty {
                save_or_log(&life);
            }
            return;
        }
        // A life never ends on a goal: done draws the next one; stuck
        // records the causes and moves on to a goal that does not retrain
        // the stuck skills. Nothing left = stand by for the operator.
        state @ (GraphState::Finished | GraphState::Stuck { .. }) => {
            let stuck = match &state {
                GraphState::Stuck { abandoned } => {
                    note_stuck(&mut life, abandoned);
                    true
                }
                _ => false,
            };
            match crate::brain_succession::succeed_goal(&mut life, &live, stuck) {
                Some(label) => {
                    crate::logln!("brain {}: goal {} -> next goal {label}", job.account, if stuck { "stuck" } else { "done" });
                    lifescape_planner::normalize(&mut life);
                    save_or_log(&life);
                }
                None => {
                    crate::logln!("brain {}: no goal left to draw, standing by (operator decides)", job.account);
                    save_or_log(&life);
                }
            }
            return;
        }
    }
    let ready = life.nodes.iter().position(|node| {
        node.status == NodeStatus::Ready
            // The tutorial only ever starts through its gate.
            && !matches!(&node.kind, NodeKind::CompleteQuest { quest } if quest == "tutorial")
    });
    let Some(position) = ready else {
        if dirty {
            save_or_log(&life);
        }
        return;
    };
    let mut ctx = {
        let node = life.nodes[position].clone();
        ready_ctx(&life, &node)
    };
    // Own wallet gates the market leg (same coverage rule as the
    // carnet): a broke bot farms instead of placing doomed orders.
    // Carried tools gate the gather legs the same way (t04: rod-less
    // sardines never dispatch).
    ctx.coins = holdings_count(job, "coins");
    if let NodeKind::ObtainItems { item, .. } = &life.nodes[position].kind {
        ctx.held = holdings_count(job, item);
    }
    ctx.tools = carried_tools(job);
    ctx.bank_known = live.get("bank_streamed").as_bool() == Some(true);
    // The brain picks the site: the bot's combat level and position.
    ctx.combat = live_combat(&live);
    ctx.position = live.get("position").as_arr().and_then(|cells| {
        let part = |slot: usize| cells.get(slot).and_then(|cell| cell.as_i64());
        Some((part(1)?, part(2)?, part(0)?))
    });
    crate::brain_market::fill_ctx(&mut ctx, &life.nodes[position], &live);
    // A coin target already met closes without a launch (PRD3 lot 4).
    if let NodeKind::HaveCoins { amount } = life.nodes[position].kind {
        if ctx.coins >= amount {
            let id = life.nodes[position].id.clone();
            life.nodes[position].status = NodeStatus::Done;
            life.nodes[position].progress_current = ctx.coins;
            push_history(&mut life, &id, "goal_reached");
            crate::logln!("brain {}: {id} holds {} coins >= {amount}, done", job.account, ctx.coins);
            lifescape_planner::normalize(&mut life);
            save_or_log(&life);
            return;
        }
    }
    {
        let node = &mut life.nodes[position];
        resolve_ready(node, &ctx);
        crate::logln!("brain {}: resolve {} -> {:?} (ctx coins={})", job.account, node.id, node.method, ctx.coins);
    }
    // The planner blocks typed when no method is workable (tool to
    // fund): the block becomes an obtain sub-task now, never a node that
    // flips Ready/Blocked forever without launching.
    if let NodeStatus::Blocked { reason } = life.nodes[position].status.clone() {
        let missing: Vec<String> = reason.strip_prefix("missing:").map(|need| vec![need.to_string()]).unwrap_or_default();
        let id = life.nodes[position].id.clone();
        if missing.is_empty() {
            lifescape_planner::record_contract_error(&mut life, position, &format!("resolve blocked: {reason}"));
            push_history(&mut life, &id, "abandoned");
        } else {
            let fold = lifescape_planner::record_block(&mut life, position, &missing);
            push_history(&mut life, &id, if fold == lifescape_planner::FailureFold::Abandoned { "abandoned" } else { "blocked" });
        }
        crate::logln!("brain {}: {id} blocked at resolve ({reason}) -> {}", job.account, life.nodes[position].status.as_str());
        lifescape_planner::normalize(&mut life);
        save_or_log(&life);
        return;
    }
    let method = life.nodes[position].method.clone().unwrap_or_default();
    if method.is_empty() {
        let id = life.nodes[position].id.clone();
        crate::logln!("brain {}: no method resolvable for {id}, abandoning", job.account);
        lifescape_planner::record_contract_error(&mut life, position, "no method resolvable (no workable source for this node)");
        push_history(&mut life, &id, "abandoned");
        lifescape_planner::normalize(&mut life);
        save_or_log(&life);
        return;
    }
    // Combat requirements are graph dependencies now: the site pick in
    // `resolve_ready` blocked typed (`missing:combat:N`) above, and the
    // block filed a `reach_combat` node the node depends on.
    let launch = position;
    let method = life.nodes[launch].method.clone().unwrap_or_default();
    // A redirect target without a method never fails typed on the empty
    // string: wait a pass (the gate re-carries it, then it launches).
    if method.is_empty() {
        crate::logln!("brain {}: redirect {} has no method yet, waiting", job.account, life.nodes[launch].id);
        let _ = save_life(&life);
        return;
    }
    // Iron never trades (D15): a stale market lock from an old file resets
    // instead of launching; the planner never locks one on fresh resolves.
    if !lifescape_planner::launch_allowed(&method, life.profile.ironman) {
        crate::logln!("brain {}: refusing {method} for Ironman", job.account);
        let node_id = life.nodes[launch].id.clone();
        life.nodes[launch].status = NodeStatus::Ready;
        life.nodes[launch].method = None;
        if life.active.as_deref() == Some(node_id.as_str()) {
            life.active = None;
        }
        let _ = save_life(&life);
        return;
    }
    // Engagement: money draws [6000, 18000] at the activity start;
    // reach boxes at one window and retries; farm obtains work one
    // window then recheck holdings; cleanup works one window per pass.
    let budget: Option<(u64, u64)> = match &life.nodes[launch].kind {
        NodeKind::MakeMoney => {
            let chosen = draw_engagement_ticks(life.seed, life.history.len() as u64);
            life.engagement = Some(Engagement {
                node: life.nodes[launch].id.clone(),
                min_ticks: 6000,
                max_ticks: chosen.max(6000),
                chosen: chosen.max(6000),
            });
            let max = life.engagement.as_ref().map(|engaged| engaged.max_ticks).unwrap_or(6000);
            Some((6000, max))
        }
        NodeKind::ReachLevel { .. } | NodeKind::ReachCombat { .. } => Some((0, 18000)),
        // Every production leg of an obtain works one window, then the
        // holdings recheck (PRD3: smith, spin, mill… not only farm).
        NodeKind::ObtainItems { .. } if !method.starts_with("market:") && !method.starts_with("shop:") => Some((6000, 6000)),
        NodeKind::HaveCoins { .. } => Some((6000, 6000)),
        NodeKind::SpaceCleanup => Some((0, 6000)),
        _ => None,
    };
    let node_snapshot = life.nodes[launch].clone();
    // A market buy needs no worker (PRD3 lot 4): the brain places the
    // bid on the daemon market and the node waits for the fill.
    if method == "market:buy" {
        let id = node_snapshot.id.clone();
        match crate::brain_market::place_obtain_bid(job, &node_snapshot, &ctx) {
            Ok(_) => {
                life.nodes[launch].progress_current = ctx.held;
                lifescape_planner::mark_launched(&mut life, launch);
                push_history(&mut life, &id, "awaiting_fill");
            }
            Err(error) => {
                let fold = lifescape_planner::record_failure(&mut life, launch, &format!("market bid refused: {error}"));
                push_history(&mut life, &id, if fold == lifescape_planner::FailureFold::Abandoned { "abandoned" } else { "failed" });
            }
        }
        save_or_log(&life);
        return;
    }
    let Some((script, args)) = map_method(&method, &node_snapshot, budget) else {
        // No executable for this method: typed failure, never a silent
        // spin — the dashboard shows it and the 3-strike rule can
        // abandon it (replan on new scripts, never ghost-retry).
        fail_no_script(&mut life, launch, &method);
        let _ = save_life(&life);
        return;
    };
    let mut args = args;
    if matches!(node_snapshot.kind, NodeKind::ObtainItems { .. }) && !method.starts_with("shop:") {
        crate::brain_market::place_standing_bid(job, &node_snapshot, &ctx);
    }
    // D52: flip and cleanup never sell worn, tools or graph items. The
    // graph keep merges the live worn names here; tools ride in the same
    // gate via `sale_blocked`, so the merchant no longer flips its net.
    // The flip leaves auto-pick only with no safe target (sans objet).
    if method == "market:flip" {
        let keep = cleanup_keep(&life);
        let worn = worn_names(job);
        if let Some((item, qty)) = pick_flip_target(&live_stacks(job), &keep, &worn) {
            crate::logln!("brain {}: flip D52 target {item} x{qty}", job.account);
            args.push(item);
            args.push(qty.min(100).max(1).to_string());
        } else {
            crate::logln!("brain {}: flip D52 no safe stock, clock ends it", job.account);
        }
    }
    // Cleanup carries its keep-list (graph obtains, quest checks) and the
    // Iron flag (D2 direct, no carnet) to the worker; coins, food and best
    // tools resolve worker-side.
    if method == "space:cleanup" {
        for kept in merge_keep(cleanup_keep(&life), worn_names(job)) {
            args.push("--keep".to_string());
            args.push(kept);
        }
        if life.profile.ironman {
            args.push("--iron".to_string());
        }
    }
    if !crate::registry::script_known(&script) {
        // The method is wired but its P3 script has not landed yet:
        // same typed failure, the script name rides in the error.
        fail_no_script(&mut life, launch, &format!("{method} -> {script}"));
        let _ = save_life(&life);
        return;
    }
    let values: Vec<Value> = args.iter().map(|arg| lifescape_json::str_value(arg)).collect();
    crate::logln!("brain {}: launching {script} {}", job.account, args.join(" "));
    // Queued, not run: the launcher thread owns the blocking worker RPC.
    // Mark Active now so the next pass does not double-queue; a refused
    // launch re-queues via crash recovery (no report ever arrives).
    lifescape_planner::mark_launched(&mut life, launch);
    let launched_id = life.nodes[launch].id.clone();
    push_history(&mut life, &launched_id, "launched");
    note_launch(&job.account);
    // Saved before the blocking RPC: a report racing the reply must find
    // the node active.
    save_or_log(&life);
    // Blocking worker RPC is fine here: this is the brain thread, not
    // the supervisor. A refused launch re-queues via crash recovery.
    let reply = orch.run_job(&job.account, &script, &values);
    if reply.get("ok").as_bool() != Some(true) {
        crate::logln!("brain {}: launch refused: {}", job.account, reply.get("error").as_str().unwrap_or("?"));
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
                        crate::logln!("job {account}: rates ingest failed: {error}");
                    }
                    crate::logln!("brain {}: folded report stop={}", account, report.get("stop").as_str().unwrap_or("?"));
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
        crate::logln!("brain {}: inbox full, tick deferred to next pass", job.account);
    }
}

#[cfg(test)]
mod tests {
    use super::{cleanup_keep, draw_profile, generate_pseudo, iron_allowed, maybe_insert_cleanup, parse_goal, MAX_IRON};
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
        assert!(iron.starts_with("Iron") && iron.len() <= 12, "Iron + name, 12 max, got {iron}");
        assert!(crate::names::valid_name(&generate_pseudo(false, 42)), "natural and valid");
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
    fn space_pressure_inserts_even_on_terminal_graphs() {        let mut life = terminal_life();
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

    /// P3.6 integration: every family method the planner emits names a
    /// script the registry dispatches. P3.1-P3.5 all landed (72
    /// scripts), so the pending set is empty: an unmapped script fails
    /// this test, a missing one fails typed at launch.
    #[test]
    fn mapped_scripts_are_registered() {
        let obtain = Node::new("n9", NodeKind::ObtainItems { item: "feather".to_string(), count: 2000 }, Vec::new(), 2000);
        let reach = Node::new("n1", NodeKind::ReachLevel { skill: "smithing".to_string(), level: 30 }, Vec::new(), 30);
        for method in [
            "gather:fishing/raw_trout", "gather:woodcut/willow_logs", "gather:mining/coal",
            "gather:sheep/wool", "gather:wheat/grain",
            "cook/raw_shrimp", "cook:trout", "bake:bread",
            "smelt:steel_bar", "smith:steel_platebody",
            "tan:cow_hide", "craft:leather_gloves",
            "fire:willow_logs", "spin:ball_of_wool", "pottery:pot", "jewelry:gold_ring",
            "string:unstrung_gold_amulet", "alch:gold_ring", "superheat:iron_bar",
            "enchant:strung_sapphire_amulet",
            "runecraft:airrune", "pick:grain", "mill:pot_flour", "fill:jug_water",
            "essence:blankrune",
            "fight:melee/cows", "fight:ranged/cows", "fight:magic/cows",
            "farm:feather", "market:buy", "shop:buy", "market:flip", "space:cleanup",
            "equip:melee", "quest:cook", "access:varrock",
        ] {
            let node = if method.starts_with("gather:") || method.starts_with("smelt:") || method.starts_with("cook/") {
                &reach
            } else {
                &obtain
            };
            let (script, _) = lifescape_planner::map_method(method, node, None)
                .unwrap_or_else(|| panic!("{method} maps"));
            assert!(crate::registry::script_known(&script), "{method} -> {script}: not registered");
        }
    }

    /// P3.6: a method without script is a contract error: abandoned at
    /// once with its cause (relaunching the same method can never work).
    #[test]
    fn no_script_abandons_typed_never_silent() {
        let mut life = terminal_life();
        life.nodes[0].status = lifescape_planner::NodeStatus::Active;
        life.active = Some("n1".to_string());
        super::fail_no_script(&mut life, 0, "smelt:iron_bar");
        assert_eq!(life.nodes[0].status, lifescape_planner::NodeStatus::Abandoned);
        assert!(life.active.is_none(), "no stranded lock");
        assert_eq!(life.nodes[0].last_error, "no-script: smelt:iron_bar", "the missing method rides in the error");
        assert_eq!(life.history.iter().filter(|entry| entry.outcome == "abandoned").count(), 1, "one typed entry in history");
    }
}
