//! Node lifecycle: the single place that moves graph statuses. Every
//! mutation of a life (report fold, launch pass, resolve) ends with
//! `normalize`, which restores the invariants below; the brain never
//! patches statuses by hand. Pure: no clock, no files (history stamps
//! stay in the brain).
//!
//! Invariants after `normalize`:
//! - a node whose dependency is abandoned (or names no node, or sits in a
//!   dependency cycle) is abandoned, transitively;
//! - `Ready` ⇔ every dependency is `Done` (`Pending`/`Blocked` otherwise);
//! - at most one `Active` node, and it is `life.active`;
//! - liveness: a graph with an unfinished node has either an active node
//!   or a ready one — never an open graph with nothing to launch;
//! - an open obtain node never carries a leg that names no executable.
//!
//! Stuck graphs: `revive_stuck` is the one rule that reopens abandoned
//! nodes — after `REVIVE_COOLDOWN_TICKS` of a stuck spell, at most
//! `MAX_REVIVES` times per node; past the cap the operator decides.

use std::collections::HashSet;

use crate::model::{Life, Node, NodeKind, NodeStatus};
use crate::replan::{replan_node, ReplanCause};

/// Runtime failures before a node is abandoned (typed, with dossier).
pub const MAX_FAILURES: u32 = 3;
/// Blocked stops (missing tool/input) before a node is abandoned: a node
/// that keeps blocking after its sub-tasks landed never heals alone.
pub const MAX_BLOCKS: u32 = 3;
/// Length of a stuck spell before its abandoned nodes are retried (D1
/// ticks: 1 h in prod at 600 ms, 6 min in dev at 60 ms). Abandon causes
/// are often transient (operator redeploys, empty market, nav timeouts,
/// a wallet seen empty): a bounded, slow retry beats idling forever.
pub const REVIVE_COOLDOWN_TICKS: u64 = 6000;
/// Revivals per node: past it the abandonment is final (rule 7: the
/// operator decides), so a node that can never work costs at most
/// `MAX_REVIVES` × its failure budget.
pub const MAX_REVIVES: u32 = 3;

/// What the launch pass may do with a normalized graph.
#[derive(Debug, Clone, PartialEq)]
pub enum GraphState {
    /// A node is running (or awaiting its fill): nothing to launch.
    Running(String),
    /// At least one node is ready.
    Launchable,
    /// Every node is done (an empty graph counts as done).
    Finished,
    /// Nothing left to run and some nodes were abandoned: the brain draws
    /// the next goal, else waits for `revive_stuck` (cooldown, capped);
    /// past the cap the operator decides (rule 7).
    Stuck { abandoned: Vec<String> },
}

/// Result of folding a failure into a node.
#[derive(Debug, Clone, PartialEq)]
pub enum FailureFold {
    /// The node goes back to ready with a fresh method resolve.
    Retry,
    /// The failure budget is spent (or the error can never heal).
    Abandoned,
}

fn is_terminal(status: &NodeStatus) -> bool {
    matches!(status, NodeStatus::Done | NodeStatus::Abandoned)
}

fn release_active(life: &mut Life, index: usize) {
    if life.active.as_deref() == Some(life.nodes[index].id.as_str()) {
        life.active = None;
    }
}

/// Next free `n<k>` id (ids never reuse a number, even after growth).
pub fn next_id(life: &Life) -> String {
    format!("n{}", max_id_number(life) + 1)
}

fn max_id_number(life: &Life) -> usize {
    // History ids count too: a withdrawn ladder step leaves the graph,
    // never its number (`withdraw_gear_step`, logged by the brain).
    life.nodes
        .iter()
        .map(|node| node.id.as_str())
        .chain(life.history.iter().map(|entry| entry.node.as_str()))
        .filter_map(|id| id.strip_prefix('n').and_then(|raw| raw.parse::<usize>().ok()))
        .max()
        .unwrap_or(0)
        .max(life.nodes.len())
}

/// Structural abandon (cascade, cycle): the dependency is the cause, so
/// it replaces whatever the node last waited on (bbkjz2vk n19 kept
/// "Blocked: missing coins:1" after its coin node died, hiding the root).
fn abandon(node: &mut Node, why: String, changes: &mut Vec<String>) {
    changes.push(format!("{} abandoned: {why}", node.id));
    node.status = NodeStatus::Abandoned;
    node.method = None;
    node.last_error = why;
}

/// Open obtain nodes whose legs name no executable (legacy legs frozen
/// in the life file by an older planner) re-derive them from the obtain
/// index: the legs are game data, never a choice to preserve.
fn rederive_dead_legs(life: &mut Life, changes: &mut Vec<String>) {
    let ironman = life.profile.ironman;
    let produce = produce_item(life);
    for node in &mut life.nodes {
        if is_terminal(&node.status) || node.status == NodeStatus::Active {
            continue;
        }
        let dead = crate::obtain_legs::dead_legs(node);
        if dead.is_empty() {
            continue;
        }
        if let NodeKind::ObtainItems { item, .. } = &node.kind {
            node.alternatives = crate::obtain_legs::legs_for(item, ironman, produce.as_deref());
            node.method = None;
            let fresh: Vec<&str> = node.alternatives.iter().map(|alt| alt.method.as_str()).collect();
            changes.push(format!("{} legs re-derived ({} name no script) -> {}", node.id, dead.join(", "), fresh.join(", ")));
        }
    }
}

/// Item of the current production quota: its obtain node keeps
/// production legs only when re-derived (never bought back in).
fn produce_item(life: &Life) -> Option<String> {
    match &life.goal {
        crate::model::GoalKind::Produce { item, .. } => Some(item.clone()),
        _ => None,
    }
}

/// Restore every invariant; returns one human line per change (for the
/// daemon log). Idempotent: a second call returns no change.
pub fn normalize(life: &mut Life) -> Vec<String> {
    let mut changes = Vec::new();
    // Legacy `failed` statuses (older files, typed launch failures) are
    // retries: the failure itself was already counted.
    for node in &mut life.nodes {
        if let NodeStatus::Failed { error } = &node.status {
            if node.last_error.is_empty() {
                node.last_error = error.clone();
            }
            changes.push(format!("{} failed -> ready", node.id));
            node.status = NodeStatus::Ready;
            node.method = None;
        }
    }
    // Active: only `life.active` may be active (locks first, so the
    // cascade below sees orphans as the open nodes they are).
    if let Some(active) = life.active.clone() {
        let live = life.nodes.iter().any(|node| node.id == active && node.status == NodeStatus::Active);
        if !live {
            changes.push(format!("active {active} is not running, cleared"));
            life.active = None;
        }
    }
    let active = life.active.clone();
    for node in &mut life.nodes {
        if node.status == NodeStatus::Active && active.as_deref() != Some(node.id.as_str()) {
            changes.push(format!("{} orphan active -> ready", node.id));
            node.status = NodeStatus::Ready;
            node.method = None;
        }
    }
    rederive_dead_legs(life, &mut changes);
    cascade_abandon(life, &mut changes);
    abandon_unresolvable(life, &mut changes);
    let done: HashSet<String> = life.nodes.iter().filter(|node| node.status == NodeStatus::Done).map(|node| node.id.clone()).collect();
    for node in &mut life.nodes {
        let deps_done = node.deps.iter().all(|dep| done.contains(dep));
        match &node.status {
            NodeStatus::Pending | NodeStatus::Blocked { .. } if deps_done => {
                changes.push(format!("{} {} -> ready", node.id, node.status.as_str()));
                node.status = NodeStatus::Ready;
            }
            NodeStatus::Ready if !deps_done => {
                changes.push(format!("{} ready with open deps -> pending", node.id));
                node.status = NodeStatus::Pending;
            }
            _ => {}
        }
    }
    changes
}

/// Abandoned (or dangling) dependencies abandon their open dependents,
/// transitively. A running node is left alone: its report decides.
fn cascade_abandon(life: &mut Life, changes: &mut Vec<String>) {
    loop {
        let known: HashSet<String> = life.nodes.iter().map(|node| node.id.clone()).collect();
        let dead: HashSet<String> =
            life.nodes.iter().filter(|node| node.status == NodeStatus::Abandoned).map(|node| node.id.clone()).collect();
        let mut moved = false;
        for node in &mut life.nodes {
            if is_terminal(&node.status) || node.status == NodeStatus::Active {
                continue;
            }
            let culprit = node.deps.iter().find(|dep| dead.contains(*dep) || !known.contains(*dep)).cloned();
            if let Some(dep) = culprit {
                let why = if known.contains(&dep) { format!("dependency {dep} abandoned") } else { format!("dependency {dep} does not exist") };
                abandon(node, why, changes);
                moved = true;
            }
        }
        if !moved {
            return;
        }
    }
}

/// Open nodes that can never finish (dependency cycle) are abandoned:
/// a node can finish when it is done, running, or all its deps can.
fn abandon_unresolvable(life: &mut Life, changes: &mut Vec<String>) {
    let mut resolvable: HashSet<String> = life
        .nodes
        .iter()
        .filter(|node| matches!(node.status, NodeStatus::Done | NodeStatus::Active))
        .map(|node| node.id.clone())
        .collect();
    loop {
        let mut grew = false;
        for node in &life.nodes {
            if resolvable.contains(&node.id) || is_terminal(&node.status) {
                continue;
            }
            if node.deps.iter().all(|dep| resolvable.contains(dep)) {
                resolvable.insert(node.id.clone());
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    let mut moved = false;
    for node in &mut life.nodes {
        if !is_terminal(&node.status) && !resolvable.contains(&node.id) {
            abandon(node, "dependency cycle".to_string(), changes);
            moved = true;
        }
    }
    if moved {
        cascade_abandon(life, changes);
    }
}

/// State of a normalized graph (call `normalize` first).
pub fn graph_state(life: &Life) -> GraphState {
    if let Some(active) = &life.active {
        return GraphState::Running(active.clone());
    }
    if life.nodes.iter().any(|node| node.status == NodeStatus::Ready) {
        return GraphState::Launchable;
    }
    if life.nodes.iter().all(|node| node.status == NodeStatus::Done) {
        return GraphState::Finished;
    }
    GraphState::Stuck {
        abandoned: life.nodes.iter().filter(|node| node.status == NodeStatus::Abandoned).map(|node| node.id.clone()).collect(),
    }
}

/// The cooldown revive (live 2026-11-12: bh99tw2f and bbkjz2vk idled
/// forever on abandons caused by operator redeploys and legacy legs).
/// `stuck_ticks` is how long the graph has been stuck (the brain owns the
/// clock). Below `REVIVE_COOLDOWN_TICKS`, or on a graph that is not
/// stuck, nothing moves. Otherwise every abandoned node under
/// `MAX_REVIVES` reopens with fresh failure/block budgets and re-derived
/// legs (its cascade reopens with it); cleanup janitors keep their own
/// stop rule. Returns one line per change; empty = the operator decides.
pub fn revive_stuck(life: &mut Life, stuck_ticks: u64) -> Vec<String> {
    if stuck_ticks < REVIVE_COOLDOWN_TICKS || !matches!(graph_state(life), GraphState::Stuck { .. }) {
        return Vec::new();
    }
    let ironman = life.profile.ironman;
    let produce = produce_item(life);
    let style = crate::combat::goal_combat_style(&life.goal).to_string();
    let mut changes = Vec::new();
    for node in &mut life.nodes {
        if node.status != NodeStatus::Abandoned || node.revives >= MAX_REVIVES || matches!(node.kind, NodeKind::SpaceCleanup) {
            continue;
        }
        node.revives += 1;
        node.status = NodeStatus::Pending;
        node.failures = 0;
        node.blocks = 0;
        node.method = None;
        node.site = None;
        node.spent_ticks = 0;
        rederive_legs(node, ironman, &style, produce.as_deref());
        changes.push(format!("{} revived ({}/{MAX_REVIVES}) after {stuck_ticks} stuck ticks; was: {}", node.id, node.revives, node.last_error));
    }
    if !changes.is_empty() {
        changes.extend(normalize(life));
    }
    changes
}

/// Market rescue (`bans_scope`, user decision 2026-11-13): an abandoned
/// obtain whose item the wallet can buy goes back to work as a purchase
/// only (`market:buy`), fresh counters, and every node its abandonment
/// cascaded to (`dependency <id> abandoned`, transitively) waits on it
/// again. Returns one line per change; empty when `index` is no
/// abandoned obtain.
pub fn retarget_market(life: &mut Life, index: usize) -> Vec<String> {
    let node = &mut life.nodes[index];
    if node.status != NodeStatus::Abandoned || !matches!(node.kind, NodeKind::ObtainItems { .. }) {
        return Vec::new();
    }
    let mut changes = vec![format!("{} rescued on the market (was: {})", node.id, node.last_error)];
    node.alternatives = vec![crate::model::Alternative { method: "market:buy".to_string(), est_ticks: 0, est_coins: 0 }];
    node.status = NodeStatus::Pending;
    node.method = None;
    node.site = None;
    node.toll = false;
    node.boat = false;
    node.failures = 0;
    node.blocks = 0;
    node.spent_ticks = 0;
    node.last_error = format!("market rescue after: {}", node.last_error);
    let mut revived = vec![node.id.clone()];
    let mut cursor = 0;
    while cursor < revived.len() {
        let cause = format!("dependency {} abandoned", revived[cursor]);
        for node in &mut life.nodes {
            if node.status == NodeStatus::Abandoned && node.last_error == cause {
                node.status = NodeStatus::Pending;
                node.method = None;
                node.last_error = String::new();
                changes.push(format!("{} back with its dependency", node.id));
                revived.push(node.id.clone());
            }
        }
        cursor += 1;
    }
    changes.extend(normalize(life));
    changes
}

/// A revived node starts from the planner's current legs: obtain legs
/// from the obtain index (a travel ban is forgotten with the rest), a
/// tranche or combat training whose legs were all banned gets its
/// creation method back.
fn rederive_legs(node: &mut Node, ironman: bool, style: &str, produce: Option<&str>) {
    let method = match &node.kind {
        NodeKind::ObtainItems { item, .. } => {
            node.alternatives = crate::obtain_legs::legs_for(item, ironman, produce);
            return;
        }
        NodeKind::ReachLevel { skill, level } if node.alternatives.is_empty() => {
            let start = crate::tranches::tranches(skill, 1, *level).into_iter().filter(|target| target < level).last().unwrap_or(1);
            crate::tranches::reach_method(skill, start)
        }
        NodeKind::ReachCombat { .. } if node.alternatives.is_empty() => match crate::combat::training_method(style) {
            Ok(method) => method.to_string(),
            Err(_) => return,
        },
        _ => return,
    };
    node.alternatives.push(crate::model::Alternative { method, est_ticks: 0, est_coins: 0 });
}

/// Mark `index` launched: the only way a node becomes active.
pub fn mark_launched(life: &mut Life, index: usize) {
    life.nodes[index].status = NodeStatus::Active;
    life.active = Some(life.nodes[index].id.clone());
}

/// A node whose target is met on positive proof (its report, the
/// holdings after a market fill, the wallet, the live level, the bot
/// seen off the island): done, lock released, its engagement over.
/// `progress` is the caller's proof (level, held count, coins).
pub fn complete_node(life: &mut Life, index: usize, progress: i64) {
    release_active(life, index);
    if life.engagement.as_ref().is_some_and(|engaged| engaged.node == life.nodes[index].id) {
        life.engagement = None;
    }
    let node = &mut life.nodes[index];
    node.status = NodeStatus::Done;
    node.progress_current = progress;
}

/// Back to ready without spending any budget: operator stop, daemon
/// restart, a partial market fill, a stale lock refused at launch. The
/// method drops so the next pass resolves afresh.
pub fn retry_uncounted(life: &mut Life, index: usize) {
    release_active(life, index);
    let node = &mut life.nodes[index];
    node.status = NodeStatus::Ready;
    node.method = None;
}

/// A market buy placed daemon-side rests on the book while the bot works
/// on (user rule 2026-11-13: no idle bot). The node leaves the lock and
/// stays ready, its `market:buy` method kept; the brain skips it while
/// its history says `awaiting_fill` and settles it every pass (filled,
/// partial, deadline) with the moves above.
pub fn park_for_fill(life: &mut Life, index: usize) {
    release_active(life, index);
    life.nodes[index].status = NodeStatus::Ready;
}

/// An engagement window ended (`budget_exhausted` short of the goal):
/// money closes (one engagement is the task), every other node retries.
pub fn end_engagement(life: &mut Life, index: usize) {
    release_active(life, index);
    let mut counter = 0;
    replan_node(&mut life.nodes[index], &ReplanCause::EngagementEnd, &[], &mut counter);
}

/// Live proof that a climb is already over: `Some(level)` when the live
/// level (`level_of`, 1 when unknown) or combat level meets the target of
/// a `reach_level` / `reach_combat` node. Pure decision; the brain closes
/// the node with `complete_node`, never launching a replayed tranche.
pub fn reached_live(node: &Node, level_of: &dyn Fn(&str) -> u8, combat: u8) -> Option<u8> {
    match &node.kind {
        NodeKind::ReachLevel { skill, level } => Some(level_of(skill)).filter(|live| live >= level),
        NodeKind::ReachCombat { level } => Some(combat).filter(|live| live >= level),
        _ => None,
    }
}

/// File a node that runs at once (space janitor, tutorial): ready on
/// arrival, no dependency. Returns its index.
pub fn insert_ready(life: &mut Life, mut node: Node) -> usize {
    node.status = NodeStatus::Ready;
    life.nodes.push(node);
    life.nodes.len() - 1
}

/// File an equipment ladder step (`gear_ladder`): with `item`, an
/// `obtain_items` of one piece bought on the market only (never a shop:
/// every ladder piece is producible, D-A), ready, then the `equip_best`
/// that wears it; without, the `equip_best` alone (a piece already owned).
/// Returns the new ids. The caller files one step at a time
/// (`gear_ladder::step_open`).
pub fn insert_gear_step(life: &mut Life, item: Option<&str>, style: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut deps = Vec::new();
    if let Some(item) = item {
        let id = next_id(life);
        let mut obtain = Node::new(&id, NodeKind::ObtainItems { item: item.to_string(), count: 1 }, Vec::new(), 1);
        obtain.alternatives.push(crate::model::Alternative { method: "market:buy".to_string(), est_ticks: 0, est_coins: 0 });
        obtain.status = NodeStatus::Ready;
        life.nodes.push(obtain);
        deps.push(id.clone());
        ids.push(id);
    }
    let id = next_id(life);
    let mut equip = Node::new(&id, NodeKind::EquipBest { style: style.to_string() }, deps.clone(), 1);
    equip.alternatives.push(crate::model::Alternative { method: format!("equip:{style}"), est_ticks: 400, est_coins: 0 });
    equip.alternatives.push(crate::model::Alternative { method: crate::gear_ladder::LADDER_MARKER.to_string(), est_ticks: 0, est_coins: 0 });
    equip.status = if deps.is_empty() { NodeStatus::Ready } else { NodeStatus::Pending };
    life.nodes.push(equip);
    ids.push(id);
    ids
}

/// Withdraw the ladder step whose chain holds node `index`: its open
/// nodes leave the graph (done ones stay as the record), unless a node
/// outside the step still depends on them. Used when the seller left
/// before the purchase (the standing bid takes over) or when the step
/// died (abandoned): an equipment step never strands the goal's graph.
/// Returns the removed ids.
pub fn withdraw_gear_step(life: &mut Life, index: usize) -> Vec<String> {
    let id = life.nodes[index].id.clone();
    let equip = (0..life.nodes.len())
        .find(|&at| crate::gear_ladder::is_ladder_equip(&life.nodes[at]) && crate::gear_ladder::step_chain(life, at).contains(&id));
    let Some(equip) = equip else { return Vec::new() };
    let chain = crate::gear_ladder::step_chain(life, equip);
    let removable: Vec<String> = chain
        .iter()
        .filter(|member| {
            life.nodes.iter().any(|node| &node.id == *member && !matches!(node.status, NodeStatus::Done | NodeStatus::Active))
                && !life.nodes.iter().any(|node| !chain.contains(&node.id) && node.deps.contains(member))
        })
        .cloned()
        .collect();
    life.nodes.retain(|node| !removable.contains(&node.id));
    removable
}

/// A runtime failure (`Failed`, `Sdk`, bad exit): counted, retried with a
/// fresh resolve until `MAX_FAILURES`, then abandoned.
pub fn record_failure(life: &mut Life, index: usize, error: &str) -> FailureFold {
    release_active(life, index);
    let node = &mut life.nodes[index];
    node.failures = node.failures.saturating_add(1);
    node.last_error = error.to_string();
    node.method = None;
    if node.failures >= MAX_FAILURES {
        node.status = NodeStatus::Abandoned;
        FailureFold::Abandoned
    } else {
        node.status = NodeStatus::Ready;
        FailureFold::Retry
    }
}

/// A contract error (`Usage`, no script for the method): the same
/// launch fails the same way, so the node is abandoned at once.
pub fn record_contract_error(life: &mut Life, index: usize, error: &str) {
    release_active(life, index);
    let node = &mut life.nodes[index];
    node.failures = node.failures.saturating_add(1);
    node.last_error = error.to_string();
    node.method = None;
    node.status = NodeStatus::Abandoned;
}

/// A reported level (or combat level) for a reach node. Real progress
/// clears its block count: a transformation skill blocks on its inputs
/// once per inventory load by design (cook, smelt, burn), and only a
/// block that never leads anywhere must count toward `MAX_BLOCKS`.
pub fn record_progress(life: &mut Life, index: usize, current: i64) {
    let node = &mut life.nodes[index];
    if current > node.progress_current {
        node.blocks = 0;
    }
    node.progress_current = current;
}

/// XP a run earned toward a climb (`gained`, from its report): progress
/// even when no level landed. Live 2026-11-13 (t01, eric_smith,
/// bb4v5d9m): each 20-shrimp refill of a melee climb counted one block
/// while every fight earned XP, the 4th abandoned the climb and banned
/// the skill. Earned XP clears the block count like a level does.
pub fn record_xp(life: &mut Life, index: usize, gained: i64) {
    if gained > 0 {
        life.nodes[index].blocks = 0;
    }
}

/// `record_block_from` with every live level unknown (level 1): offline
/// replays and tests. The brain passes the live levels.
pub fn record_block(life: &mut Life, index: usize, missing: &[String]) -> FailureFold {
    record_block_from(life, index, missing, &|_| 1)
}

/// A blocked stop: the node waits on one `obtain_items` per missing
/// item, reusing an open obtain for the same item instead of stacking
/// duplicates. A missing skill level climbs from `level_of` (the live
/// level): never a tranche the bot already holds. Past `MAX_BLOCKS` the
/// node is abandoned.
pub fn record_block_from(life: &mut Life, index: usize, missing: &[String], level_of: &dyn Fn(&str) -> u8) -> FailureFold {
    release_active(life, index);
    let blocks = life.nodes[index].blocks.saturating_add(1);
    life.nodes[index].blocks = blocks;
    life.nodes[index].last_error = format!("Blocked: missing {}", missing.join(", "));
    // The blocked method sizes the input batches (`replan::need_count`):
    // it clears once the sub-tasks are filed.
    if blocks > MAX_BLOCKS {
        life.nodes[index].method = None;
        life.nodes[index].status = NodeStatus::Abandoned;
        return FailureFold::Abandoned;
    }
    let mut reused = Vec::new();
    let mut fresh_needs = Vec::new();
    let mut fresh_nodes: Vec<Node> = Vec::new();
    // Neither the blocked node nor anything waiting on it is reused as
    // its dependency (t01 n8-n10: a cooking 10->20 climb blocked on a
    // cooking level reused itself and its own successors, a cycle
    // `normalize` abandoned with the whole chain).
    let me = life.nodes[index].id.clone();
    let waiting: Vec<String> = crate::bans_scope::dependents(life, &me).into_iter().map(|position| life.nodes[position].id.clone()).collect();
    for need in missing {
        // A combat level is a dependency like an item: one `reach_combat`
        // node trains it (goal style, a site safe for fresh bots), reused
        // when an open one already aims at least that high.
        if let Some(level) = need.strip_prefix("combat:").and_then(|raw| raw.parse::<u8>().ok()) {
            let open = life.nodes.iter().find(|node| {
                !waiting.contains(&node.id) && !is_terminal(&node.status) && matches!(&node.kind, NodeKind::ReachCombat { level: aimed } if *aimed >= level)
            });
            match open {
                Some(node) => reused.push(node.id.clone()),
                None => {
                    let id = format!("n{}", max_id_number(life) + 1 + fresh_nodes.len());
                    let mut training = Node::new(&id, NodeKind::ReachCombat { level }, Vec::new(), i64::from(level));
                    let style = crate::combat::goal_combat_style(&life.goal);
                    if let Ok(method) = crate::combat::training_method(style) {
                        training.alternatives.push(crate::model::Alternative { method: method.to_string(), est_ticks: 0, est_coins: 0 });
                    }
                    reused.push(id);
                    fresh_nodes.push(training);
                }
            }
            continue;
        }
        // PRD3 lot 4: a purchase short of coins waits on a `have_coins`
        // node (earned by the most profitable money method), reused when
        // an open one already aims at least that high.
        if let Some(amount) = need.strip_prefix("coins:").and_then(|raw| raw.parse::<i64>().ok()) {
            let open = life.nodes.iter().find(|node| {
                !waiting.contains(&node.id) && !is_terminal(&node.status) && matches!(&node.kind, NodeKind::HaveCoins { amount: aimed } if *aimed >= amount)
            });
            match open {
                Some(node) => reused.push(node.id.clone()),
                None => {
                    let id = format!("n{}", max_id_number(life) + 1 + fresh_nodes.len());
                    let mut earn = Node::new(&id, NodeKind::HaveCoins { amount }, Vec::new(), amount);
                    earn.alternatives.push(crate::model::Alternative { method: "gather:sheep/wool".to_string(), est_ticks: 0, est_coins: 0 });
                    reused.push(id);
                    fresh_nodes.push(earn);
                }
            }
            continue;
        }
        // A quest the only workable ways need (Rune Mysteries before
        // essence): the quest's own expansion (skill gates, item checks,
        // the quest node), reused when an open quest node exists.
        if let Some(quest) = need.strip_prefix("quest:") {
            // The block is live proof the quest is NOT done: a done quest
            // node of it (a quest script that reported success short of
            // the completion, live 2026-11-13) proves nothing any more.
            for node in life.nodes.iter_mut() {
                if node.status == NodeStatus::Done && matches!(&node.kind, NodeKind::CompleteQuest { quest: done } if done == quest) && !node.last_error.starts_with(crate::quests::DISPROVED) {
                    node.last_error = format!("{}{me} blocked on quest:{quest}", crate::quests::DISPROVED);
                }
            }
            let open = life.nodes.iter().find(|node| {
                !waiting.contains(&node.id) && !is_terminal(&node.status) && matches!(&node.kind, NodeKind::CompleteQuest { quest: aimed } if aimed == quest)
            });
            match open {
                Some(node) => reused.push(node.id.clone()),
                None if lifescape_gamedata::quest_requirements::by_key(quest).is_some() => {
                    let mut counter = max_id_number(life) + fresh_nodes.len();
                    let chain = crate::expand::expand_quest_from(quest, &life.profile, &mut counter, level_of);
                    if let Some(last) = chain.last() {
                        reused.push(last.id.clone());
                    }
                    fresh_nodes.extend(chain);
                }
                None => {}
            }
            continue;
        }
        // An area the work needs (`access:<area>`: a guild, ...): its
        // gates expanded (skill climb, uniform), reused when open.
        if let Some(area) = need.strip_prefix("access:") {
            let open = life.nodes.iter().find(|node| {
                !waiting.contains(&node.id) && !is_terminal(&node.status) && matches!(&node.kind, NodeKind::AccessArea { area: aimed } if aimed == area)
            });
            match open {
                Some(node) => reused.push(node.id.clone()),
                None => {
                    let mut counter = max_id_number(life) + fresh_nodes.len();
                    let chain = crate::expand::expand_access(area, &life.profile, &mut counter, level_of);
                    if let Some(last) = chain.last() {
                        reused.push(last.id.clone());
                    }
                    fresh_nodes.extend(chain);
                }
            }
            continue;
        }
        // A skill level a production needs: the reach chain for it,
        // reused when an open reach node already aims at least that high.
        if let Some((skill, level)) = need.strip_prefix("level:").and_then(|rest| rest.rsplit_once(':')).and_then(|(skill, raw)| raw.parse::<u8>().ok().map(|level| (skill.to_string(), level))) {
            let open = life.nodes.iter().find(|node| {
                !waiting.contains(&node.id)
                    && !is_terminal(&node.status)
                    && matches!(&node.kind, NodeKind::ReachLevel { skill: aimed_skill, level: aimed } if *aimed_skill == skill && *aimed >= level)
            });
            match open {
                Some(node) => reused.push(node.id.clone()),
                None => {
                    let mut counter = max_id_number(life) + fresh_nodes.len();
                    let mut after = String::new();
                    let chain = crate::skeleton_deps::reach_chain_planned(&skill, level_of(&skill), level, &mut counter, &mut after, crate::expand::NESTED_DEPTH);
                    if let Some(last) = chain.last() {
                        reused.push(last.id.clone());
                    }
                    fresh_nodes.extend(chain);
                }
            }
            continue;
        }
        let open = life.nodes.iter().find(|node| {
            !waiting.contains(&node.id) && !is_terminal(&node.status) && matches!(&node.kind, NodeKind::ObtainItems { item, .. } if item == need)
        });
        match open {
            Some(node) => reused.push(node.id.clone()),
            None => fresh_needs.push(need.clone()),
        }
    }
    life.nodes.extend(fresh_nodes);
    let mut counter = max_id_number(life);
    let live = match &life.nodes[index].kind {
        NodeKind::ReachLevel { skill, .. } => Some(level_of(skill)),
        _ => None,
    };
    let fresh = crate::replan::replan_node_from(&mut life.nodes[index], &ReplanCause::Blocked, &fresh_needs, &mut counter, live);
    let node = &mut life.nodes[index];
    node.method = None;
    for id in reused {
        if !node.deps.contains(&id) {
            node.deps.push(id);
        }
    }
    node.status = NodeStatus::Pending;
    life.nodes.extend(fresh);
    FailureFold::Retry
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "lifecycle_revive_tests.rs"]
mod revive_tests;

#[cfg(test)]
#[path = "lifecycle_moves_tests.rs"]
mod moves_tests;
