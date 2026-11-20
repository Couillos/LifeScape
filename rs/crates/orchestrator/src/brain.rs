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
use lifescape_planner::{Archetype, GoalKind, HistoryEntry, Life, Node, NodeKind, NodeStatus, Profile, expand_goal, life_from_value, life_to_value};

use crate::brain_fold::handle_report;
use crate::daemon::Orchestrator;
use crate::jobs::Job;
pub use crate::brain_plan::{TRIP_TICKS, choose_money};

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
    let _phase = crate::cpu_profile::phase("load_life");
    let path = life_path(account);
    let stamp = file_stamp(&path)?;
    if let Some(life) = cached_life(account, stamp) {
        return Some(life);
    }
    let _miss = crate::cpu_profile::phase("load_life.parse");
    let life = read_life_file(&path)?;
    remember_life(&life, stamp);
    Some(life)
}

/// Parse a life file, its finished nodes retired: files written before
/// the retirement (2026-11-14) shed them on first read (one log line);
/// the next save writes the small file. A market maker role takes the
/// balanced partition's count and the common target
/// (`market_maker::migrate_role`, one log line).
pub(crate) fn read_life_file(path: &std::path::Path) -> Option<Life> {
    let text = std::fs::read_to_string(path).ok()?;
    let value = lifescape_json::parse(&text).ok()?;
    let mut life = life_from_value(&value)?;
    if crate::market_maker::migrate_role(&mut life) {
        if let Some(role) = lifescape_planner::maker_role(&life) {
            crate::logln!("brain {}: market maker role migrated to the balanced slices: slice {}/{}, common capital target {}", life.account, role.slice, role.slices, role.capital_target);
        }
    }
    if lifescape_planner::retire_due(&life) {
        let retirement = lifescape_planner::retire_finished(&mut life);
        if retirement.floor_inferred.is_some() || retirement.done.len() + retirement.abandoned.len() > 0 {
            crate::logln!("brain {}: life loaded: {} ({} nodes left)", life.account, retirement.describe(), life.nodes.len());
        }
    }
    Some(life)
}

/// Write a life, its finished nodes retired first (`lifecycle_retire`:
/// done nodes leave the graph, abandoned ones with their goal, history
/// compacted). The caller's copy is untouched: indexes it holds stay
/// valid until its pass ends; the next load reads the retired graph.
pub fn save_life(life: &Life) -> Result<(), String> {
    let _phase = crate::cpu_profile::phase("save_life");
    let path = life_path(&life.account);
    let retired = write_life_file(&path, life)?;
    crate::brain_chain::note_saved(&life.account);
    if let Some(stamp) = file_stamp(&path) {
        remember_life(retired.as_ref().unwrap_or(life), stamp);
    }
    Ok(())
}

/// Write `life` to `path`, retired first when due; returns the retired
/// copy when one was made (the version on disk).
pub(crate) fn write_life_file(path: &std::path::Path, life: &Life) -> Result<Option<Life>, String> {
    let retired = lifescape_planner::retire_due(life).then(|| {
        let mut copy = life.clone();
        lifescape_planner::retire_finished(&mut copy);
        copy
    });
    let on_disk = retired.as_ref().unwrap_or(life);
    crate::paths::write_json(path, &life_to_value(on_disk)).map_err(|error| format!("brain save {}: {error}", life.account))?;
    Ok(retired)
}

/// File identity a cached life is valid for: modification time and size.
/// Any outside write (operator edit, `--goal` reset) changes it and the
/// next load parses the file again.
type LifeStamp = (std::time::SystemTime, u64);

fn file_stamp(path: &std::path::Path) -> Option<LifeStamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Parsed lives by account (CPU, VPS 2026-11-14): the brain loaded and
/// parsed each life JSON (up to ~600 KB) on every tick of every account —
/// the bulk of `brain-tick`'s CPU. A clone of the parsed graph replaces
/// the parse while the file is unchanged.
fn life_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, (LifeStamp, Life)>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, (LifeStamp, Life)>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn cached_life(account: &str, stamp: LifeStamp) -> Option<Life> {
    let cache = life_cache().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.get(account).filter(|(seen, _)| *seen == stamp).map(|(_, life)| life.clone())
}

fn remember_life(life: &Life, stamp: LifeStamp) {
    let mut cache = life_cache().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.insert(life.account.clone(), (stamp, life.clone()));
}

/// Accepted `--goal` shapes (besides the pin `ambition:<skill>:<level>`,
/// `parse_pin`): `max:a,b:40`, `obtain:feather:2000`,
/// `produce:bronze_platebody:20` (made, never bought),
/// `money`, `quest:cook`, `equip:melee`, `area:karamja`, `rich` (the
/// producer money methods; the merchant variant was removed in wave 3).
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
    if let Some(rest) = text.strip_prefix("produce:") {
        let (item, count) = rest.rsplit_once(':').ok_or_else(|| "goal produce:<item>:<count>".to_string())?;
        let count: i64 = count.parse().map_err(|_| "goal count must be a number".to_string())?;
        if item.is_empty() {
            return Err("goal produce:<item>:<count>".to_string());
        }
        return Ok(GoalKind::Produce { item: item.to_string(), count: count.max(1) });
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
        return Ok(GoalKind::GetRich);
    }
    Err(format!("unknown goal {text:?} (max:|obtain:|produce:|money|quest:|equip:|area:|rich)"))
}

/// `ambition:<skill>:<level>` (operator pin, user decision 2026-11-14):
/// `Some((skill, level))`, `None` for every other goal text.
pub fn parse_pin(text: &str) -> Result<Option<(String, u8)>, String> {
    let Some(rest) = text.trim().strip_prefix("ambition:") else { return Ok(None) };
    let usage = || "goal ambition:<skill>:<level> (level 2-99)".to_string();
    let (skill, level) = rest.rsplit_once(':').ok_or_else(usage)?;
    let level: u8 = level.parse().map_err(|_| usage())?;
    if skill.is_empty() || !(2..=99).contains(&level) {
        return Err(usage());
    }
    Ok(Some((skill.to_string(), level)))
}

/// `create-user --goal ambition:overall` (user 2026-11-15): the
/// operator-only highscore ambition (first overall: total level, then XP).
pub const OVERALL_GOAL: &str = "ambition:overall";

/// The overall ambition of a fresh account and its first goal: the lowest
/// F2P skill (`goals::lowest_skill_goal`), no milestone.
fn overall_start(seed: u64) -> Result<(lifescape_planner::Ambition, GoalKind), String> {
    let level_of = |name: &str| if name == "hitpoints" { 10 } else { 1 };
    let held = |_: &str| 0;
    let now = lifescape_planner::BotNow { level_of: &level_of, coins: 0, held: &held };
    let mut ambition = lifescape_planner::new_ambition(lifescape_planner::OVERALL, "operator:overall", &now, seed, &[], crate::brain_ambition::stamp_now()).map_err(|error| error.to_string())?;
    // An operator's ambition: never undone by the D3 review.
    ambition.draw_rule = lifescape_planner::OPERATOR_RULE.to_string();
    let goal = lifescape_planner::lowest_skill_goal(lifescape_planner::OVERALL_SKILLS, &level_of, &[], seed, 0).ok_or("overall: no trainable skill")?;
    Ok((ambition, goal))
}

/// The pinned ambition of a fresh account (levels 1, hitpoints 10, no
/// coins), its first open milestone's goal and that milestone's id.
fn pinned_start(skill: &str, level: u8, seed: u64) -> Result<(lifescape_planner::Ambition, GoalKind, String), String> {
    let level_of = |name: &str| if name == "hitpoints" { 10 } else { 1 };
    let held = |_: &str| 0;
    let now = lifescape_planner::BotNow { level_of: &level_of, coins: 0, held: &held };
    let ambition = lifescape_planner::pinned_ambition(skill, level, &now, seed, &[], crate::brain_ambition::stamp_now()).map_err(|error| error.to_string())?;
    let first = lifescape_planner::next_milestone(&ambition, &[]).ok_or_else(|| format!("pinned ambition {} has no open milestone", ambition.kind))?;
    let goal = lifescape_planner::goal_for_milestone(first);
    let origin = first.id.clone();
    Ok((ambition, goal, origin))
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

/// Profile draw from the seed: affinity and patience uniform in [0,1];
/// every drawn bot is a producer (the 10 % merchant draw was removed in
/// wave 3, plan marché Q6: market makers are created on purpose,
/// `create-user --role market_maker`).
pub fn draw_profile(seed: u64, iron: bool) -> Profile {
    let affinity = (xorshift(seed, 101) % 1000) as f64 / 999.0;
    let patience = (xorshift(seed, 102) % 1000) as f64 / 999.0;
    Profile::new(iron, affinity, patience, seed, Archetype::Producer)
}

/// `create-user --role <role>`: the one role an operator creates.
pub const MARKET_MAKER_ROLE: &str = "market_maker";

/// `--role`: true for a market maker; typed refusals for an Iron maker
/// (an Iron never trades) and an unknown role.
pub fn maker_flag(role: Option<&str>, iron: bool) -> Result<bool, String> {
    match role {
        None => Ok(false),
        Some(MARKET_MAKER_ROLE) if !lifescape_planner::MAKERS_ACTIVE => Err("market makers are unused (MAKERS_ACTIVE false, user 2026-11-20)".to_string()),
        Some(MARKET_MAKER_ROLE) if iron => Err("an Iron never trades: no Iron market maker".to_string()),
        Some(MARKET_MAKER_ROLE) => Ok(true),
        Some(other) => Err(format!("unknown role {other:?} (known: {MARKET_MAKER_ROLE})")),
    }
}

/// The market maker's start (plan marché Q6): its slice and capital
/// target, its operator-only coin ladder (`market_maker` ambition, held
/// for good) and that ladder's first goal (money, producer methods).
fn maker_start(seed: u64, taken: &[u32]) -> Result<(Archetype, lifescape_planner::Ambition, GoalKind, String), String> {
    let role = crate::market_maker::new_role(taken)?;
    let level_of = |name: &str| if name == "hitpoints" { 10 } else { 1 };
    let held = |_: &str| 0;
    let now = lifescape_planner::BotNow { level_of: &level_of, coins: 0, held: &held };
    let mut ambition = lifescape_planner::new_ambition(lifescape_planner::MARKET_MAKER_KIND, "operator:market_maker", &now, seed, &[], crate::brain_ambition::stamp_now()).map_err(|error| error.to_string())?;
    // An operator's ambition: never undone by the D3 review.
    ambition.draw_rule = lifescape_planner::OPERATOR_RULE.to_string();
    let first = lifescape_planner::next_milestone(&ambition, &[]).ok_or("market maker ladder has no open milestone")?;
    let goal = lifescape_planner::goal_for_milestone(first);
    let origin = first.id.clone();
    Ok((Archetype::MarketMaker(role), ambition, goal, origin))
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
/// `role`: `Some("market_maker")` makes a dedicated market maker (plan
/// marché Q6): next free slice, its capital target, its coin ladder.
pub fn ensure_life(account: &str, iron: bool, goal_text: &str, role: Option<&str>) -> Result<Life, String> {
    if let Some(life) = load_life(account) {
        return Ok(life);
    }
    if iron && !iron_allowed(iron_count()) {
        return Err(format!("iron cap reached ({MAX_IRON}): no new Ironman"));
    }
    let maker = maker_flag(role, iron)?;
    let overall = goal_text.trim() == OVERALL_GOAL;
    let pin = if overall { None } else { parse_pin(goal_text)? };
    let goal = match &pin {
        Some(_) => GoalKind::MakeMoney,
        None if overall => GoalKind::MakeMoney,
        None => parse_goal(goal_text)?,
    };
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|span| span.as_nanos() as u64)
        .unwrap_or(1)
        ^ (std::process::id() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut profile = draw_profile(seed, iron);
    // A pin starts on its ambition's first milestone, linked (`m1`).
    let (goal, ambition, origin) = match pin {
        _ if maker => {
            let (archetype, ambition, goal, origin) = maker_start(seed, &crate::market_maker::taken_slices())?;
            profile.archetype = archetype;
            (goal, Some(ambition), Some(origin))
        }
        Some((skill, level)) => {
            let (ambition, goal, origin) = pinned_start(&skill, level, seed)?;
            (goal, Some(ambition), Some(origin))
        }
        None if overall => {
            let (ambition, goal) = overall_start(seed)?;
            (goal, Some(ambition), None)
        }
        None => (goal, None, None),
    };
    let nodes = expand_goal(&goal, &profile);
    let mut life = Life {
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
        ambition,
        goal_origin: None,
        memory: Default::default(),
    };
    lifescape_planner::mark_drawn(&mut life, origin.as_deref());
    save_life(&life)?;
    Ok(life)
}

/// `create-user [pseudo] [--iron] [--goal kind] [--role market_maker]`:
/// pseudo generated when omitted, life file written, then the CLI opens
/// the session and runs the tutorial (tutorial first, brain takes over
/// whenever idle after).
pub fn create_user(account_opt: Option<String>, iron: bool, goal: Option<String>, role: Option<String>) -> Result<String, String> {
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
    ensure_life(&account, iron, goal.as_deref().unwrap_or("max:fishing:40"), role.as_deref())?;
    Ok(account)
}

/// Keep-list for a cleanup order: unfinished graph obtain items plus the
/// item checks of unfinished quests, plus what the ambition's own open
/// milestones will consume (`market_fit::own_use_items`: a firemaker's
/// logs, a cook's raw fish, a smith's bars and ores, a priest's bones —
/// never sold, never dropped), deduplicated and sorted. Coins, food and
/// best tools are added worker-side (planner triage, kit specs).
pub fn cleanup_keep(life: &Life) -> Vec<String> {
    let mut keep = life.ambition.as_ref().map(lifescape_planner::market_fit::own_use_items).unwrap_or_default();
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
    // Inputs bought for a 100 % market lot wait for its craft (R5).
    keep.extend(lifescape_planner::trade_craft::open_inputs(life));
    keep.sort();
    keep.dedup();
    keep
}

/// Space-pressure insert (L8) for one life: terminal graphs included, a
/// full bank needs its janitor even when everything else is done.
/// Returns true when a node was inserted.
pub fn maybe_insert_cleanup(life: &mut Life, bank_type_count: usize) -> bool {
    maybe_insert_cleanup_at(life, bank_type_count, unix_now())
}

/// `maybe_insert_cleanup` at `now` (Unix seconds): a janitor pass that
/// ended short (`CLEANUP_SHORT_TAG`) holds the next insert for
/// `CLEANUP_RETRY_SECS`, never a tight relaunch loop.
pub fn maybe_insert_cleanup_at(life: &mut Life, bank_type_count: usize, now: i64) -> bool {
    if lifescape_planner::cleanup_cooling(&life.history, now) {
        return false;
    }
    let open = life.nodes.iter().any(|node| {
        matches!(node.kind, NodeKind::SpaceCleanup)
            && !matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned)
    });
    // Done breaks a failure streak; two abandoned janitors in a row mean
    // the pressure never drains, so inserts stop instead of looping.
    // Retired janitors count too (`memory.recent`).
    let stalled = lifescape_planner::janitor_stalled(life);
    if !lifescape_planner::cleanup_needed(open, stalled, bank_type_count) {
        return false;
    }
    // Past every id ever issued (retired ones included).
    let id = lifescape_planner::next_id(life);
    lifescape_planner::insert_ready(life, Node::new(&id, NodeKind::SpaceCleanup, Vec::new(), 0));
    true
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
}

pub(crate) fn push_history(life: &mut Life, node: &str, outcome: &str) {
    let now = unix_now();
    life.history.push(HistoryEntry { node: node.to_string(), started: now, ended: now, outcome: outcome.to_string() });
}

/// Brain inbox: the supervisor never waits on files, the store or a
/// session socket. It only try-sends here; one brain thread owns every
/// blocking brain step (life files, market/store reads, worker RPC).
/// A wedged worker or store stalls the brain, never the supervision or
/// the CLI dispatch. A full queue drops loudly (the next pass re-queues).
/// Inbox room: every account's pending tick plus a burst of reports.
const INBOX_CAPACITY: usize = 128;

/// Accounts with a plain tick already queued (deduplication).
/// Wall time between two plain brain ticks of one account (CPU, VPS
/// 2026-11-14: a full `maybe_launch` every 500 ms supervisor pass burnt
/// 30–85 % of a core doing nothing new). A report always goes through at
/// once; a plain tick (idle detection, parked buys, periodic sales)
/// waits at most this long. Time, not a pass count (live 2026-11-14, 60
/// bots: one supervisor sweep took ~8 s, so "every 6th pass" meant one
/// tick per ~50–90 s and idle bots waited that long between decisions:
/// ryan_mason one node per ~1.5 min, owen_wood/lily_brook 2 min idle
/// after a gear step closed without a launch).
pub(crate) const PLAIN_TICK_SECS: f64 = 3.0;

/// True when this supervisor pass carries the account's plain tick:
/// at least `PLAIN_TICK_SECS` since the last one (`None`: never ticked).
pub(crate) fn plain_tick_due(last: &mut Option<f64>, now: f64) -> bool {
    if last.is_some_and(|at| now - at < PLAIN_TICK_SECS) {
        return false;
    }
    *last = Some(now);
    true
}

fn plain_tick_passes() -> &'static std::sync::Mutex<std::collections::HashMap<String, Option<f64>>> {
    static PASSES: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Option<f64>>>> = std::sync::OnceLock::new();
    PASSES.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn queued_ticks() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static QUEUED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    QUEUED.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

/// Brain threads (live 2026-11-14, 60 bots): one serial thread spent
/// 56 s of every 60 s waiting (worker launch round trips, life saves)
/// and reached each bot once every ~80 s, so most sat idle. Accounts
/// are sharded by a stable hash: one account always lands on the same
/// thread (its reports and ticks keep their order), different accounts
/// decide in parallel; shared tables are behind their own locks.
const BRAIN_SHARDS: usize = 4;

type BrainSender = std::sync::mpsc::SyncSender<(String, Option<Value>)>;

fn inbox() -> &'static std::sync::Mutex<Option<Vec<BrainSender>>> {
    static INBOX: std::sync::OnceLock<std::sync::Mutex<Option<Vec<BrainSender>>>> = std::sync::OnceLock::new();
    INBOX.get_or_init(|| std::sync::Mutex::new(None))
}

/// Stable shard of an account (FNV-1a): same account, same thread.
fn shard_of(account: &str, shards: usize) -> usize {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in account.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (hash % shards.max(1) as u64) as usize
}

fn ensure_brain(orch: &Arc<Orchestrator>) {
    {
        let slot = inbox().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_some() {
            return;
        }
    }
    let mut senders = Vec::with_capacity(BRAIN_SHARDS);
    let mut receivers = Vec::with_capacity(BRAIN_SHARDS);
    for _ in 0..BRAIN_SHARDS {
        let (sender, receiver) = std::sync::mpsc::sync_channel::<(String, Option<Value>)>(INBOX_CAPACITY);
        senders.push(sender);
        receivers.push(receiver);
    }
    {
        let mut slot = inbox().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = Some(senders);
    }
    for (index, receiver) in receivers.into_iter().enumerate() {
        let orch = orch.clone();
        std::thread::Builder::new()
            .name(format!("brain-tick-{index}"))
            .spawn(move || {
                crate::cpu_profile::mark_brain_thread();
                while let Ok((account, report)) = receiver.recv() {
                    if report.is_none() {
                        queued_ticks().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&account);
                    }
                    let job = orch.jobs.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&account).cloned();
                    let Some(job) = job else {
                        if report.is_some() {
                            crate::brain_inflight::folded(&account);
                        }
                        continue;
                    };
                    if let Some(report) = report {
                        // The life graph first, then the rates (plan P10:
                        // the rates ingest took up to 35 s under load and
                        // the idle detector read the run as unreported).
                        // Both are bounded file/store writes on the brain
                        // thread, never the supervisor.
                        {
                            let _phase = crate::cpu_profile::phase("tick.handle_report");
                            handle_report(&orch, &job, &report);
                        }
                        crate::brain_inflight::folded(&account);
                        crate::logln!("brain {}: folded report stop={}", account, report.get("stop").as_str().unwrap_or("?"));
                        let _phase = crate::cpu_profile::phase("tick.ingest_report");
                        if let Err(error) = crate::jobs::ingest_report_persisted(&report) {
                            crate::logln!("job {account}: rates ingest failed: {error}");
                        }
                    }
                    let _phase = crate::cpu_profile::phase("tick.maybe_launch");
                    crate::brain_chain::run(&orch, &job);
                }
            })
            .expect("brain thread spawns");
    }
}

/// Supervisor entry: enqueue only, never block. The brain thread folds
/// reports and launches; `had_report` mirrors the flag the loop already
/// drained, so the brain never misses a push.
pub fn on_supervise(orch: &Arc<Orchestrator>, job: &Arc<Job>, report: Option<Value>) {
    ensure_brain(orch);
    // No silent idle (2026-11-14): a brain-run worker idle 5 min alerts.
    crate::brain_idle::watch(job);
    let had_report = report.is_some();
    // One plain tick per account in flight: the supervisor passes every
    // 500 ms over ~26 jobs (~52 ticks/s) and a capacity-16 inbox was
    // always full — 2 340 "inbox full" lines in 36 min, every decision
    // delayed for nothing. A report always goes through.
    if !had_report {
        let mut passes = plain_tick_passes().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !plain_tick_due(passes.entry(job.account.clone()).or_insert(None), crate::jobs::now_secs()) {
            return;
        }
    }
    if !had_report {
        let mut waiting = queued_ticks().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !waiting.insert(job.account.clone()) {
            return;
        }
    }
    // Counted before the send: the brain thread may fold it at once.
    if had_report {
        crate::brain_inflight::queued(&job.account);
    }
    let queued = inbox()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .map(|senders| senders[shard_of(&job.account, senders.len())].try_send((job.account.clone(), report)))
        .is_some_and(|sent| sent.is_ok());
    if had_report && !queued {
        crate::brain_inflight::folded(&job.account);
    }
    if !queued && !had_report {
        queued_ticks().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&job.account);
    }
    if !queued {
        // The drained report goes back to pending (`last_report` still
        // holds it): dropped here, its node read "no report" later.
        let kept = had_report && job.inner.try_lock().map(|mut inner| inner.report_pending = true).is_ok();
        let note = match (had_report, kept) {
            (true, true) => " (report kept pending)",
            (true, false) => " (report dropped: job contended)",
            _ => "",
        };
        crate::logln!("brain {}: inbox full, tick deferred to next pass{note}", job.account);
    }
}

#[cfg(test)]
#[path = "brain_retire_tests.rs"]
mod retire_tests;

#[cfg(test)]
mod tests {
    use super::{cleanup_keep, draw_profile, generate_pseudo, iron_allowed, maybe_insert_cleanup, maybe_insert_cleanup_at, parse_goal, parse_pin, pinned_start, MAX_IRON};
    use lifescape_planner::{GoalKind, Life, Node, NodeKind};

    /// `--goal ambition:<skill>:<level>` pins the kind and starts on its
    /// first milestone (linked); other goal texts are no pin; bad pins
    /// are typed errors, never a silent default.
    #[test]
    fn pin_goal_starts_on_its_first_milestone() {
        assert_eq!(parse_pin("ambition:magic:99"), Ok(Some(("magic".to_string(), 99))));
        assert_eq!(parse_pin("max:magic:99"), Ok(None));
        assert!(parse_pin("ambition:magic").is_err());
        assert!(parse_pin("ambition:magic:100").is_err());
        let (ambition, goal, origin) = pinned_start("smithing", 99, 11).expect("pinned");
        assert_eq!(ambition.kind, "smith");
        assert!(lifescape_planner::pinned(&ambition));
        assert_eq!(origin, "m1");
        assert!(matches!(goal, GoalKind::MaxSkills { ref skills, level: 10 } if skills == &vec!["smithing".to_string()]), "smith starts on smithing 10 (computed ladder): {goal:?}");
        assert!(pinned_start("hitpoints", 99, 11).is_err());
    }

    /// A plain tick is due on wall time, whatever the sweep length: a
    /// slow sweep (60 bots, ~8 s) ticks every account each sweep, a fast
    /// one (500 ms) every `PLAIN_TICK_SECS`.
    #[test]
    fn plain_ticks_fire_on_wall_time_not_pass_count() {
        let mut last = None;
        assert!(super::plain_tick_due(&mut last, 100.0), "the first pass ticks at once (fresh account)");
        let fast: Vec<bool> = (1..=12).map(|step| super::plain_tick_due(&mut last, 100.0 + f64::from(step) * 0.5)).collect();
        assert_eq!(fast.iter().filter(|fired| **fired).count(), 2, "6 s of 500 ms passes: one tick per {} s", super::PLAIN_TICK_SECS);
        let mut slow = Some(200.0);
        for step in 1..=5 {
            assert!(super::plain_tick_due(&mut slow, 200.0 + f64::from(step) * 8.0), "an 8 s sweep ticks every pass");
        }
    }

    #[test]
    fn goals_parse() {
        assert!(matches!(parse_goal("money"), Ok(GoalKind::MakeMoney)));
        assert!(matches!(parse_goal(""), Ok(GoalKind::MakeMoney)));
        let max = parse_goal("max:fishing,cooking:40").expect("max parses");
        assert!(matches!(max, GoalKind::MaxSkills { level: 40, .. }));
        assert!(parse_goal("max:fishing").is_err(), "level required");
        assert!(matches!(parse_goal("quest:cook"), Ok(GoalKind::CompleteQuest { .. })));
        assert!(matches!(parse_goal("produce:bronze_platebody:20"), Ok(GoalKind::Produce { count: 20, .. })));
        assert!(matches!(parse_goal("rich"), Ok(GoalKind::GetRich)));
        assert!(parse_goal(&["rich:", "merchant"].concat()).is_err(), "the merchant role is gone (wave 3)");
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

    /// Wave 3 (plan marché Q6): no drawn merchant any more; a market
    /// maker is created on purpose, on the first free slice, with its
    /// coin ladder held for good; never an Iron.
    #[test]
    fn every_drawn_bot_is_a_producer_and_makers_are_created() {
        for seed in 0..200 {
            assert_eq!(draw_profile(seed, false).archetype, lifescape_planner::Archetype::Producer);
        }
        let (archetype, ambition, goal, origin) = super::maker_start(7, &[0, 1]).expect("slice 2 free");
        let lifescape_planner::Archetype::MarketMaker(role) = archetype else { panic!("a maker") };
        assert_eq!((role.slice, role.slices), (2, 11));
        assert_eq!(role.capital_target, crate::market_maker::common_capital_target(), "one target for every maker");
        assert_eq!(ambition.kind, lifescape_planner::MARKET_MAKER_KIND);
        assert!(lifescape_planner::pinned(&ambition));
        assert_eq!((goal, origin.as_str()), (GoalKind::GetRich, "m1"), "money first (producer methods)");
        assert!(super::maker_start(7, &(0..11).collect::<Vec<u32>>()).is_err(), "every slice taken");
        // Makers unused (user 2026-11-20): the role is refused while off.
        assert_eq!(super::maker_flag(Some(super::MARKET_MAKER_ROLE), false).is_ok(), lifescape_planner::MAKERS_ACTIVE);
        assert_eq!(super::maker_flag(None, true), Ok(false));
        assert!(super::maker_flag(Some(super::MARKET_MAKER_ROLE), true).is_err(), "never an Iron maker");
        assert!(super::maker_flag(Some("merchant"), false).is_err(), "unknown role");
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
            ambition: None,
            goal_origin: None,
            memory: Default::default(),
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
            ambition: None,
            goal_origin: None,
            memory: Default::default(),
        }
    }

    #[test]
    fn cached_life_serves_until_the_file_changes() {
        let dir = std::env::temp_dir().join(format!("lifescape-life-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("t99.json");
        std::fs::write(&path, "{}").expect("first write");
        let stamp = super::file_stamp(&path).expect("stamp");
        let mut life = terminal_life();
        life.account = format!("cache-test-{}", std::process::id());
        super::remember_life(&life, stamp);
        assert_eq!(super::cached_life(&life.account, stamp), Some(life.clone()), "unchanged file: the parsed life, no parse");
        std::fs::write(&path, "{ \"changed\": true }").expect("outside write");
        let moved = super::file_stamp(&path).expect("stamp");
        assert_ne!(moved, stamp, "size changed");
        assert_eq!(super::cached_life(&life.account, moved), None, "changed file: parse again");
        let _ = std::fs::remove_dir_all(&dir);
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

    /// Live bdr53jee 2026-11-14: a janitor done with 56 types left was
    /// followed by a fresh one 7 s later, fifteen times. The short pass
    /// holds inserts for `CLEANUP_RETRY_SECS`, then the next one runs.
    #[test]
    fn a_short_janitor_pass_backs_off() {
        let mut life = terminal_life();
        life.history.push(lifescape_planner::HistoryEntry { node: "n2".to_string(), started: 1000, ended: 1000, outcome: lifescape_planner::CLEANUP_SHORT_TAG.to_string() });
        assert!(!maybe_insert_cleanup_at(&mut life, 56, 1007), "seven seconds later: no fresh janitor");
        assert_eq!(life.nodes.len(), 1, "life untouched");
        assert!(maybe_insert_cleanup_at(&mut life, 56, 1000 + lifescape_planner::CLEANUP_RETRY_SECS), "the window over: the next pass runs");
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
            "farm:feather", "market:buy", "shop:buy", "space:cleanup",
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
        // The maker's post maps on its site (plan Q6).
        let mut post = Node::new("n8", NodeKind::MakeMoney, Vec::new(), 0);
        post.site = Some(lifescape_planner::MAKER_POSTS[0]);
        let (script, _) = lifescape_planner::map_method(lifescape_planner::MAKE_METHOD, &post, None).expect("the post maps");
        assert!(crate::registry::script_known(&script));
    }

    /// P3.6: a method without script is a contract error: abandoned at
    /// once with its cause (relaunching the same method can never work).
    #[test]
    fn no_script_abandons_typed_never_silent() {
        let mut life = terminal_life();
        life.nodes[0].status = lifescape_planner::NodeStatus::Active;
        life.active = Some("n1".to_string());
        crate::brain_launch::fail_no_script(&mut life, 0, "smelt:iron_bar");
        assert_eq!(life.nodes[0].status, lifescape_planner::NodeStatus::Abandoned);
        assert!(life.active.is_none(), "no stranded lock");
        assert_eq!(life.nodes[0].last_error, "no-script: smelt:iron_bar", "the missing method rides in the error");
        assert_eq!(life.history.iter().filter(|entry| entry.outcome == "abandoned").count(), 1, "one typed entry in history");
    }
}
