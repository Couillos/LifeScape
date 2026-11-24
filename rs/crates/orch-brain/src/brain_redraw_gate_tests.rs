//! The redraw gate (lot 3.3): the fingerprint moves with each family of
//! inputs and only with them, the modes behave as documented, and, through
//! the production redraw (`brain_succession::redraw`) over raw worlds, the
//! gate never says "skip" where the old code draws something or changes the
//! life (the divergence the live shadow hour must also show none of).

use std::path::Path;

use lifescape_json::{object, Value};
use lifescape_planner::{HistoryEntry, Life};

use super::{fingerprint, Fingerprint, Gate, Mode, MAX_SKIP_SECS};
use crate::brain_raw_world_tests::{place, wall_now, xp_of_level, RawWorld, TestMarket};

fn fixtures() -> Vec<Life> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lives");
    crate::brain_launch_equiv_tests::load_dir(&dir)
}

fn entry(outcome: &str, at: i64) -> HistoryEntry {
    HistoryEntry { node: "goal".to_string(), started: at, ended: at, outcome: outcome.to_string() }
}

fn live_at(level: u8) -> Value {
    let skills: Vec<Value> = (0u8..23).map(|stat| object(vec![("id", Value::Int(i64::from(stat))), ("xp", Value::Int(xp_of_level(level))), ("level", Value::Int(i64::from(level)))])).collect();
    object(vec![("skills", Value::Arr(skills)), ("inv", Value::Arr(Vec::new())), ("bank", Value::Arr(Vec::new())), ("worn", Value::Arr(Vec::new()))])
}

fn closed(mut life: Life, account: &str) -> Life {
    life.account = account.to_string();
    life.history.push(entry(&format!("{}mining", lifescape_planner::goals::DONE_TAG), wall_now() - 100));
    life
}

#[test]
fn the_mode_reads_the_switch_and_defaults_to_shadow() {
    assert_eq!(Mode::parse(""), Mode::Shadow);
    assert_eq!(Mode::parse("shadow"), Mode::Shadow);
    assert_eq!(Mode::parse("garbage"), Mode::Shadow);
    assert_eq!(Mode::parse("ON"), Mode::On);
    assert_eq!(Mode::parse(" on "), Mode::On);
    assert_eq!(Mode::parse("off"), Mode::Off);
    assert_eq!(Mode::parse("0"), Mode::Off);
}

fn moved(before: &Fingerprint, after: &Fingerprint) -> Vec<&'static str> {
    let mut parts = Vec::new();
    for (name, left, right) in [("life", before.life, after.life), ("bans", before.bans, after.bans), ("live", before.live, after.live), ("market", before.market, after.market), ("guides", before.guides, after.guides), ("fleet", before.fleet, after.fleet)] {
        if left != right {
            parts.push(name);
        }
    }
    parts
}

#[test]
fn the_fingerprint_moves_with_each_family_and_only_with_them() {
    let base = closed(fixtures().into_iter().next().expect("fixture"), "gate_fp_a");
    let live = live_at(30);
    let market = TestMarket::open("gate_fp");
    // The fleet's crowd is the process's own (the test writes it below).
    let deps = &crate::brain_deps::BrainDeps::with_market(Some(market.service.clone()));
    {
        let print = fingerprint(&base, &live, deps);
        // Nothing changed: the same print, twice.
        assert_eq!(moved(&print, &fingerprint(&base, &live, deps)), Vec::<&str>::new());
        // The life: a history entry.
        let mut grown = base.clone();
        grown.history.push(entry("next:mining 40", wall_now()));
        assert!(moved(&print, &fingerprint(&grown, &live, deps)).contains(&"life"));
        // The bot: a level.
        assert!(moved(&print, &fingerprint(&base, &live_at(31), deps)).contains(&"live"));
        // The market: an order in a book.
        let service = &market.service;
        let item = crate::brain_raw_world_tests::object_id("logs").expect("logs");
        service.sync_holdings("gate_fp_b", 1_000_000, &[(item, 100)]);
        place(service, "gate_fp_b", "buy", item, 5, 3);
        assert!(moved(&print, &fingerprint(&base, &live, deps)).contains(&"market"));
        // The fleet: another bot takes a site.
        let after_market = fingerprint(&base, &live, deps);
        crate::site_crowd::note("gate_fp_other", Some((3200, 3200, 0)), "farm:gate_fp_logs");
        let crowded = fingerprint(&base, &live, deps);
        crate::site_crowd::note("gate_fp_other", None, "");
        assert!(moved(&after_market, &crowded).contains(&"fleet"));
        service.retire_account("gate_fp_b", "gate");
    }
}

/// A ban crossing its expiry moves the bans print though no entry was
/// written (the clock alone changed the state).
#[test]
fn a_ban_running_out_moves_the_bans_print() {
    let deps = &crate::brain_deps::BrainDeps::live();
    let mut life = closed(fixtures().into_iter().next().expect("fixture"), "gate_fp_ban");
    let live = live_at(30);
    let ticks = 1_000u64;
    let seconds_for = |ticks: u64| (ticks * crate::market_eta::live_tick_ms() / 1000) as i64;
    let now = wall_now();
    let ban = format!("{}mining@{ticks}:contract error (node n1, x)", lifescape_planner::goals::STUCK_TAG);
    life.history.push(entry(&ban, now - seconds_for(ticks) + 3));
    let live_ban = fingerprint(&life, &live, deps).bans;
    assert_eq!(fingerprint(&life, &live, deps).bans, live_ban, "stable while the ban runs");
    let mut over = life.clone();
    over.history.pop();
    over.history.push(entry(&ban, now - seconds_for(ticks) - 3));
    assert_ne!(fingerprint(&over, &live, deps).bans, live_ban, "the expiry is in the print");
}

fn scripted(outcomes: Vec<(Option<&'static str>, bool)>) -> impl FnMut(&mut Life, &Value, &crate::brain_deps::BrainDeps) -> (Option<String>, bool) {
    let mut queue = outcomes.into_iter();
    move |_life, _live, _deps| queue.next().map(|(label, changed)| (label.map(str::to_string), changed)).unwrap_or((None, false))
}

#[test]
fn a_repeated_retry_is_skipped_in_on_mode_and_only_counted_in_shadow() {
    let deps = &crate::brain_deps::BrainDeps::live();
    let live = live_at(30);
    for (mode, account) in [(Mode::Shadow, "gate_mode_shadow"), (Mode::On, "gate_mode_on")] {
        let gate = Gate::default();
        let mut life = closed(fixtures().into_iter().next().expect("fixture"), account);
        let now = wall_now();
        let ran = std::cell::Cell::new(0);
        let run = |life: &mut Life, at: i64| {
            gate.redraw_in(mode, at, life, &live, |_, _, _| {
                ran.set(ran.get() + 1);
                (None, false)
            }, deps)
        };
        // First retry: nothing known, it runs.
        assert_eq!(run(&mut life, now), (None, false));
        // Same state a few seconds later: skippable.
        assert_eq!(run(&mut life, now + 10), (None, false));
        assert_eq!(run(&mut life, now + 20), (None, false));
        // Past the age bound: it runs again.
        assert_eq!(run(&mut life, now + 20 + MAX_SKIP_SECS + 1), (None, false));
        let counts = gate.counts();
        if mode == Mode::On {
            assert_eq!(ran.get(), 2, "on: the two skippable retries did not run");
            assert_eq!((counts.retried, counts.skipped, counts.skip_agreed), (2, 2, 0));
        } else {
            assert_eq!(ran.get(), 4, "shadow: the old code ran every time");
            assert_eq!((counts.retried, counts.skipped, counts.skip_agreed, counts.skip_drew, counts.skip_changed), (2, 0, 2, 0, 0));
        }
        // A changed state runs at once.
        life.history.push(entry("next:mining 41", now + 400));
        let before = gate.counts().retried;
        run(&mut life, now + 401);
        assert_eq!(gate.counts().retried, before + 1);
    }
}

#[test]
fn a_skippable_retry_the_old_code_answered_is_a_divergence() {
    let deps = &crate::brain_deps::BrainDeps::live();
    let live = live_at(30);
    let gate = Gate::default();
    let mut life = closed(fixtures().into_iter().next().expect("fixture"), "gate_divergence");
    let now = wall_now();
    let mut old = scripted(vec![(None, false), (Some("mining 50"), true), (None, false), (None, true)]);
    gate.redraw_in(Mode::Shadow, now, &mut life, &live, &mut old, deps);
    // The fake draws without touching the life: the print is unchanged, a skip was allowed.
    let drawn = gate.redraw_in(Mode::Shadow, now + 5, &mut life, &live, &mut old, deps);
    assert_eq!(drawn.0.as_deref(), Some("mining 50"), "shadow: the old answer is the answer");
    assert_eq!(gate.counts().skip_drew, 1);
    // A drawn goal forgets the attempt: the next retry runs, it is no skip.
    gate.redraw_in(Mode::Shadow, now + 6, &mut life, &live, &mut old, deps);
    assert_eq!(gate.counts().retried, 2);
    gate.redraw_in(Mode::Shadow, now + 7, &mut life, &live, &mut old, deps);
    assert_eq!(gate.counts().skip_changed, 1, "a changed life with no goal is counted too");
}

/// The proof through production code: `brain_succession::redraw` on closed
/// goals of the fixture lives, in raw worlds that change between passes
/// (a bid, a level, a stack, bans, an ambition given or taken, a
/// holdings-less market), the gate in shadow mode: it must never allow a
/// skip the old redraw answers with a goal or a changed life.
fn shadow_run(lives: &[Life], scenarios: usize, seed: u64) -> (u64, u64, u64, u64) {
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
    }
    let gate = Gate::default();
    let market = TestMarket::open("gate_shadow");
    let mut rng = Rng(seed);
    let tradeable: Vec<i64> = ["logs", "iron_ore", "coal", "raw_shrimp", "bronze_bar", "uncut_sapphire", "feather", "copper_ore"].iter().filter_map(|name| crate::brain_raw_world_tests::object_id(name)).collect();
    for (index, base) in lives.iter().enumerate() {
        for scenario in 0..scenarios {
            let account = format!("{}gate_{index}_{scenario}", crate::brain_commerce::PROBE_PREFIX);
            let mut life = closed(base.clone(), &account);
            if rng.below(100) < 30 {
                life.ambition = None;
            }
            let mut level = [5u8, 20, 45, 70][rng.below(4) as usize];
            let mut live = live_at(level);
            let world = RawWorld::new(&account, live.clone(), Some(&market));
            let deps = world.deps(&crate::brain_raw_world_tests::Seed::default());
            for pass in 0..8 {
                match rng.below(10) {
                    // A bid of another account appears (the market moves).
                    0 => {
                        let item = tradeable[rng.below(tradeable.len() as u64) as usize];
                        let other = format!("{account}_bidder");
                        market.service.sync_holdings(&other, 50_000_000, &[]);
                        place(&market.service, &other, "buy", item, 1 + rng.below(40) as i64, 200 + rng.below(5_000) as i64);
                    }
                    // The bot levels up.
                    1 => {
                        level = (level + 10).min(99);
                        live = live_at(level);
                    }
                    // A ban lands on a skill, then an unban.
                    2 => life.history.push(entry(&format!("{}mining@600000:contract error (node n1, x)", lifescape_planner::goals::STUCK_TAG), wall_now())),
                    3 => life.history.push(entry(&format!("{}mining", lifescape_planner::bans::UNBAN_TAG), wall_now())),
                    // The ambition is given back or taken.
                    4 => life.ambition = if life.ambition.is_some() { None } else { base.ambition.clone() },
                    _ => {}
                }
                let at = wall_now();
                gate.redraw_in(Mode::Shadow, at, &mut life, &live, crate::brain_succession::redraw, &deps);
                // Close the goal again after a draw, to keep the retry going.
                if !lifescape_planner::goal_closed(&life.history) && pass < 7 {
                    life.history.push(entry(&format!("{}mining", lifescape_planner::goals::DONE_TAG), wall_now()));
                }
            }
            world.clear(&crate::brain_raw_world_tests::Seed::default());
            market.service.retire_account(&format!("{account}_bidder"), "gate");
            crate::market_claims::release(&account);
            crate::site_crowd::disengage(&account);
        }
    }
    let counts = gate.counts();
    (counts.retried, counts.skip_agreed, counts.skip_drew, counts.skip_changed)
}

#[test]
fn the_shadow_gate_never_skips_a_retry_the_old_redraw_answers_on_the_fixture_lives() {
    let lives = fixtures();
    let (retried, agreed, drew, changed) = shadow_run(&lives, 40, 0x6A7E);
    eprintln!("redraw gate (fixtures): {} lives, retried {retried}, skip agreed {agreed}, skip but drew {drew}, skip but changed {changed}", lives.len());
    assert_eq!((drew, changed), (0, 0), "the gate would have skipped something the old code did");
    assert!(agreed > 0, "the gate never skipped: the proof is empty");
}

/// Every life of `LIFESCAPE_LIVES_DIR` (a copy of `run/lives`, never written).
#[test]
#[ignore = "reads the real lives: LIFESCAPE_LIVES_DIR=/path/to/copy cargo test --profile release-test the_shadow_gate_never_skips_a_retry_the_old_redraw_answers_on_the_real_lives -- --ignored --nocapture"]
fn the_shadow_gate_never_skips_a_retry_the_old_redraw_answers_on_the_real_lives() {
    let source = std::env::var("LIFESCAPE_LIVES_DIR").expect("set LIFESCAPE_LIVES_DIR to a copy of run/lives");
    let temp = std::env::temp_dir().join(format!("lifescape_gate_lives_{}", std::process::id()));
    std::fs::create_dir_all(&temp).expect("temp dir");
    for entry in std::fs::read_dir(&source).expect("source dir").flatten() {
        if entry.path().extension().and_then(|ext| ext.to_str()) == Some("json") {
            std::fs::copy(entry.path(), temp.join(entry.file_name())).expect("copy");
        }
    }
    let lives = crate::brain_launch_equiv_tests::load_dir(&temp);
    let (retried, agreed, drew, changed) = shadow_run(&lives, 40, 0x6A7E_0001);
    eprintln!("redraw gate (real lives): {} lives, retried {retried}, skip agreed {agreed}, skip but drew {drew}, skip but changed {changed}", lives.len());
    assert_eq!((drew, changed), (0, 0), "the gate would have skipped something the old code did");
    let _ = std::fs::remove_dir_all(&temp);
}
