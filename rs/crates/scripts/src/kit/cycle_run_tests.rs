//! `cycle::run` across market pauses (a load whose step failed while a
//! delivery banked the pack is taken again, bounded) and spot orders.

use super::{parse_cli, run, StepControl};
use crate::kit::site::PASS_PAUSE_RESTARTS;
use crate::tables_live::attach_live_tables;
use crate::ScriptError;
use lifescape_sdk::Bot;

fn offline_bot() -> Bot {
    let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
    attach_live_tables(&mut bot);
    bot
}

/// Live 2026-11-19 after the 17:41 deploy (ida_reed, tim_thorn,
/// brook_daisy, pia_brown... 22 smelts; nina_87, Zezima smithing;
/// wolfe_jack jewellery): the market delivery closed during the walk to
/// the furnace (or the anvil chain) and banked the pack but the tools;
/// the step then read "smelting consumed nothing" / "nothing smithed"
/// at the furnace or the Varrock square and the run failed. A load that
/// failed across a closed pause is taken again (the next step restocks
/// from the bank), never a failure counted against the node.
#[test]
fn a_load_failed_across_a_pause_is_taken_again() {
    let mut bot = offline_bot();
    let spec = parse_cli("smelt", &[]).expect("smelt parses");
    let mut calls = 0u32;
    let outcome = run(&mut bot, &spec, |step_bot, _| {
        calls += 1;
        if calls == 1 {
            step_bot.pauses_done += 1;
            return Err(ScriptError::Failed("smelting consumed nothing at (0, 3226, 3254)".to_string()));
        }
        Ok(StepControl::GoalReached)
    });
    assert_eq!(outcome.expect("the load is taken again"), 0);
    assert_eq!(calls, 2, "one reload after the pause");
}

/// Live 2026-11-20 (68 smiths `item 2353 is not in the backpack` at
/// the Port Sarim anvil, 20 potters on clay): the delivery banked the
/// bars during the walk to the anvil, and the anvil's use read the
/// missing bar as an SDK state error, which the reload rule skipped. A
/// state error across a pause is the pause's doing: the load is taken
/// again; without a pause, or any other SDK error, it stands.
#[test]
fn a_state_error_across_a_pause_is_taken_again() {
    let mut bot = offline_bot();
    let spec = parse_cli("smith", &[]).expect("smith parses");
    let mut calls = 0u32;
    let outcome = run(&mut bot, &spec, |step_bot, _| {
        calls += 1;
        if calls == 1 {
            step_bot.pauses_done += 1;
            return Err(ScriptError::Sdk(lifescape_sdk::Error::State("item 2353 is not in the backpack at (0, 2998, 3144)".to_string())));
        }
        Ok(StepControl::GoalReached)
    });
    assert_eq!(outcome.expect("the load is taken again"), 0);
    assert_eq!(calls, 2, "one reload after the pause");
    let unpaused = run(&mut bot, &spec, |_, _| Err(ScriptError::Sdk(lifescape_sdk::Error::State("item 2353 is not in the backpack".to_string()))));
    assert!(matches!(unpaused, Err(ScriptError::Sdk(_))), "no pause: the state error stands, got {unpaused:?}");
    let io = run(&mut bot, &spec, |step_bot, _| {
        step_bot.pauses_done += 1;
        Err(ScriptError::Sdk(lifescape_sdk::Error::Io("socket closed".to_string())))
    });
    assert!(matches!(io, Err(ScriptError::Sdk(_))), "a dead socket is never retried, got {io:?}");
}

/// Without a pause during the step, a failure ends the run as before.
#[test]
fn a_failure_without_a_pause_still_fails() {
    let mut bot = offline_bot();
    let spec = parse_cli("smelt", &[]).expect("smelt parses");
    let mut calls = 0u32;
    let outcome = run(&mut bot, &spec, |_, _| {
        calls += 1;
        Err(ScriptError::Failed("smelting consumed nothing".to_string()))
    });
    assert!(matches!(outcome, Err(ScriptError::Failed(_))), "real failure kept, got {outcome:?}");
    assert_eq!(calls, 1);
}

/// Bounded like the SDK trips: a step that fails across a pause every
/// time fails after `PASS_PAUSE_RESTARTS` reloads; usage errors are never
/// retried (a pause never explains them).
#[test]
fn pause_reloads_are_bounded_and_only_for_failures() {
    let mut bot = offline_bot();
    let spec = parse_cli("smith", &[]).expect("smith parses");
    let mut calls = 0u32;
    let outcome = run(&mut bot, &spec, |step_bot, _| {
        calls += 1;
        step_bot.pauses_done += 1;
        Err(ScriptError::Failed("nothing smithed at (0, 3185, 3436)".to_string()))
    });
    assert!(matches!(outcome, Err(ScriptError::Failed(_))), "got {outcome:?}");
    assert_eq!(calls, PASS_PAUSE_RESTARTS + 1);
    let mut usage_calls = 0u32;
    let usage = run(&mut bot, &spec, |step_bot, _| {
        usage_calls += 1;
        step_bot.pauses_done += 1;
        Err(ScriptError::Usage("no anvil".to_string()))
    });
    assert!(matches!(usage, Err(ScriptError::Usage(_))), "got {usage:?}");
    assert_eq!(usage_calls, 1, "a contract error is never a pause's doing");
}

/// A delivery that closes while a restock walks to the bank banks what
/// the restock had withdrawn: the step then reads its inputs as missing
/// (`Blocked`) though the bank holds them (8 of 169 blocks of the
/// 2026-11-22 08:45-09:50 hour followed a pause with no load done). The
/// step is taken again, which re-prepares (re-withdraws); a block that
/// stands with no new pause is a real one and ends the run as before.
#[test]
fn a_block_across_a_pause_re_prepares_once_and_a_real_block_stands() {
    let mut bot = offline_bot();
    let spec = parse_cli("smelt", &[]).expect("smelt parses");
    let mut calls = 0u32;
    let outcome = run(&mut bot, &spec, |step_bot, _| {
        calls += 1;
        if calls == 1 {
            step_bot.pauses_done += 1;
            return Err(ScriptError::Blocked { missing: vec!["iron_ore".to_string()] });
        }
        Ok(StepControl::GoalReached)
    });
    assert_eq!(outcome.expect("the step re-prepares after the pause"), 0);
    assert_eq!(calls, 2);
    let mut real_calls = 0u32;
    let real = run(&mut bot, &spec, |step_bot, _| {
        real_calls += 1;
        if real_calls == 1 {
            step_bot.pauses_done += 1;
        }
        Err(ScriptError::Blocked { missing: vec!["iron_ore".to_string()] })
    });
    assert!(matches!(real, Err(ScriptError::Blocked { .. })), "still missing after re-preparing: blocked, got {real:?}");
    assert_eq!(real_calls, 2, "one re-preparation, then the block stands");
    let unpaused = run(&mut bot, &spec, |_, _| Err(ScriptError::Blocked { missing: vec!["iron_ore".to_string()] }));
    assert!(matches!(unpaused, Err(ScriptError::Blocked { .. })), "no pause: blocked at once, got {unpaused:?}");
}

#[test]
fn combat_styles_take_spot_names() {
    for activity in ["melee", "ranged", "magic"] {
        let spec = parse_cli(activity, &["cows".to_string(), "--ticks".to_string(), "600".to_string()]).expect("spot parses");
        assert_eq!(spec.resource, vec!["cows".to_string()], "{activity}");
        assert_eq!(spec.budget_max_ticks, 600);
    }
    let counted = parse_cli("magic", &["25".to_string()]).expect("count parses");
    assert_eq!(counted.targets.count, Some(25));
}
