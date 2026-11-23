//! Life files and retirement (user request 2026-11-14: finished tasks
//! leave the graph). An old file is retired on first read and written
//! small; the hot load/save path gets cheaper with it.
//!
//! The ignored bench measures real files on a COPY of `run/lives`:
//! `LIFESCAPE_BENCH_LIVES=/copy/lives cargo test -p lifescape-orchestrator
//! --lib life_retire_bench -- --ignored --nocapture`

use lifescape_json::Value;
use lifescape_planner::{life_from_value, life_to_value, Archetype, GoalKind, HistoryEntry, Life, Node, NodeKind, NodeStatus, Profile};

use crate::brain::{read_life_file, write_life_file};

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lifescape-retire-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// A life as the brain left them before the retirement: 400 nodes over
/// many goals (most done, old abandons with cascades), 3000 history
/// entries, the current mining goal stuck on its own abandon.
fn grown_life() -> Life {
    let mut nodes = Vec::new();
    for number in 1..=396 {
        let mut node = Node::new(&format!("n{number}"), NodeKind::ObtainItems { item: "logs".to_string(), count: 28 }, Vec::new(), 28);
        node.status = if number % 4 == 0 { NodeStatus::Abandoned } else { NodeStatus::Done };
        if number % 4 == 0 && number > 4 {
            node.deps = vec![format!("n{}", number - 4)];
            node.last_error = format!("dependency n{} abandoned", number - 4);
        }
        nodes.push(node);
    }
    let mut ore = Node::new("n397", NodeKind::ObtainItems { item: "tin_ore".to_string(), count: 14 }, Vec::new(), 14);
    ore.status = NodeStatus::Abandoned;
    ore.last_error = "Failed: no rocks".to_string();
    ore.failures = 3;
    let mut climb = Node::new("n398", NodeKind::ReachLevel { skill: "mining".to_string(), level: 55 }, vec!["n397".to_string()], 55);
    climb.status = NodeStatus::Abandoned;
    climb.last_error = "dependency n397 abandoned".to_string();
    nodes.push(ore);
    nodes.push(climb);
    let history = (0..3000)
        .map(|at| HistoryEntry { node: format!("n{}", at % 398 + 1), started: at, ended: at, outcome: "launched".to_string() })
        .collect();
    Life {
        schema: 1,
        account: "retire-test".to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer),
        goal: GoalKind::MaxSkills { skills: vec!["mining".to_string()], level: 55 },
        nodes,
        engagement: None,
        active: None,
        history,
        ambition: None,
        goal_origin: Some("m1".to_string()),
        memory: Default::default(),
    }
}

/// The JSON of a file written before the memory existed.
fn old_format(life: &Life) -> String {
    let mut value = life_to_value(life);
    if let Value::Obj(map) = &mut value {
        map.remove("memory");
    }
    lifescape_json::stringify(&value)
}

#[test]
fn an_old_life_file_is_retired_on_load_and_written_small() {
    let dir = temp_dir("load");
    let path = dir.join("retire-test.json");
    let grown = grown_life();
    let before = old_format(&grown);
    std::fs::write(&path, &before).expect("old file");
    let loaded = read_life_file(&path).expect("old file loads");
    let ids: Vec<&str> = loaded.nodes.iter().map(|node| node.id.as_str()).collect();
    assert_eq!(ids, vec!["n397", "n398"], "only the current goal's stuck work stays");
    assert_eq!(loaded.goal, grown.goal, "the goal is kept");
    assert_eq!(loaded.goal_origin.as_deref(), Some("m1"), "and its milestone");
    assert_eq!(loaded.memory.last_number, 398);
    assert_eq!(lifescape_planner::next_id(&loaded), "n399", "no id reused");
    assert!(lifescape_planner::stuck_cause(&loaded).is_some_and(|cause| cause.node == "n397"), "the ban still finds its root");
    // The brain saves the life it loaded: the file on disk is the small one.
    let written = write_life_file(&path, &grown).expect("save").expect("a retired copy");
    let after = std::fs::read_to_string(&path).expect("written");
    assert!(after.len() * 5 < before.len(), "{} bytes -> {} bytes", before.len(), after.len());
    let back = life_from_value(&lifescape_json::parse(&after).expect("json")).expect("life");
    assert_eq!(back, written, "the disk holds the retired copy");
    assert_eq!(read_life_file(&path).expect("reload"), written, "a reload retires nothing more");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_saved_life_never_carries_done_nodes() {
    let dir = temp_dir("save");
    let path = dir.join("retire-test.json");
    let mut life = grown_life();
    life.nodes.retain(|node| node.status != NodeStatus::Done);
    life.memory.floor = Some(397);
    let mut fresh = Node::new("n399", NodeKind::ObtainItems { item: "bronze_pickaxe".to_string(), count: 1 }, Vec::new(), 1);
    fresh.status = NodeStatus::Done;
    life.nodes.push(fresh);
    write_life_file(&path, &life).expect("save");
    let back = read_life_file(&path).expect("reload");
    assert!(back.nodes.iter().all(|node| node.status != NodeStatus::Done));
    assert_eq!(back.memory.recent.last().map(|row| row.id.as_str()), Some("n399"));
    assert!(back.history.len() <= lifescape_planner::lifecycle_retire::HISTORY_COMPACT_AT);
    let _ = std::fs::remove_dir_all(&dir);
}

fn timed_ms<R>(rounds: u32, mut work: impl FnMut() -> R) -> f64 {
    let _ = work();
    let probe = crate::cpu_profile::Probe::start();
    for _ in 0..rounds {
        let _ = work();
    }
    let (_, cpu) = probe.elapsed();
    cpu as f64 / 1e6 / f64::from(rounds)
}

/// Size and load/save CPU of real lives, before and after retirement.
#[test]
#[ignore]
fn life_retire_bench() {
    let Ok(dir) = std::env::var("LIFESCAPE_BENCH_LIVES") else { return };
    assert!(!dir.starts_with("/home/josh/dev/LifeScape/run"), "bench on a copy only");
    let mut names: Vec<std::path::PathBuf> = std::fs::read_dir(&dir).expect("lives copy").flatten().map(|entry| entry.path()).collect();
    names.sort();
    println!("{:<14} {:>9} {:>9} {:>6} {:>6} {:>6} {:>6} {:>8} {:>8} {:>8} {:>8}", "life", "bytes", "after", "nodes", "after", "hist", "after", "load ms", "after", "save ms", "after");
    for path in names.iter().filter(|path| path.extension().is_some_and(|ext| ext == "json")) {
        let text = std::fs::read_to_string(path).expect("read");
        let Some(life) = lifescape_json::parse(&text).ok().as_ref().and_then(life_from_value) else { continue };
        let mut retired = life.clone();
        lifescape_planner::retire_finished(&mut retired);
        let after = lifescape_json::stringify(&life_to_value(&retired));
        let load = |raw: &str| lifescape_json::parse(raw).ok().as_ref().and_then(life_from_value).map(|life| life.nodes.len());
        let save = |life: &Life| lifescape_json::stringify(&life_to_value(life)).len();
        let name = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or("?");
        println!(
            "{name:<14} {:>9} {:>9} {:>6} {:>6} {:>6} {:>6} {:>8.3} {:>8.3} {:>8.3} {:>8.3}",
            text.len(),
            after.len(),
            life.nodes.len(),
            retired.nodes.len(),
            life.history.len(),
            retired.history.len(),
            timed_ms(10, || load(&text)),
            timed_ms(10, || load(&after)),
            timed_ms(10, || save(&life)),
            timed_ms(10, || save(&retired)),
        );
    }
}
