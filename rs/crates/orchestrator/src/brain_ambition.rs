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
//! - `before_draw`: milestones read the live levels and coins, a demand
//!   milestone answers an open bid, an achieved ambition is redrawn.
//! - `after_close`: a goal close folds into its milestone; blocked
//!   `MILESTONE_MAX_BLOCKS` times redraws the ambition (approved policy,
//!   logged), a completed milestone may change it (optional, at most once
//!   every `AMBITION_CHANGE_TICKS`).

use std::sync::Mutex;

use lifescape_json::Value;
use lifescape_planner::ambition_catalog::{group_skills, primary_group};
use lifescape_planner::{
    achieved, assign_fleet, coverage_gaps, describe_ambition, draw_kind, new_ambition, on_goal_closed, optional_change, refresh, revise,
    CloseOutcome, DemandItem, FleetBot, KindDemand, Life, Stamp, GROUPS, MILESTONE_MAX_BLOCKS,
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

/// Coins on the bot (pack + bank + worn) from the live snapshot.
fn live_coins(live: &Value) -> i64 {
    let mut total = 0;
    for key in ["inv", "bank", "worn"] {
        for entry in live.get(key).as_arr().cloned().unwrap_or_default() {
            if entry.get("name").as_str().is_some_and(|name| name.eq_ignore_ascii_case("coins")) {
                total += entry.get("count").as_i64().unwrap_or(1).max(0);
            }
        }
    }
    total
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

/// Every other life on disk (unreadable files are skipped and logged).
fn other_lives(account: &str) -> Vec<Life> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(lives_dir()) else { return out };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()).map(str::to_string) else { continue };
        if name.eq_ignore_ascii_case(account) {
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
    (coverage_gaps(&kinds), redundant)
}

/// First pass on a life without ambition: assign the whole fleet. Returns
/// true when this life got one (save it).
pub(crate) fn ensure(orch: &Orchestrator, life: &mut Life, live: &Value) -> bool {
    if life.ambition.is_some() {
        return false;
    }
    let _guard = fleet_guard().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (items, weights) = crate::market_demand::kind_demand();
    let others = other_lives(&life.account);
    let mut bots = vec![fleet_bot(life, account_levels(&life.account, Some(live)))];
    for other in &others {
        let snapshot = live_of(orch, &other.account);
        bots.push(fleet_bot(other, account_levels(&other.account, snapshot.as_ref())));
    }
    let stamp = stamp_now();
    let mut mine = false;
    for assignment in assign_fleet(&bots, &weights) {
        let Some(bot) = bots.iter().find(|bot| bot.account == assignment.account) else { continue };
        let level_of = |skill: &str| bot.level(skill);
        let Some(ambition) = new_ambition(&assignment.kind, &assignment.why, &level_of, bot.seed, &items, stamp) else {
            crate::logln!("brain {}: ambition {} unknown to the catalogue, not assigned", assignment.account, assignment.kind);
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

/// Redraw this life's ambition now (blocked or achieved): the fleet's
/// open gaps first, never back to `exclude`.
fn redraw(life: &mut Life, live: &Value, exclude: &[&str], revision_why: &str) {
    let (items, weights) = crate::market_demand::kind_demand();
    let (gaps, _) = fleet_view(life);
    let levels = account_levels(&life.account, Some(live));
    let bot = fleet_bot(life, levels);
    let (kind, why) = draw_kind(&bot, &gaps, &weights, exclude);
    replace(life, &bot, &kind, &why, revision_why, &items);
}

fn replace(life: &mut Life, bot: &FleetBot, kind: &str, why: &str, revision_why: &str, items: &[DemandItem]) {
    let stamp = stamp_now();
    let level_of = |skill: &str| bot.level(skill);
    let Some(next) = new_ambition(kind, why, &level_of, life.seed, items, stamp) else {
        crate::logln!("brain {}: ambition {kind} unknown to the catalogue, kept the current one", life.account);
        return;
    };
    let from = life.ambition.as_ref().map(|ambition| ambition.kind.clone()).unwrap_or_default();
    crate::logln!("brain {}: ambition {from} -> {} [{revision_why}]", life.account, describe_ambition(&next));
    revise(life, next, revision_why, stamp);
}

/// Before a draw: milestones read live levels and coins, open bids may
/// add a demand milestone, an achieved ambition is redrawn (not optional).
pub(crate) fn before_draw(life: &mut Life, live: &Value) {
    let level_of = |skill: &str| crate::brain_succession::live_level(live, skill);
    let coins = live_coins(live);
    let items = crate::market_demand::demand_items(&crate::market_demand::signals());
    let seed = life.seed;
    let Some(ambition) = life.ambition.as_mut() else { return };
    refresh(ambition, &level_of, coins, seed, &items);
    if achieved(ambition) {
        let kind = ambition.kind.clone();
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
    let (items, weights): (Vec<DemandItem>, Vec<KindDemand>) = crate::market_demand::kind_demand();
    let (gaps, redundant) = fleet_view(life);
    let bot = fleet_bot(life, account_levels(&life.account, Some(live)));
    if let Some((kind, why)) = optional_change(&ambition, &bot, &gaps, redundant, &weights, &ticks_since) {
        replace(life, &bot, &kind, &why, &format!("change:{why}"), &items);
    }
}

#[cfg(test)]
mod tests {
    use super::live_coins;
    use lifescape_json::{object, str_value, Value};

    #[test]
    fn coins_count_pack_and_bank() {
        let coins = |count: i64| object(vec![("name", str_value("Coins")), ("count", Value::Int(count))]);
        let live = object(vec![("inv", Value::Arr(vec![coins(120)])), ("bank", Value::Arr(vec![coins(5000)]))]);
        assert_eq!(live_coins(&live), 5120);
        assert_eq!(live_coins(&Value::Null), 0);
    }
}
