//! A4 (2) in the brain (user 2026-11-21): the one-time rebalance of the
//! fleet. No market, no fleet file: the demand is empty, the plan is
//! handed in.

use super::{apply_row, covers, rows_of, step, Marker, PlanRow};
use lifescape_json::{object, Value};
use lifescape_planner::{
    new_ambition, Ambition, Archetype, BotNow, FleetBot, GoalKind, Life, Pick, Profile, Reassignment, Stamp, MilestoneStatus,
};
use std::cell::Cell;

fn row(account: &str, from: &str, kind: &str) -> PlanRow {
    PlanRow { account: account.to_string(), from: from.to_string(), kind: kind.to_string(), why: "value:amulet_of_power".to_string(), numbers: "value 1 per 6000 ticks".to_string() }
}

fn marker() -> Marker {
    Marker { computed_secs: 5, plan: vec![row("a1", "woodcutter", "crafter"), row("A2", "fisher", "miner")], applied: Vec::new() }
}

fn ambition(kind: &str, why: &str) -> Ambition {
    new_ambition(kind, why, &BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 }, 7, &[], Stamp { tick: 0, secs: 1 }).expect("an ambition")
}

fn life(account: &str, held: Ambition) -> Life {
    Life {
        schema: 1,
        account: account.to_string(),
        created_at: 0,
        seed: 11,
        profile: Profile::new(false, 0.5, 0.5, 11, Archetype::Producer),
        goal: GoalKind::MakeMoney,
        nodes: Vec::new(),
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: Some(held),
        goal_origin: None,
        memory: Default::default(),
    }
}

fn live() -> Value {
    let skill = |id: i64, level: i64| object(vec![("id", Value::Int(id)), ("level", Value::Int(level))]);
    object(vec![("skills", Value::Arr((0..=20).map(|id| skill(id, 30)).collect()))])
}

/// The marker survives its file: rows, applied list.
#[test]
fn the_marker_round_trips_through_json() {
    let mut original = marker();
    original.applied.push("a1".to_string());
    let text = lifescape_json::stringify_pretty(&original.to_value());
    let back = Marker::from_value(&lifescape_json::parse(&text).expect("json")).expect("a marker");
    assert_eq!(back, original);
}

/// Planned once: with the marker present the plan is never computed again,
/// whatever the restarts; each bot gets its row once; an account without a
/// row (protected, operator, absent) gets nothing.
#[test]
fn the_pass_runs_once_and_each_row_applies_once() {
    let calls = Cell::new(0);
    let plan = || {
        calls.set(calls.get() + 1);
        Some(marker())
    };
    let first = step(None, &plan, "a1");
    assert_eq!(calls.get(), 1, "no marker: planned");
    assert_eq!(first.row.as_ref().map(|found| found.kind.as_str()), Some("crafter"));
    let persisted = first.persist.expect("the marker is written");
    assert_eq!(persisted.applied, vec!["a1".to_string()]);
    // A restart, then every other bot: the marker is there, no new plan.
    let second = step(Some(persisted.clone()), &plan, "a2");
    assert_eq!(calls.get(), 1, "the marker stops any new plan");
    assert_eq!(second.row.as_ref().map(|found| found.kind.as_str()), Some("miner"), "case-insensitive account");
    let again = step(second.persist, &plan, "a1");
    assert_eq!(again.row, None, "a1 applied its row already: no second change");
    for stranger in ["Zezima", "N0valyfe", "Yogosun", "pin_one"] {
        assert_eq!(step(again.persist.clone(), &plan, stranger).row, None, "{stranger} has no row");
    }
    assert_eq!(calls.get(), 1);
}

/// An incomplete fleet plans nothing and writes nothing (the next pass
/// tries again): the plan never covers the few bots a start registered.
#[test]
fn an_incomplete_fleet_is_not_planned() {
    let outcome = step(None, &|| None, "a1");
    assert_eq!(outcome, super::Step { persist: None, row: None });
    let fleet: std::collections::BTreeSet<String> = ["a1".to_string()].into_iter().collect();
    assert!(!covers(&fleet, &["a1".to_string(), "a2".to_string()]), "a2 has no job yet");
    assert!(covers(&fleet, &["a1".to_string()]));
    assert!(!covers(&std::collections::BTreeSet::new(), &[]), "no job at all is no fleet");
}

/// The planner's rebalance never lists the protected accounts nor the
/// operators: the rows the brain stores are the movable bots only.
#[test]
fn the_plan_rows_leave_out_the_protected_and_the_operators() {
    let bots: Vec<FleetBot> = ["Zezima", "N0valyfe", "Yogosun", "pinned", "worker"]
        .iter()
        .map(|name| FleetBot {
            account: name.to_string(),
            levels: Vec::new(),
            ironman: false,
            market_affinity: 0.5,
            seed: 1,
            kind: Some("woodcutter".to_string()),
            market_maker: false,
            operator: *name == "pinned",
        })
        .collect();
    let price = |_: &str| None;
    let xp = |_: &str, _: u8| None;
    let rate = |_: &str, _: u8| None;
    let context = lifescape_planner::ValueContext { bids: &[], alch_offers: &[], surplus: &[], price: &price, xp_rate: &xp, rate: &rate, roster: None, targets: &[] };
    let moved = lifescape_planner::rebalance(&bots, &context);
    let rows = rows_of(&moved);
    assert_eq!(rows.iter().map(|found| found.account.as_str()).collect::<Vec<_>>(), vec!["worker"]);
    let _ = Reassignment { account: String::new(), from: None, pick: Pick { kind: String::new(), score: 0.0, value_6000: 0.0, outlet_6000: 0.0, kept: false, holders: 0, driver: None, climb_ticks: 0.0 } };
}

/// A bot applies its row to ITS life: the kind changes, with a revision
/// `rebalance`; an operator or protected life never changes; a bot
/// already on the planned kind is only logged.
#[test]
fn applying_a_row_changes_only_a_movable_bot() {
    let deps = &crate::brain_deps::BrainDeps::live();
    let mut mover = life("a1", ambition("woodcutter", "random"));
    assert!(apply_row(&mut mover, &live(), &row("a1", "woodcutter", "crafter"), deps));
    let now = mover.ambition.as_ref().expect("an ambition");
    assert_eq!(now.kind, "crafter");
    assert_eq!(now.revisions.last().map(|revision| revision.why.as_str()), Some("rebalance"));
    assert!(now.milestones.iter().any(|stone| stone.status != MilestoneStatus::Done), "a fresh ladder");
    let mut protected = life("Zezima", ambition("woodcutter", "random"));
    assert!(!apply_row(&mut protected, &live(), &row("Zezima", "woodcutter", "crafter"), deps));
    assert_eq!(protected.ambition.as_ref().map(|held| held.kind.as_str()), Some("woodcutter"));
    let mut pinned = life("pin_one", ambition("alcher", "operator"));
    assert!(!apply_row(&mut pinned, &live(), &row("pin_one", "alcher", "crafter"), deps));
    assert_eq!(pinned.ambition.as_ref().map(|held| held.kind.as_str()), Some("alcher"));
    let mut same = life("a3", ambition("crafter", "random"));
    assert!(!apply_row(&mut same, &live(), &row("a3", "crafter", "crafter"), deps));
}

/// After the pass A2 holds: a bot that applied its row keeps its new
/// ambition, a second pass touches nothing (the row is applied), and the
/// ordinary draw never replaces a running ambition.
#[test]
fn after_the_pass_a_running_ambition_is_not_modified_again() {
    let deps = &crate::brain_deps::BrainDeps::live();
    let mut mover = life("a1", ambition("woodcutter", "random"));
    let plan = || Some(marker());
    let first = step(None, &plan, "a1");
    let row = first.row.expect("a row");
    assert!(apply_row(&mut mover, &live(), &row, deps));
    let after = mover.ambition.clone();
    let second = step(first.persist, &plan, "a1");
    assert_eq!(second.row, None);
    super::super::brain_ambition::before_draw(&mut mover, &live(), deps);
    assert_eq!(mover.ambition.as_ref().map(|held| (&held.kind, held.revisions.len())), after.as_ref().map(|held| (&held.kind, held.revisions.len())), "an ambition changes only once achieved");
}

/// A marker that cannot be written is a typed error (the pass then applies
/// nothing and stops, never replanning at every step).
#[test]
fn an_unwritable_marker_is_a_typed_error() {
    let missing = std::env::temp_dir().join("lifescape-no-such-dir-a4").join("marker.json");
    let error = super::write_marker_at(&missing, &marker()).expect_err("the directory is missing");
    assert!(matches!(&error, super::MarkerError::Write { path, .. } if path.ends_with("marker.json")), "{error:?}");
    assert!(error.to_string().contains("not written"), "{error}");
    let good = std::env::temp_dir().join(format!("lifescape-marker-{}.json", std::process::id()));
    super::write_marker_at(&good, &marker()).expect("written");
    assert!(good.exists());
    let _ = std::fs::remove_file(&good);
}

/// A row planned from an ambition the bot no longer holds is stale: the bot
/// was changed meanwhile (A4), the row applies nothing.
#[test]
fn a_stale_row_applies_nothing() {
    let deps = &crate::brain_deps::BrainDeps::live();
    let mut moved_on = life("a1", ambition("fisher", "value"));
    assert!(!apply_row(&mut moved_on, &live(), &row("a1", "woodcutter", "crafter"), deps), "planned from woodcutter, the bot holds fisher");
    assert_eq!(moved_on.ambition.as_ref().map(|held| held.kind.as_str()), Some("fisher"));
    assert!(moved_on.ambition.as_ref().is_some_and(|held| held.revisions.is_empty()));
    let mut empty_from = life("a2", ambition("fisher", "value"));
    assert!(!apply_row(&mut empty_from, &live(), &row("a2", "", "crafter"), deps), "planned with no ambition, the bot holds one now");
}
