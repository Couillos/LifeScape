//! `migrate-lives` (lot 5): the D3 mark applied offline, the A4 rows
//! reported. The equivalence with the old loop (migration, then the loop
//! without the rules, equals the loop that ran them) lives in
//! `brain_upkeep_equiv_tests`.

use std::path::{Path, PathBuf};

use lifescape_planner::Life;

use super::{migrate_life, row_effect};
use crate::brain_rebalance::{Marker, PlanRow};

fn load_dir(dir: &Path) -> Vec<Life> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir).expect("lives dir").flatten().map(|entry| entry.path()).filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json")).collect();
    paths.sort();
    paths
        .iter()
        .filter_map(|path| lifescape_planner::model::life_from_value(&lifescape_json::parse(&std::fs::read_to_string(path).ok()?).ok()?))
        .collect()
}

fn fixtures() -> Vec<Life> {
    load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../orch-brain/tests/fixtures/lives"))
}

/// An old-rule ambition (drawn by need before the mark) is marked once,
/// never retired; marked, level-led and operator ambitions are left alone.
#[test]
fn the_d3_mark_is_applied_once_and_only_to_an_old_rule_ambition() {
    let mut life = fixtures().into_iter().find(|life| life.ambition.is_some()).expect("a life with an ambition");
    let ambition = life.ambition.as_mut().expect("ambition");
    ambition.draw_rule.clear();
    ambition.why = "need:copper_ore".to_string();
    let kind = ambition.kind.clone();
    let line = migrate_life(&mut life).expect("marked");
    assert!(line.contains("marked need (D3)"), "{line}");
    let ambition = life.ambition.as_ref().expect("ambition");
    assert_eq!((ambition.draw_rule.as_str(), ambition.kind.as_str()), ("need", kind.as_str()), "marked, never retired");
    assert_eq!(migrate_life(&mut life), None, "a second run changes nothing");
    for (rule, why) in [("level", "need"), ("operator", "operator"), ("", "value:amulet_of_power")] {
        let held = life.ambition.as_mut().expect("ambition");
        held.draw_rule = rule.to_string();
        held.why = why.to_string();
        assert_eq!(migrate_life(&mut life), None, "rule {rule:?}, why {why:?}");
    }
    life.ambition = None;
    assert_eq!(migrate_life(&mut life), None, "no ambition, nothing to mark");
}

fn marker(applied: &[&str]) -> Marker {
    let row = |account: &str, from: &str, kind: &str| PlanRow { account: account.to_string(), from: from.to_string(), kind: kind.to_string(), why: "value:x".to_string(), numbers: "1".to_string() };
    Marker { computed_secs: 1, plan: vec![row("a1", "miner", "crafter"), row("a2", "miner", "miner")], applied: applied.iter().map(|name| name.to_string()).collect() }
}

/// A pending A4 row is reported with what it would do; an applied row, or
/// a bot outside the plan, is not.
#[test]
fn a_pending_a4_row_is_reported_and_an_applied_one_is_not() {
    let mut life = fixtures().into_iter().find(|life| life.ambition.is_some()).expect("a life with an ambition");
    life.ambition.as_mut().expect("ambition").kind = "miner".to_string();
    life.ambition.as_mut().expect("ambition").pin = None;
    life.account = "a1".to_string();
    let (account, effect) = row_effect(&life, &marker(&[])).expect("pending");
    assert_eq!(account, "a1");
    assert!(effect.starts_with("give"), "{effect}");
    life.account = "a2".to_string();
    let (_, effect) = row_effect(&life, &marker(&[])).expect("pending");
    assert!(effect.starts_with("kept"), "already on the kind: {effect}");
    life.account = "a1".to_string();
    assert_eq!(row_effect(&life, &marker(&["A1"])), None, "applied (case-insensitive)");
    life.account = "outside_the_plan".to_string();
    assert_eq!(row_effect(&life, &marker(&[])), None);
}

/// The real state, read only: how many lives need the D3 mark, how many
/// A4 rows are pending. `LIFESCAPE_LIVES_DIR=run/lives` and
/// `LIFESCAPE_A4_MARKER=run/ambition_rebalance_a4c.json`.
#[test]
#[ignore = "reads the real lives and marker: LIFESCAPE_LIVES_DIR=... LIFESCAPE_A4_MARKER=... cargo test --release migration_need_on_the_real_state -- --ignored --nocapture"]
fn migration_need_on_the_real_state() {
    let source = std::env::var("LIFESCAPE_LIVES_DIR").expect("set LIFESCAPE_LIVES_DIR");
    let lives = load_dir(Path::new(&source));
    let marker = std::fs::read_to_string(std::env::var("LIFESCAPE_A4_MARKER").expect("set LIFESCAPE_A4_MARKER"))
        .ok()
        .and_then(|text| lifescape_json::parse(&text).ok())
        .and_then(|value| Marker::from_value(&value))
        .expect("marker");
    let mut d3 = Vec::new();
    let mut a4 = Vec::new();
    for life in &lives {
        let mut copy = life.clone();
        if migrate_life(&mut copy).is_some() {
            d3.push(life.account.clone());
        }
        a4.extend(row_effect(life, &marker));
    }
    eprintln!("real state: {} lives, D3 mark needed by {} {d3:?}, A4 rows pending {} {a4:?} (marker: {} rows, {} applied)", lives.len(), d3.len(), a4.len(), marker.plan.len(), marker.applied.len());
}
