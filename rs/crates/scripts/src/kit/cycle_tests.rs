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
