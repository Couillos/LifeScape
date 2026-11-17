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
    /// Item quantity target (`--want <item> N`): the run ends once N more
    /// of the item are held (pack plus known bank) than at its start.
    pub want_item: Option<String>,
    pub want_count: Option<u32>,
}

impl TaskTargets {
    /// True when the run has an objective the brain can check (XP,
    /// level, quantity, item, quest).
    pub fn any(&self) -> bool {
        self.xp_amount.is_some() || self.level.is_some() || self.count.is_some() || self.quest.is_some() || self.want_count.is_some()
    }
}

/// Exit code of a run that ended its own work loop (plan P15, live
/// 2026-11-17: melee fillers without `--level`/`--want` ended
/// `goal_reached` on their own criterion, the brain's goal check read
/// short, 162 failures banned attack). With an objective the loop's end
/// is the goal (0); without one it is the end of a window
/// (`BUDGET_EXIT`, `budget_exhausted`): never `goal_reached`. Tour 3
/// point 6 (P15c, live ryan_mason x3: melee `--want naturerune 434`
/// ended `goal_reached` at its ~6 000-tick budget, short of the want):
/// with an objective, only a MET one is the goal; the loop ending short
/// of it is the end of the window too.
pub fn ended_code(bot: &Bot, state: &CycleState, spec: &TaskSpec) -> i32 {
    ended_code_for(spec, goal_met(bot, state, spec))
}

/// Pure `ended_code`: `met` = an objective of the run is met (a quest
/// objective, which the brain checks itself, reads as met).
pub fn ended_code_for(spec: &TaskSpec, met: bool) -> i32 {
    let quest_only = spec.targets.quest.is_some() && !(spec.targets.xp_amount.is_some() || spec.targets.level.is_some() || spec.targets.count.is_some() || spec.targets.want_count.is_some());
    if met || quest_only {
        0
    } else {
        BUDGET_EXIT
    }
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
    /// Ground objects never picked up (`--skip-loot NAME`, plan marché
    /// M5: worth less than the pickup ticks at the bot's rate).
    pub skip_loot: Vec<String>,
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
    /// The Karamja boat allowed (`boat`): 30 coins each way from the
    /// pack, a round trip kept (`kit::toll::arm_fares`).
    pub allow_boat: bool,
    /// Hunter zones allowed (`wild`, woodcut only today).
    pub allow_wild: bool,
    /// Spell the brain sized the runes for (`--spell`, magic): cast
    /// exactly this one, never the strongest the level unlocks.
    pub spell: Option<String>,
    /// Uniform the guild door checks (`--wear`, repeatable): worn before
    /// the run walks to its site (`kit::uniform`).
    pub wear: Vec<String>,
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
            skip_loot: Vec::new(),
            allow_market: false,
            allow_shop: true,
            node_id: String::new(),
            site: None,
            chain: None,
            no_bank: false,
            allow_paid: false,
            allow_boat: false,
            allow_wild: false,
            spell: None,
            wear: Vec::new(),
        }
    }
}

/// Leading resource positional of a production script (`smelt
/// iron_bar ...`): the first argument unless it is a count, a flag or a
/// `toll`/`wild` word. Returns it and the rest for `parse_cli`.
pub fn split_resource(args: &[String]) -> (Option<String>, Vec<String>) {
    let resource = args
        .first()
        .filter(|first| first.parse::<u32>().is_err() && first.as_str() != "toll" && first.as_str() != "boat" && first.as_str() != "wild" && !first.starts_with("--"))
        .cloned();
    let rest = if resource.is_some() { args[1..].to_vec() } else { args.to_vec() };
    (resource, rest)
}

/// Scripts whose resource positional is split off before `parse_cli`
/// (gatherers, cook and the combat styles take it inside `parse_cli`).
pub const SPLIT_RESOURCE_SCRIPTS: [&str; 20] = [
    "alchemy", "bury_loot", "cook_meals", "cut_gems", "dialogue_buy", "dye", "enchant_teleport", "fill_water", "firemaking", "ground_pickup", "leatherwork",
    "picking", "pottery", "runecraft", "smelt", "smelt_jewelry", "smith", "spin", "stringing", "superheat",
];

/// The brain's order as a script reads it (contract check, no side
/// effect): split resource for production scripts, then `parse_cli`.
pub fn parse_order(script: &'static str, args: &[String]) -> Result<TaskSpec, ScriptError> {
    if SPLIT_RESOURCE_SCRIPTS.contains(&script) {
        let (resource, rest) = split_resource(args);
        let mut spec = parse_cli(script, &rest)?;
        spec.resource.extend(resource);
        return Ok(spec);
    }
    parse_cli(script, args)
}

/// Parse the legacy CLI plus the task flags into a `TaskSpec`. Legacy
/// positionals keep their meaning (`woodcut 3` = 3 loads, melee spots);
/// `--kills N` is the quantity target, `--ticks N` the max budget.
pub fn parse_cli(activity: &'static str, args: &[String]) -> Result<TaskSpec, ScriptError> {
    let usage = || {
        ScriptError::Usage(format!(
            "{activity} [count|spot...] [toll] [boat] [wild] [--kills N] [--ticks N] [--no-bank] [--xp SKILL N] [--level SKILL N] [--quest KEY] [--budget-min N] [--budget-max N] [--keep NAME]... [--node ID] [--site X,Z,LEVEL] [--want ITEM N] [--spell SPELL] [--wear ITEM]... [--skip-loot NAME]... [--allow-market] [--no-shop]"
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
            "boat" => {
                spec.allow_boat = true;
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
            "--skip-loot" => {
                let name = args.get(index + 1).cloned().filter(|name| !name.is_empty() && !name.starts_with("--")).ok_or_else(usage)?;
                spec.skip_loot.push(name);
                index += 2;
            }
            "--site" => {
                let raw = args.get(index + 1).ok_or_else(usage)?;
                let parts: Vec<i32> = raw.split(',').map(|part| part.trim().parse::<i32>()).collect::<Result<_, _>>().map_err(|_| usage())?;
                let [x, z, level] = parts[..] else { return Err(usage()) };
                spec.site = Some((x, z, level));
                index += 2;
            }
            "--want" => {
                let item = args.get(index + 1).cloned().filter(|item| !item.is_empty() && !item.starts_with("--")).ok_or_else(usage)?;
                let count = args.get(index + 2).and_then(|arg| arg.parse::<u32>().ok()).ok_or_else(usage)?;
                spec.targets.want_item = Some(item);
                spec.targets.want_count = Some(count);
                index += 3;
            }
            "--node" => {
                let node = args.get(index + 1).cloned().filter(|node| !node.is_empty()).ok_or_else(usage)?;
                spec.node_id = node;
                index += 2;
            }
            "--spell" => {
                spec.spell = Some(args.get(index + 1).cloned().filter(|spell| !spell.is_empty() && !spell.starts_with("--")).ok_or_else(usage)?);
                index += 2;
            }
            "--wear" => {
                spec.wear.push(args.get(index + 1).cloned().filter(|item| !item.is_empty() && !item.starts_with("--")).ok_or_else(usage)?);
                index += 2;
            }
            other if other.starts_with("--") => return Err(usage()),
            other => {
                // Combat spots ride positionally for every style: the
                // brain sends `ranged cows` / `magic chickens` like
                // `melee cows` (PRD3 lot 6: both failed Usage before).
                if matches!(activity, "melee" | "ranged" | "magic" | "curse") && other.parse::<u32>().is_err()
                    || activity == "melee"
                    || (activity == "cook" && spec.resource.is_empty() && other.parse::<u32>().is_err())
                    // Gatherers: the brain's target resource rides first
                    // (`fishing raw_trout`, `mining gold_ore`).
                    || (matches!(activity, "fishing" | "mining" | "woodcut") && spec.resource.is_empty() && other.parse::<u32>().is_err())
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
    // A gatherer counts the resource it works (paul_rhodes n60: an oak
    // quota launched without its logs cut willow until the budget ended,
    // 25 runs). `--want <item>` alone names the target; a positional
    // naming another item is a crossed order that never stops on its goal.
    if matches!(activity, "fishing" | "mining" | "woodcut") {
        if let Some(item) = spec.targets.want_item.clone() {
            match spec.resource.first() {
                None => spec.resource.push(item),
                Some(resource) if resource.eq_ignore_ascii_case(&item) => {}
                Some(resource) => {
                    return Err(ScriptError::Usage(format!("{activity} works {resource} but --want counts {item}: one item per order")));
                }
            }
        }
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
    /// Held count of the `--want` item at start (pack + known bank).
    want_base: Option<u32>,
    /// Silent-loop guard (`kit::progress`), observed after every step of
    /// `run`; `load_done` reads it to tell an `ok load` from an empty one.
    progress: crate::kit::progress::ProgressGuard,
}

impl CycleState {
    pub fn start(bot: &Bot) -> CycleState {
        CycleState {
            start_tick: bot.world.ticks,
            start_pause: bot.pause_ticks,
            start_xp: bot.world.stats.clone(),
            units: 0,
            want_base: None,
            progress: crate::kit::progress::ProgressGuard::start("run", &crate::kit::progress::mark(bot)),
        }
    }

    /// End of one load: logs `ok load N <what>` only when the load
    /// produced (XP, or an object above its high mark since the last
    /// progress: a deposit or a withdrawal moves objects, never makes
    /// them), else `empty load N <what>` — the guard fails the run on
    /// the third empty load in a row. Returns `UnitDone`.
    pub fn load_done(&self, bot: &Bot, what: &str) -> StepControl {
        let verdict = if self.progress.progressed(&crate::kit::progress::mark(bot)) { "ok load" } else { "empty load" };
        bot.log(&format!("{verdict} {} {what}", self.units + 1));
        StepControl::UnitDone
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

    /// `start` plus the `--want` baseline: the entry for loops that poll
    /// `poll_stop` themselves (combat styles).
    pub fn begin(bot: &Bot, spec: &TaskSpec) -> CycleState {
        let mut state = CycleState::start(bot);
        state.note_want_base(bot, spec);
        state
    }

    /// Record the `--want` baseline (call once at run start).
    pub fn note_want_base(&mut self, bot: &Bot, spec: &TaskSpec) {
        self.want_base = spec.targets.want_item.as_deref().map(|item| held_total(bot, item));
    }

    /// The `--want` goal for the load driver (`LoadCfg::until`): the
    /// ordered object and the held total that meets it. `None` without
    /// `--want` (or an unknown name, which holds nothing).
    pub fn want_goal(&self, bot: &Bot, spec: &TaskSpec) -> Option<WantGoal> {
        let item = spec.targets.want_item.as_deref()?;
        let obj = (bot.tables.find_obj_by_name)(item)?;
        Some(WantGoal { obj, held: self.want_base?.saturating_add(spec.targets.want_count?) })
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

/// True once one of the run's objectives (`--want`, count, XP, level)
/// is met.
pub fn goal_met(bot: &Bot, state: &CycleState, spec: &TaskSpec) -> bool {
    if let (Some(item), Some(want), Some(base)) = (spec.targets.want_item.as_deref(), spec.targets.want_count, state.want_base) {
        if held_total(bot, item).saturating_sub(base) >= want {
            return true;
        }
    }
    if spec.targets.count.is_some_and(|want| state.units >= want) {
        return true;
    }
    if let (Some(skill), Some(amount)) = (spec.targets.xp_skill.as_deref(), spec.targets.xp_amount) {
        if state.xp_gain(bot, skill) >= amount {
            return true;
        }
    }
    if let (Some(skill), Some(level)) = (spec.targets.level_skill.as_deref(), spec.targets.level) {
        if state.level_now(bot, skill) >= level {
            return true;
        }
    }
    false
}

/// One stop check: goal first (targets), then budget (deferred to the
/// next safe point, forced past the defer cap). `min` wins over `max`
/// when inconsistent; `max == 0` means unbounded.
pub fn poll_stop(bot: &Bot, state: &CycleState, spec: &TaskSpec) -> Option<StopReason> {
    let elapsed = state.work_ticks(bot);
    if goal_met(bot, state, spec) {
        return Some(StopReason::GoalReached);
    }
    let budget = spec.budget_max_ticks.max(spec.budget_min_ticks);
    if budget > 0 && elapsed >= budget {
        if crate::kit::survival::at_safe_point(bot) || elapsed >= budget + SAFE_DEFER_TICKS {
            return Some(StopReason::BudgetExhausted);
        }
    }
    None
}

/// A run's `--want` goal as the load driver checks it: the ordered
/// object and the held total (pack + known bank) that meets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WantGoal {
    pub obj: u16,
    pub held: u32,
}

impl WantGoal {
    /// True once pack plus known bank hold `held` units of `obj`.
    pub fn met(&self, bot: &Bot) -> bool {
        self.left(bot) == 0
    }
}

/// Units of `item` held: pack plus the last known bank view (unknown
/// names hold nothing).
pub fn held_total(bot: &Bot, item: &str) -> u32 {
    let Some(id) = (bot.tables.find_obj_by_name)(item) else { return 0 };
    crate::kit::pack::pack_total(bot, id).saturating_add(crate::kit::pack::banked_usable(bot, id))
}

/// Typed gap for a level the task needs (`level:<skill>:<N>`): the brain
/// files a reach-level sub-task, never a Usage retry.
pub fn blocked_level(skill: &str, level: i64) -> ScriptError {
    ScriptError::Blocked { missing: vec![format!("level:{skill}:{level}")] }
}

/// Typed gap for a quest the task needs (`quest:<key>`).
pub fn blocked_quest(key: &str) -> ScriptError {
    ScriptError::Blocked { missing: vec![format!("quest:{key}")] }
}

/// Typed gap for an area the task cannot enter (`access:<area>`): a toll,
/// a boat, a guild door. The brain funds or routes, never the script.
pub fn blocked_access(area: &str) -> ScriptError {
    ScriptError::Blocked { missing: vec![format!("access:{area}")] }
}

/// Drive one activity until its stop conditions: checks the spec, runs
/// one unit, repeat. Goal and budget end the run with their exit codes;
/// every other error propagates (blocked included).
pub fn run(bot: &mut Bot, spec: &TaskSpec, mut step: impl FnMut(&mut Bot, &mut CycleState) -> Result<StepControl, ScriptError>) -> crate::ScriptResult {
    bot.assigned_site = spec.site;
    // A guild site: the uniform goes on before any step walks to the door.
    crate::kit::uniform::wear_uniform(bot, &spec.wear)?;
    let mut state = CycleState::start(bot);
    state.note_want_base(bot, spec);
    // No load loop runs silent: a run of steps that produce nothing (no
    // XP, no object above its high mark, bank churn) fails typed
    // `no progress: …` instead of looping to the budget (`kit::progress`).
    state.progress = crate::kit::progress::ProgressGuard::start(spec.activity, &crate::kit::progress::mark(bot));
    loop {
        if let Some(stop) = poll_stop(bot, &state, spec) {
            return exit_code(stop);
        }
        let unit = match step(bot, &mut state)? {
            StepControl::Continue => false,
            StepControl::UnitDone => {
                state.note_unit();
                true
            }
            StepControl::GoalReached => return Ok(0),
        };
        state.progress.observe(&crate::kit::progress::mark(bot), unit)?;
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
        assert!(parse_cli("woodcut", &["logs".to_string(), "x".to_string()]).is_err(), "junk count fails loud");
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
        // P15: a run without objective never ends `goal_reached`.
        let bare = parse_cli("melee", &["men".to_string()]).expect("bare melee");
        assert!(!bare.targets.any());
        assert_eq!(super::ended_code_for(&bare, false), BUDGET_EXIT);
        let leveled = parse_cli("melee", &["men".to_string(), "--level".to_string(), "attack".to_string(), "20".to_string()]).expect("melee to 20");
        assert!(leveled.targets.any());
        assert_eq!(super::ended_code_for(&leveled, true), 0);
        // Tour 3 point 6 (P15c, ryan_mason): `--want naturerune 434` at
        // the end of its budget, short of the want: a window, never the
        // goal.
        let wanted = parse_cli("melee", &["men".to_string(), "--want".to_string(), "naturerune".to_string(), "434".to_string()]).expect("melee for runes");
        assert!(wanted.targets.any());
        assert_eq!(super::ended_code_for(&wanted, false), BUDGET_EXIT);
        assert_eq!(super::ended_code_for(&wanted, true), 0);
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
        let outcome = run(&mut bot, &spec, |step_bot, _| {
            calls += 1;
            gain_xp(step_bot);
            Ok(StepControl::UnitDone)
        });
        assert_eq!(outcome.unwrap(), 0);
        assert_eq!(calls, 3, "one unit per load, no more");
    }

    /// One load's worth of woodcutting XP (a producing step).
    fn gain_xp(bot: &mut Bot) {
        let (level, xp) = bot.world.stats.get(&8).copied().unwrap_or((1, 0));
        bot.world.stats.insert(8, (level, xp + 250));
    }

    /// Live 2026-11-14 (pottery, ~1400 bank opens in 30 min): loads that
    /// produce nothing never run to the budget in silence. Three empty
    /// loads in a row fail typed `no progress: …`.
    #[test]
    fn run_fails_loud_on_loads_without_output() {
        let mut bot = offline_bot();
        let spec = parse_cli("pottery", &["--budget-max".to_string(), "6000".to_string()]).expect("budget parses");
        let mut calls = 0u32;
        let outcome = run(&mut bot, &spec, |step_bot, _| {
            calls += 1;
            step_bot.world.ticks += 65;
            Ok(StepControl::UnitDone)
        });
        let error = outcome.expect_err("silent loop fails");
        assert!(error.to_string().contains("no progress: pottery 3 loads in a row produced nothing"), "{error}");
        assert_eq!(calls, 3);
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
            gain_xp(step_bot);
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
            gain_xp(step_bot);
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

    /// The brain's gather order: the target resource rides first
    /// (`fishing raw_trout --site ...`), a count may follow; `--want`
    /// sets an item quantity target.
    #[test]
    fn gatherers_take_the_target_resource_and_want() {
        for (activity, target) in [("fishing", "raw_trout"), ("mining", "gold_ore"), ("woodcut", "willow_logs")] {
            let spec = parse_cli(activity, &[target.to_string(), "--site".to_string(), "3104,3424,0".to_string()]).expect("target parses");
            assert_eq!(spec.resource, vec![target.to_string()], "{activity}");
            assert_eq!(spec.site, Some((3104, 3424, 0)));
        }
        let counted = parse_cli("fishing", &["raw_trout".to_string(), "2".to_string()]).expect("target then count");
        assert_eq!((counted.resource.len(), counted.targets.count), (1, Some(2)));
        let want = parse_cli("mining", &["gold_ore".to_string(), "--want".to_string(), "gold_ore".to_string(), "100".to_string()]).expect("want parses");
        assert_eq!((want.targets.want_item.as_deref(), want.targets.want_count), (Some("gold_ore"), Some(100)));
        assert!(parse_cli("mining", &["--want".to_string(), "gold_ore".to_string()]).is_err(), "want needs a count");
    }

    /// paul_rhodes n60 (`obtain oak_logs 308`): launched `woodcut --site
    /// ...` without the logs, the run cut 3,649 willow logs over 25
    /// budget-ended runs. A gatherer's `--want` names its target: a bare
    /// `--want oak_logs N` cuts oak, a positional naming another item is a
    /// Usage error (the counted item would never move).
    #[test]
    fn gatherer_want_names_the_resource() {
        for (activity, target, other) in [("woodcut", "oak_logs", "willow_logs"), ("mining", "tin_ore", "iron_ore"), ("fishing", "raw_trout", "raw_salmon")] {
            let bare = parse_cli(activity, &["--want".to_string(), target.to_string(), "308".to_string()]).expect("bare want parses");
            assert_eq!(bare.resource, vec![target.to_string()], "{activity}: the want item is the target");
            let same = parse_cli(activity, &[target.to_string(), "--want".to_string(), target.to_string(), "70".to_string()]).expect("same item parses");
            assert_eq!(same.resource, vec![target.to_string()], "{activity}");
            let crossed = parse_cli(activity, &[other.to_string(), "--want".to_string(), target.to_string(), "70".to_string()]);
            assert!(matches!(crossed, Err(crate::ScriptError::Usage(_))), "{activity} {other} --want {target} must fail Usage, got {crossed:?}");
        }
    }

    /// Typed gaps the brain files as sub-goals (never a Usage retry).
    #[test]
    fn gap_kinds_are_typed_blocks() {
        let named = |error: crate::ScriptError| match error {
            crate::ScriptError::Blocked { missing } => missing,
            other => panic!("blocked expected, got {other:?}"),
        };
        assert_eq!(named(super::blocked_level("smithing", 30)), vec!["level:smithing:30".to_string()]);
        assert_eq!(named(super::blocked_quest("runemysteries")), vec!["quest:runemysteries".to_string()]);
        assert_eq!(named(super::blocked_access("al_kharid_toll")), vec!["access:al_kharid_toll".to_string()]);
    }

    /// `--want item N`: the run ends once N more are held (pack + known
    /// bank) than at its start, whatever the loads count.
    #[test]
    fn want_target_counts_pack_and_bank_gains() {
        let mut bot = offline_bot();
        let ore = crate::kit::pack::obj_id(&bot, "gold_ore").expect("gold_ore");
        let pack = (bot.tables.com_id)("inventory:inv").expect("inventory");
        let bank = (bot.tables.com_id)("bank_main:inv").expect("bank");
        bot.world.inv.insert(pack, [(0u8, (ore, 3u32))].into_iter().collect());
        let spec = parse_cli("mining", &["gold_ore".to_string(), "--want".to_string(), "gold_ore".to_string(), "10".to_string()]).expect("want parses");
        let mut state = CycleState::start(&bot);
        state.note_want_base(&bot, &spec);
        assert!(poll_stop(&bot, &state, &spec).is_none(), "nothing gained");
        bot.world.inv.insert(bank, [(0u8, (ore, 9u32))].into_iter().collect());
        assert!(poll_stop(&bot, &state, &spec).is_none(), "9 of 10 gained");
        bot.world.inv.insert(pack, [(0u8, (ore, 4u32))].into_iter().collect());
        assert_eq!(poll_stop(&bot, &state, &spec), Some(StopReason::GoalReached), "10 gained across pack and bank");
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
