//! The invariant of the side market passes (lot 5 of the launch refactor,
//! user rule 2026-11-23: never preempt an active milestone for lucrative
//! market bids, XP first): `COMMERCE_STEPS` never change `life.goal`, the
//! ambition and its milestone statuses, nor the status, kind or method of
//! a node that already exists. The only changes allowed are the standing
//! kit and gear steps they file (added nodes; the withdrawal of a gear
//! step's own chain) and the market's bookkeeping (orders, outside the
//! life).
//!
//! The steps run for real over a `Job` carrying a generated live snapshot
//! (levels, coins, pack, bank, worn); no market service exists in a unit
//! test, so the market-bound branches (a covered ask, a standing bid) answer
//! as an empty market. The read-only steps take `&Life`: the compiler
//! proves them; the test proves the two that file steps and the composition.
//! Fixtures run in CI; the real lives (`LIFESCAPE_LIVES_DIR`) run with
//! `--ignored`, copied to a temporary directory first (never written).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use lifescape_json::{object, str_value, Value};
use lifescape_planner::{Archetype, GoalKind, Life, MakerRole, NodeStatus};

use super::{Commerce, COMMERCE_STEPS};

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

const STACKS: [&str; 24] = [
    "coins", "bronze_axe", "iron_axe", "steel_axe", "rune_axe", "bronze_pickaxe", "iron_pickaxe", "mithril_pickaxe", "rune_pickaxe", "hammer", "tinderbox", "iron_ore", "coal", "logs",
    "raw_shrimp", "bronze_platebody", "iron_platebody", "rune_scimitar", "mithril_chainbody", "nature_rune", "fire_rune", "cert_coal", "yew_logs", "bronze_bar",
];

fn stack(name: &str, count: i64) -> Option<Value> {
    let obj = lifescape_gamedata::objs_by_name(name).into_iter().next()?;
    Some(object(vec![("id", Value::Int(obj.obj_id as i64)), ("name", str_value(obj.display.unwrap_or(name))), ("count", Value::Int(count))]))
}

fn stacks(rng: &mut Rng, max: u64, coins: Option<i64>) -> Value {
    let mut rows: Vec<Value> = coins.and_then(|amount| stack("coins", amount)).into_iter().collect();
    for _ in 0..rng.below(max) {
        let name = *rng.pick(&STACKS[1..]);
        let count = *rng.pick(&[1i64, 1, 5, 40, 900]);
        rows.extend(stack(name, count));
    }
    Value::Arr(rows)
}

/// A generated live snapshot: levels, coins, pack, bank, worn.
fn live_of(rng: &mut Rng) -> Value {
    // Experience of levels 1, 5, 20, 41, 60, 75, 99.
    let xp = |rng: &mut Rng| *rng.pick(&[0i64, 388, 4_470, 40_000, 273_742, 1_210_421, 13_034_431]);
    let skills: Vec<Value> = (0u8..23).map(|stat| object(vec![("id", Value::Int(i64::from(stat))), ("xp", Value::Int(xp(rng)))])).collect();
    let coins = *rng.pick(&[0i64, 100, 5_000, 90_000, 5_000_000]);
    object(vec![
        ("skills", Value::Arr(skills)),
        ("inv", stacks(rng, 6, Some(coins))),
        ("bank", stacks(rng, 10, None)),
        ("worn", stacks(rng, 3, None)),
        ("inv_streamed", Value::Bool(true)),
        ("bank_streamed", Value::Bool(rng.chance(85))),
        ("updated", Value::Float(1_800_000_000.0)),
    ])
}

fn shape(life: &mut Life, rng: &mut Rng) {
    if rng.chance(12) {
        life.profile.ironman = true;
    }
    if rng.chance(10) {
        let mut role = MakerRole::new(2, 8, 100_000);
        if rng.chance(50) {
            role.phase = lifescape_planner::MakerPhase::Quoting;
        }
        life.profile.archetype = Archetype::MarketMaker(role);
    }
    // Nodes in every status: a ready, an active, a blocked and a done one.
    for node in life.nodes.iter_mut() {
        match rng.below(8) {
            0 => node.status = NodeStatus::Done,
            1 => node.status = NodeStatus::Blocked { reason: "missing:coins:5".to_string() },
            2 => node.status = NodeStatus::Ready,
            _ => {}
        }
    }
    if rng.chance(30) {
        life.goal = GoalKind::MaxSkills { skills: vec!["mining".to_string()], level: *rng.pick(&[5u8, 30, 70]) };
    }
}

/// What the invariant reads before and after.
struct View {
    goal: GoalKind,
    goal_origin: Option<String>,
    ambition: Option<lifescape_planner::ambition::Ambition>,
    nodes: Vec<(String, lifescape_planner::NodeKind, NodeStatus, Option<String>)>,
    gear_chain: BTreeSet<String>,
    goal_history: usize,
}

fn view(life: &Life) -> View {
    let mut gear_chain = BTreeSet::new();
    for (index, node) in life.nodes.iter().enumerate() {
        if lifescape_planner::gear_ladder::is_ladder_equip(node) {
            gear_chain.extend(lifescape_planner::gear_ladder::step_chain(life, index));
        }
    }
    View {
        goal: life.goal.clone(),
        goal_origin: life.goal_origin.clone(),
        ambition: life.ambition.clone(),
        nodes: life.nodes.iter().map(|node| (node.id.clone(), node.kind.clone(), node.status.clone(), node.method.clone())).collect(),
        gear_chain,
        goal_history: life.history.iter().filter(|entry| entry.node == "goal").count(),
    }
}

#[derive(Default)]
struct Tally {
    scenarios: usize,
    runs: usize,
    quoting: usize,
    nodes_added: usize,
    gear_nodes_removed: usize,
    dirty: usize,
}

/// Runs the list on `life` and checks the invariant; returns the life after.
fn check(label: &str, life: &mut Life, rng: &mut Rng, round: usize, tally: &mut Tally) {
    let before = view(life);
    let live = live_of(rng);
    let account = format!("{}{}_{round}_{}", super::PROBE_PREFIX, life.account, rng.next());
    let job = crate::jobs::Job::new(&account);
    job.inner.lock().expect("job").live = Some(live.clone());
    let quoting = lifescape_planner::quoting(life) || rng.chance(5);
    let mut commerce = Commerce { job: &job, life, live: &live, quoting, dirty: false };
    super::run(&mut commerce);
    let dirty = commerce.dirty;
    let after = view(life);
    tally.runs += 1;
    tally.quoting += usize::from(quoting);
    tally.dirty += usize::from(dirty);
    assert_eq!(before.goal, after.goal, "{label}: life.goal changed");
    assert_eq!(before.goal_origin, after.goal_origin, "{label}: goal_origin changed");
    assert_eq!(before.ambition, after.ambition, "{label}: the ambition or a milestone changed");
    assert_eq!(before.goal_history, after.goal_history, "{label}: a goal history entry was written");
    for (id, kind, status, method) in &before.nodes {
        match after.nodes.iter().find(|(other, ..)| other == id) {
            Some((_, kind_after, status_after, method_after)) => {
                if !before.gear_chain.contains(id) {
                    assert_eq!((kind, status, method), (kind_after, status_after, method_after), "{label}: node {id} changed (status, kind or method)");
                }
            }
            None => {
                assert!(before.gear_chain.contains(id), "{label}: node {id} left the graph and is no gear step");
                tally.gear_nodes_removed += 1;
            }
        }
    }
    tally.nodes_added += after.nodes.len().saturating_sub(before.nodes.len());
    // A node filed is a standing-kit or gear step: an obtain of one item
    // (count 1) or its equip_best, never anything else.
    for (id, kind, ..) in after.nodes.iter().filter(|(id, ..)| !before.nodes.iter().any(|(other, ..)| other == id)) {
        let filed = match kind {
            lifescape_planner::NodeKind::EquipBest { .. } => true,
            lifescape_planner::NodeKind::ObtainItems { count, .. } => *count == 1,
            _ => false,
        };
        assert!(filed, "{label}: commerce filed node {id} of kind {kind:?}");
    }
}

impl Tally {
    fn merge(&mut self, other: Tally) {
        self.scenarios += other.scenarios;
        self.runs += other.runs;
        self.quoting += other.quoting;
        self.nodes_added += other.nodes_added;
        self.gear_nodes_removed += other.gear_nodes_removed;
        self.dirty += other.dirty;
    }
}

fn run_life(base: &Life, variants: usize, seed: u64, tally: &mut Tally) {
    for variant in 0..variants {
        let mut rng = Rng(seed ^ (variant as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ base.seed.rotate_left(11));
        rng.next();
        let mut life = base.clone();
        shape(&mut life, &mut rng);
        // The pass normalizes before the commerce steps.
        lifescape_planner::normalize(&mut life);
        // Three rounds: the second sees the step the first filed (open
        // ladder step, withdrawal paths), the third the one after.
        for round in 0..3 {
            check(&format!("{} variant {variant} round {round}", base.account), &mut life, &mut rng, round, tally);
        }
        tally.scenarios += 1;
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
    eprintln!(
        "commerce invariant {name}: {lives} lives, {} scenarios, {} runs of the {} steps ({} as a quoting maker), {} nodes filed, {} gear nodes withdrawn, {} runs changed the life: no goal, ambition or existing node touched",
        tally.scenarios,
        tally.runs,
        COMMERCE_STEPS.len(),
        tally.quoting,
        tally.nodes_added,
        tally.gear_nodes_removed,
        tally.dirty
    );
}

/// The list is the one the pass always ran, in its order.
#[test]
fn the_commerce_list_keeps_the_order_of_the_pass() {
    let names: Vec<&str> = COMMERCE_STEPS.iter().map(|step| step.name).collect();
    assert_eq!(
        names,
        ["quest_gate.publish", "sales.list_surplus", "market_maker.quote_pass", "wishlist.bid_wishes", "alch.market_pass", "arbitrage.pass", "stock.future_pass", "tool_shop_pass", "gear_pass"]
    );
}

/// The committed fixtures: real lives, trimmed.
#[test]
fn commerce_never_preempts_the_ambition_on_the_fixture_lives() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lives");
    let lives = load_dir(&dir);
    assert!(lives.len() >= 9, "fixtures missing");
    let mut tally = Tally::default();
    for life in &lives {
        run_life(life, 60, 0xC0FFEE, &mut tally);
    }
    report("fixtures", lives.len(), &tally);
    assert!(tally.nodes_added > 0, "no kit or gear step was ever filed: the test reaches nothing");
}

/// The real fleet: every life of `LIFESCAPE_LIVES_DIR` (read-only, copied to
/// a temporary directory first).
#[test]
#[ignore = "reads the real lives: LIFESCAPE_LIVES_DIR=/path/to/run/lives cargo test --release commerce_never_preempts_the_ambition_on_the_real_lives -- --ignored --nocapture"]
fn commerce_never_preempts_the_ambition_on_the_real_lives() {
    let source = std::env::var("LIFESCAPE_LIVES_DIR").expect("set LIFESCAPE_LIVES_DIR to run/lives");
    let temp = std::env::temp_dir().join(format!("lifescape_commerce_lives_{}", std::process::id()));
    std::fs::create_dir_all(&temp).expect("temp dir");
    for entry in std::fs::read_dir(&source).expect("source dir").flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            std::fs::copy(&path, temp.join(entry.file_name())).expect("copy");
        }
    }
    let lives = load_dir(&temp);
    // Each run takes its own job account (the passes rate-limit per account);
    // variants per life: `LIFESCAPE_COMMERCE_VARIANTS`.
    let variants = std::env::var("LIFESCAPE_COMMERCE_VARIANTS").ok().and_then(|raw| raw.parse().ok()).unwrap_or(40);
    let mut tally = Tally::default();
    for part in crate::par_harness::par_lives(&lives, |life, tally: &mut Tally| run_life(life, variants, 0xFEED_FACE, tally)) {
        tally.merge(part);
    }
    report("real lives", lives.len(), &tally);
    let _ = std::fs::remove_dir_all(&temp);
}
