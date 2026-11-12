//! Offline "30 accounts" bench for the brain loop: real graphs from
//! `expand_goal`, the real resolve/map/lifecycle/verdict code, and
//! random run outcomes (success, runtime failure, contract error, typed
//! block, operator stop, lost report). Checks what the live lot broke:
//! - liveness: an unfinished graph always has a running or ready node;
//! - termination: every account ends finished or stuck (operator) within
//!   a bounded number of launches (no relaunch loop, no ghost retries);
//! - every launched method maps to a registered script.

use lifescape_json::Value;
use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::{
    expand_goal, graph_state, map_method, mark_launched, normalize, record_block, record_contract_error, record_failure, resolve_ready, Archetype,
    GoalKind, GraphState, Life, NodeKind, NodeStatus, Profile,
};

use super::{verdict, Verdict};

struct Dice(u64);

impl Dice {
    fn roll(&mut self, bound: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % bound
    }
}

fn goals() -> Vec<GoalKind> {
    let skills = |names: &[&str], level: u8| GoalKind::MaxSkills { skills: names.iter().map(|name| name.to_string()).collect(), level };
    vec![
        // Fishing past 35 needs Karamja tuna: no bank reachable without a
        // boat in the site model yet (`karamja_tuna_is_unworkable_until_boats`).
        skills(&["fishing", "cooking"], 30),
        skills(&["fishing"], 20),
        skills(&["mining"], 30),
        skills(&["woodcutting"], 30),
        skills(&["attack", "strength", "defence"], 20),
        skills(&["cooking"], 20),
        skills(&["smithing"], 20),
        skills(&["crafting"], 20),
        skills(&["firemaking"], 15),
        skills(&["magic"], 10),
        GoalKind::Obtain { item: "copper_ore".to_string(), count: 100 },
        GoalKind::Obtain { item: "feather".to_string(), count: 500 },
        GoalKind::Obtain { item: "logs".to_string(), count: 50 },
        GoalKind::CompleteQuest { quest: "cook".to_string() },
        GoalKind::CompleteQuest { quest: "sheep".to_string() },
        GoalKind::MakeMoney,
        GoalKind::GetRich { merchant: Some(false) },
        GoalKind::EquipBest { style: "melee".to_string() },
    ]
}

fn life_for(index: usize, goal: GoalKind) -> Life {
    let ironman = index % 7 == 3;
    let profile = Profile::new(ironman, 0.6, 0.3, index as u64 + 11, Archetype::Producer);
    let nodes = expand_goal(&goal, &profile);
    Life {
        schema: 1,
        account: format!("sim{index:02}"),
        created_at: 0,
        seed: index as u64 + 11,
        profile,
        goal,
        nodes,
        engagement: None,
        active: None,
        history: Vec::new(),
    }
}

fn assert_live(life: &Life, step: usize) {
    let unfinished = life.nodes.iter().any(|node| !matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned));
    if unfinished {
        assert!(
            life.active.is_some() || life.nodes.iter().any(|node| node.status == NodeStatus::Ready),
            "{} step {step}: unfinished graph with nothing to launch: {:?}",
            life.account,
            life.nodes.iter().map(|node| (node.id.clone(), node.status.as_str(), node.deps.clone())).collect::<Vec<_>>()
        );
    }
}

/// One brain launch pass plus the run it starts, folded like
/// `handle_report` does. Returns false when the graph is terminal.
fn step(life: &mut Life, dice: &mut Dice) -> bool {
    normalize(life);
    match graph_state(life) {
        GraphState::Finished | GraphState::Stuck { .. } => return false,
        GraphState::Running(_) => panic!("{}: nothing runs between passes in the bench", life.account),
        GraphState::Launchable => {}
    }
    let position = life.nodes.iter().position(|node| node.status == NodeStatus::Ready).expect("launchable has a ready node");
    let mut ctx = ReadyCtx::blind(0.6);
    ctx.ironman = life.profile.ironman;
    ctx.coins = [0, 25, 400][dice.roll(3) as usize];
    ctx.tools = if dice.roll(2) == 0 { vec!["net".to_string(), "bronze_axe".to_string(), "bronze_pickaxe".to_string()] } else { Vec::new() };
    resolve_ready(&mut life.nodes[position], &ctx);
    if let NodeStatus::Blocked { reason } = life.nodes[position].status.clone() {
        match reason.strip_prefix("missing:") {
            Some(need) => {
                record_block(life, position, &[need.to_string()]);
            }
            None => record_contract_error(life, position, &reason),
        }
        return true;
    }
    let Some(method) = life.nodes[position].method.clone() else {
        record_contract_error(life, position, "no method resolvable");
        return true;
    };
    let budget = match &life.nodes[position].kind {
        NodeKind::MakeMoney => Some((6000, 12000)),
        NodeKind::ReachLevel { .. } => Some((0, 18000)),
        _ => None,
    };
    let Some((script, _args)) = map_method(&method, &life.nodes[position], budget) else {
        record_contract_error(life, position, &format!("no-script: {method}"));
        return true;
    };
    assert!(crate::registry::script_known(&script), "{}: {method} maps to unregistered {script}", life.account);
    mark_launched(life, position);
    // The run's outcome.
    let roll = dice.roll(100);
    let typed = |kind: &str, message: &str| Some((kind.to_string(), message.to_string()));
    let decision = match roll {
        0..=44 => verdict("goal_reached", None, &[], true, false),
        45..=54 => verdict("budget_exhausted", None, &[], false, false),
        55..=74 => verdict("failed", typed("Failed", "fish failed at (0, 3086, 3227)"), &[], false, false),
        75..=77 => verdict("failed", typed("Usage", "bad cli"), &[], false, false),
        78..=84 => verdict("blocked", typed("Blocked", "missing"), &["small_fishing_net".to_string()], false, false),
        85..=89 => verdict("stopped", None, &[], false, false),
        90..=94 => verdict("failed", typed("Sdk", "travel failure NoRoute seg=None"), &[], false, false),
        _ => Verdict::Failure("no report: worker went idle without reporting (crash or restart)".to_string()),
    };
    match decision {
        Verdict::Done => {
            life.nodes[position].status = NodeStatus::Done;
            life.active = None;
        }
        Verdict::AwaitFill => panic!("bench never places market orders"),
        Verdict::EngagementEnd => {
            let mut counter = 0;
            lifescape_planner::replan_node(&mut life.nodes[position], &lifescape_planner::ReplanCause::EngagementEnd, &[], &mut counter);
            life.active = None;
        }
        Verdict::Stopped => {
            life.nodes[position].status = NodeStatus::Ready;
            life.nodes[position].method = None;
            life.active = None;
        }
        Verdict::Blocked(needs) => {
            record_block(life, position, &needs);
        }
        Verdict::Contract(text) => record_contract_error(life, position, &text),
        Verdict::Travel(text) | Verdict::Failure(text) => {
            record_failure(life, position, &text);
        }
    }
    true
}

#[test]
fn thirty_accounts_stay_live_and_terminate() {
    let goals = goals();
    let mut finished = 0;
    let mut stuck = 0;
    for index in 0..30 {
        let goal = goals[index % goals.len()].clone();
        let mut life = life_for(index, goal);
        let mut dice = Dice(0xA5A5_5A5A_1234_5678 ^ (index as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let mut launches = 0usize;
        loop {
            normalize(&mut life);
            assert_live(&life, launches);
            if !step(&mut life, &mut dice) {
                break;
            }
            launches += 1;
            // EngagementEnd and stops are uncounted retries; everything
            // else is bounded per node (3 failures, 3 blocks).
            assert!(launches < 5000, "{}: relaunch loop ({launches} launches)", life.account);
        }
        normalize(&mut life);
        match graph_state(&life) {
            GraphState::Finished => finished += 1,
            GraphState::Stuck { abandoned } => {
                stuck += 1;
                for id in abandoned {
                    let node = life.nodes.iter().find(|node| node.id == id).expect("abandoned node exists");
                    assert!(!node.last_error.is_empty(), "{}: {id} abandoned without a cause", life.account);
                }
            }
            other => panic!("{}: ended {other:?}", life.account),
        }
    }
    assert_eq!(finished + stuck, 30);
    assert!(finished > 0, "some accounts finish under 45% success");
    let _ = Value::Null;
}

/// The same bench with a perfect world: every goal finishes (no contract
/// error hides in the real graphs: every resolved method maps to a
/// registered script, every node becomes launchable in turn).
#[test]
fn perfect_world_finishes_every_goal() {
    for (index, goal) in goals().into_iter().enumerate() {
        let mut life = life_for(index, goal);
        let mut launches = 0;
        loop {
            normalize(&mut life);
            assert_live(&life, launches);
            match graph_state(&life) {
                GraphState::Finished => break,
                GraphState::Stuck { abandoned } => {
                    let causes: Vec<String> = life
                        .nodes
                        .iter()
                        .filter(|node| abandoned.contains(&node.id))
                        .map(|node| format!("{}={} ({:?})", node.id, node.last_error, node.kind))
                        .collect();
                    panic!("{} goal {:?} stuck in a perfect world: {causes:?}", life.account, life.goal);
                }
                GraphState::Running(_) => unreachable!(),
                GraphState::Launchable => {}
            }
            let position = life.nodes.iter().position(|node| node.status == NodeStatus::Ready).expect("ready");
            let mut ctx = ReadyCtx::blind(0.6);
            ctx.ironman = life.profile.ironman;
            ctx.coins = 100_000;
            ctx.tools = ["net", "fishing_rod", "fishing_bait", "fly_fishing_rod", "feather", "harpoon", "lobster_pot", "bronze_axe", "bronze_pickaxe", "shortbow", "bronze_arrow", "mindrune", "airrune", "shears", "bucket_empty"]
                .iter()
                .map(|tool| tool.to_string())
                .collect();
            resolve_ready(&mut life.nodes[position], &ctx);
            let method = life.nodes[position].method.clone();
            match method.as_deref().and_then(|method| map_method(method, &life.nodes[position], None)) {
                Some((script, _)) => {
                    assert!(crate::registry::script_known(&script), "{}: {method:?} maps to unregistered {script}", life.account);
                    mark_launched(&mut life, position);
                    life.nodes[position].status = NodeStatus::Done;
                    life.active = None;
                }
                None => {
                    let text = format!("perfect world: {:?} resolved {method:?} with no script", life.nodes[position].kind);
                    record_contract_error(&mut life, position, &text);
                }
            }
            launches += 1;
            assert!(launches < 500, "{}: no progress", life.account);
        }
    }
}

/// Known gap, stated: tuna (fishing 35+) only exists at Karamja in F2P,
/// whose water has no bank within the trek limit (the boat is not in the
/// site model). The brain blocks it as unworkable, typed, instead of
/// sending bots there blind.
#[test]
fn karamja_tuna_is_unworkable_until_boats() {
    assert_eq!(lifescape_planner::min_combat_for_method("gather:fishing/raw_tuna"), lifescape_gamedata::danger::NEVER);
    assert_eq!(lifescape_planner::pick_site("gather:fishing/raw_tuna", 126, None), lifescape_planner::SitePick::Needs(lifescape_gamedata::danger::NEVER));
}
