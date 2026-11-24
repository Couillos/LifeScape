//! Tests of the launch decision and its life changes, without a daemon.

use crate::expand::ReadyCtx;
use crate::launch_apply::{apply_to_life, settle_market_buy, ApplyEnv, PlanSource};
use crate::launch_decide::{decide, Decide, Emit, Known, LaunchDecision, LaunchSave, Rules};
use crate::launch_inputs::{read_stacks, LaunchInputs};
use crate::plan_read::PlanRead;
use crate::{Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};
use lifescape_json::{object, str_value, Value};

struct Prior;

impl PlanSource for Prior {
    fn with_read(&self, _account: &str, _profile: &Profile, _quests: &[String], body: &mut dyn FnMut(&PlanRead)) {
        body(&PlanRead::prior());
    }
}

fn rules() -> Rules {
    Rules { short_inputs: |_, _, _| Vec::new(), input_each: |_, _| 1, script_known: |_| true }
}

fn life_of(node: Node, goal: GoalKind) -> Life {
    Life {
        schema: 1,
        account: "t_decide".to_string(),
        created_at: 0,
        seed: 1,
        profile: Profile::new(false, 0.5, 0.5, 1, Archetype::Producer),
        goal,
        nodes: vec![node],
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
        memory: Default::default(),
    }
}

fn inputs_with(stacks: Vec<Value>) -> LaunchInputs {
    let live = object(vec![("inv", Value::Arr(stacks))]);
    LaunchInputs { inv: read_stacks(&live, "inv"), now: 1_000, tick_ms: 600, ..LaunchInputs::blank_for_tests() }
}

fn stack(name: &str, count: i64) -> Value {
    let obj = lifescape_gamedata::objs_by_name(name).into_iter().next().expect("in the dump");
    object(vec![("id", Value::Int(obj.obj_id as i64)), ("name", str_value(obj.display.unwrap_or(name))), ("count", Value::Int(count))])
}

fn run(life: &Life, ctx: &ReadyCtx, inputs: &LaunchInputs, known: &Known) -> crate::launch_decide::Decided {
    let rules = rules();
    let timed = |_: &str, body: &mut dyn FnMut()| body();
    decide(&Decide { life, position: 0, ctx, committed: 0, inputs, rules: &rules, known, timed: &timed })
}

/// The decision reads its arguments and writes none: the life and the
/// context are the same after it, however far it went.
#[test]
fn deciding_changes_nothing() {
    let mut node = Node::new("n1", NodeKind::ObtainItems { item: "feather".to_string(), count: 5 }, Vec::new(), 5);
    node.status = NodeStatus::Ready;
    node.alternatives = vec![crate::Alternative { method: "market:buy".to_string(), est_ticks: 10, est_coins: 5 }];
    let life = life_of(node, GoalKind::Obtain { item: "feather".to_string(), count: 5 });
    let ctx = ReadyCtx::blind(0.5);
    let inputs = inputs_with(vec![stack("coins", 1_000)]);
    let (before_life, before_inputs) = (life.clone(), inputs.clone());
    let decided = run(&life, &ctx, &inputs, &Known::default());
    assert_eq!(life, before_life);
    assert_eq!(format!("{inputs:?}"), format!("{before_inputs:?}"));
    assert!(ctx.drawn.borrow().is_none(), "the resolve ran on a copy of the context");
    assert!(decided.boundary.is_some(), "the resolve happened");
}

/// An obtain the holdings meet: the decision, then its life changes.
#[test]
fn held_copies_close_the_node() {
    let mut node = Node::new("n1", NodeKind::ObtainItems { item: "feather".to_string(), count: 5 }, Vec::new(), 5);
    node.status = NodeStatus::Ready;
    let mut life = life_of(node, GoalKind::Obtain { item: "feather".to_string(), count: 5 });
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.held = 7;
    let inputs = inputs_with(vec![stack("feather", 7)]);
    let known = Known { recounted: true, ..Known::default() };
    let decided = run(&life, &ctx, &inputs, &known);
    assert_eq!(decided.decision, LaunchDecision::AlreadyHeld { item: "feather".to_string(), held: 7 });
    assert_eq!(decided.save(), LaunchSave::Always);
    let env = ApplyEnv { inputs: &inputs, quests_done: &[], reader: &Prior };
    let emits = apply_to_life(&mut life, &decided, &env);
    assert_eq!(emits, vec![Emit::CancelOpenBuys("feather".to_string()), Emit::Log("brain t_decide: n1 holds 7 feather, done without launch".to_string())]);
    assert_eq!(life.history.last().map(|entry| entry.outcome.as_str()), Some("already_held"));
}

/// Copies held but listed: the decision asks for the withdrawal, then,
/// once answered, counts what is free now.
#[test]
fn listed_copies_ask_for_a_withdrawal_first() {
    let mut node = Node::new("n1", NodeKind::ObtainItems { item: "feather".to_string(), count: 5 }, Vec::new(), 5);
    node.status = NodeStatus::Ready;
    let life = life_of(node, GoalKind::Obtain { item: "feather".to_string(), count: 5 });
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.held = 7;
    let id = lifescape_gamedata::objs_by_name("feather").into_iter().next().expect("dump").obj_id as i64;
    let mut inputs = inputs_with(vec![stack("feather", 7)]);
    inputs.listed.insert(id, 7);
    let known = Known { recounted: true, ..Known::default() };
    assert_eq!(run(&life, &ctx, &inputs, &known).decision, LaunchDecision::WithdrawOwnListings { items: vec!["feather".to_string()] });
    // The listings are gone: the same pass goes on and the node is done.
    inputs.listed.clear();
    let known = Known { recounted: true, released: Some(1), ..Known::default() };
    let decided = run(&life, &ctx, &inputs, &known);
    assert_eq!(decided.decision, LaunchDecision::AlreadyHeld { item: "feather".to_string(), held: 7 });
    assert_eq!(decided.emits, vec![Emit::Log("brain t_decide: n1 holds 7 feather but its listings locked them: 1 listing(s) withdrawn, 7 free now".to_string())]);
}

/// A coin target the wallet meets closes without a launch.
#[test]
fn a_met_coin_target_closes_the_node() {
    let mut node = Node::new("n1", NodeKind::HaveCoins { amount: 500 }, Vec::new(), 500);
    node.status = NodeStatus::Ready;
    let mut life = life_of(node, GoalKind::MakeMoney);
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.coins = 900;
    let inputs = inputs_with(Vec::new());
    let decided = run(&life, &ctx, &inputs, &Known::default());
    assert_eq!(decided.decision, LaunchDecision::CoinTargetMet { amount: 500, coins: 900 });
    let env = ApplyEnv { inputs: &inputs, quests_done: &[], reader: &Prior };
    let emits = apply_to_life(&mut life, &decided, &env);
    assert_eq!(emits, vec![Emit::Log("brain t_decide: n1 holds 900 coins >= 500, done".to_string())]);
    assert_eq!(life.nodes[0].status, NodeStatus::Done);
}

/// Tour 3 point 1 (live 2026-11-17 14:19 alice_knight `blocked at
/// resolve (full:gather:mining/mithril_ore) -> abandoned`): the full
/// method cools, the node stays open and ready (next method on the
/// next resolve); full again with the method cooled, it waits open.
#[test]
fn a_full_resolve_never_abandons_the_node() {
    use crate::launch_decide::{Boundary, Decided};
    let reason = "full:gather:mining/mithril_ore";
    let mut node = Node::new("n1", NodeKind::ReachLevel { skill: "mining".to_string(), level: 70 }, Vec::new(), 70);
    node.status = NodeStatus::Blocked { reason: reason.to_string() };
    let mut life = life_of(node.clone(), GoalKind::MaxSkills { skills: vec!["mining".to_string()], level: 70 });
    let inputs = inputs_with(Vec::new());
    let env = ApplyEnv { inputs: &inputs, quests_done: &[], reader: &Prior };
    let cooled = |reason: &str| Decided {
        position: 0,
        coin_share: None,
        boundary: Some(Boundary { node: node.clone(), memory: None, emits: Vec::new(), coin_share: None }),
        plan: None,
        emits: Vec::new(),
        decision: LaunchDecision::CooledWait { reason: reason.to_string() },
    };
    let emits = apply_to_life(&mut life, &cooled(reason), &env);
    assert_eq!(life.nodes[0].status, NodeStatus::Ready);
    assert_eq!(life.nodes[0].failures, 0);
    assert!(matches!(&emits[0], Emit::Log(line) if line.contains("gather:mining/mithril_ore cooled")), "{emits:?}");
    let keys = crate::cooldown::cooled_keys(&life.history, &|_| 0);
    assert!(keys.iter().any(|key| key == "gather:mining/mithril_ore"), "the method cooled");
    // Every method cooled: the node waits, open.
    life.nodes[0].status = NodeStatus::Blocked { reason: "cooled:gather:mining/mithril_ore".to_string() };
    let emits = apply_to_life(&mut life, &cooled("cooled:gather:mining/mithril_ore"), &env);
    assert_eq!(life.nodes[0].status, NodeStatus::Ready);
    assert!(matches!(&emits[0], Emit::Log(line) if line.contains("every method cooled")), "{emits:?}");
}

/// A market buy parks the node for its fill; a refusal counts a failure.
#[test]
fn a_market_buy_parks_or_fails_the_node() {
    let mut node = Node::new("n1", NodeKind::ObtainItems { item: "feather".to_string(), count: 5 }, Vec::new(), 5);
    node.status = NodeStatus::Active;
    let mut life = life_of(node, GoalKind::Obtain { item: "feather".to_string(), count: 5 });
    life.active = Some("n1".to_string());
    settle_market_buy(&mut life, 0, 2, Ok(()), 10);
    assert_eq!(life.nodes[0].status, NodeStatus::Ready);
    assert_eq!(life.nodes[0].progress_current, 2);
    assert_eq!(life.history.last().map(|entry| entry.outcome.as_str()), Some("awaiting_fill"));
    settle_market_buy(&mut life, 0, 2, Err("no seller".to_string()), 11);
    assert_eq!(life.nodes[0].failures, 1);
    assert_eq!(life.history.last().map(|entry| entry.outcome.as_str()), Some("failed"));
}
