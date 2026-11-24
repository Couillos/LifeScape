//! Equivalence of the life upkeep and the graph state (lot 4 of the launch
//! refactor): the reference (`brain_upkeep_reference`, the rules as they
//! were) against the real path (`brain_upkeep_drive`: planner decisions,
//! applied to the life, effects through `UpkeepFx`) over the same recording
//! fake, on lives crossed with generated inputs. Compared: the way the
//! pass flows (`dirty`, quoting, stop and save), the life it leaves (nodes,
//! statuses, history, memory, ambition and milestones, engagement), and
//! every read, effect, phase label and log line in order. Fixtures under
//! `tests/fixtures/lives` run in CI; the real lives (`LIFESCAPE_LIVES_DIR`)
//! run with `--ignored`, copied to a temporary directory first (the source
//! is never written).
//!
//! Lot 5: the one-time D3 mark and A4 rebalance left the loop. The real
//! path is `brain_ambition::migrate_life` applied offline to the life, then the
//! loop without the rule, compared with the reference, which still runs
//! both in the loop (trace entries of those two rules removed from the
//! reference, their D3 line compared with the migration's, `dirty` equal
//! once the migration's save is counted). No A4 row is pending in the real
//! marker (71 of 71 applied), so the fake world holds none. Every scenario
//! is run for two passes in a row.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lifescape_planner::ambition::{Ambition, Milestone, MilestoneStatus, Revision, Target};
use lifescape_planner::serve_value::ServeVerdict;
use lifescape_planner::upkeep::ServeFacts;
use lifescape_planner::upkeep_graph::RouteFacts;
use lifescape_planner::{Archetype, GoalKind, HistoryEntry, Life, MakerRole, Node, NodeKind, NodeStatus};

use lifescape_json::{object, Value};

use crate::brain_launch_pass::Flow;
use crate::brain_raw_world_tests::{object_id, stack_value, wall_now, xp_of_level, RawWorld, Seed, TestMarket};
use crate::brain_upkeep_fx::{LiveUpkeepFx, UpkeepFx};
use lifescape_planner::upkeep_graph::RescueReader;
use crate::brain_upkeep_drive as drive;
use crate::brain_upkeep_reference as reference;
use crate::brain_upkeep_reference::{Ev, RecUpkeep, World};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound.max(1)
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

fn entry(node: &str, outcome: &str, ended: i64) -> HistoryEntry {
    HistoryEntry { node: node.to_string(), started: ended, ended, outcome: outcome.to_string() }
}

fn demand_milestone(id: &str, item: &str) -> Milestone {
    Milestone {
        id: id.to_string(),
        label: format!("produce {item}"),
        target: Target::Produce { item: item.to_string(), count: 100, skill: "crafting".to_string() },
        status: MilestoneStatus::Pending,
        progress: (0, 100),
        blocked_count: 0,
        origin: format!("demand:{item}"),
    }
}

/// What a scenario starts from.
#[derive(Clone)]
struct Scenario {
    life: Life,
    world: World,
    dirty: bool,
}

const ITEMS: [&str; 6] = ["mithril_ore", "iron_ore", "uncut_sapphire", "diamond", "logs", "bronze_bar"];

/// The world of one scenario, read the way the daemon reads it: a `Job` with
/// a live JSON state and a test market service (resting orders, other
/// bots' asks), then `LiveUpkeepFx`'s own readers fill what the fake
/// answers from the raw world: the clock (`clock`), the levels
/// (`level_of`), the wallet (`coins`), the holdings (`holdings_count`), the
/// resting orders (`orders`), a maker's slice (`slice_items`), the draw
/// stamp (`stamp`) and each node's market facts (`facts`: `ready_ctx` +
/// `fill_ctx`). What no single raw world holds stays scripted: the filed
/// operator switch, the draws and ambition givings, the fleet's piles, the
/// time-value service's facts and verdicts.
fn build_world(life: &Life, template: &Ambition, rng: &mut Rng, market: &TestMarket, account: &str) -> World {
    let skill_level = *rng.pick(&[1u8, 15, 40, 99]);
    let skills: Vec<Value> = (0u8..23).map(|stat| object(vec![("id", Value::Int(i64::from(stat))), ("xp", Value::Int(xp_of_level(skill_level))), ("level", Value::Int(i64::from(skill_level)))])).collect();
    let mut verdicts = Vec::new();
    for item in ITEMS {
        verdicts.push((
            item.to_string(),
            match rng.below(4) {
                0 => ServeVerdict::Pays { per_tick: 5.0 },
                1 | 2 => ServeVerdict::Unpaying { per_tick: 0.01, time_value: 0.5 },
                _ => ServeVerdict::NoData,
            },
        ));
    }
    let slice = rng.below(5) as u32 + 1;
    let market_up = rng.chance(75);
    let coins = *rng.pick(&[0i64, 50, 2_000, 500_000, 5_000_000]);
    let (mut inv, mut bank, mut seed) = (Vec::new(), Vec::new(), Seed::default());
    if let Some(entry) = stack_value("coins", coins).filter(|_| coins > 0) {
        inv.push(entry);
    }
    seed.coins = coins;
    for item in ITEMS {
        let units = *rng.pick(&[0i64, 3, 400]);
        if let (true, Some(entry), Some(id)) = (units > 0, stack_value(item, units), object_id(item)) {
            if rng.chance(50) { inv.push(entry) } else { bank.push(entry) }
            seed.holdings.push((id, units));
        }
    }
    // The bot's resting orders (a maker's quotes among them), some dead.
    for index in 0..rng.below(6) {
        let id = object_id(ITEMS[rng.below(3) as usize]).unwrap_or(1);
        let buy = rng.chance(50);
        seed.orders.push((buy, id, 1 + index as i64, if buy { 1 } else { 4_000 }, rng.chance(50), rng.chance(80)));
    }
    // Other sellers' asks on the nodes' items (the best ask a route reads).
    for node in &life.nodes {
        if let (NodeKind::ObtainItems { item, .. }, true) = (&node.kind, rng.chance(50)) {
            if let Some(id) = object_id(item) {
                seed.asks.push((id, 50, *rng.pick(&[5i64, 80, 4_000])));
            }
        }
    }
    let live = object(vec![
        ("skills", Value::Arr(skills)),
        ("inv", Value::Arr(inv)),
        ("bank", Value::Arr(bank)),
        ("worn", Value::Arr(Vec::new())),
        ("position", Value::Arr(vec![Value::Int(0), Value::Int(3222), Value::Int(3218)])),
        ("inv_streamed", Value::Bool(true)),
        ("bank_streamed", Value::Bool(true)),
    ]);
    let raw = RawWorld::new(account, live, market_up.then_some(market));
    raw.seed(&seed);
    let (clock, levels, wallet, held, orders, slice_items, stamp, routes) = raw.run(|| {
        let fx = LiveUpkeepFx::new(&raw.orch, &raw.job, &raw.live);
        let levels: Vec<(String, u8)> = (0u8..23).map(|stat| lifescape_sdk::provenance::skill_name(stat).to_string()).map(|name| { let level = fx.level_of(&name); (name, level) }).collect();
        let held: Vec<(String, i64)> = ITEMS.iter().map(|item| (item.to_string(), crate::brain_plan::holdings_count(&raw.job, item))).collect();
        let routes: Vec<(String, RouteFacts)> = life.nodes.iter().map(|node| (node.id.clone(), fx.facts(life, node))).collect();
        (fx.clock(), levels, fx.coins(), held, fx.orders(), fx.slice_items(slice), fx.stamp(), routes)
    });
    raw.clear(&seed);
    World {
        clock,
        levels,
        coins: wallet,
        held,
        pending: rng.chance(25).then(|| rng.pick(&["alcher", "natural", "cook", "fisher", "crafter", "pirate", "market_maker", "firemaker", "smith"]).to_string()),
        stamp,
        assign_ok: rng.chance(70),
        draw_ok: rng.chance(60),
        redraw_changed: rng.chance(30),
        orders,
        cancel_ok: rng.chance(80),
        slice: slice_items,
        // Every ambition a draw gives carries its mark (`mark_rule`): the fake's
        // draw too, or the old loop's D3 would mark what no real draw leaves bare.
        ambition: rng.chance(85).then(|| Ambition { draw_rule: if template.draw_rule.is_empty() { "need".to_string() } else { template.draw_rule.clone() }, ..template.clone() }),
        // Lot 5: the real marker holds no pending row (71 of 71 applied).
        rebalance: None,
        surplus: vec![("copper_ore".to_string(), 900.0), ("raw_shrimp".to_string(), 3000.0)],
        serve: rng.chance(75).then(|| ServeFacts { margin: *rng.pick(&[1.0, 50.0, 5000.0]), per_tick: *rng.pick(&[0.0, 0.0005, 0.05]), time_value: *rng.pick(&[0.1, 2.0]) }),
        verdicts,
        reengage: rng.chance(50),
        routes,
    }
}

/// A bid goal that waited on refused sites, a side demand goal, a demand
/// milestone goal, a goal an earlier ambition drew.
fn set_goal_shape(life: &mut Life, rng: &mut Rng) {
    let item = rng.pick(&ITEMS).to_string();
    let node_ids: Vec<String> = life.nodes.iter().map(|node| node.id.clone()).collect();
    match rng.below(6) {
        0 | 1 => {
            // A bid answer, its node waiting on a cooled method for a while.
            life.goal = GoalKind::Produce { item: item.clone(), count: 5 };
            life.goal_origin = Some(format!("bid:{item}"));
            life.history.push(entry("goal", &format!("next:produce {item} 5 (bid:{item})"), wall_now() - 9_000));
            if rng.chance(25) {
                life.history.push(entry("goal", lifescape_planner::bid_reach::PLANNED_RULE_TAG, wall_now() - 8_999));
            }
            if rng.chance(15) {
                life.history.push(entry("goal", &format!("{}{item}", lifescape_planner::bid_trade::WAIT_ANSWER_TAG), wall_now() - 8_998));
            }
            if !node_ids.is_empty() {
                let node = rng.pick(&node_ids).clone();
                let age = *rng.pick(&[200i64, 2_000, 5_000, 8_000]);
                life.history.push(entry(&node, "launched", wall_now() - age - 10));
                life.history.push(entry(&node, "cool_wait:gather:mining/mithril_ore", wall_now() - age));
                life.history.push(entry("cool", "cool:gather:mining/mithril_ore|6000|full", wall_now() - 100));
            }
        }
        2 => {
            life.goal = GoalKind::Produce { item: item.clone(), count: 50 };
            life.goal_origin = Some(format!("side:demand:{item}"));
            life.history.push(entry("goal", &format!("next:produce {item} 50 (side:demand:{item})"), wall_now() - 5_000));
        }
        3 => {
            // A demand milestone of the ambition runs as the goal.
            if let Some(ambition) = life.ambition.as_mut() {
                ambition.milestones.insert(0, demand_milestone("m9001", &item));
                if rng.chance(50) {
                    ambition.milestones.insert(1, demand_milestone("m9002", rng.pick(&ITEMS)));
                }
                life.goal = GoalKind::Produce { item: item.clone(), count: 100 };
                life.goal_origin = Some("m9001".to_string());
                life.history.push(entry("goal", &format!("next:produce {item} 100"), wall_now() - 3_000));
            }
        }
        4 => {
            // A goal drawn before the ambition last changed.
            life.goal_origin = None;
            life.history.push(entry("goal", "next:mining 40", wall_now() - 500_000));
            if let Some(ambition) = life.ambition.as_mut() {
                ambition.revisions.push(Revision { tick: 1, at_secs: wall_now() - 1_000, from: "x".to_string(), to: ambition.kind.clone(), why: "test".to_string() });
            }
        }
        _ => {}
    }
}

fn set_graph_shape(life: &mut Life, rng: &mut Rng) {
    match rng.below(7) {
        0 => life.nodes.iter_mut().for_each(|node| node.status = NodeStatus::Done),
        1 | 2 | 3 => {
            // Nothing launchable, abandoned nodes: a stuck graph, an obtain
            // on the failed path the market may sell.
            for node in life.nodes.iter_mut() {
                if node.status != NodeStatus::Done {
                    node.status = NodeStatus::Abandoned;
                }
            }
            let item = rng.pick(&["logs", "bronze_bar", "iron_ore", "coal", "raw_shrimp"]).to_string();
            let error = match rng.below(4) {
                0 => format!("market order for 5 {item} unfilled after 6000 ticks (held 0, 1 cancelled)"),
                1 => "dependency n1 abandoned".to_string(),
                _ => "blocked at resolve (missing:coins:500)".to_string(),
            };
            let mut obtain = Node::new("n9101", NodeKind::ObtainItems { item: item.clone(), count: 5 }, Vec::new(), 5);
            obtain.status = NodeStatus::Abandoned;
            obtain.last_error = error;
            obtain.failures = 3;
            obtain.alternatives.clear();
            life.nodes.push(obtain);
            life.history.push(entry("n9101", "abandoned", wall_now() - 20));
            if rng.chance(20) {
                life.history.push(entry("n9101", lifescape_planner::MARKET_RESCUE_TAG, wall_now() - 10));
            }
            if rng.chance(30) {
                // A skill goal the live levels may meet: Finished, not stuck.
                life.goal = GoalKind::MaxSkills { skills: vec!["mining".to_string()], level: *rng.pick(&[5u8, 30, 70]) };
            } else if rng.chance(20) {
                life.goal = GoalKind::Produce { item, count: 5 };
            }
        }
        4 => {
            // One node runs.
            if let Some(node) = life.nodes.first_mut() {
                life.active = Some(node.id.clone());
                node.status = NodeStatus::Active;
            }
        }
        5 => {
            for node in life.nodes.iter_mut().filter(|node| node.status == NodeStatus::Ready) {
                node.status = NodeStatus::Blocked { reason: "missing:coins:5".to_string() };
            }
        }
        _ => {}
    }
    // The spell as the history keeps it.
    match rng.below(7) {
        0 => life.history.push(entry("graph", "stuck", wall_now() - 100)),
        1 => life.history.push(entry("graph", "stuck", wall_now() - 4_000)),
        2 => life.history.push(entry("graph", "stuck", wall_now() - 100_000)),
        3 => life.history.push(entry("graph", "final", wall_now() - 50)),
        4 => life.history.push(entry("graph", "revived", wall_now() - 50)),
        _ => {}
    }
    match rng.below(6) {
        0 => life.history.push(entry("goal", "done:mining", wall_now() - 60)),
        1 => life.history.push(entry("goal", &format!("{}mining@6000:contract error (node n1, x)", lifescape_planner::goals::STUCK_TAG), wall_now() - 60)),
        2 => life.history.push(entry("goal", "next:produce logs 5", wall_now() - 60)),
        _ => {}
    }
}

fn build(base: &Life, template: &Ambition, rng: &mut Rng, graph: bool, market: &TestMarket, account: &str) -> Scenario {
    let mut life = base.clone();
    if rng.chance(10) {
        life.ambition = None;
    } else if life.ambition.is_none() {
        life.ambition = Some(template.clone());
    }
    if rng.chance(10) {
        life.profile.ironman = true;
    }
    if rng.chance(5) {
        life.account = "Zezima".to_string();
    }
    if rng.chance(12) {
        let mut role = MakerRole::new(2, 8, 100_000);
        if rng.chance(35) {
            role.phase = lifescape_planner::MakerPhase::Quoting;
        }
        life.profile.archetype = Archetype::MarketMaker(role);
        // The maker's constructor marks its ambition `operator`: a maker
        // never holds a bare need ambition (D3 never reviews it).
        if let Some(ambition) = life.ambition.as_mut() {
            ambition.draw_rule = lifescape_planner::OPERATOR_RULE.to_string();
        }
    }
    // Old-rule ambition: unmarked, drawn by need (never a maker's).
    if rng.chance(20) && lifescape_planner::maker_role(&life).is_none() {
        if let Some(ambition) = life.ambition.as_mut() {
            ambition.draw_rule.clear();
            ambition.why = if rng.chance(50) { "need".to_string() } else { "need:copper_ore".to_string() };
        }
    }
    // A stale quota the catalogue no longer offers: the ladder revised.
    if rng.chance(15) {
        if let Some(ambition) = life.ambition.as_mut() {
            ambition.milestones.push(Milestone { origin: "ladder".to_string(), ..demand_milestone("m9500", "zzz_removed_item") });
        }
    }
    if rng.chance(15) {
        life.memory.floor = Some(life.memory.last_number.saturating_sub(rng.below(20) as usize).max(1));
    }
    // A role-switch ban: lifted at once.
    if rng.chance(12) {
        life.history.push(entry("goal", &format!("{}smithing@6000:contract error (node n1, {})", lifescape_planner::goals::STUCK_TAG, lifescape_planner::OPERATOR_ROLE_CAUSE), wall_now() - 30));
    }
    if rng.chance(70) {
        set_goal_shape(&mut life, rng);
    }
    if graph || rng.chance(20) {
        set_graph_shape(&mut life, rng);
    } else if rng.chance(18) {
        if let Some(node) = life.nodes.first() {
            life.active = Some(node.id.clone());
        }
    }
    let world = build_world(&life, template, rng, market, account);
    Scenario { life, world, dirty: rng.chance(30) }
}

#[derive(Debug, Clone, PartialEq)]
struct Outcome {
    /// D3 lines of the offline migration (the reference prints them in its loop).
    migrated: Vec<String>,
    life: Life,
    dirty: bool,
    quoting: bool,
    flow: Option<Flow>,
    trace: Vec<Ev>,
}

fn run_upkeep_reference(s: &Scenario) -> Outcome {
    let (mut life, mut dirty) = (s.life.clone(), s.dirty);
    let fx = RecUpkeep::new(s.world.clone());
    let quoting = reference::ref_operator_and_maker(&fx, &mut life, &mut dirty);
    reference::ref_ambition_upkeep(&fx, &mut life, &mut dirty, quoting);
    Outcome { migrated: Vec::new(), life, dirty, quoting, flow: None, trace: fx.trace.into_inner() }
}

fn run_upkeep_new(s: &Scenario) -> Outcome {
    let (mut life, mut dirty) = (s.life.clone(), s.dirty);
    // `orchestrator migrate-lives`, offline, before the daemon starts.
    let migrated: Vec<String> = crate::brain_ambition::migrate_life(&mut life).into_iter().collect();
    let fx = RecUpkeep::new(s.world.clone());
    let quoting = drive::operator_and_maker(&fx, &mut life, &mut dirty);
    drive::ambition_upkeep(&fx, &mut life, &mut dirty, quoting);
    Outcome { migrated, life, dirty, quoting, flow: None, trace: fx.trace.into_inner() }
}

fn run_graph_reference(s: &Scenario) -> Outcome {
    let mut life = s.life.clone();
    let fx = RecUpkeep::new(s.world.clone());
    let flow = reference::ref_graph_state(&fx, &mut life, s.dirty);
    Outcome { migrated: Vec::new(), life, dirty: s.dirty, quoting: false, flow: Some(flow), trace: fx.trace.into_inner() }
}

fn run_graph_new(s: &Scenario) -> Outcome {
    let mut life = s.life.clone();
    let fx = RecUpkeep::new(s.world.clone());
    let flow = drive::graph_state(&fx, &mut life, s.dirty);
    Outcome { migrated: Vec::new(), life, dirty: s.dirty, quoting: false, flow: Some(flow), trace: fx.trace.into_inner() }
}

/// The maker's switch alone: while makers are unused the retirement takes a
/// maker first in the pass, so the switch is compared rule to rule.
fn run_switch(s: &Scenario, new: bool) -> Outcome {
    let mut life = s.life.clone();
    let mut role = MakerRole::new(2, 8, 100_000);
    role.phase = lifescape_planner::MakerPhase::Earning;
    life.profile.archetype = Archetype::MarketMaker(role);
    let fx = RecUpkeep::new(s.world.clone());
    let changed = if new { drive::maybe_switch(&fx, &mut life) } else { reference::ref_maybe_switch(&fx, &mut life) };
    Outcome { migrated: Vec::new(), life, dirty: changed, quoting: lifescape_planner::quoting(&s.life), flow: None, trace: fx.trace.into_inner() }
}

/// The reference's trace without the two rules that left the loop (their
/// phase labels, the A4 row read) and its D3 lines, which are the
/// migration's.
fn without_one_time_rules(reference: &Outcome) -> (Vec<Ev>, Vec<String>) {
    let mut d3 = Vec::new();
    let mut trace = Vec::new();
    for event in &reference.trace {
        match event {
            Ev::Timed("ambition.review" | "ambition.rebalance") => {}
            Ev::Fx(text) if text == "rebalance row" => {}
            Ev::Log(line) if line.contains("marked need (D3)") => d3.push(line.clone()),
            other => trace.push(other.clone()),
        }
    }
    (trace, d3)
}

fn same(label: &str, s: &Scenario, reference: &Outcome, new: &Outcome) {
    let (ref_trace, ref_d3) = without_one_time_rules(reference);
    let reference = &Outcome { trace: ref_trace, ..reference.clone() };
    // The migration marks the old-rule ambition the life holds at rest. The
    // loop marked it too, unless the pass replaced the ambition before the
    // mark (an operator switch, a maker's retirement or switch): then the
    // mark is moot, the lives compared below are equal all the same.
    if ref_d3 != new.migrated {
        assert!(ref_d3.is_empty() && !new.migrated.is_empty(), "{label}: the D3 mark of the loop is the migration's: {ref_d3:?} vs {:?}", new.migrated);
        assert_ne!(reference.life.ambition, s.life.ambition, "{label}: the loop skipped a mark the migration makes, the ambition was not replaced");
    }
    // The migration saved the life offline: the loop that followed has
    // nothing to save for it.
    assert!(reference.dirty == new.dirty || (!ref_d3.is_empty() && reference.dirty && !new.dirty), "{label}: dirty");
    assert_eq!(reference.quoting, new.quoting, "{label}: quoting");
    assert_eq!(reference.flow, new.flow, "{label}: flow");
    if reference.trace != new.trace {
        let at = reference.trace.iter().zip(&new.trace).position(|(left, right)| left != right).unwrap_or(reference.trace.len().min(new.trace.len()));
        panic!("{label}: trace differs at {at}\n ref: {:?}\n new: {:?}", reference.trace.iter().skip(at.saturating_sub(1)).take(4).collect::<Vec<_>>(), new.trace.iter().skip(at.saturating_sub(1)).take(4).collect::<Vec<_>>());
    }
    if reference.life != new.life {
        panic!(
            "{label}: life differs\n ref history: {:?}\n new history: {:?}\n ref ambition==new: {}\n nodes equal: {}\n start history len {}",
            &reference.life.history[s.life.history.len().min(reference.life.history.len())..],
            &new.life.history[s.life.history.len().min(new.life.history.len())..],
            reference.life.ambition == new.life.ambition,
            reference.life.nodes == new.life.nodes,
            s.life.history.len()
        );
    }
}

/// What the passes did, by rule: how many scenarios reached each outcome.
#[derive(Default)]
struct Tally {
    upkeep: usize,
    graph: usize,
    counts: BTreeMap<&'static str, usize>,
}

impl Tally {
    fn merge(&mut self, other: Tally) {
        self.upkeep += other.upkeep;
        self.graph += other.graph;
        for (key, count) in other.counts {
            *self.counts.entry(key).or_insert(0) += count;
        }
    }
}

type Probe = (&'static str, fn(&str) -> bool);

const LOG_PROBES: [Probe; 39] = [
    ("operator.lift_bans", |line| line.contains("(an operator role switch caused it)")),
    ("operator.dropped_protected", |line| line.contains("dropped (protected account")),
    ("operator.natural_refused_maker", |line| line.contains("natural refused (protected account or market maker)")),
    ("operator.natural_not_given", |line| line.contains("natural balance not given")),
    ("operator.natural_given", |line| line.contains("ambition -> natural balance (operator)")),
    ("operator.natural_first_goal", |line| line.contains(": first goal ")),
    ("operator.trade_not_given", |line| line.contains("not given (nothing buildable or refused)")),
    ("operator.trade_given", |line| line.contains("ambition -> ") && line.contains(" (operator), ") && !line.contains("natural balance") && !line.contains("alcher")),
    ("operator.trade_first_goal", |line| line.contains("'s first goal ") && !line.contains("alcher's")),
    ("operator.alcher_given", |line| line.contains("ambition -> alcher (operator)")),
    ("operator.alcher_refused", |line| line.contains("operator ambition alcher refused")),
    ("operator.alcher_first_goal", |line| line.contains("alcher's first goal")),
    ("operator.alcher_no_goal", |line| line.contains("alcher drew no goal yet")),
    ("maker.retired", |line| line.contains("market maker retired")),
    ("maker.switched", |line| line.contains("quoting for good")),
    ("bid.blocked_given_back", |line| line.contains("ticks on refused sites")),
    ("bid.off_trade_given_back", |line| line.contains("given back: off the")),
    ("bid.beyond_reach_given_back", |line| line.contains("(drawn under the old rule)")),
    ("serve.unpaying_given_back", |line| line.contains("units a tick <")),
    ("milestone.set_aside", |line| line.contains("set aside, ambition")),
    ("milestone.running_goal_given_back", |line| line.contains(": goal of ") && line.contains(" given back, ")),
    ("stale.goal_replaced", |line| line.contains("replaced for the")),
    ("stale.work_abandoned", |line| line.contains("open nodes of earlier goals abandoned")),
    ("review.need_mark", |line| line.contains("marked need (D3)")),
    ("catalog.milestones_dropped", |line| line.contains("ladder (catalogue revised)")),
    ("catalog.supply_reaimed", |line| line.contains("re-aimed by the fleet's supply")),
    ("catalog.running_closed", |line| line.contains("goal of a dropped milestone closed")),
    ("draw.next_goal_after_give_back", |line| line.contains("next goal ") && line.contains("(after ")),
    ("draw.nothing_drawable", |line| line.contains("nothing drawable")),
    ("graph.spell_opened", |line| line.contains("graph stuck, nothing left to run")),
    ("graph.spell_over", |line| line.contains("stuck spell over")),
    ("graph.revived", |line| line.contains(": revive ") && !line.contains("revive cap")),
    ("graph.revive_cap", |line| line.contains("revive cap reached")),
    ("graph.rescue_market", |line| line.contains("instead of banning")),
    ("graph.rescue_produce", |line| line.contains("produced instead of bought again")),
    ("graph.no_market_route", |line| line.contains("no market route for")),
    ("graph.goal_done_next", |line| line.contains(": goal done -> next goal")),
    ("graph.goal_stuck_next", |line| line.contains(": goal stuck -> next goal")),
    ("graph.no_goal_left", |line| line.contains("no goal left to draw")),
];

const FX_PROBES: [(&str, &str); 8] = [
    ("fx.operator_filed_switch_taken", "take pending"),
    ("fx.cancel_orders", "cancel "),
    ("fx.free_board", "free board"),
    ("fx.redraw_neediest", "redraw neediest"),
    ("fx.ensure_gave_ambition", "ensure"),
    ("fx.reengage", "reengage"),
    ("fx.succeed_goal_stuck", "succeed goal stuck=true"),
    ("fx.close_goal_finished", "close goal"),
];

fn tally(out: &Outcome, tally: &mut Tally) {
    let mut bump = |key: &'static str| *tally.counts.entry(key).or_insert(0) += 1;
    // The D3 lines of the offline migration.
    for line in &out.migrated {
        for (name, probe) in LOG_PROBES {
            if probe(line) {
                bump(name);
            }
        }
    }
    for event in &out.trace {
        match event {
            Ev::Log(line) => {
                for (name, probe) in LOG_PROBES {
                    if probe(line) {
                        bump(name);
                    }
                }
            }
            Ev::Fx(text) => {
                for (name, needle) in FX_PROBES {
                    if text.contains(needle) {
                        bump(name);
                    }
                }
            }
            Ev::Timed(label) => bump(match *label {
                "rescue.market" => "phase.rescue.market",
                "stuck.on_stuck" => "phase.stuck.on_stuck",
                "succession.redraw" => "phase.succession.redraw",
                _ => "phase.other",
            }),
            Ev::Read(_) => {}
        }
    }
    if let Some(flow) = out.flow {
        bump(match flow {
            Flow::Continue => "flow.continue",
            Flow::Stop(crate::brain_launch_pass::Save::None) => "flow.stop_save_none",
            Flow::Stop(crate::brain_launch_pass::Save::IfDirty) => "flow.stop_save_if_dirty",
            Flow::Stop(crate::brain_launch_pass::Save::Always) => "flow.stop_save_always",
            Flow::Stop(crate::brain_launch_pass::Save::Silent) => "flow.stop_save_silent",
        });
    } else {
        if out.dirty {
            bump("upkeep.dirty");
        }
        if out.quoting {
            bump("upkeep.quoting_maker");
        }
    }
}

fn template_of(lives: &[Life]) -> Ambition {
    lives.iter().find_map(|life| life.ambition.clone()).expect("a life with an ambition")
}

fn run_life(life: &Life, template: &Ambition, variants: usize, seed: u64, tally_out: &mut Tally) {
    let market = TestMarket::open(&format!("up_{}", life.account));
    for variant in 0..variants {
        for graph in [false, true] {
            let mut rng = Rng(seed ^ (variant as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (u64::from(graph) << 40) ^ (life.seed.rotate_left(7)));
            rng.next();
            let account = format!("{}up_{}_{variant}_{graph}", crate::brain_commerce::PROBE_PREFIX, life.account);
            let scenario = build(life, template, &mut rng, graph, &market, &account);
            let label = format!("{} variant {variant} graph={graph}", life.account);
            if !graph && variant % 4 == 0 {
                let mut switch_scenario = scenario.clone();
                switch_scenario.world.coins = *rng.pick(&[0i64, 99_999, 100_000, 5_000_000]);
                let (reference, new) = (run_switch(&switch_scenario, false), run_switch(&switch_scenario, true));
                same(&format!("{label} switch"), &switch_scenario, &reference, &new);
                tally_out.upkeep += 1;
                tally(&new, tally_out);
            }
            if graph {
                let (reference, new) = (run_graph_reference(&scenario), run_graph_new(&scenario));
                same(&label, &scenario, &reference, &new);
                tally_out.graph += 1;
                tally(&new, tally_out);
            } else {
                let (reference, new) = (run_upkeep_reference(&scenario), run_upkeep_new(&scenario));
                same(&label, &scenario, &reference, &new);
                tally_out.upkeep += 1;
                tally(&new, tally_out);
                // The pass after: from the life both left (equal), same world.
                let next = Scenario { life: new.life.clone(), world: scenario.world.clone(), dirty: false };
                let (reference, new) = (run_upkeep_reference(&next), run_upkeep_new(&next));
                assert!(new.migrated.is_empty(), "{label}: the migration left a D3 mark to make");
                same(&format!("{label} second pass"), &next, &reference, &new);
                tally_out.upkeep += 1;
                tally(&new, tally_out);
            }
        }
    }
}

fn load_dir(dir: &Path) -> Vec<Life> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir).expect("lives dir").flatten().map(|entry| entry.path()).filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json")).collect();
    paths.sort();
    paths
        .iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(path).ok()?;
            lifescape_planner::model::life_from_value(&lifescape_json::parse(&text).ok()?)
        })
        .collect()
}

fn report(name: &str, lives: usize, tally: &Tally) {
    eprintln!("equivalence {name}: {lives} lives, {} upkeep scenarios and {} graph-state scenarios identical", tally.upkeep, tally.graph);
    for (key, count) in &tally.counts {
        eprintln!("  {key}: {count}");
    }
    let unreached: Vec<&str> = LOG_PROBES.iter().map(|(name, _)| *name).chain(FX_PROBES.iter().map(|(name, _)| *name)).filter(|name| !tally.counts.contains_key(name)).collect();
    eprintln!("  never reached: {unreached:?}");
}

/// The committed fixtures: nine real lives, trimmed.
#[test]
fn the_upkeep_equals_the_reference_on_the_fixture_lives() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lives");
    let lives = load_dir(&dir);
    assert!(lives.len() >= 9, "fixtures missing");
    let template = template_of(&lives);
    let mut tally = Tally::default();
    for life in &lives {
        run_life(life, &template, 400, 0xC0FFEE, &mut tally);
    }
    report("fixtures", lives.len(), &tally);
    for key in [
        "bid.blocked_given_back",
        "bid.off_trade_given_back",
        "bid.beyond_reach_given_back",
        "serve.unpaying_given_back",
        "milestone.set_aside",
        "stale.goal_replaced",
        "review.need_mark",
        "maker.switched",
        "operator.alcher_given",
        "graph.spell_opened",
        "graph.rescue_market",
        "graph.goal_done_next",
    ] {
        assert!(tally.counts.get(key).copied().unwrap_or(0) > 0, "{key} never reached: {:?}", tally.counts);
    }
}

/// The real fleet: every life of `LIFESCAPE_LIVES_DIR` (read-only, copied
/// to a temporary directory first), generated scenarios per life.
#[test]
#[ignore = "reads the real lives: LIFESCAPE_LIVES_DIR=/path/to/run/lives cargo test --release the_upkeep_equals_the_reference_on_the_real_lives -- --ignored --nocapture"]
fn the_upkeep_equals_the_reference_on_the_real_lives() {
    let source = std::env::var("LIFESCAPE_LIVES_DIR").expect("set LIFESCAPE_LIVES_DIR to run/lives");
    let temp = std::env::temp_dir().join(format!("lifescape_upkeep_lives_{}", std::process::id()));
    std::fs::create_dir_all(&temp).expect("temp dir");
    for entry in std::fs::read_dir(&source).expect("source dir").flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            std::fs::copy(&path, temp.join(entry.file_name())).expect("copy");
        }
    }
    let lives = load_dir(&temp);
    let template = template_of(&lives);
    let mut tally = Tally::default();
    for part in crate::par_harness::par_lives(&lives, |life, tally: &mut Tally| run_life(life, &template, 600, 0xFEED_FACE, tally)) {
        tally.merge(part);
    }
    report("real lives", lives.len(), &tally);
    let _ = std::fs::remove_dir_all(&temp);
}
