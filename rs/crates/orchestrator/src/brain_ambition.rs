#![forbid(unsafe_code)]
//! Ambitions in the brain (user decision 2026-11-13). The pure rules live
//! in the planner (`ambition`, `ambition_catalog`, `ambition_fleet`);
//! this file reads the fleet (life files, live snapshots, engine saves),
//! the market demand (`market_demand`), stamps, logs and writes.
//!
//! - `ensure`: the first pass that meets a life without ambition runs the
//!   fleet assignment over EVERY life file (coverage of the 12 F2P groups
//!   and a merchant first, then random weighted by demand and levels),
//!   writes each one and logs `brain <acct>: ambition <kind> (<why>) —
//!   milestones …`.
//! - `before_draw`: milestones read the live levels, coins and stock, a
//!   demand milestone answers an open bid that pays (not already held);
//!   a milestone closed here weighs an optional change; an achieved
//!   ambition is redrawn past the throttle when another bot holds its
//!   group, else its ladder extends.
//! - `after_close`: a goal close folds into its milestone; blocked
//!   `MILESTONE_MAX_BLOCKS` times redraws the ambition (approved policy,
//!   logged), a completed milestone may change it (optional, at most once
//!   every `AMBITION_CHANGE_TICKS`).
//! - `coverage_pass`: uncovered groups are logged once per change and a
//!   redundant bot past the throttle fills one (review 2026-11-13).
//! - A draw that gives the bot no milestone is refused and drawn again
//!   (never an ambition achieved at birth, redrawn every pass).

use std::sync::Mutex;

use lifescape_json::Value;
use lifescape_planner::ambition_catalog::{group_skills, primary_group, SPECS};
use lifescape_planner::{
    achieved, assign_fleet, coverage_gaps, coverage_repair, describe_ambition, draw_kind, extend_ambition, new_ambition, new_ambition_extended,
    on_goal_closed, optional_change, refresh, revise, Ambition, BotNow, CloseOutcome, DemandItem, FleetBot, KindDemand, Life, Stamp, GROUPS,
    MILESTONE_MAX_BLOCKS,
};

use crate::brain::{lives_dir, load_life, save_life};
use crate::daemon::Orchestrator;

/// One fleet pass at a time (the brain thread is single, the guard keeps
/// a future second caller from double-assigning).
fn fleet_guard() -> &'static Mutex<()> {
    static GUARD: Mutex<()> = Mutex::new(());
    &GUARD
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
}

/// Daemon tick clock (wall time over the measured tick: 60 ms dev, 600
/// ms prod) and unix seconds; elapsed ticks are always measured from the
/// seconds (`ticks_since`), so a tick-rate change never jumps a throttle.
pub fn stamp_now() -> Stamp {
    let millis = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_millis() as u64).unwrap_or(0);
    Stamp { tick: millis / crate::market_eta::live_tick_ms().max(1), secs: now_secs() }
}

fn ticks_since(stamp: i64) -> u64 {
    crate::brain_stuck::seconds_to_ticks(now_secs() - stamp)
}

/// Every skill the groups train (melee: attack, strength, defence, hp).
fn tracked_skills() -> Vec<&'static str> {
    GROUPS.iter().flat_map(|group| group_skills(group)).collect()
}

/// Levels of one account: the live snapshot when it carries skills, else
/// the engine save (written at logout), else nothing (level 1).
fn account_levels(account: &str, live: Option<&Value>) -> Vec<(String, u8)> {
    let streamed = live.is_some_and(|snapshot| snapshot.get("skills").as_arr().is_some_and(|rows| !rows.is_empty()));
    if let (true, Some(snapshot)) = (streamed, live) {
        return tracked_skills().into_iter().map(|skill| (skill.to_string(), crate::brain_succession::live_level(snapshot, skill))).collect();
    }
    let Some(save) = lifescape_sdk::save::load_account(account, None) else { return Vec::new() };
    tracked_skills()
        .into_iter()
        .filter_map(|skill| {
            let stat = lifescape_sdk::provenance::skill_id(skill)?;
            save.stats.get(&stat).map(|(level, _)| (skill.to_string(), (*level).clamp(1, 99)))
        })
        .collect()
}

fn fleet_bot(life: &Life, levels: Vec<(String, u8)>) -> FleetBot {
    FleetBot {
        account: life.account.clone(),
        levels,
        ironman: life.profile.ironman,
        merchant_archetype: life.profile.archetype == lifescape_planner::Archetype::Merchant,
        market_affinity: life.profile.market_affinity,
        seed: life.seed,
        kind: life.ambition.as_ref().map(|ambition| ambition.kind.clone()),
    }
}

/// Accounts the daemon runs (a job each), minus the dev/personal ones.
/// Refreshed on every launch (`ensure`): coverage counts the fleet, never
/// the old lives `run/lives` keeps (2026-11-13: t01/t10/t18/t19 held 6
/// coverage roles while not running).
fn fleet_accounts() -> &'static Mutex<std::collections::BTreeSet<String>> {
    static FLEET: Mutex<std::collections::BTreeSet<String>> = Mutex::new(std::collections::BTreeSet::new());
    &FLEET
}

fn refresh_fleet(orch: &Orchestrator) {
    let Ok(jobs) = orch.jobs.try_lock() else { return };
    let excluded = crate::economy_guard::excluded_accounts();
    let current: std::collections::BTreeSet<String> = jobs
        .keys()
        .filter(|name| !excluded.contains(&name.to_lowercase()))
        .cloned()
        .collect();
    drop(jobs);
    *fleet_accounts().lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = current;
}

/// Every other fleet life on disk (unreadable files are skipped and logged).
fn other_lives(account: &str) -> Vec<Life> {
    let mut out = Vec::new();
    let fleet = fleet_accounts().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    let Ok(entries) = std::fs::read_dir(lives_dir()) else { return out };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()).map(str::to_string) else { continue };
        if name.eq_ignore_ascii_case(account) || !fleet.contains(&name) {
            continue;
        }
        match load_life(&name) {
            Some(life) => out.push(life),
            None => crate::logln!("brain {account}: ambition fleet: life {name} unreadable, left out"),
        }
    }
    out.sort_by(|left, right| left.account.cmp(&right.account));
    out
}

fn live_of(orch: &Orchestrator, account: &str) -> Option<Value> {
    let job = orch.jobs.try_lock().ok()?.get(account).cloned()?;
    let inner = job.inner.try_lock().ok()?;
    inner.live.clone()
}
/// Gaps the rest of the fleet leaves open, and whether another bot holds
/// this life's primary group too.
fn fleet_view(life: &Life) -> (Vec<String>, bool) {
    let others = other_lives(&life.account);
    let kinds: Vec<&str> = others.iter().filter_map(|other| other.ambition.as_ref().map(|ambition| ambition.kind.as_str())).collect();
    let mine = life.ambition.as_ref().and_then(|ambition| primary_group(&ambition.kind));
    let redundant = mine.is_some_and(|group| kinds.iter().any(|kind| primary_group(kind) == Some(group)));
    let mut all = kinds.clone();
    if let Some(ambition) = life.ambition.as_ref() {
        all.push(ambition.kind.as_str());
    }
    (coverage_gaps(&all), redundant)
}

/// Units of one object on the bot (pack, bank, worn) in the live
/// snapshot, by debug or display name. Listed stock stays on the bot
/// until its deal is delivered, so it counts.
pub(crate) fn live_count(live: &Value, item: &str) -> i64 {
    let debug = item.to_lowercase();
    let display = lifescape_gamedata::objs_by_name(item)
        .into_iter()
        .next()
        .and_then(|entry| entry.display.or(entry.name))
        .unwrap_or(item)
        .to_lowercase();
    let mut total = 0;
    for key in ["inv", "bank", "worn"] {
        for entry in live.get(key).as_arr().cloned().unwrap_or_default() {
            match &entry {
                Value::Obj(_) => {
                    let name = entry.get("name").as_str().unwrap_or("").to_lowercase();
                    if name == debug || name == display {
                        total += entry.get("count").as_i64().unwrap_or(1).max(0);
                    }
                }
                Value::Str(name) if name.to_lowercase() == debug || name.to_lowercase() == display => total += 1,
                _ => {}
            }
        }
    }
    total
}

/// Coins on the bot (pack + bank + worn) from the live snapshot.
fn live_coins(live: &Value) -> i64 {
    live_count(live, "coins")
}

/// The draw inputs shared by a redraw: demand items, kind weights, gaps.
struct DrawInputs {
    items: Vec<DemandItem>,
    weights: Vec<KindDemand>,
    gaps: Vec<String>,
}

/// The ambition `first` names for this bot, else the next draw: a coverage
/// role extends past its ladder (its holder is the group's only one), any
/// other ladder that gives the bot no milestone is refused (typed, logged)
/// and drawn again without it, never an ambition achieved at birth. The
/// generalist always builds, so the loop ends.
fn build(account: &str, bot: &FleetBot, now: &BotNow, first: (String, String), inputs: &DrawInputs, exclude: &[&str], stamp: Stamp) -> Option<(Ambition, String)> {
    let mut excluded: Vec<String> = exclude.iter().map(|kind| kind.to_string()).collect();
    let (mut kind, mut why) = first;
    for _ in 0..=SPECS.len() {
        let made = if why.starts_with("coverage:") {
            new_ambition_extended(&kind, &why, now, bot.seed, &inputs.items, stamp)
        } else {
            new_ambition(&kind, &why, now, bot.seed, &inputs.items, stamp)
        };
        match made {
            Ok(ambition) => return Some((ambition, why)),
            Err(error) => {
                crate::logln!("brain {account}: {error} ({why}), drawing again without it");
                excluded.push(kind.clone());
                let refs: Vec<&str> = excluded.iter().map(String::as_str).collect();
                (kind, why) = draw_kind(bot, &inputs.gaps, &inputs.weights, &refs);
            }
        }
    }
    None
}

/// What the ambition reads of the bot, borrowed from the live snapshot.
fn with_now<T>(live: &Value, body: impl FnOnce(&BotNow) -> T) -> T {
    let level_of = |skill: &str| crate::brain_succession::live_level(live, skill);
    let held = |item: &str| live_count(live, item);
    body(&BotNow { level_of: &level_of, coins: live_coins(live), held: &held })
}

/// First pass on a life without ambition: assign the whole fleet. Returns
/// true when this life got one (save it).
pub(crate) fn ensure(orch: &Orchestrator, life: &mut Life, live: &Value) -> bool {
    refresh_fleet(orch);
    if life.ambition.is_some() {
        return false;
    }
    let _guard = fleet_guard().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (items, weights) = crate::market_demand::kind_demand();
    let others = other_lives(&life.account);
    let mut bots = vec![(fleet_bot(life, account_levels(&life.account, Some(live))), Some(live.clone()))];
    for other in &others {
        let snapshot = live_of(orch, &other.account);
        bots.push((fleet_bot(other, account_levels(&other.account, snapshot.as_ref())), snapshot));
    }
    let fleet: Vec<FleetBot> = bots.iter().map(|(bot, _)| bot.clone()).collect();
    let stamp = stamp_now();
    let inputs = DrawInputs { items, weights, gaps: Vec::new() };
    let mut mine = false;
    for assignment in assign_fleet(&fleet, &inputs.weights) {
        let Some((bot, snapshot)) = bots.iter().find(|(bot, _)| bot.account == assignment.account) else { continue };
        let snapshot = snapshot.clone().unwrap_or(Value::Null);
        let level_of = |skill: &str| bot.level(skill);
        let held = |item: &str| live_count(&snapshot, item);
        let now = BotNow { level_of: &level_of, coins: live_coins(&snapshot), held: &held };
        let first = (assignment.kind.clone(), assignment.why.clone());
        let Some((ambition, _)) = build(&assignment.account, bot, &now, first, &inputs, &[], stamp) else {
            crate::logln!("brain {}: no ambition buildable, left unassigned", assignment.account);
            continue;
        };
        crate::logln!("brain {}: ambition {}", assignment.account, describe_ambition(&ambition));
        if assignment.account == life.account {
            life.ambition = Some(ambition);
            mine = true;
            continue;
        }
        let Some(mut other) = load_life(&assignment.account) else { continue };
        if other.ambition.is_some() {
            continue;
        }
        other.ambition = Some(ambition);
        if let Err(error) = save_life(&other) {
            crate::logln!("{error}");
        }
    }
    mine
}

/// Redraw this life's ambition now (blocked, achieved past the throttle):
/// the fleet's open gaps first, never back to `exclude`.
fn redraw(life: &mut Life, live: &Value, exclude: &[&str], revision_why: &str) {
    let (items, weights) = crate::market_demand::kind_demand_for(&life.account);
    let (gaps, _) = fleet_view(life);
    let bot = fleet_bot(life, account_levels(&life.account, Some(live)));
    let first = draw_kind(&bot, &gaps, &weights, exclude);
    let inputs = DrawInputs { items, weights, gaps };
    replace(life, live, &bot, first, &inputs, exclude, revision_why);
}

fn replace(life: &mut Life, live: &Value, bot: &FleetBot, first: (String, String), inputs: &DrawInputs, exclude: &[&str], revision_why: &str) {
    let stamp = stamp_now();
    let built = with_now(live, |now| build(&life.account, bot, now, first, inputs, exclude, stamp));
    let Some((next, _)) = built else {
        crate::logln!("brain {}: no ambition buildable, kept the current one", life.account);
        return;
    };
    let from = life.ambition.as_ref().map(|ambition| ambition.kind.clone()).unwrap_or_default();
    crate::logln!("brain {}: ambition {from} -> {} [{revision_why}]", life.account, describe_ambition(&next));
    revise(life, next, revision_why, stamp);
}

/// Before a draw: milestones read live levels, coins and holdings, open
/// bids that pay may add a demand milestone. A level or coin milestone
/// closed here weighs an optional change like a closed goal. An achieved
/// ambition is redrawn only when the 72 000-tick throttle allows it and
/// another bot holds its group; otherwise its ladder extends (the sole
/// holder never leaves its group, the throttle is the user's rule).
pub(crate) fn before_draw(life: &mut Life, live: &Value) {
    let items = crate::market_demand::demand_items(&crate::market_demand::signals_for(&life.account));
    let seed = life.seed;
    let account = life.account.clone();
    let Some(ambition) = life.ambition.as_mut() else { return };
    let closed = with_now(live, |now| refresh(ambition, now, seed, &items));
    for milestone_id in &closed {
        crate::logln!("brain {account}: milestone {milestone_id} done (live levels/coins)");
    }
    if achieved(ambition) {
        on_achieved(life, live);
    } else if !closed.is_empty() {
        maybe_change(life, live);
    }
}

fn on_achieved(life: &mut Life, live: &Value) {
    let Some(ambition) = life.ambition.as_ref() else { return };
    let kind = ambition.kind.clone();
    let throttled = !lifescape_planner::may_change(ambition, &ticks_since);
    let (_, redundant) = fleet_view(life);
    if redundant && !throttled {
        redraw(life, live, &[kind.as_str()], "achieved");
        return;
    }
    let why = if redundant { "changed less than 72000 ticks ago" } else { "sole holder of its group" };
    let seed = life.seed;
    let Some(ambition) = life.ambition.as_mut() else { return };
    let added = with_now(live, |now| extend_ambition(ambition, now, seed));
    if added > 0 {
        crate::logln!("brain {}: ambition {kind} ladder done, extended ({why}): {}", life.account, describe_ambition(ambition));
    } else {
        crate::logln!("brain {}: ambition {kind} ladder done and nothing left to climb, redrawing", life.account);
        redraw(life, live, &[kind.as_str()], "achieved");
    }
}

/// After a goal close: fold it into its milestone. Blocked
/// `MILESTONE_MAX_BLOCKS` times redraws the ambition at once (approved
/// policy); a done milestone may change it (optional, throttled).
pub(crate) fn after_close(life: &mut Life, live: &Value, stuck: bool) {
    match on_goal_closed(life, stuck) {
        CloseOutcome::Unlinked | CloseOutcome::Reopened(_) => {}
        CloseOutcome::Blocked { id, count } => {
            crate::logln!("brain {}: milestone {id} blocked ({count}/{MILESTONE_MAX_BLOCKS})", life.account);
        }
        CloseOutcome::RedrawNeeded { id, count } => {
            let kind = life.ambition.as_ref().map(|ambition| ambition.kind.clone()).unwrap_or_default();
            crate::logln!("brain {}: milestone {id} blocked {count} times, redrawing ambition {kind} (approved policy)", life.account);
            redraw(life, live, &[kind.as_str()], &format!("blocked:{id}x{count}"));
        }
        CloseOutcome::Done(id) => {
            crate::logln!("brain {}: milestone {id} done", life.account);
            maybe_change(life, live);
        }
    }
}

fn maybe_change(life: &mut Life, live: &Value) {
    let Some(ambition) = life.ambition.clone() else { return };
    if !lifescape_planner::may_change(&ambition, &ticks_since) {
        return;
    }
    let (items, weights): (Vec<DemandItem>, Vec<KindDemand>) = crate::market_demand::kind_demand_for(&life.account);
    let (gaps, redundant) = fleet_view(life);
    let bot = fleet_bot(life, account_levels(&life.account, Some(live)));
    if let Some((kind, why)) = optional_change(&ambition, &bot, &gaps, redundant, &weights, &ticks_since) {
        let current = ambition.kind.clone();
        let revision_why = format!("change:{why}");
        let inputs = DrawInputs { items, weights, gaps };
        replace(life, live, &bot, (kind, why), &inputs, &[current.as_str()], &revision_why);
    }
}

/// Wall seconds between two coverage checks of one account.
pub const COVERAGE_EVERY_SECS: f64 = 300.0;

fn last_coverage() -> &'static Mutex<std::collections::HashMap<String, f64>> {
    static TABLE: std::sync::OnceLock<Mutex<std::collections::HashMap<String, f64>>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// Last gap set logged for the fleet (one line per change).
fn last_gaps() -> &'static Mutex<Vec<String>> {
    static GAPS: Mutex<Vec<String>> = Mutex::new(Vec::new());
    &GAPS
}

/// Coverage repair (review 2026-11-13), every `COVERAGE_EVERY_SECS` per
/// account on the launch pass: the fleet's uncovered groups are logged
/// once per change (a closed bot, a holder that moved on); a bot whose
/// group another bot also holds and whose last change is 72 000 ticks old
/// takes the gap at once (its current goal runs on, unlinked). Returns
/// true when the life changed (save it).
pub(crate) fn coverage_pass(life: &mut Life, live: &Value) -> bool {
    let wall = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs_f64()).unwrap_or(0.0);
    {
        let mut table = last_coverage().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if table.get(&life.account).is_some_and(|last| wall - last < COVERAGE_EVERY_SECS) {
            return false;
        }
        table.insert(life.account.clone(), wall);
    }
    let Some(ambition) = life.ambition.clone() else { return false };
    let (gaps, redundant) = fleet_view(life);
    {
        let mut logged = last_gaps().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if *logged != gaps {
            if gaps.is_empty() {
                crate::logln!("brain: fleet coverage complete (every F2P group and a merchant held)");
            } else {
                crate::logln!(
                    "brain: fleet coverage gap: {} held by no running bot; a bot whose group another also holds takes it once its last change is {} ticks old",
                    gaps.join(", "),
                    lifescape_planner::AMBITION_CHANGE_TICKS
                );
            }
            *logged = gaps.clone();
        }
    }
    let bot = fleet_bot(life, account_levels(&life.account, Some(live)));
    // A gap draw never reads the demand weights (coverage wins first).
    let Some((kind, why)) = coverage_repair(&ambition, &bot, &gaps, redundant, &[], &ticks_since) else { return false };
    let items = crate::market_demand::demand_items(&crate::market_demand::signals_for(&life.account));
    let inputs = DrawInputs { items, weights: Vec::new(), gaps };
    let before = life.ambition.clone();
    let current = ambition.kind.clone();
    replace(life, live, &bot, (kind, why.clone()), &inputs, &[current.as_str()], &format!("repair:{why}"));
    life.ambition != before
}

#[cfg(test)]
mod tests {
    use super::{live_coins, live_count};
    use lifescape_json::{object, str_value, Value};

    #[test]
    fn coins_count_pack_and_bank() {
        let coins = |count: i64| object(vec![("name", str_value("Coins")), ("count", Value::Int(count))]);
        let live = object(vec![("inv", Value::Arr(vec![coins(120)])), ("bank", Value::Arr(vec![coins(5000)]))]);
        assert_eq!(live_coins(&live), 5120);
        assert_eq!(live_coins(&Value::Null), 0);
    }

    /// The stock a demand milestone and a quota baseline read: pack, bank
    /// and worn, by display or debug name.
    #[test]
    fn stock_counts_every_place_by_either_name() {
        let stack = |name: &str, count: i64| object(vec![("name", str_value(name)), ("count", Value::Int(count))]);
        let live = object(vec![
            ("inv", Value::Arr(vec![stack("Shrimps", 20)])),
            ("bank", Value::Arr(vec![stack("shrimp", 80), stack("Coins", 9)])),
            ("worn", Value::Arr(vec![str_value("bronze_scimitar")])),
        ]);
        assert_eq!(live_count(&live, "shrimp"), 100);
        assert_eq!(live_count(&live, "bronze_scimitar"), 1);
        assert_eq!(live_count(&live, "raw_shrimp"), 0);
    }
}
