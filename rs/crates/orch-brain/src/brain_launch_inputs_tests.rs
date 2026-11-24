#![forbid(unsafe_code)]
//! Guard of the production launch inputs (2026-11-23 live bug: lot 3 added
//! `now`, `tick_ms`, `levels`, `market_up` and `filler_ticks` to
//! `LaunchInputs`, the production `gather` never set them, and only the
//! test harnesses did: every history stamp read 0, every market fill wait
//! expired at once, every level read 1). These tests call the PRODUCTION
//! readers on a live-shaped snapshot and fail on any plain-data field left
//! at its default.

use std::path::Path;
use std::sync::Arc;

use lifescape_json::{object, Value};
use lifescape_planner::launch_apply::settle_market_buy;
use lifescape_planner::{LaunchInputs, Life, NodeKind, NodeStatus};

use crate::brain_launch_fx::{Fx, LiveFx};
use crate::brain_deps::BrainDeps;
use crate::brain_raw_world_tests::{wall_now as now_wall, xp_of_level, IdleFleet, RawWorld, Seed, TestMarket};
use crate::fleet::Fleet;
use crate::jobs::Job;

/// A live snapshot: every skill at a distinct level, a pack, a position.
fn live_snapshot() -> Value {
    let skills: Vec<Value> = (0u8..23).map(|stat| object(vec![("id", Value::Int(i64::from(stat))), ("xp", Value::Int(388)), ("level", Value::Int(i64::from(stat) + 5))])).collect();
    let coins = object(vec![("name", Value::Str("coins".to_string())), ("count", Value::Int(500))]);
    object(vec![
        ("skills", Value::Arr(skills)),
        ("inv", Value::Arr(vec![coins])),
        ("bank", Value::Arr(Vec::new())),
        ("worn", Value::Arr(Vec::new())),
        ("position", Value::Arr(vec![Value::Int(0), Value::Int(3222), Value::Int(3218)])),
        ("inv_streamed", Value::Bool(true)),
        ("bank_streamed", Value::Bool(true)),
    ])
}

fn probe_job(tag: &str) -> Arc<Job> {
    let job = Arc::new(Job::new(&format!("{}inputs_{tag}", crate::brain_commerce::PROBE_PREFIX)));
    job.inner.lock().expect("job").live = Some(std::sync::Arc::new(live_snapshot()));
    job
}

/// Every plain-data field the lot 3 harnesses set by hand must come out of
/// the production readers.
fn assert_plain_fields(inputs: &LaunchInputs, live: &Value) {
    let wall = now_wall();
    assert!((wall - inputs.now).abs() <= 5, "now {} is not the wall clock {wall}", inputs.now);
    assert!(inputs.tick_ms > 0, "tick_ms unset");
    assert_eq!(inputs.tick_ms, crate::market_eta::live_tick_ms());
    assert!(!inputs.levels.is_empty(), "levels empty: every skill would read level 1");
    assert_eq!(inputs.levels, crate::brain_launch_inputs::live_skill_levels(live));
    assert_eq!(inputs.levels.len(), 23);
    for (name, level) in &inputs.levels {
        assert_eq!(*level, crate::brain_succession::live_level(live, name), "{name}");
        assert_eq!(inputs.level_of(name), *level);
    }
    assert!(inputs.levels.iter().any(|(_, level)| *level > 1));
    assert_eq!(inputs.filler_ticks, crate::brain_filler::FILLER_TICKS);
    assert!(inputs.filler_ticks > 0);
    assert_eq!(inputs.market_up, crate::market_service::shared().is_some());
}

#[test]
fn gather_fills_every_plain_data_field() {
    let job = probe_job("gather");
    let live = live_snapshot();
    let inputs = crate::brain_launch_inputs::gather(&job, &live, &BrainDeps::live());
    assert_plain_fields(&inputs, &live);
    assert!(inputs.inv_streamed && inputs.bank_streamed);
    assert!(inputs.position.is_some());
}

#[test]
fn the_market_read_keeps_the_fields_and_reads_the_clock_again() {
    let job = probe_job("market");
    let live = live_snapshot();
    let deps = BrainDeps::live();
    let mut inputs = crate::brain_launch_inputs::gather(&job, &live, &deps);
    inputs.now = 1;
    inputs.tick_ms = 0;
    crate::brain_launch_inputs::gather_market(&job, &mut inputs, &deps);
    assert_plain_fields(&inputs, &live);
}

#[test]
fn live_fx_refreshes_the_clock_before_each_decision() {
    let job = probe_job("fx");
    let live = live_snapshot();
    let orch: Arc<dyn Fleet> = IdleFleet::shared();
    let mut fx = LiveFx::new(&orch, &job, &live, BrainDeps::live());
    let mut inputs = LaunchInputs::blank_for_tests();
    assert_eq!(inputs.now, 0);
    fx.refresh_clock(&mut inputs);
    assert!((now_wall() - inputs.now).abs() <= 5);
    inputs.now = 0;
    fx.refresh_market(&mut inputs);
    assert!((now_wall() - inputs.now).abs() <= 5, "refresh_market left the clock stale");
}

/// A history entry written by the launch decision carries the wall clock,
/// and a market buy placed just now is neither expired nor settled on the
/// next pass (the live bug: `waited = now - 0`).
#[test]
fn a_placed_market_buy_is_stamped_now_and_not_expired() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lives");
    let mut life: Life = crate::brain_launch_equiv_tests::load_dir(&dir).into_iter().next().expect("fixture");
    life.history.clear();
    let mut node = lifescape_planner::Node::new("n9100", NodeKind::ObtainItems { item: "logs".to_string(), count: 5 }, Vec::new(), 5);
    node.status = NodeStatus::Ready;
    life.nodes.push(node);
    let position = life.nodes.len() - 1;
    let job = probe_job("buy");
    let live = live_snapshot();
    let orch: Arc<dyn Fleet> = IdleFleet::shared();
    let deps = BrainDeps::live();
    let mut fx = LiveFx::new(&orch, &job, &live, deps.clone());
    let mut inputs = crate::brain_launch_inputs::gather(&job, &live, &deps);
    fx.refresh_clock(&mut inputs);
    // What `launch_drive::finish` does with a placed buy.
    settle_market_buy(&mut life, position, 0, Ok(()), inputs.now);
    let entry = life.history.last().expect("history entry");
    assert_eq!(entry.outcome, "awaiting_fill");
    assert!(entry.started > 0 && entry.ended > 0, "unstamped history entry");
    assert!((now_wall() - entry.ended).abs() <= 5);
    // The next pass: the wait counts from the placement, not from 1970.
    let waited = crate::brain_launch::seconds_since_history(&life, "n9100", "awaiting_fill").expect("waiting");
    assert!((0..=5).contains(&waited), "waited {waited} s right after the placement");
    let deadline = (crate::brain_await::AWAIT_FILL_TICKS * crate::market_eta::live_tick_ms() / 1000) as i64;
    assert!(waited < deadline, "the fill wait expired at once ({waited} >= {deadline})");
    let mut probe = life.clone();
    let settled = crate::brain_await::settle_fill(&job, &mut probe, position, "logs", 5, waited, &deps);
    assert!(settled, "a fresh buy was settled as expired");
    assert!(probe.history.iter().all(|entry| entry.outcome != "failed" && entry.outcome != "abandoned"));
}

fn id_of(name: &str) -> i64 {
    lifescape_gamedata::objs_by_name(name).into_iter().next().map(|obj| obj.obj_id as i64).expect(name)
}

fn stack_of(name: &str, count: i64) -> Value {
    object(vec![("id", Value::Int(id_of(name))), ("name", Value::Str(name.to_string())), ("count", Value::Int(count))])
}

/// A raw world where EVERY field of `LaunchInputs` has a value other than
/// its blank one: the pack, bank and worn lists, the position, the flags,
/// the combat level, a resting sell (locks, listed), a resting bid and a
/// matched buy (committed coins, flat coins), two other bots at a site
/// (crowd, seats), a launch just noted, a report queued and pending.
/// The assertion destructures the struct without `..`: a field added to
/// `LaunchInputs` fails to compile here until it is asserted too.
#[test]
fn every_field_of_the_launch_inputs_is_read_from_the_raw_world() {
    let account = format!("{}every_field", crate::brain_commerce::PROBE_PREFIX);
    let (coal, iron, logs) = (id_of("coal"), id_of("iron_ore"), id_of("logs"));
    let skills: Vec<Value> = (0u8..23).map(|stat| object(vec![("id", Value::Int(i64::from(stat))), ("xp", Value::Int(388)), ("level", Value::Int(i64::from(stat) + 5))])).collect();
    let live = object(vec![
        ("skills", Value::Arr(skills)),
        ("inv", Value::Arr(vec![stack_of("coins", 500), stack_of("logs", 4)])),
        ("bank", Value::Arr(vec![stack_of("coal", 30), stack_of("iron_ore", 2)])),
        ("worn", Value::Arr(vec![stack_of("bronze_dagger", 1)])),
        ("combat", Value::Int(77)),
        ("position", Value::Arr(vec![Value::Int(0), Value::Int(3222), Value::Int(3218)])),
        ("inv_streamed", Value::Bool(true)),
        ("bank_streamed", Value::Bool(true)),
    ]);
    let site = (3200, 3201, 0);
    let seed = Seed {
        coins: 500,
        holdings: vec![(coal, 30), (iron, 2), (logs, 4)],
        asks: vec![(logs, 5, 20)],
        sells: vec![(coal, 10, 4_000)],
        buys: vec![(iron, 5, 1), (logs, 3, 25)],
        crowd: vec![(site, "farm:every_field".to_string()), (site, "farm:every_field".to_string())],
        orders: Vec::new(),
    };
    let market = TestMarket::open("every_field");
    let world = RawWorld::new(&account, live, Some(&market));
    world.seed(&seed);
    crate::brain_launch::note_launch(&account);
    crate::brain_inflight::queued(&account);
    world.job.inner.lock().expect("job").report_pending = true;
    let wall = now_wall();
    // The pass's handles are doubles: this world's market, a crowd of two bots at `site`.
    let deps = world.deps(&seed);
    let mut inputs = crate::brain_launch_inputs::gather(&world.job, &world.live, &deps);
    // The start-of-pass read leaves the market state for `gather_market`.
    assert!(inputs.locks.is_empty() && inputs.listed.is_empty() && inputs.commit_rows.is_empty() && inputs.commit_flat == 0 && inputs.crowd.is_empty() && inputs.seats.is_empty());
    crate::brain_launch_inputs::gather_market(&world.job, &mut inputs, &deps);
    let deal_price = market.service.lock_store().open_deals_for_account(&account).expect("deals").first().map(|deal| deal.price).expect("a matched buy deal");
    let LaunchInputs { position, ctx_position, inv_streamed, bank_streamed, combat, bank_types, stacks, worn, inv, bank, wearing, locks, listed, commit_rows, commit_flat, crowd, seats, recent_launch, report_in_flight, report_pending, now, tick_ms, levels, market_up, filler_ticks } = inputs;
    assert_eq!(position, Some((3222, 3218, 0)), "position");
    assert_eq!(ctx_position, Some((3222, 3218, 0)), "ctx_position");
    assert!(inv_streamed && bank_streamed, "streamed flags");
    assert_eq!(combat, 77, "combat");
    assert_eq!(bank_types, 2, "bank_types");
    let stack_names: Vec<&str> = stacks.iter().map(|(name, _)| name.as_str()).collect();
    for name in ["coins", "logs", "coal", "iron_ore"] {
        assert!(stack_names.contains(&name), "stacks lack {name}: {stacks:?}");
    }
    assert_eq!(worn, vec!["bronze_dagger".to_string()], "worn");
    assert_eq!((inv.len(), bank.len(), wearing.len()), (2, 2, 1), "raw pack, bank, worn");
    assert_eq!(locks.get(&coal), Some(&10), "locks: the resting sell");
    assert_eq!(listed.get(&coal), Some(&10), "listed: the resting sell");
    assert_eq!(commit_rows.len(), 1, "commit_rows: the resting bid only (the matched buy is a deal): {commit_rows:?}");
    assert_eq!(commit_rows[0].0, iron, "commit_rows item");
    assert_eq!(commit_rows[0].1, crate::market_units::deal_coins(iron, 5, 1), "commit_rows coins");
    assert_eq!(commit_flat, crate::market_units::deal_coins(logs, 3, deal_price), "commit_flat: the matched buy's coins");
    assert!(crowd.iter().any(|(cell, heads)| *cell == site && *heads == 2), "crowd: {crowd:?}");
    assert!(seats.iter().any(|(cell, method, heads)| *cell == site && method == "farm:every_field" && *heads == 2), "seats: {seats:?}");
    assert!(recent_launch, "recent_launch");
    assert!(report_in_flight, "report_in_flight");
    assert!(report_pending, "report_pending");
    assert!((wall - now).abs() <= 5, "now {now} is not the wall clock {wall}");
    assert_eq!(tick_ms, crate::market_eta::live_tick_ms(), "tick_ms");
    assert!(tick_ms > 0);
    assert_eq!(levels.len(), 23, "levels");
    assert!(levels.iter().all(|(name, level)| *level == crate::brain_succession::live_level(&world.live, name)) && levels.iter().any(|(_, level)| *level > 1));
    assert!(market_up, "market_up: the test market is the thread's service");
    assert_eq!(filler_ticks, crate::brain_filler::FILLER_TICKS, "filler_ticks");
    world.clear(&seed);
    crate::brain_inflight::folded(&account);
}

/// The resolve context the pass builds from the same raw world carries the
/// wallet, the holdings, the tools, the site facts and the fleet's sites
/// the inputs read (`build_ctx` copies them; none is set by a test).
#[test]
fn the_resolve_context_is_built_from_the_raw_world() {
    let account = format!("{}ctx_world", crate::brain_commerce::PROBE_PREFIX);
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lives");
    let mut life: Life = crate::brain_launch_equiv_tests::load_dir(&dir).into_iter().next().expect("fixture");
    let mut node = lifescape_planner::Node::new("n9200", NodeKind::ObtainItems { item: "logs".to_string(), count: 5 }, Vec::new(), 5);
    node.status = NodeStatus::Ready;
    life.nodes.push(node);
    let position = life.nodes.len() - 1;
    let logs = id_of("logs");
    let live = object(vec![
        ("skills", Value::Arr((0u8..23).map(|stat| object(vec![("id", Value::Int(i64::from(stat))), ("xp", Value::Int(xp_of_level(20))), ("level", Value::Int(20))])).collect())),
        ("inv", Value::Arr(vec![stack_of("coins", 900), stack_of("logs", 2), stack_of("bronze_axe", 1)])),
        ("bank", Value::Arr(Vec::new())),
        ("worn", Value::Arr(Vec::new())),
        ("combat", Value::Int(41)),
        ("position", Value::Arr(vec![Value::Int(0), Value::Int(3222), Value::Int(3218)])),
        ("inv_streamed", Value::Bool(true)),
        ("bank_streamed", Value::Bool(false)),
    ]);
    let market = TestMarket::open("ctx_world");
    let world = RawWorld::new(&account, live, Some(&market));
    let seed = Seed { coins: 900, holdings: vec![(logs, 2)], asks: vec![(logs, 50, 7)], ..Seed::default() };
    world.seed(&seed);
    let (inputs, ctx, committed) = world.launch_read(&life, position, &seed);
    world.clear(&seed);
    assert_eq!(ctx.coins, 900, "wallet");
    assert_eq!(ctx.held, 2, "held logs");
    assert_eq!(committed, 0);
    assert!(ctx.tools.iter().any(|tool| tool == "bronze_axe"), "carried tools: {:?}", ctx.tools);
    assert!(!ctx.bank_known, "bank not streamed");
    assert_eq!(ctx.combat, 41);
    assert_eq!(ctx.position, inputs.ctx_position);
    assert_eq!(ctx.best_ask, Some(7), "best ask of the book");
    // The context reads the base level from the experience.
    assert_eq!(ctx.levels.iter().find(|(name, _)| name == "mining").map(|(_, level)| *level), Some(20), "ctx levels: {:?}", ctx.levels);
    assert_eq!(inputs.level_of("mining"), 20);
    assert_eq!(ctx.crowd, inputs.crowd);
    assert_eq!(ctx.seats, inputs.seats);
    assert_eq!(ctx.ironman, life.profile.ironman);
    assert_eq!(ctx.draw_seed, lifescape_planner::guide_pick::seed_for(life.profile.seed, "n9200", life.memory.draws));
}

/// A clock double decides the pass's time, and `gather_market` reads it
/// again from the same handle: nothing of the pass names the wall clock.
#[test]
fn a_clock_double_decides_the_time_of_the_pass() {
    struct Fixed(std::sync::atomic::AtomicI64);
    impl crate::brain_deps::Clock for Fixed {
        fn now_secs(&self) -> i64 {
            self.0.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn tick_ms(&self) -> u64 {
            77
        }
    }
    let job = probe_job("clock_double");
    let live = live_snapshot();
    let clock = Arc::new(Fixed(std::sync::atomic::AtomicI64::new(1_234)));
    let deps = BrainDeps::with_market(None).with_clock(clock.clone());
    let mut inputs = crate::brain_launch_inputs::gather(&job, &live, &deps);
    assert_eq!((inputs.now, inputs.tick_ms, inputs.market_up), (1_234, 77, false));
    clock.0.store(5_678, std::sync::atomic::Ordering::SeqCst);
    crate::brain_launch_inputs::gather_market(&job, &mut inputs, &deps);
    assert_eq!((inputs.now, inputs.tick_ms), (5_678, 77), "the market read takes the time again from the handle");
}
