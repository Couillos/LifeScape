//! Money nodes drawing from the money guide (R7): the planner's gates
//! (workable now first, quarantine, tools) stay the first criterion, the
//! draw of `guide_pick` decides within the class.

use super::class_of;
use crate::expand::ReadyCtx;
use crate::guide_pick::{seed_for, GuideLine};
use crate::model::{Node, NodeKind, NodeStatus};
use crate::resolve_obtain::resolve_money;

/// The jim_ellis bot of `food_gate_tests`: 10 HP, no food, an axe.
fn jim() -> ReadyCtx {
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.bank_known = true;
    ctx.combat = 3;
    ctx.coins = 29;
    ctx.levels = vec![("hitpoints".to_string(), 10), ("attack".to_string(), 2), ("fishing".to_string(), 1), ("woodcutting".to_string(), 87)];
    ctx.tools = ["iron_axe", "bronze_pickaxe", "hammer", "tinderbox"].iter().map(|name| name.to_string()).collect();
    ctx
}

fn line(method: &str, profit: f64, runs: u64, feasible: bool) -> GuideLine {
    GuideLine { method: method.to_string(), family: "farm".to_string(), profit_6000: profit, weight: profit, runs, feasible }
}

fn money_with(ctx: &ReadyCtx, counter: u64) -> (Node, Option<crate::guide_pick::Choice>) {
    let mut ctx = ctx.clone();
    ctx.draw_seed = seed_for(7, "n5", counter);
    *ctx.drawn.borrow_mut() = None;
    let mut node = Node::new("n5", NodeKind::MakeMoney, Vec::new(), 0);
    resolve_money(&mut node, &ctx);
    let drawn = ctx.drawn.borrow().clone();
    (node, drawn)
}

/// A bot works what it can NOW first (jim_ellis: men bite, no food), the
/// draw then decides among the band of that class: willows and yews, the
/// men never, and the same seed replays the same pick.
#[test]
fn the_draw_runs_among_the_lines_workable_now() {
    let mut ctx = jim();
    ctx.money_guide = vec![
        line("fight:melee/men", 9000.0, 9, true),
        line("gather:woodcut/yew_logs", 1000.0, 9, true),
        line("gather:woodcut/willow_logs", 950.0, 9, true),
        line("fight:melee/chickens", 100.0, 9, true),
    ];
    assert_eq!(class_of(&ctx.money_guide[0], &ctx), Some(1), "men need food first");
    let mut seen = std::collections::BTreeSet::new();
    for counter in 0..200 {
        let (node, drawn) = money_with(&ctx, counter);
        let method = node.method.clone().expect("a method");
        assert!(method == "gather:woodcut/yew_logs" || method == "gather:woodcut/willow_logs", "{method}");
        let drawn = drawn.expect("a recorded choice");
        assert_eq!((drawn.kind, drawn.class, drawn.drawn.band, drawn.drawn.candidates), ("money", 0, 2, 3));
        assert_eq!(money_with(&ctx, counter).0.method, node.method, "replayable");
        seen.insert(method);
    }
    assert_eq!(seen.len(), 2, "both lines of the band drawn: {seen:?}");
}

/// A line the bot cannot work yet only draws when nothing works now: the
/// net to obtain first blocks the node on it, the method kept so the
/// block's `blocked_by` takes it again once the net is held (sticky).
#[test]
fn a_tool_short_line_blocks_on_its_tool_and_comes_back_after() {
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.money_guide = vec![line("gather:fishing/raw_shrimp", 800.0, 9, false)];
    let (node, drawn) = money_with(&ctx, 0);
    assert_eq!(node.status, NodeStatus::Blocked { reason: "missing:net".to_string() });
    assert_eq!(node.method.as_deref(), Some("gather:fishing/raw_shrimp"));
    assert_eq!(drawn.map(|choice| choice.class), Some(2));
    // The net obtained: a better line appeared, the node still fishes.
    ctx.tools.push("net".to_string());
    ctx.money_guide.push(line("fight:melee/cows", 820.0, 9, true));
    ctx.money_guide[0].feasible = true;
    let mut back = Node::new("n5", NodeKind::MakeMoney, Vec::new(), 0);
    back.blocked_by = Some("gather:fishing/raw_shrimp".to_string());
    let mut sticky = ctx.clone();
    for counter in 0..20 {
        sticky.draw_seed = seed_for(7, "n5", counter);
        resolve_money(&mut back, &sticky);
        assert_eq!(back.method.as_deref(), Some("gather:fishing/raw_shrimp"), "the tool bought for it is used");
        assert!(sticky.drawn.borrow().as_ref().is_some_and(|choice| choice.sticky));
    }
}

/// The garde-fous stay ahead of the draw: a quarantined or cooled line
/// and a method a dependency cycle excluded never draw; nothing drawable
/// leaves the static money methods to rank (no choice recorded).
#[test]
fn quarantine_cooldown_and_cycles_stay_ahead_of_the_draw() {
    let mut ctx = jim();
    ctx.money_guide = vec![line("gather:woodcut/yew_logs", 1000.0, 9, true), line("fight:melee/cows", 990.0, 9, true)];
    ctx.quarantined = vec!["gather:woodcut/yew_logs".to_string()];
    for counter in 0..30 {
        assert_eq!(money_with(&ctx, counter).0.method.as_deref(), Some("fight:melee/cows"));
    }
    let mut node = Node::new("n5", NodeKind::MakeMoney, Vec::new(), 0);
    node.excluded = vec!["fight:melee/cows".to_string()];
    ctx.draw_seed = 3;
    resolve_money(&mut node, &ctx);
    assert!(ctx.drawn.borrow().is_none(), "nothing drawable: the static methods ranked");
    assert!(node.method.is_some(), "a static money method still runs: {:?}", node.alternatives);
}

/// Exploration reaches an unmeasured line far below the band (D14), about
/// one draw in twenty.
#[test]
fn an_unmeasured_line_is_explored_now_and_then() {
    let mut ctx = jim();
    ctx.money_guide = vec![line("gather:woodcut/yew_logs", 1000.0, 9, true), line("fight:melee/cows", 300.0, 0, true)];
    let explored = (0..2000).filter(|counter| money_with(&ctx, *counter).1.is_some_and(|choice| choice.drawn.explored && choice.method == "fight:melee/cows")).count();
    assert!((60..=140).contains(&explored), "{explored} of 2000");
}
