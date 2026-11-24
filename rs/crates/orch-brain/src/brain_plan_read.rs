#![forbid(unsafe_code)]
//! The planner's readers on the brain side (plan rendements R7,
//! decisions 2026-11-17 (b) and (c), `lifescape_planner::plan_read`):
//! - the MEASURED rate of a method at a level (`measured_rate`): the
//!   estimator's read of the published yields (`rates_read`: n >= 3
//!   blended with the game prior, else the prior), as a planner
//!   `WorkRate`: its flows (debug names, the method's own objects) and the
//!   XP derived from its units; the actions per 6000 are the prior's
//!   scaled by the measured XP over the prior's (an action pays the XP
//!   the game gives it, the measure says how many the bot makes), a read
//!   on the prior alone is the prior itself (`work_rate::prior_rate`);
//! - `guide_trains`: a level tranche the training guide will choose at
//!   resolve (`brain_train::pick_from_guide` finds a line within the
//!   bot's reach at the tranche's start level, in the last published set):
//!   the skeleton files nothing of the static method ahead.
//!
//! `with_plan_read` builds both for one bot (its live levels, quests,
//! stock, wallet) and hands them to the planner's `*_with` functions
//! (`append_goal_with`, `record_block_with`); offline, nothing published
//! or no snapshot: the prior and no guide (`PlanRead::prior`).

use lifescape_json::Value;
use lifescape_planner::plan_read::PlanRead;
use lifescape_planner::work_rate::{prior_rate, style_of, WorkRate};
use lifescape_planner::{Life, Profile};

/// The measured rate of `method` at `level` in `book` (module doc);
/// `None` when neither a measure nor a prior exists.
pub(crate) fn measured_rate(book: &crate::rates_estimator::YieldBook, method: &str, level: u8, now: i64) -> Option<WorkRate> {
    let prior = prior_rate(method, level);
    let query = crate::rates_read::RateQuery { method: method.to_string(), style: style_of(method).to_string(), site: None, tool: String::new(), level: level.max(1) };
    let estimate = crate::rates_read::estimate_quiet(book, &query, now, &crate::rates_read::cached_prior).ok()?;
    if estimate.prior {
        return prior;
    }
    let flows_6000: Vec<(String, String, f64)> = estimate.flows.iter().map(|(item, cause, qty)| (crate::rates_names::debug_in(method, item, *qty < 0.0), cause.clone(), *qty)).collect();
    let xp_6000 = lifescape_planner::unit_xp::derived_xp(method, style_of(method), &crate::rates_names::units_in_debug(&estimate.units), level.max(1));
    let measured_xp: f64 = xp_6000.iter().map(|(_, xp)| *xp).sum();
    let actions_6000 = match &prior {
        Some(base) => {
            let prior_xp: f64 = base.xp_6000.iter().map(|(_, xp)| *xp).sum();
            if prior_xp > 0.0 && measured_xp > 0.0 {
                base.actions_6000 * measured_xp / prior_xp
            } else {
                base.actions_6000
            }
        }
        None => 0.0,
    };
    Some(WorkRate { actions_6000, xp_6000, flows_6000 })
}

/// The measured reader of the live snapshot (module doc).
pub(crate) fn live_rate(deps: &crate::brain_deps::BrainDeps) -> impl Fn(&str, u8) -> Option<WorkRate> {
    let snapshot = deps.rates.clone();
    let now = crate::rates_service::now_secs();
    move |method: &str, level: u8| measured_rate(snapshot.book(), method, level, now)
}

/// What the readers know of one bot (copied out of its life: the body
/// borrows the life mutably).
pub(crate) struct BotView {
    pub account: String,
    pub profile: Profile,
    pub quests: Vec<String>,
}

impl BotView {
    pub(crate) fn of(life: &Life) -> BotView {
        BotView { account: life.account.clone(), profile: life.profile.clone(), quests: crate::brain_rescue::quests_done(life) }
    }
}

/// Runs `body` with the planner's readers for one bot (module doc): the
/// measured rates, and the training guide of the last published set
/// judged on the bot's live state (`live`: levels, stock, wallet).
pub(crate) fn with_plan_read<T>(bot: &BotView, live: &Value, body: impl FnOnce(&PlanRead) -> T, deps: &crate::brain_deps::BrainDeps) -> T {
    let rate = live_rate(deps);
    let set = deps.guides.clone();
    let levels = crate::brain_market::live_levels(live);
    let combat = crate::brain_plan::live_combat(live);
    let coins = crate::brain_ambition::live_count(live, "coins").max(0) as f64;
    let locks = crate::brain_locks::locked_items_in(deps.market.as_deref(), &bot.account);
    let held = |item: &str| crate::brain_locks::free_count(crate::brain_ambition::live_count(live, item), crate::brain_locks::locked_in(&locks, item)) as f64;
    let trains = |skill: &str, from: u8, to: u8| {
        let Some(set) = set.as_ref() else { return false };
        let level_of = |name: &str| if name == skill { from } else { levels.iter().find(|(known, _)| known == name).map(|(_, level)| *level).unwrap_or(1) };
        let asks = |item: &str| crate::brain_trade::cycle_asks(set, item);
        let judged = lifescape_planner::guide_feasible::BotState { level_of: &level_of, combat, quests_done: &bot.quests, held: &held, coins: Some(coins), ironman: bot.profile.ironman, asks: &asks };
        crate::brain_train::guide_covers(set, skill, from, to, &bot.profile, &judged)
    };
    body(&PlanRead { rate: &rate, guide_trains: &trains })
}

/// One work session at a time: the open input obtains of the bot's climbs
/// come down to what one session burns (`planner::session_resize`, the
/// rule applied to the persisted state). Returns true when a node changed.
pub(crate) fn resize_session_inputs(life: &mut Life, live: &Value, deps: &crate::brain_deps::BrainDeps) -> bool {
    let _phase = crate::cpu_profile::phase("resize_session_inputs");
    // Fast exit (lot 3.4): no open climb with an open input obtain, nothing
    // to resize: the readers below are not built (the same `false`).
    if !lifescape_planner::session_resize::has_open_climb_inputs(life) {
        return false;
    }
    resize_session_inputs_full(life, live, deps)
}

/// The resize with its readers built, whatever the graph holds (the
/// reference of the fast exit).
pub(crate) fn resize_session_inputs_full(life: &mut Life, live: &Value, deps: &crate::brain_deps::BrainDeps) -> bool {
    let view = BotView::of(life);
    let level_of = |skill: &str| crate::brain_succession::live_level(live, skill);
    let lines = with_plan_read(&view, live, |read| lifescape_planner::session_resize::resize_climb_inputs(life, &level_of, read.rate), deps);
    for line in &lines {
        crate::logln!("brain {}: {line}", life.account);
    }
    !lines.is_empty()
}

#[cfg(test)]
mod tests {
    use super::{measured_rate, resize_session_inputs, resize_session_inputs_full};

    /// The fast exit equals the full resize: same answer, same life, on every
    /// fixture life with its graph put in states that do and do not hold an
    /// open climb input (statuses, methods, dependencies).
    #[test]
    fn the_fast_exit_equals_the_full_resize() {
        let deps = &crate::brain_deps::BrainDeps::live();
        use lifescape_planner::{NodeKind, NodeStatus};
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lives");
        let lives = crate::brain_launch_equiv_tests::load_dir(&dir);
        let live = lifescape_json::object(vec![("skills", lifescape_json::Value::Arr((0u8..23).map(|stat| lifescape_json::object(vec![("id", lifescape_json::Value::Int(i64::from(stat))), ("xp", lifescape_json::Value::Int(0)), ("level", lifescape_json::Value::Int(30))])).collect()))]);
        let (mut fast_exits, mut resized, mut compared) = (0, 0, 0);
        for base in &lives {
            for variant in 0..64u32 {
                let mut life = base.clone();
                // A smith's climb with the open bar obtain it feeds (a third of the graphs).
                if variant % 3 == 0 {
                    let mut climb = lifescape_planner::Node::new("n9000", NodeKind::ReachLevel { skill: "smithing".to_string(), level: 70 }, vec!["n9001".to_string()], 70);
                    climb.method = Some("smith:steel_med_helm".to_string());
                    life.nodes.push(climb);
                    life.nodes.push(lifescape_planner::Node::new("n9001", NodeKind::ObtainItems { item: "steel_bar".to_string(), count: 12_000 }, Vec::new(), 12_000));
                }
                // Put the climbs and their input obtains in states the exit tells apart.
                for (index, node) in life.nodes.iter_mut().enumerate() {
                    let bit = (variant >> (index % 6)) & 1;
                    match &node.kind {
                        NodeKind::ReachLevel { .. } if bit == 1 => node.status = NodeStatus::Abandoned,
                        NodeKind::ObtainItems { .. } if (variant >> 3) & 1 == 1 => node.status = NodeStatus::Done,
                        NodeKind::ReachLevel { .. } if variant % 5 == 0 => node.method = None,
                        _ => {}
                    }
                }
                let exits = !lifescape_planner::session_resize::has_open_climb_inputs(&life);
                let (mut fast, mut full) = (life.clone(), life.clone());
                let (fast_answer, full_answer) = (resize_session_inputs(&mut fast, &live, deps), resize_session_inputs_full(&mut full, &live, deps));
                assert_eq!(fast_answer, full_answer, "{} variant {variant}", base.account);
                assert_eq!(fast, full, "{} variant {variant}: life", base.account);
                if exits {
                    assert!(!full_answer && full == life, "{} variant {variant}: the exit left something to resize", base.account);
                    fast_exits += 1;
                }
                resized += usize::from(full_answer);
                compared += 1;
            }
        }
        eprintln!("fast exit: {compared} graphs compared identical, {fast_exits} fast exits, {resized} resized");
        assert!(fast_exits > 0 && resized > 0, "the proof misses a side: {fast_exits} exits, {resized} resizes");
    }

    /// No run measured: the read is the game prior itself (the planner's
    /// numbers unchanged until three runs of a key exist).
    #[test]
    fn an_unmeasured_method_reads_the_prior() {
        let book = crate::rates_estimator::YieldBook::default();
        for (method, level) in [("gather:fishing/raw_trout", 20u8), ("cook/raw_shrimp", 5), ("smelt:iron_bar", 15)] {
            assert_eq!(measured_rate(&book, method, level, 0), lifescape_planner::work_rate::prior_rate(method, level), "{method}");
        }
    }

    /// Three runs of shrimps at twice the prior's catch: the measured read
    /// (blended with the prior's window) catches more per 6000 ticks,
    /// pays more XP, and makes more actions at the game's XP per action.
    #[test]
    fn a_measured_method_reads_its_runs() {
        let method = "gather:fishing/raw_shrimp";
        let prior = lifescape_planner::work_rate::prior_rate(method, 3).expect("prior");
        let caught = prior.flows_6000.iter().filter(|(_, cause, qty)| cause == "product" && *qty > 0.0).map(|(_, _, qty)| *qty).sum::<f64>();
        let query = crate::rates_read::RateQuery { method: method.to_string(), style: String::new(), site: None, tool: String::new(), level: 3 };
        let key = crate::rates_read::key_of_query(&query).any_site_tool();
        let mut book = crate::rates_estimator::YieldBook::default();
        let catch = (caught * 2.0).round() as i64;
        for _ in 0..3 {
            let sample = crate::rates_estimator::RunSample { work_ticks: 6000, pause_ticks: 0, flows: vec![("Raw shrimps".to_string(), "product".to_string(), catch)], units: vec![("product".to_string(), "Raw shrimps".to_string(), catch)], clock: 100 };
            book.record(&key, &sample);
        }
        let read = measured_rate(&book, method, 3, 100).expect("measured");
        let shrimps: f64 = read.flows_6000.iter().filter(|(item, cause, _)| item == "raw_shrimp" && cause == "product").map(|(_, _, qty)| *qty).sum();
        assert!(shrimps > caught * 1.3, "measured {shrimps} vs prior {caught}");
        assert!(read.xp_of("fishing") > prior.xp_of("fishing") * 1.3);
        let per_action = |rate: &lifescape_planner::work_rate::WorkRate| rate.xp_per_action("fishing").unwrap_or(0.0);
        assert!((per_action(&read) - per_action(&prior)).abs() < 1e-6, "an action pays the game's XP");
    }
}
