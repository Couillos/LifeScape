//! Cycle: task execution contract v2. `TaskSpec` is the full brain order
//! (activity, resource, targets, tick budget, keep-list, flags, node id);
//! `run` drives one activity until its stop conditions and reports the
//! stop as an exit code the session maps to a `TaskReport`. The old
//! per-script CLIs (`run woodcut 3 toll wild`) keep working through the
//! same parser (D25): positionals fill the matching targets, new flags
//! fill the rest.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use lifescape_sdk::Bot;

use crate::ScriptError;

/// Exit code for an engagement that ran out of tick budget: the session
/// maps it to `stop: budget_exhausted`. `Ok(0)` stays the goal code (and
/// the legacy success code), any other nonzero stays a failure.
pub const BUDGET_EXIT: i32 = 2;
/// A budget overrun waits for a safe point, but not forever: past this
/// many ticks over budget the run ends wherever it stands.
pub const SAFE_DEFER_TICKS: u64 = 500;

/// Stop conditions of one task: XP, level, quantity or quest. Quantity
/// units are script-defined (loads for gatherers, kills for melee); quest
/// completion is signalled by the step itself (`StepControl::GoalReached`),
/// the poll only watches numbers.
#[derive(Debug, Clone, Default)]
pub struct TaskTargets {
    /// Skill XP target: (`skill`, `amount`).
    pub xp_skill: Option<String>,
    pub xp_amount: Option<u64>,
    /// Level target: (`skill`, `level`).
    pub level_skill: Option<String>,
    pub level: Option<u8>,
    /// Quantity target (loads, kills, items).
    pub count: Option<u32>,
    /// Quest key, completed when the step says so.
    pub quest: Option<String>,
}

/// Full task specification: what the brain orders, the kit executes.
#[derive(Debug, Clone)]
pub struct TaskSpec {
    /// Activity name, as in the registry (`woodcut`, `mining`…).
    pub activity: &'static str,
    /// Explicit resource (melee spots; empty when the skill picks its
    /// own tier by level, as the brain fixes it).
    pub resource: Vec<String>,
    /// Stop conditions (XP, level, quantity, quest).
    pub targets: TaskTargets,
    /// Engagement tick budget: the run ends at `max` (goal ends it
    /// sooner); `min` bounds external replanning and wins if above max.
    pub budget_min_ticks: u64,
    pub budget_max_ticks: u64,
    /// Item names the run must never deposit or sell. Honored by the
    /// gather bank trips; combat gearing keeps its own set (gear+food)
    /// until the equip task wires it through.
    pub keep: Vec<String>,
    /// Market runs allowed (no market service yet; recorded for later).
    pub allow_market: bool,
    /// NPC shop purchases allowed: cleared, the buy paths refuse instead
    /// of spending and the run reports blocked, never broke.
    pub allow_shop: bool,
    /// Graph node id, for the session report.
    pub node_id: String,
    /// Site the brain picked (`--site x,z,level`): pickers stay there.
    pub site: Option<(i32, i32, i32)>,
    /// Production chain (PRD 2, D58): `None` for single-link v2 orders.
    pub chain: Option<super::chain_spec::ChainSpec>,
    /// Skip the bank loop (melee `--no-bank`).
    pub no_bank: bool,
    /// Paid crossings allowed (`toll`).
    pub allow_paid: bool,
    /// Hunter zones allowed (`wild`, woodcut only today).
    pub allow_wild: bool,
}

impl TaskSpec {
    pub fn named(activity: &'static str) -> TaskSpec {
        TaskSpec {
            activity,
            resource: Vec::new(),
            targets: TaskTargets::default(),
            budget_min_ticks: 0,
            budget_max_ticks: 0,
            keep: Vec::new(),
            allow_market: false,
            allow_shop: true,
            node_id: String::new(),
            site: None,
            chain: None,
            no_bank: false,
            allow_paid: false,
            allow_wild: false,
        }
    }
}

/// Parse the legacy CLI plus the task flags into a `TaskSpec`. Legacy
/// positionals keep their meaning (`woodcut 3` = 3 loads, melee spots);
/// `--kills N` is the quantity target, `--ticks N` the max budget.
pub fn parse_cli(activity: &'static str, args: &[String]) -> Result<TaskSpec, ScriptError> {
    let usage = || {
        ScriptError::Usage(format!(
            "{activity} [count|spot...] [toll] [wild] [--kills N] [--ticks N] [--no-bank] [--xp SKILL N] [--level SKILL N] [--quest KEY] [--budget-min N] [--budget-max N] [--keep NAME]... [--node ID] [--site X,Z,LEVEL] [--allow-market] [--no-shop]"
        ))
    };
    let mut spec = TaskSpec::named(activity);
    let mut index = 0;
    let mut flag_count: Option<u32> = None;
    let mut flag_ticks: Option<u64> = None;
    while index < args.len() {
        match args[index].as_str() {
            "toll" => {
                spec.allow_paid = true;
                index += 1;
            }
            "wild" => {
                spec.allow_wild = true;
                index += 1;
            }
            "--no-bank" => {
                spec.no_bank = true;
                index += 1;
            }
            "--allow-market" => {
                spec.allow_market = true;
                index += 1;
            }
            "--no-shop" => {
                spec.allow_shop = false;
                index += 1;
            }
            "--kills" => {
                flag_count = Some(
                    args.get(index + 1)
                        .and_then(|arg| arg.parse::<u32>().ok())
                        .ok_or_else(usage)?,
                );
                index += 2;
            }
            "--ticks" => {
                flag_ticks = Some(
                    args.get(index + 1)
                        .and_then(|arg| arg.parse::<u64>().ok())
                        .ok_or_else(usage)?,
                );
                index += 2;
            }
            "--xp" => {
                let skill = args.get(index + 1).cloned().ok_or_else(usage)?;
                let amount = args
                    .get(index + 2)
                    .and_then(|arg| arg.parse::<u64>().ok())
                    .ok_or_else(usage)?;
                if lifescape_sdk::provenance::skill_id(&skill.to_lowercase()).is_none() {
                    return Err(usage());
                }
                spec.targets.xp_skill = Some(skill.to_lowercase());
                spec.targets.xp_amount = Some(amount);
                index += 3;
            }
            "--level" => {
                let skill = args.get(index + 1).cloned().ok_or_else(usage)?;
                let level = args
                    .get(index + 2)
                    .and_then(|arg| arg.parse::<u8>().ok())
                    .filter(|level| *level >= 2)
                    .ok_or_else(usage)?;
                if lifescape_sdk::provenance::skill_id(&skill.to_lowercase()).is_none() {
                    return Err(usage());
                }
                spec.targets.level_skill = Some(skill.to_lowercase());
                spec.targets.level = Some(level);
                index += 3;
            }
            "--quest" => {
                let key = args.get(index + 1).cloned().filter(|key| !key.is_empty()).ok_or_else(usage)?;
                spec.targets.quest = Some(key);
                index += 2;
            }
            "--budget-min" => {
                spec.budget_min_ticks =
                    args.get(index + 1).and_then(|arg| arg.parse::<u64>().ok()).ok_or_else(usage)?;
                index += 2;
            }
            "--budget-max" => {
                spec.budget_max_ticks =
                    args.get(index + 1).and_then(|arg| arg.parse::<u64>().ok()).ok_or_else(usage)?;
                index += 2;
            }
            "--keep" => {
                let name = args.get(index + 1).cloned().filter(|name| !name.is_empty()).ok_or_else(usage)?;
                spec.keep.push(name);
                index += 2;
            }
            "--site" => {
                let raw = args.get(index + 1).ok_or_else(usage)?;
                let parts: Vec<i32> = raw.split(',').map(|part| part.trim().parse::<i32>()).collect::<Result<_, _>>().map_err(|_| usage())?;
                let [x, z, level] = parts[..] else { return Err(usage()) };
                spec.site = Some((x, z, level));
                index += 2;
            }
            "--node" => {
                let node = args.get(index + 1).cloned().filter(|node| !node.is_empty()).ok_or_else(usage)?;
                spec.node_id = node;
                index += 2;
            }
            other if other.starts_with("--") => return Err(usage()),
            other => {
                // Combat spots ride positionally for every style: the
                // brain sends `ranged cows` / `magic chickens` like
                // `melee cows` (PRD3 lot 6: both failed Usage before).
                if matches!(activity, "melee" | "ranged" | "magic") && other.parse::<u32>().is_err()
                    || activity == "melee"
                    || (activity == "cook" && spec.resource.is_empty() && other.parse::<u32>().is_err())
                {
                    spec.resource.push(other.to_string());
                } else if spec.targets.count.is_none() {
                    spec.targets.count = Some(other.parse().map_err(|_| usage())?);
                } else {
                    return Err(usage());
                }
                index += 1;
            }
        }
    }
    if let Some(count) = flag_count {
        spec.targets.count = Some(count);
    }
    if let Some(ticks) = flag_ticks {
        spec.budget_max_ticks = ticks;
    }
    Ok(spec)
}

/// Resolve the keep-list names to object ids. Unknown names fail loud:
/// silently dropping a keep would sell what the brain forbade.
pub fn resolve_keep(bot: &Bot, spec: &TaskSpec) -> Result<Vec<u16>, ScriptError> {
    let mut ids = Vec::new();
    for name in &spec.keep {
        let id = crate::kit::pack::obj_id(bot, name)?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
}

/// What one unit of work reported back to the driver.
pub enum StepControl {
    /// More of the same (a retried load, a re-pick).
    Continue,
    /// One countable unit done (a banked load, a finished spot).
    UnitDone,
    /// The task goal is met (quest done); ends the run now.
    GoalReached,
}

/// Why the driver stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    GoalReached,
    BudgetExhausted,
    Blocked { missing: Vec<String> },
}

/// Map a stop to the script exit the session reads: goal stays `Ok(0)`
/// (the legacy success code), budget is `Ok(BUDGET_EXIT)`, blocked is
/// the typed error the session maps to `stop: blocked`.
pub fn exit_code(stop: StopReason) -> crate::ScriptResult {
    match stop {
        StopReason::GoalReached => Ok(0),
        StopReason::BudgetExhausted => Ok(BUDGET_EXIT),
        StopReason::Blocked { missing } => Err(ScriptError::Blocked { missing }),
    }
}

/// Run-local clocks: tick and pause counters at entry, XP snapshot, units
/// done. Work ticks are wall ticks minus market-pause ticks, so pauses
/// never eat the engagement budget.
pub struct CycleState {
    start_tick: u64,
    start_pause: u64,
    start_xp: HashMap<u8, (u8, i32)>,
    units: u32,
}

impl CycleState {
    pub fn start(bot: &Bot) -> CycleState {
        CycleState {
            start_tick: bot.world.ticks,
            start_pause: bot.pause_ticks,
            start_xp: bot.world.stats.clone(),
            units: 0,
        }
    }

    pub fn note_unit(&mut self) {
        self.units = self.units.saturating_add(1);
    }

    pub fn add_units(&mut self, count: u32) {
        self.units = self.units.saturating_add(count);
    }

    pub fn units(&self) -> u32 {
        self.units
    }

    /// Ticks of engagement so far: wall ticks minus market pauses.
    pub fn work_ticks(&self, bot: &Bot) -> u64 {
        let wall = bot.world.ticks.saturating_sub(self.start_tick);
        wall.saturating_sub(bot.pause_ticks.saturating_sub(self.start_pause))
    }

    fn xp_gain(&self, bot: &Bot, skill: &str) -> u64 {
        let Some(stat) = lifescape_sdk::provenance::skill_id(skill) else {
            return 0;
        };
        let before = self.start_xp.get(&stat).map(|(_, xp)| *xp).unwrap_or(0);
        let now = bot.world.stats.get(&stat).map(|(_, xp)| *xp).unwrap_or(before);
        now.saturating_sub(before).max(0) as u64
    }

    fn level_now(&self, bot: &Bot, skill: &str) -> u8 {
        let Some(stat) = lifescape_sdk::provenance::skill_id(skill) else {
            return 1;
        };
        bot.world.stats.get(&stat).map(|(level, _)| *level).unwrap_or(1)
    }
}

/// One stop check: goal first (targets), then budget (deferred to the
/// next safe point, forced past the defer cap). `min` wins over `max`
/// when inconsistent; `max == 0` means unbounded.
pub fn poll_stop(bot: &Bot, state: &CycleState, spec: &TaskSpec) -> Option<StopReason> {
    let elapsed = state.work_ticks(bot);
    if let Some(want) = spec.targets.count {
        if state.units >= want {
            return Some(StopReason::GoalReached);
        }
    }
    if let (Some(skill), Some(amount)) = (spec.targets.xp_skill.as_deref(), spec.targets.xp_amount) {
        if state.xp_gain(bot, skill) >= amount {
            return Some(StopReason::GoalReached);
        }
    }
    if let (Some(skill), Some(level)) = (spec.targets.level_skill.as_deref(), spec.targets.level) {
        if state.level_now(bot, skill) >= level {
            return Some(StopReason::GoalReached);
        }
    }
    let budget = spec.budget_max_ticks.max(spec.budget_min_ticks);
    if budget > 0 && elapsed >= budget {
        if crate::kit::survival::at_safe_point(bot) || elapsed >= budget + SAFE_DEFER_TICKS {
            return Some(StopReason::BudgetExhausted);
        }
    }
    None
}

/// Drive one activity until its stop conditions: checks the spec, runs
/// one unit, repeat. Goal and budget end the run with their exit codes;
/// every other error propagates (blocked included).
pub fn run(bot: &mut Bot, spec: &TaskSpec, mut step: impl FnMut(&mut Bot, &mut CycleState) -> Result<StepControl, ScriptError>) -> crate::ScriptResult {
    bot.assigned_site = spec.site;
    let mut state = CycleState::start(bot);
    loop {
        if let Some(stop) = poll_stop(bot, &state, spec) {
            return exit_code(stop);
        }
        match step(bot, &mut state)? {
            StepControl::Continue => {}
            StepControl::UnitDone => state.note_unit(),
            StepControl::GoalReached => return Ok(0),
        }
    }
}

#[cfg(test)]
mod spot_tests {
    #[test]
    fn combat_styles_take_spot_names() {
        for activity in ["melee", "ranged", "magic"] {
            let spec = super::parse_cli(activity, &["cows".to_string(), "--ticks".to_string(), "600".to_string()]).expect("spot parses");
            assert_eq!(spec.resource, vec!["cows".to_string()], "{activity}");
            assert_eq!(spec.budget_max_ticks, 600);
        }
        let counted = super::parse_cli("magic", &["25".to_string()]).expect("count parses");
        assert_eq!(counted.targets.count, Some(25));
    }
}

#[cfg(test)]
mod tests {
    use super::{exit_code, parse_cli, poll_stop, run, CycleState, StepControl, StopReason, BUDGET_EXIT};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    /// The brain's site rides as `--site x,z,level`; the driver hands it
    /// to the pickers, which keep only the resources around it.
    #[test]
    fn site_flag_parses_and_bounds_the_pickers() {
        let args: Vec<String> = ["--site", "3086,3227,0", "--node", "n1"].iter().map(|arg| arg.to_string()).collect();
        let spec = super::parse_cli("fishing", &args).expect("site parses");
        assert_eq!(spec.site, Some((3086, 3227, 0)));
        assert!(super::parse_cli("fishing", &["--site".to_string(), "3086,3227".to_string()]).is_err(), "three numbers");
        let mut bot = lifescape_sdk::Bot::new("127.0.0.1", 43594, ".", ".");
        bot.assigned_site = spec.site;
        assert!(crate::kit::spots::in_assigned_site(&bot, 3090, 3230), "inside the working area");
        assert!(!crate::kit::spots::in_assigned_site(&bot, 2996, 3158), "Mudskipper is another site");
        bot.assigned_site = None;
        assert!(crate::kit::spots::in_assigned_site(&bot, 2996, 3158), "manual runs choose freely");
    }

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Legacy CLIs parse exactly as before, through the same parser.
    #[test]
    fn legacy_clis_parse_to_the_same_targets() {
        let woodcut = parse_cli("woodcut", &["3".to_string(), "toll".to_string(), "wild".to_string()]).expect("woodcut parses");
        assert_eq!((woodcut.targets.count, woodcut.allow_paid, woodcut.allow_wild), (Some(3), true, true));
        assert!(woodcut.resource.is_empty(), "woodcut picks its tier by level");
        assert!(woodcut.allow_shop, "shops allowed unless --no-shop");
        let mining = parse_cli("mining", &["1".to_string()]).expect("mining parses");
        assert_eq!((mining.targets.count, mining.allow_paid), (Some(1), false));
        let melee = parse_cli(
            "melee",
            &["goblins".to_string(), "--kills".to_string(), "6".to_string(), "--no-bank".to_string()],
        )
        .expect("melee parses");
        assert_eq!(melee.resource, vec!["goblins".to_string()], "spot is the explicit resource");
        assert_eq!(melee.targets.count, Some(6));
        assert_eq!(melee.budget_max_ticks, 0);
        assert!(melee.no_bank, "no-bank kept");
        assert!(parse_cli("woodcut", &["x".to_string()]).is_err(), "junk count fails loud");
        assert!(parse_cli("melee", &["--kills".to_string()]).is_err(), "dangling flag fails loud");
    }

    /// Cook positionals: an optional resource name, then an optional
    /// count. A bare number stays a count (default raw); a name fills the
    /// resource the brain would otherwise fix.
    #[test]
    fn cook_positionals_fill_resource_then_count() {
        let both = parse_cli("cook", &["raw_shrimp".to_string(), "2".to_string()]).expect("cook parses");
        assert_eq!(both.resource, vec!["raw_shrimp".to_string()]);
        assert_eq!(both.targets.count, Some(2));
        let bare = parse_cli("cook", &["3".to_string()]).expect("bare count parses");
        assert!(bare.resource.is_empty(), "bare count keeps the default raw");
        assert_eq!(bare.targets.count, Some(3));
        let named = parse_cli("cook", &["raw_shrimp".to_string()]).expect("bare resource parses");
        assert_eq!(named.resource, vec!["raw_shrimp".to_string()]);
        assert_eq!(named.targets.count, None);
    }

    /// Task flags: XP/level/quest targets, tick budget, keep-list, node,
    /// market and shop flags. Unknown flags fail loud.
    #[test]
    fn task_flags_fill_the_spec() {
        let spec = parse_cli(
            "woodcut",
            &[
                "--xp".to_string(), "woodcutting".to_string(), "9000".to_string(),
                "--budget-min".to_string(), "6000".to_string(),
                "--budget-max".to_string(), "18000".to_string(),
                "--keep".to_string(), "coins".to_string(),
                "--node".to_string(), "n9".to_string(),
                "--allow-market".to_string(),
            ],
        )
        .expect("task flags parse");
        assert_eq!((spec.targets.xp_skill.as_deref(), spec.targets.xp_amount), (Some("woodcutting"), Some(9000)));
        assert_eq!((spec.budget_min_ticks, spec.budget_max_ticks), (6000, 18000));
        assert_eq!(spec.keep, vec!["coins".to_string()]);
        assert_eq!(spec.node_id, "n9");
        assert!(spec.allow_market, "market flag kept");
        let level = parse_cli("mining", &["--level".to_string(), "mining".to_string(), "30".to_string()]).expect("level parses");
        assert_eq!((level.targets.level_skill.as_deref(), level.targets.level), (Some("mining"), Some(30)));
        let quest = parse_cli("melee", &["--quest".to_string(), "cook".to_string()]).expect("quest parses");
        assert_eq!(quest.targets.quest.as_deref(), Some("cook"));
        let no_shop = parse_cli("woodcut", &["--no-shop".to_string()]).expect("no-shop parses");
        assert!(!no_shop.allow_shop, "shop buys refused");
        assert!(parse_cli("woodcut", &["--frobnicate".to_string()]).is_err(), "unknown flag fails loud");
        assert!(parse_cli("woodcut", &["--xp".to_string(), "nope".to_string(), "9".to_string()]).is_err(), "unknown skill fails loud");
        assert!(parse_cli("woodcut", &["--level".to_string(), "mining".to_string(), "1".to_string()]).is_err(), "level 1 is not a target");
    }

    /// Exit codes: goal is 0 (legacy success), budget is BUDGET_EXIT,
    /// blocked is the typed error.
    #[test]
    fn exit_codes_map_stops() {
        assert_eq!(exit_code(StopReason::GoalReached).unwrap(), 0);
        assert_eq!(exit_code(StopReason::BudgetExhausted).unwrap(), BUDGET_EXIT);
        assert!(matches!(
            exit_code(StopReason::Blocked { missing: vec!["bronze_axe".to_string()] }),
            Err(crate::ScriptError::Blocked { .. })
        ));
    }

    /// Count target: N units end the run with the goal code.
    #[test]
    fn run_counts_units_to_goal() {
        let mut bot = offline_bot();
        let spec = parse_cli("woodcut", &["3".to_string()]).expect("count parses");
        let mut calls = 0u32;
        let outcome = run(&mut bot, &spec, |_, _| {
            calls += 1;
            Ok(StepControl::UnitDone)
        });
        assert_eq!(outcome.unwrap(), 0);
        assert_eq!(calls, 3, "one unit per load, no more");
    }

    /// Explicit goal from the step ends the run at once (quest done).
    #[test]
    fn run_stops_on_step_goal() {
        let mut bot = offline_bot();
        let spec = parse_cli("woodcut", &["--quest".to_string(), "cook".to_string()]).expect("quest parses");
        let outcome = run(&mut bot, &spec, |_, _| Ok(StepControl::GoalReached));
        assert_eq!(outcome.unwrap(), 0);
    }

    /// Budget: over budget at a safe point ends with BUDGET_EXIT.
    #[test]
    fn run_ends_at_budget_on_safe_point() {
        let mut bot = offline_bot();
        let spec = parse_cli("woodcut", &["--budget-max".to_string(), "6000".to_string()]).expect("budget parses");
        let mut calls = 0u32;
        let outcome = run(&mut bot, &spec, |step_bot, _| {
            calls += 1;
            step_bot.world.ticks += 2000;
            Ok(StepControl::Continue)
        });
        assert_eq!(outcome.unwrap(), BUDGET_EXIT);
        assert_eq!(calls, 3, "two thousand ticks a unit, three units to the budget");
    }

    /// Budget overrun off the safe point (modal open) defers: the run
    /// keeps working until the point clears, then budgets out.
    #[test]
    fn budget_overrun_defers_off_the_safe_point() {
        let mut bot = offline_bot();
        bot.world.modal = Some(lifescape_sdk::world::Modal { layer: "chat".to_string(), interface_id: 1 });
        let spec = parse_cli("woodcut", &["--budget-max".to_string(), "6000".to_string()]).expect("budget parses");
        let mut calls = 0u32;
        let outcome = run(&mut bot, &spec, |step_bot, _| {
            calls += 1;
            step_bot.world.ticks += 2000;
            if calls >= 3 {
                step_bot.world.modal = None;
            }
            Ok(StepControl::Continue)
        });
        assert_eq!(outcome.unwrap(), BUDGET_EXIT);
        assert!(calls >= 3, "deferred past the unsafe pumps, got {calls}");
    }

    /// XP target: banked XP since run start ends the run (skill products
    /// land with XP, which is the produced signal).
    #[test]
    fn xp_target_fires_on_gain() {
        let mut bot = offline_bot();
        bot.world.stats.insert(8, (2, 100));
        let spec = parse_cli("woodcut", &["--xp".to_string(), "woodcutting".to_string(), "50".to_string()]).expect("xp parses");
        let state = CycleState::start(&bot);
        assert!(poll_stop(&bot, &state, &spec).is_none(), "nothing gained yet");
        bot.world.stats.insert(8, (2, 149));
        assert!(poll_stop(&bot, &state, &spec).is_none(), "49 of 50 holds");
        bot.world.stats.insert(8, (2, 150));
        assert_eq!(poll_stop(&bot, &state, &spec), Some(StopReason::GoalReached));
    }

    /// Level target reads the live level, not the start one.
    #[test]
    fn level_target_reads_live_level() {
        let mut bot = offline_bot();
        bot.world.stats.insert(14, (1, 0));
        let spec = parse_cli("mining", &["--level".to_string(), "mining".to_string(), "2".to_string()]).expect("level parses");
        let state = CycleState::start(&bot);
        assert!(poll_stop(&bot, &state, &spec).is_none());
        bot.world.stats.insert(14, (2, 900));
        assert_eq!(poll_stop(&bot, &state, &spec), Some(StopReason::GoalReached));
    }

    /// Pause ticks never burn the budget: 6000 work ticks with 2000 pause
    /// ticks still run.
    #[test]
    fn pause_ticks_do_not_burn_the_budget() {
        let mut bot = offline_bot();
        bot.world.ticks = 6000;
        bot.pause_ticks = 2000;
        let spec = parse_cli("woodcut", &["--budget-max".to_string(), "6000".to_string()]).expect("budget parses");
        let state = CycleState::start(&bot);
        bot.world.ticks = 12_000;
        bot.pause_ticks = 4000;
        assert_eq!(state.work_ticks(&bot), 4000, "pauses excluded");
        assert!(poll_stop(&bot, &state, &spec).is_none(), "4000 work ticks of 6000 keep running");
        bot.world.ticks = 14_000;
        assert_eq!(poll_stop(&bot, &state, &spec), Some(StopReason::BudgetExhausted));
    }

    /// Inconsistent bounds: min wins over max, the run still ends.
    #[test]
    fn min_wins_over_max() {
        let mut bot = offline_bot();
        let spec = parse_cli(
            "woodcut",
            &["--budget-min".to_string(), "7000".to_string(), "--budget-max".to_string(), "1000".to_string()],
        )
        .expect("bounds parse");
        let state = CycleState::start(&bot);
        bot.world.ticks = 6999;
        assert!(poll_stop(&bot, &state, &spec).is_none());
        bot.world.ticks = 7000;
        assert_eq!(poll_stop(&bot, &state, &spec), Some(StopReason::BudgetExhausted));
    }
}
