#![forbid(unsafe_code)]
//! Ambitions in the brain (user decision 2026-11-13). The pure rules live
//! in the planner (`ambition`, `ambition_catalog`, `ambition_fleet`);
//! this file reads the fleet (life files, live snapshots, engine saves),
//! the market demand (`market_demand`), stamps, logs and writes.
//!
//! - `ensure`: the first pass that meets a life without ambition runs the
//!   fleet assignment over EVERY life file (coverage of the 12 F2P groups
//!   first, then random weighted by need; level-led for the D3
//!   exceptions, `lifescape_planner::LEVEL_LED_ACCOUNTS`),
//!   writes each one and logs `brain <acct>: ambition <kind> (<why>) —
//!   milestones …`.
//! - A4 (plan progression, user 2026-11-21): every new ambition of a bot
//!   outside the D3 exceptions is GIVEN by the value of the unserved demand
//!   (`brain_value::draw`, `lifescape_planner::ambition_value`), never
//!   drawn at random; the one-time fleet rebalance is `brain_rebalance`.
//! - `before_draw`: milestones read the live levels, coins and stock, a
//!   demand milestone answers an open bid that pays (not already held);
//!   an ambition never changes before it is achieved (A2); an achieved
//!   ambition of a need-led bot is redrawn by need (D3, never extended);
//!   an exception's or a pin's is redrawn past the throttle when another
//!   bot holds its group, else its ladder extends.
//! - `review_pass` (one-time D3 mark, never a retirement) and `replace_stale_goal` (the
//!   goal an earlier ambition drew is replaced): `ambition_review`.
//! - `after_close`: a goal close folds into its milestone; blocked
//!   `MILESTONE_MAX_BLOCKS` times, the milestone is set aside and the
//!   ambition kept (A2, user 2026-11-21), a market shortfall or gear
//!   nobody makes never counts a block (D3, A3); no optional change.
//! - `coverage_pass`: uncovered groups are logged once per change; a
//!   running ambition is never retired to fill one (A2).
//! - A draw that gives the bot no milestone is refused and drawn again
//!   (never an ambition achieved at birth, redrawn every pass).

use std::sync::Mutex;

use lifescape_json::Value;
use lifescape_planner::ambition_catalog::{group_skills, primary_group, specs};
use lifescape_planner::{
    achieved, coverage_gaps, describe_ambition, draw_kind, neediest_kind, extend_ambition, keeps_ladder_end, mark_rule, new_ambition_supplied,
    new_ambition_extended_supplied, milestone_cool_key, on_goal_closed, on_goal_shortfall, resume_set_aside, saturate, pinned, refresh_supplied, release_pin, revise,
    Ambition, BotNow, CloseOutcome, DemandItem, FleetBot, KindDemand, Life, Stamp, GROUPS, MILESTONE_MAX_BLOCKS, MILESTONE_SHORTFALL_COOL_TICKS,
    STALE_GOAL_CAUSE,
};
use lifescape_planner::cooldown::{cool_entry, COOL_NODE};

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
pub(crate) fn account_levels(account: &str, live: Option<&Value>) -> Vec<(String, u8)> {
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

pub(crate) fn fleet_bot(life: &Life, levels: Vec<(String, u8)>) -> FleetBot {
    FleetBot {
        account: life.account.clone(),
        levels,
        ironman: life.profile.ironman,
        market_affinity: life.profile.market_affinity,
        seed: life.seed,
        kind: life.ambition.as_ref().map(|ambition| ambition.kind.clone()),
        market_maker: lifescape_planner::maker_role(life).is_some(),
        operator: life.ambition.as_ref().is_some_and(|ambition| pinned(ambition) || ambition.draw_rule == lifescape_planner::ambition_review::OPERATOR_RULE)
            || lifescape_planner::maker_role(life).is_some(),
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

/// Running fleet accounts (staff and personal ones excluded), as of the
/// last launch.
pub(crate) fn fleet_size() -> usize {
    fleet_accounts().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).len()
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
/// Gaps the rest of the fleet leaves open, whether another bot holds
/// this life's primary group too, and the kinds the other bots hold
/// (the need's saturation, D3).
fn fleet_view(life: &Life) -> (Vec<String>, bool, Vec<String>) {
    let others = other_lives(&life.account);
    let kinds: Vec<&str> = others.iter().filter_map(|other| other.ambition.as_ref().map(|ambition| ambition.kind.as_str())).collect();
    let mine = life.ambition.as_ref().and_then(|ambition| primary_group(&ambition.kind));
    let redundant = mine.is_some_and(|group| kinds.iter().any(|kind| primary_group(kind) == Some(group)));
    let mut all = kinds.clone();
    if let Some(ambition) = life.ambition.as_ref() {
        all.push(ambition.kind.as_str());
    }
    let held: Vec<String> = kinds.iter().map(|kind| kind.to_string()).collect();
    (coverage_gaps(&all), redundant, held)
}

/// The draw inputs one bot reads (its own bids left out), the need
/// saturated by the kinds the other bots hold (D3).
fn need_demand(account: &str, held: &[String]) -> crate::market_demand::DrawDemand {
    let mut demand = crate::market_demand::kind_demand_for(account);
    let refs: Vec<&str> = held.iter().map(String::as_str).collect();
    demand.weights = saturate(&demand.weights, &refs);
    demand
}

/// Units of one object on the bot (pack, bank, worn) in the live
/// snapshot, by debug or display name. Listed stock stays on the bot
/// until its deal is delivered, so it counts.
pub(crate) fn live_count(live: &Value, item: &str) -> i64 {
    // By object id: homonym displays never mix (tour 3 point 7).
    crate::live_items::live_units(live, item)
}

/// Coins on the bot (pack + bank + worn) from the live snapshot.
fn live_coins(live: &Value) -> i64 {
    live_count(live, "coins")
}

/// The draw inputs shared by a redraw: demand items, kind weights, gaps,
/// and the fleet's surplus (the rung products, `ambition_supply`).
struct DrawInputs {
    items: Vec<DemandItem>,
    weights: Vec<KindDemand>,
    gaps: Vec<String>,
    surplus: Vec<(String, f64)>,
    /// The A4 value's market inputs and the kinds the other bots hold.
    value: crate::market_demand::ValueInputs,
    held: Vec<String>,
}

impl DrawInputs {
    fn from_demand(demand: crate::market_demand::DrawDemand, gaps: Vec<String>, held: Vec<String>) -> DrawInputs {
        DrawInputs { items: demand.items, weights: demand.weights, gaps, surplus: demand.surplus, value: demand.value, held }
    }
}

/// The ambition `first` names for this bot, else the next draw: a coverage
/// role extends past its ladder (its holder is the group's only one), any
/// other ladder that gives the bot no milestone is refused (typed, logged)
/// and drawn again without it, never an ambition achieved at birth. The
/// generalist always builds, so the loop ends.
fn build(account: &str, bot: &FleetBot, now: &BotNow, first: (String, String), inputs: &DrawInputs, exclude: &[&str], stamp: Stamp) -> Option<(Ambition, String)> {
    let mut excluded: Vec<String> = exclude.iter().map(|kind| kind.to_string()).collect();
    let (mut kind, mut why) = first;
    for _ in 0..=specs().len() {
        let market = (inputs.items.as_slice(), inputs.surplus.as_slice());
        let made = if why.starts_with("coverage:") {
            new_ambition_extended_supplied(&kind, &why, now, bot.seed, market, stamp)
        } else {
            new_ambition_supplied(&kind, &why, now, bot.seed, market, stamp)
        };
        match made {
            Ok(mut ambition) => {
                // The rule this draw followed (D3): `need`, or `level`
                // for the exceptions; the one-time review reads it.
                mark_rule(&mut ambition, bot);
                return Some((ambition, why));
            }
            Err(error) => {
                crate::logln!("brain {account}: {error} ({why}), drawing again without it");
                excluded.push(kind.clone());
                let refs: Vec<&str> = excluded.iter().map(String::as_str).collect();
                match crate::brain_value::draw(account, bot, &inputs.gaps, &inputs.weights, &inputs.value, &inputs.surplus, &inputs.held, &refs) {
                    Ok(next) => (kind, why) = next,
                    Err(error) => {
                        crate::logln!("brain {account}: {error}");
                        return None;
                    }
                }
            }
        }
    }
    None
}

/// What the ambition reads of the bot, borrowed from the live snapshot.
pub(crate) fn with_now<T>(live: &Value, body: impl FnOnce(&BotNow) -> T) -> T {
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
    let demand = crate::market_demand::kind_demand();
    let others = other_lives(&life.account);
    let mut bots = vec![(fleet_bot(life, account_levels(&life.account, Some(live))), Some(live.clone()))];
    for other in &others {
        let snapshot = live_of(orch, &other.account);
        bots.push((fleet_bot(other, account_levels(&other.account, snapshot.as_ref())), snapshot));
    }
    let fleet: Vec<FleetBot> = bots.iter().map(|(bot, _)| bot.clone()).collect();
    let stamp = stamp_now();
    let inputs = DrawInputs::from_demand(demand, Vec::new(), Vec::new());
    let mut mine = false;
    let assignments = crate::brain_value::with_context(&inputs.value, &inputs.surplus, |context| lifescape_planner::assign_by_value(&fleet, &inputs.weights, context));
    for assignment in assignments {
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

/// Accounts of the running fleet (jobs minus staff and personal ones).
pub(crate) fn fleet_names() -> std::collections::BTreeSet<String> {
    fleet_accounts().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
}

/// Every fleet bot as the planner sees it, this life first (levels from
/// the live snapshots, else the engine saves), with the snapshots.
pub(crate) fn fleet_bots(orch: &Orchestrator, life: &Life, live: &Value) -> (Vec<FleetBot>, Vec<Option<Value>>) {
    let mut bots = vec![fleet_bot(life, account_levels(&life.account, Some(live)))];
    let mut snapshots = vec![Some(live.clone())];
    for other in &other_lives(&life.account) {
        let snapshot = live_of(orch, &other.account);
        bots.push(fleet_bot(other, account_levels(&other.account, snapshot.as_ref())));
        snapshots.push(snapshot);
    }
    (bots, snapshots)
}

/// The one-time A4 rebalance gives this life `kind` (`brain_rebalance`,
/// the only exception to A2), built like any redraw over the fleet's
/// demand; the figures that chose it ride in the log.
pub(crate) fn assign_rebalanced(life: &mut Life, live: &Value, kind: &str, why: &str, numbers: &str) {
    let (gaps, _, held) = fleet_view(life);
    let demand = need_demand(&life.account, &held);
    let bot = fleet_bot(life, account_levels(&life.account, Some(live)));
    let inputs = DrawInputs::from_demand(demand, gaps, held);
    crate::logln!("brain {}: A4 rebalance gives {kind} ({numbers})", life.account);
    replace(life, live, &bot, (kind.to_string(), why.to_string()), &inputs, &[], "rebalance");
}

/// Redraw this life's ambition now (blocked, achieved past the throttle):
/// the fleet's open gaps first, never back to `exclude`.
fn redraw(life: &mut Life, live: &Value, exclude: &[&str], revision_why: &str) -> bool {
    let (gaps, _, held) = fleet_view(life);
    let demand = need_demand(&life.account, &held);
    let bot = fleet_bot(life, account_levels(&life.account, Some(live)));
    let first = match crate::brain_value::draw(&life.account, &bot, &gaps, &demand.weights, &demand.value, &demand.surplus, &held, exclude) {
        Ok(first) => first,
        Err(error) => {
            crate::logln!("brain {}: {error}, the current ambition is kept", life.account);
            return false;
        }
    };
    let inputs = DrawInputs::from_demand(demand, gaps, held);
    replace(life, live, &bot, first, &inputs, exclude, revision_why)
}

/// `ambition <compte> natural` (operator): the pin and operator type are
/// already gone; the kind comes from the A4 value (`brain_value::draw`,
/// the tenants of the other bots sharing the demand), nothing excluded.
pub(crate) fn assign_natural(life: &mut Life, live: &Value) -> bool {
    redraw(life, live, &[], "operator:natural")
}

/// Redraw by need (D3 review, need-led ladder end), nothing excluded: the
/// sole holder of its primary group keeps it (`redraw_gaps`: its group is
/// its only gap), a redundant bot fills the fleet's open gaps first.
fn redraw_by_need(life: &mut Life, live: &Value, revision_why: &str) {
    let (_, _, held) = fleet_view(life);
    let kind = life.ambition.as_ref().map(|ambition| ambition.kind.clone()).unwrap_or_default();
    let others: Vec<&str> = held.iter().map(String::as_str).collect();
    let gaps = lifescape_planner::redraw_gaps(&kind, &others);
    let demand = need_demand(&life.account, &held);
    let bot = fleet_bot(life, account_levels(&life.account, Some(live)));
    let first = match crate::brain_value::draw(&life.account, &bot, &gaps, &demand.weights, &demand.value, &demand.surplus, &held, &[]) {
        Ok(first) => first,
        Err(error) => {
            crate::logln!("brain {}: {error}, the current ambition is kept", life.account);
            return;
        }
    };
    let inputs = DrawInputs::from_demand(demand, gaps, held);
    replace(life, live, &bot, first, &inputs, &[], revision_why);
}

/// A retired market maker's trade (user 2026-11-20): the neediest kind
/// of the fleet (`neediest_kind`, the need saturated by the kinds the
/// other bots hold), built like any redraw; nothing drawable, the usual
/// need draw.
pub(crate) fn redraw_neediest(life: &mut Life, live: &Value, revision_why: &str) {
    let (gaps, _, held) = fleet_view(life);
    let demand = need_demand(&life.account, &held);
    let bot = fleet_bot(life, account_levels(&life.account, Some(live)));
    let first = neediest_kind(&bot, &demand.weights, &[]).unwrap_or_else(|| draw_kind(&bot, &gaps, &demand.weights, &[]));
    let inputs = DrawInputs::from_demand(demand, gaps, held);
    replace(life, live, &bot, first, &inputs, &[], revision_why);
}

/// An operator's trade for this life (`brain_operator`): `kind` built
/// like a redraw over the fleet's demand and surplus.
pub(crate) fn assign_kind(life: &mut Life, live: &Value, kind: &str, revision_why: &str) -> bool {
    let (gaps, _, held) = fleet_view(life);
    let demand = need_demand(&life.account, &held);
    let bot = fleet_bot(life, account_levels(&life.account, Some(live)));
    let inputs = DrawInputs::from_demand(demand, gaps, held);
    replace(life, live, &bot, (kind.to_string(), revision_why.to_string()), &inputs, &[], revision_why)
}

/// True when the ambition was replaced; false (nothing written, logged)
/// when none could be built or the change is refused (`revise`).
fn replace(life: &mut Life, live: &Value, bot: &FleetBot, first: (String, String), inputs: &DrawInputs, exclude: &[&str], revision_why: &str) -> bool {
    let stamp = stamp_now();
    let built = with_now(live, |now| build(&life.account, bot, now, first, inputs, exclude, stamp));
    let Some((next, _)) = built else {
        crate::logln!("brain {}: no ambition buildable, kept the current one", life.account);
        return false;
    };
    let from = life.ambition.as_ref().map(|ambition| ambition.kind.clone()).unwrap_or_default();
    let line = format!("ambition {from} -> {} [{revision_why}]", describe_ambition(&next));
    if !revise(life, next, revision_why, stamp) {
        crate::logln!("brain {}: change refused, kept the current one ({line})", life.account);
        return false;
    }
    crate::logln!("brain {}: {line}", life.account);
    true
}

/// Before a draw: milestones read live levels, coins and holdings, open
/// bids that pay may add a demand milestone. An achieved
/// ambition is redrawn only when the 72 000-tick throttle allows it and
/// another bot holds its group; otherwise its ladder extends (the sole
/// holder never leaves its group, the throttle is the user's rule).
pub(crate) fn before_draw(life: &mut Life, live: &Value) {
    // Protected accounts always hold the overall ambition (never a draw).
    if lifescape_planner::protected(&life.account) && life.ambition.as_ref().is_some_and(|held| held.kind != lifescape_planner::OVERALL) {
        crate::logln!("brain {}: protected account, ambition set to overall", life.account);
        assign_kind(life, live, lifescape_planner::OVERALL, lifescape_planner::PROTECTED_WHY);
        return;
    }
    let (items, surplus) = crate::market_demand::refresh_inputs_for(&life.account);
    let seed = life.seed;
    let account = life.account.clone();
    let Some(ambition) = life.ambition.as_mut() else { return };
    if let Some(pin) = release_pin(ambition, &|skill| crate::brain_succession::live_level(live, skill)) {
        crate::logln!("brain {account}: pin {} {} reached, ambition {} free to change", pin.skill, pin.level, ambition.kind);
    }
    let closed = with_now(live, |now| refresh_supplied(ambition, now, seed, (&items, &surplus)));
    for milestone_id in &closed {
        crate::logln!("brain {account}: milestone {milestone_id} done (live levels/coins)");
    }
    if achieved(ambition) {
        on_achieved(life, live);
        return;
    }
    // Set-aside milestones come back (blocks reset) when nothing else is
    // drawable and their cooldown lapsed: the ambition never changes
    // before it is achieved (A2). Meanwhile: the side goal.
    let cooled = lifescape_planner::cooldown::cooled_keys(&life.history, &ticks_since);
    if let Some(ambition) = life.ambition.as_mut() {
        let resumed = resume_set_aside(ambition, &cooled);
        if !resumed.is_empty() {
            crate::logln!("brain {account}: milestones {} set aside resumed (cooldown over, nothing else drawable); ambition {} kept", resumed.join(", "), ambition.kind);
        }
    }
}

fn on_achieved(life: &mut Life, live: &Value) {
    let Some(ambition) = life.ambition.as_ref() else { return };
    let kind = ambition.kind.clone();
    // D3 (user 2026-11-19): outside the exceptions and the operator pins,
    // a ladder end never extends the same ambition (woodcutting 99 drawn
    // past the need, « ladder done, extended (changed less than 72000
    // ticks ago) »): it redraws by need, which may give the same kind.
    if !keeps_ladder_end(ambition, &fleet_bot(life, Vec::new())) {
        crate::logln!("brain {}: ambition {kind} ladder done, redrawn by need (D3: no extension outside the exceptions)", life.account);
        redraw_by_need(life, live, "achieved");
        return;
    }
    let throttled = !lifescape_planner::may_change(ambition, &ticks_since);
    let (_, redundant, _) = fleet_view(life);
    // An operator pin never leaves its kind: it extends toward its level.
    let held = pinned(ambition);
    if redundant && !throttled && !held {
        redraw(life, live, &[kind.as_str()], "achieved");
        return;
    }
    let why = if held {
        "operator pin"
    } else if redundant {
        "changed less than 72000 ticks ago"
    } else {
        "sole holder of its group"
    };
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

/// How a goal ended, as its milestone sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GoalEnd {
    Done,
    Stuck,
    /// Stuck on a buy the market did not fill or refused
    /// (`lifescape_planner::market_shortfall`): never a milestone block
    /// (decision D3, user 2026-11-19).
    MarketShortfall,
    /// Stuck on a piece of gear nobody makes (`brain_gear::unmade_equip_piece`):
    /// an `equip` milestone waits, cooled, never a block (A3, user
    /// 2026-11-21). Carries the piece.
    UnmadeGear(String),
}

/// After a goal close: fold it into its milestone. Blocked
/// `MILESTONE_MAX_BLOCKS` times, the milestone is set aside and the
/// ambition kept (A2); a market shortfall or gear nobody makes only
/// reopens it, cooled (D3, A3); a done milestone changes nothing.
pub(crate) fn after_close(life: &mut Life, end: GoalEnd) {
    let outcome = match end {
        GoalEnd::MarketShortfall => {
            let kind = life.ambition.as_ref().map(|ambition| ambition.kind.clone()).unwrap_or_default();
            let outcome = on_goal_shortfall(life);
            if let CloseOutcome::Reopened(milestone_id) = &outcome {
                // Cooled, never relaunched every cycle (user 2026-11-19).
                let key = milestone_cool_key(&kind, milestone_id);
                crate::brain::push_history(life, COOL_NODE, &cool_entry(&key, MILESTONE_SHORTFALL_COOL_TICKS, "market shortfall"));
                crate::logln!("brain {}: milestone {milestone_id}: market shortfall, not a block; cooled {MILESTONE_SHORTFALL_COOL_TICKS} ticks", life.account);
            }
            outcome
        }
        GoalEnd::UnmadeGear(item) => {
            let kind = life.ambition.as_ref().map(|ambition| ambition.kind.clone()).unwrap_or_default();
            let outcome = on_goal_shortfall(life);
            if let CloseOutcome::Reopened(milestone_id) = &outcome {
                let key = milestone_cool_key(&kind, milestone_id);
                crate::brain::push_history(life, COOL_NODE, &cool_entry(&key, MILESTONE_SHORTFALL_COOL_TICKS, "gear nobody makes"));
                crate::logln!(
                    "brain {}: milestone {milestone_id} waits: {item} is neither held, nor on the market, nor made by anyone; not a block, cooled {MILESTONE_SHORTFALL_COOL_TICKS} ticks, the other milestones go on",
                    life.account
                );
            }
            outcome
        }
        GoalEnd::Done => on_goal_closed(life, false),
        GoalEnd::Stuck => on_goal_closed(life, true),
    };
    match outcome {
        CloseOutcome::Unlinked | CloseOutcome::Reopened(_) => {}
        CloseOutcome::Blocked { id, count } => {
            crate::logln!("brain {}: milestone {id} blocked ({count}/{MILESTONE_MAX_BLOCKS})", life.account);
        }
        CloseOutcome::SetAside { id, count } => {
            let kind = life.ambition.as_ref().map(|ambition| ambition.kind.clone()).unwrap_or_default();
            // Cooled for `AMBITION_CHANGE_TICKS`: resumed (blocks reset) only
            // once nothing else is drawable and the cooldown lapsed.
            let key = milestone_cool_key(&kind, &id);
            crate::brain::push_history(life, COOL_NODE, &cool_entry(&key, lifescape_planner::AMBITION_CHANGE_TICKS, "set aside"));
            crate::logln!("brain {}: milestone {id} blocked {count} times, set aside; ambition {kind} kept, next milestone", life.account);
        }
        CloseOutcome::Dropped { id, count } => {
            let kind = life.ambition.as_ref().map(|ambition| ambition.kind.clone()).unwrap_or_default();
            crate::logln!("brain {}: milestone {id} blocked {count} times, dropped; pinned ambition {kind} kept", life.account);
        }
        CloseOutcome::Done(id) => {
            crate::logln!("brain {}: milestone {id} done", life.account);
        }
    }
}

/// Catalogue sync: open ladder production milestones the catalogue no
/// longer offers leave the ambition (`drop_stale_products`); the goal one
/// of them was running closes done (no ban: the bot did nothing wrong)
/// and the next one is drawn from the revised ladder. True when the life
/// changed (save it).
pub(crate) fn sync_catalog(life: &mut Life, live: &Value) -> bool {
    let Some(ambition) = life.ambition.as_mut() else { return false };
    let mut dropped = lifescape_planner::drop_stale_products(ambition);
    if !dropped.is_empty() {
        crate::logln!("brain {}: milestones {} left the {} ladder (catalogue revised)", life.account, dropped.join(", "), ambition.kind);
    }
    // The supply rule applies to the persisted quotas too (user rule,
    // 2026-11-20): a tool nobody can get, an idle quota eating no pile
    // while one waits (`ambition_supply::revise_supplied`).
    let surplus = crate::market_demand::fleet_surplus();
    let running = life.goal_origin.clone().filter(|_| !lifescape_planner::goal_closed(&life.history));
    let seed = life.seed;
    let revised = with_now(live, |now| lifescape_planner::ambition_supply::revise_supplied(ambition, now, seed, &surplus, running.as_deref()));
    if !revised.is_empty() {
        crate::logln!(
            "brain {}: milestones {} re-aimed by the fleet's supply (piles to eat, tools someone can get): {}",
            life.account,
            revised.join(", "),
            describe_ambition(ambition)
        );
        dropped.extend(revised);
    }
    if dropped.is_empty() {
        return false;
    }
    let running = life.goal_origin.as_ref().is_some_and(|origin| dropped.contains(origin));
    if running && !lifescape_planner::goal_closed(&life.history) {
        match crate::brain_succession::succeed_goal(life, live, false) {
            Some(label) => {
                crate::logln!("brain {}: goal of a dropped milestone closed, next goal {label}", life.account);
                lifescape_planner::normalize(life);
            }
            None => crate::logln!("brain {}: goal of a dropped milestone closed, nothing drawable yet", life.account),
        }
    }
    true
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

/// Coverage log, every `COVERAGE_EVERY_SECS` per account on the launch
/// pass: the fleet's uncovered groups are logged once per change (a
/// closed bot, a holder that moved on). A running ambition is never
/// retired to fill a gap (A2, user 2026-11-21): the gap serves the draw of
/// the next achieved ambition only.
pub(crate) fn coverage_pass(life: &Life) {
    let wall = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs_f64()).unwrap_or(0.0);
    {
        let mut table = last_coverage().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if table.get(&life.account).is_some_and(|last| wall - last < COVERAGE_EVERY_SECS) {
            return;
        }
        table.insert(life.account.clone(), wall);
    }
    if life.ambition.is_none() {
        return;
    }
    let (gaps, _, _) = fleet_view(life);
    let mut logged = last_gaps().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if *logged != gaps {
        if gaps.is_empty() {
            crate::logln!("brain: fleet coverage complete (every F2P group held)");
        } else {
            crate::logln!("brain: fleet coverage gap: {} held by no running bot; filled by the next ambition drawn (no running ambition is retired)", gaps.join(", "));
        }
        *logged = gaps;
    }
}

/// One-time D3 mark (user 2026-11-19, then A2 of 2026-11-21): an
/// ambition a need-led bot holds without the `need` mark and drawn by the
/// need before the mark is only marked. It is never retired: an ambition
/// changes only once achieved (the old-rule redraw at the daemon start is
/// gone). True when the life changed (save it).
pub(crate) fn review_pass(life: &mut Life) -> bool {
    review_at(life)
}

fn review_at(life: &mut Life) -> bool {
    let account = life.account.clone();
    let Some(ambition) = life.ambition.as_mut() else { return false };
    if lifescape_planner::adopt_need_mark(ambition) {
        crate::logln!("brain {account}: ambition {} ({}) drawn by need before the mark, marked need (D3)", ambition.kind, ambition.why);
        return true;
    }
    false
}

/// A changed ambition replaces the goal an earlier one drew (D3,
/// mila_wade and nina_rhodes 2026-11-19: woodcutter -> fisher at 18:24,
/// « woodcutting 95 » still cut 2.5 h later): its nodes are abandoned
/// (`abandon_goal_work`: the running node, market orders, gear steps and
/// the janitor stay), the goal closes like a catalogue-dropped one
/// (`sync_catalog`: done, no ban) and the next goal is drawn from the new
/// ambition. Waits while a node runs (its report decides first); a side,
/// bid or alchemy goal runs on. True when the life changed (save it).
pub(crate) fn replace_stale_goal(life: &mut Life, live: &Value) -> bool {
    if life.active.is_some() || !lifescape_planner::goal_from_earlier_ambition(life) {
        return false;
    }
    let old = life.history.iter().rev().find(|entry| entry.node == "goal" && entry.outcome.starts_with(lifescape_planner::ambition_review::NEXT_TAG)).map(|entry| entry.outcome.clone()).unwrap_or_default();
    let kind = life.ambition.as_ref().map(|ambition| ambition.kind.clone()).unwrap_or_default();
    let abandoned = lifescape_planner::abandon_goal_work(life, STALE_GOAL_CAUSE);
    lifescape_planner::normalize(life);
    crate::logln!(
        "brain {}: goal ({old}) drawn by an earlier ambition, replaced for the {kind} ambition; {} nodes abandoned: {}",
        life.account,
        abandoned.len(),
        abandoned.join(",")
    );
    match crate::brain_succession::succeed_goal(life, live, false) {
        Some(label) => crate::logln!("brain {}: next goal {label} ({kind})", life.account),
        None => crate::logln!("brain {}: nothing drawable for the {kind} ambition yet (see the reason above)", life.account),
    }
    lifescape_planner::normalize(life);
    true
}

#[cfg(test)]
#[path = "brain_ambition_review_tests.rs"]
mod review_tests;

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
