//! Equivalence of the launch decision (lot 3 of the launch refactor): the
//! reference (`brain_launch_reference::reference_pass`, the node and
//! dispatch steps as they were) against the real path
//! (`brain_launch_drive::drive`: planner `decide` + `apply_to_life` +
//! effects) over a recording fake, on lives crossed with generated inputs.
//! Compared: what the pass saves, the life it leaves (nodes, statuses,
//! history, memory, engagement), every effect and log line in order, and
//! the context the effects saw. Fixtures committed under
//! `tests/fixtures/lives` run in CI; the real lives
//! (`LIFESCAPE_LIVES_DIR`, 84 files) run with `--ignored`, copied to a
//! temporary directory first (the source is never written).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lifescape_json::{object, str_value, Value};
use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::{LaunchInputs, Life, NodeKind, NodeStatus};

use crate::brain_launch_drive::{drive, Drive};
use crate::brain_launch_pass::Save;
use crate::brain_launch_reference::{reference_pass, Ev, Knobs, RecFx};
use crate::brain_raw_world_tests::{wall_now, xp_of_level, RawWorld, Seed, TestMarket};

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

fn stack(name: &str, count: i64) -> Option<Value> {
    let obj = lifescape_gamedata::objs_by_name(name).into_iter().next()?;
    Some(object(vec![("id", Value::Int(obj.obj_id as i64)), ("name", str_value(obj.display.unwrap_or(name))), ("count", Value::Int(count))]))
}

fn obj_id(name: &str) -> Option<i64> {
    lifescape_gamedata::objs_by_name(name).into_iter().next().map(|obj| obj.obj_id as i64)
}

struct Scenario {
    life: Life,
    position: usize,
    inputs: LaunchInputs,
    ctx: ReadyCtx,
    committed: i64,
    knobs: Knobs,
}

/// Every name the node's methods consume or need (inputs and tools).
fn related_names(life: &Life, position: usize) -> Vec<String> {
    let node = &life.nodes[position];
    let mut names = Vec::new();
    if let NodeKind::ObtainItems { item, .. } = &node.kind {
        names.push(item.clone());
    }
    for alt in &node.alternatives {
        if let Some((_, way)) = lifescape_planner::method_way::method_way(&alt.method).or_else(|| lifescape_planner::spell_methods::cast_method_way(&alt.method)) {
            names.extend(way.inputs.iter().map(|(name, _)| name.clone()));
            names.extend(way.tools.iter().cloned());
        }
    }
    names.sort();
    names.dedup();
    names
}

fn build(base: &Life, position: usize, variant: u64, market: &TestMarket, rng: &mut Rng) -> Scenario {
    let mut life = base.clone();
    // The pass picked this node: ready, nothing locked yet half the time.
    life.nodes[position].status = NodeStatus::Ready;
    if rng.chance(70) {
        life.nodes[position].method = None;
    }
    if rng.chance(15) {
        life.profile.ironman = true;
    }
    // A cheap extra leg of every family the decision treats apart: fights
    // (food, loot), a cleanup, a production, a purchase, a quarantined one.
    if rng.chance(35) {
        let pool = ["fight:melee/men", "fight:melee/chickens", "fight:ranged/cows", "fight:melee/goblins", "smelt:iron_bar", "cook/raw_shrimp", "space:cleanup", "market:buy", "shop:buy", "gather:mining/coal"];
        let method = rng.pick(&pool).to_string();
        life.nodes[position].alternatives.insert(0, lifescape_planner::Alternative { method, est_ticks: 10, est_coins: 0 });
    }
    // Bans on the skills a money node may steer around.
    if rng.chance(30) {
        for skill in ["mining", "fishing", "woodcutting", "smithing", "cooking", "fletching"] {
            if rng.chance(60) {
                life.history.push(lifescape_planner::HistoryEntry { node: "goal".to_string(), started: wall_now() - 10, ended: wall_now() - 10, outcome: format!("{}{skill}@400000:test", lifescape_planner::goals::STUCK_TAG) });
            }
        }
    }
    let names = related_names(&life, position);
    let mut inv = Vec::new();
    let mut bank = Vec::new();
    let mut worn = Vec::new();
    let mut seed = Seed::default();
    let mode = rng.below(8);
    let wanted = match &life.nodes[position].kind {
        NodeKind::ObtainItems { count, .. } => *count,
        _ => 5,
    };
    for name in &names {
        let units = match mode {
            0 => 0,
            1 => (wanted - 1).max(0),
            2 => wanted,
            3 | 5 => wanted + 2,
            _ => 25,
        };
        if units > 0 {
            if let Some(entry) = stack(name, units) {
                match rng.below(5) {
                    0 | 1 => bank.push(entry),
                    2 if units == 1 => worn.push(entry),
                    _ => inv.push(entry),
                }
                if let Some(id) = obj_id(name) {
                    seed.holdings.push((id, units));
                }
            }
        }
        // Listings lock units (the held-but-locked and listed-input cases):
        // the bot's own resting sells in the market, read back by
        // `locked_items` and `gather_market` like in production.
        if matches!(mode, 3 | 5 | 7) && units > 0 {
            if let Some(id) = obj_id(name) {
                seed.sells.push((id, units, 4_000));
            }
        }
        if mode == 6 && rng.chance(30) {
            if let Some(id) = obj_id(name) {
                seed.sells.push((id, 3, 4_000));
            }
        }
    }
    let coins = *rng.pick(&[0i64, 50, 2_000, 500_000, 5_000_000]);
    if coins > 0 {
        if let Some(entry) = stack("coins", coins) {
            inv.push(entry);
        }
    }
    seed.coins = coins;
    // A resting bid (committed coins) and, now and then, asks of other
    // sellers on the node's item (the best ask the context reads).
    if rng.chance(30) {
        seed.buys.push((obj_id("coal").unwrap_or(1), 5, 1));
    }
    if rng.chance(50) {
        if let Some(id) = names.first().and_then(|name| obj_id(name)) {
            seed.asks.push((id, 50, *rng.pick(&[5i64, 80, 4_000])));
        }
    }
    let skill_level = *rng.pick(&[1u8, 15, 40, 99]);
    let hitpoints = *rng.pick(&[10u8, 30, 70]);
    let skills: Vec<Value> = (0u8..23)
        .map(|stat| {
            let level = if stat == 3 { hitpoints } else { skill_level };
            // The context reads the base level from the experience: both agree.
            let xp = xp_of_level(level);
            object(vec![("id", Value::Int(i64::from(stat))), ("xp", Value::Int(xp)), ("level", Value::Int(i64::from(level)))])
        })
        .collect();
    let site = life.nodes[position].site;
    if rng.chance(30) {
        for (index, cell) in site.into_iter().enumerate() {
            let method = if index == 0 { "someone".to_string() } else { "someone_else".to_string() };
            seed.crowd.extend(std::iter::repeat_n((cell, method), 4));
        }
    }
    let live = object(vec![
        ("skills", Value::Arr(skills)),
        ("inv", Value::Arr(inv)),
        ("bank", Value::Arr(bank)),
        ("worn", Value::Arr(worn)),
        ("combat", Value::Int(i64::from(*rng.pick(&[3u8, 30, 90, 126])))),
        ("position", Value::Arr(vec![Value::Int(0), Value::Int(3222), Value::Int(3218)])),
        ("inv_streamed", Value::Bool(true)),
        ("bank_streamed", Value::Bool(rng.chance(85))),
    ]);
    let market_up = rng.chance(75);
    let account = format!("{}eq_{}_{position}_{variant}", crate::brain_commerce::PROBE_PREFIX, life.account);
    let world = RawWorld::new(&account, live, market_up.then_some(market));
    world.seed(&seed);
    let (inputs, ctx, committed) = world.launch_read(&life, position);
    world.clear(&seed);
    let knobs = Knobs { stubborn: rng.chance(40), refuse_buy: rng.chance(30), retarget_food: rng.chance(50), shop: rng.below(4) as u8, rate_tenths: *rng.pick(&[1i64, 5, 30]) };
    Scenario { committed, life, position, inputs, ctx, knobs }
}

/// What one run leaves, for the comparison.
struct Outcome {
    save: Save,
    life: Life,
    trace: Vec<Ev>,
    share: Option<i64>,
}

fn run_reference(s: &Scenario) -> Outcome {
    let (mut life, mut inputs, mut ctx) = (s.life.clone(), s.inputs.clone(), s.ctx.clone());
    let fx = RecFx::new(s.inputs.clone(), s.knobs.clone());
    let save = reference_pass(&fx, &mut life, &mut inputs, &mut ctx, s.position, s.committed);
    Outcome { save, life, trace: fx.trace.into_inner(), share: ctx.coin_share_pct }
}

fn run_new(s: &Scenario) -> Outcome {
    let (mut life, mut inputs, mut ctx) = (s.life.clone(), s.inputs.clone(), s.ctx.clone());
    let mut fx = RecFx::new(s.inputs.clone(), s.knobs.clone());
    let save = drive(&mut fx, Drive { life: &mut life, inputs: &mut inputs, ctx: &mut ctx, position: s.position, committed: s.committed });
    Outcome { save, life, trace: fx.trace.into_inner(), share: ctx.coin_share_pct }
}

/// The kind of decision a trace shows, for the coverage report.
fn kind_of(out: &Outcome) -> &'static str {
    let logs: Vec<&str> = out.trace.iter().filter_map(|event| if let Ev::Log(line) = event { Some(line.as_str()) } else { None }).collect();
    let has = |needle: &str| logs.iter().any(|line| line.contains(needle));
    if out.trace.iter().any(|event| matches!(event, Ev::RunJob { .. })) {
        "RunScript"
    } else if out.trace.iter().any(|event| matches!(event, Ev::MarketBuy { .. })) {
        "PlaceMarketBuy"
    } else if has("done without launch") {
        "AlreadyHeld"
    } else if has("gear step") {
        "GearStepWithdrawn"
    } else if has("coins >=") {
        "CoinTargetMet"
    } else if has("every method cooled") || has("cooled ") && has("stays open") {
        "CooledWait"
    } else if has("blocked at resolve") {
        "BlockedAtResolve/Abandon"
    } else if has("no method resolvable") {
        "NoMethod"
    } else if has("refusing") {
        "RefuseIron"
    } else if has("inputs short") {
        "BlockedInputs"
    } else if has("foodless") {
        "BlockedFood"
    } else if has("no script for") {
        "NoScript"
    } else if has(" waits: ") {
        "WaitUncounted"
    } else {
        "CoinCycle/other"
    }
}

#[derive(Default)]
struct Tally {
    scenarios: usize,
    kinds: BTreeMap<&'static str, usize>,
    withdrawn_held: usize,
    withdrawn_inputs: usize,
    asked_food: usize,
    asked_loot: usize,
    asked_cleanup: usize,
    standing_bids: usize,
    /// Raw-world coverage: scenarios whose production reads returned a lock,
    /// a committed bid, a crowd, a best ask, a market that is up.
    locked: usize,
    committed: usize,
    crowded: usize,
    asked: usize,
    market_up: usize,
}

impl Tally {
    fn merge(&mut self, other: Tally) {
        self.scenarios += other.scenarios;
        for (kind, count) in other.kinds {
            *self.kinds.entry(kind).or_insert(0) += count;
        }
        self.withdrawn_held += other.withdrawn_held;
        self.withdrawn_inputs += other.withdrawn_inputs;
        self.asked_food += other.asked_food;
        self.asked_loot += other.asked_loot;
        self.asked_cleanup += other.asked_cleanup;
        self.standing_bids += other.standing_bids;
        self.locked += other.locked;
        self.committed += other.committed;
        self.crowded += other.crowded;
        self.asked += other.asked;
        self.market_up += other.market_up;
    }
}

fn same(label: &str, s: &Scenario, reference: &Outcome, new: &Outcome) {
    assert_eq!(reference.save, new.save, "{label}: save");
    assert_eq!(reference.share, new.share, "{label}: coin share on the context");
    if reference.trace != new.trace {
        let at = reference.trace.iter().zip(&new.trace).position(|(left, right)| left != right).unwrap_or(reference.trace.len().min(new.trace.len()));
        panic!("{label}: trace differs at {at}\n ref: {:?}\n new: {:?}\n node {:?}", reference.trace.iter().take(at + 3).collect::<Vec<_>>(), new.trace.iter().take(at + 3).collect::<Vec<_>>(), s.life.nodes[s.position]);
    }
    if reference.life != new.life {
        let node_of = |out: &Outcome| format!("{:#?}", out.life.nodes);
        panic!("{label}: life differs\n ref nodes: {}\n new nodes: {}\n ref history: {:?}\n new history: {:?}", node_of(reference), node_of(new), &reference.life.history[s.life.history.len()..], &new.life.history[s.life.history.len()..]);
    }
}

fn run_life(life: &Life, variants: usize, seed: u64, tally: &mut Tally) {
    let market = TestMarket::open(&life.account);
    for position in 0..life.nodes.len() {
        for variant in 0..variants {
            let mut rng = Rng(seed ^ ((position as u64 + 1) << 32) ^ (variant as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
            // Warm the generator.
            rng.next();
            let scenario = build(life, position, variant as u64, &market, &mut rng);
            if std::env::var_os("LIFESCAPE_EQUIV_TRACE").is_some() {
                eprintln!("scenario {} node {} variant {variant}", life.account, life.nodes[position].id);
            }
            let reference = run_reference(&scenario);
            let new = run_new(&scenario);
            let label = format!("{} node {} variant {variant}", life.account, life.nodes[position].id);
            same(&label, &scenario, &reference, &new);
            tally.scenarios += 1;
            tally.locked += usize::from(!scenario.inputs.locks.is_empty());
            tally.committed += usize::from(!scenario.inputs.commit_rows.is_empty() || scenario.inputs.commit_flat > 0);
            tally.crowded += usize::from(!scenario.inputs.crowd.is_empty());
            tally.asked += usize::from(scenario.ctx.best_ask.is_some());
            tally.market_up += usize::from(scenario.inputs.market_up);
            *tally.kinds.entry(kind_of(&new)).or_insert(0) += 1;
            for event in &new.trace {
                match event {
                    Ev::Release(items) if reference.trace.iter().filter(|e| matches!(e, Ev::Release(_))).count() > 0 => {
                        let _ = items;
                    }
                    Ev::Food(..) => tally.asked_food += 1,
                    Ev::Loot(_) => tally.asked_loot += 1,
                    Ev::Cleanup(_) => tally.asked_cleanup += 1,
                    Ev::Bid { .. } => tally.standing_bids += 1,
                    _ => {}
                }
            }
            let releases = new.trace.iter().filter(|event| matches!(event, Ev::Release(_))).count();
            if releases > 0 {
                if new.trace.iter().any(|event| matches!(event, Ev::Log(line) if line.contains("listings locked them"))) {
                    tally.withdrawn_held += 1;
                } else {
                    tally.withdrawn_inputs += 1;
                }
            }
        }
    }
}

pub(crate) fn load_dir(dir: &Path) -> Vec<Life> {
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
    eprintln!("equivalence {name}: {lives} lives, {} scenarios identical; kinds {:?}; held-listing withdrawals {}, input-listing withdrawals {}, food asks {}, loot asks {}, cleanup asks {}, standing bids {}; raw world: locked {}, committed {}, crowded {}, best ask {}, market up {}", tally.scenarios, tally.kinds, tally.withdrawn_held, tally.withdrawn_inputs, tally.asked_food, tally.asked_loot, tally.asked_cleanup, tally.standing_bids, tally.locked, tally.committed, tally.crowded, tally.asked, tally.market_up);
}

/// The committed fixtures: nine real lives, trimmed.
#[test]
fn the_decision_equals_the_reference_on_the_fixture_lives() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lives");
    let lives = load_dir(&dir);
    assert!(lives.len() >= 9, "fixtures missing");
    let mut tally = Tally::default();
    for life in &lives {
        run_life(life, 120, 0xC0FFEE, &mut tally);
    }
    report("fixtures", lives.len(), &tally);
    for kind in ["RunScript", "AlreadyHeld", "BlockedInputs"] {
        assert!(tally.kinds.get(kind).copied().unwrap_or(0) > 0, "{kind} never reached: {:?}", tally.kinds);
    }
}

/// The outcomes the random crossing seldom reaches, built by hand: a coin
/// node under a blocked coin node (a cycle through the coins) and a money
/// node with no method to steer to. Each runs on both paths.
#[test]
fn the_rare_outcomes_equal_the_reference_too() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lives");
    let base = load_dir(&dir).into_iter().find(|life| life.account == "ryan_park").expect("fixture");
    let mut tally = Tally::default();
    // Coin cycle: n_a (have coins) is waited on by a blocked have-coins node.
    let mut life = base.clone();
    let mut a = lifescape_planner::Node::new("n9001", NodeKind::HaveCoins { amount: 100_000 }, Vec::new(), 100_000);
    a.status = NodeStatus::Ready;
    let mut b = lifescape_planner::Node::new("n9002", NodeKind::HaveCoins { amount: 5_000 }, vec!["n9001".to_string()], 5_000);
    b.status = NodeStatus::Blocked { reason: "missing:coins:500".to_string() };
    b.blocks = 2;
    life.nodes.push(a);
    life.nodes.push(b);
    let cycle = life.nodes.len() - 2;
    // A money node with nothing to work: the steering refuses.
    let mut money = lifescape_planner::Node::new("n9003", NodeKind::MakeMoney, Vec::new(), 1);
    money.status = NodeStatus::Ready;
    life.nodes.push(money);
    let steer = life.nodes.len() - 1;
    let market = TestMarket::open("rare");
    for position in [cycle, steer] {
        for variant in 0..200u64 {
            let mut rng = Rng(0xABCD ^ (position as u64) << 20 ^ variant.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            rng.next();
            let scenario = build(&life, position, variant, &market, &mut rng);
            // The wallet decides the coin-target branch: only the draws that
            // leave the bot short of the target count (read, never set).
            if scenario.ctx.coins >= 100_000 {
                continue;
            }
            let (reference, new) = (run_reference(&scenario), run_new(&scenario));
            same(&format!("rare node {} variant {variant}", life.nodes[position].id), &scenario, &reference, &new);
            tally.scenarios += 1;
            let changed = new.life != scenario.life;
            *tally.kinds.entry(if position == cycle && changed { "coin-cycle path" } else if position == steer { "money-steer path" } else { "unchanged" }).or_insert(0) += 1;
        }
    }
    eprintln!("rare outcomes: {} scenarios identical; {:?}", tally.scenarios, tally.kinds);
}

/// The real fleet: every life of `LIFESCAPE_LIVES_DIR` (read-only, copied
/// to a temporary directory first), 150 generated scenarios per node.
#[test]
#[ignore = "reads the real lives: LIFESCAPE_LIVES_DIR=/path/to/run/lives cargo test --release decision_equals_the_reference_on_the_real_lives -- --ignored --nocapture"]
fn the_decision_equals_the_reference_on_the_real_lives() {
    let source = std::env::var("LIFESCAPE_LIVES_DIR").expect("set LIFESCAPE_LIVES_DIR to run/lives");
    let temp = std::env::temp_dir().join(format!("lifescape_equiv_lives_{}", std::process::id()));
    std::fs::create_dir_all(&temp).expect("temp dir");
    for entry in std::fs::read_dir(&source).expect("source dir").flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            std::fs::copy(&path, temp.join(entry.file_name())).expect("copy");
        }
    }
    let lives = load_dir(&temp);
    let mut tally = Tally::default();
    for part in crate::par_harness::par_lives(&lives, |life, tally: &mut Tally| run_life(life, 150, 0xFEED_FACE, tally)) {
        tally.merge(part);
    }
    report("real lives", lives.len(), &tally);
    let _ = std::fs::remove_dir_all(&temp);
}
