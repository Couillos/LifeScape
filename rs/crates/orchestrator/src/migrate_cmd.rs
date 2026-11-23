//! `orchestrator migrate-lives [--check]` (lot 5 of the launch refactor,
//! 2026-11-23): the two one-time rules that ran in every launch pass for
//! every bot, now applied offline, on demand.
//!
//! - D3 mark (`upkeep_ambition::review_mark`, user 2026-11-19, A2
//!   2026-11-21): an ambition drawn by need before the `need` mark existed
//!   (`draw_rule` empty, `why` `need` or `need:<item>`) is only marked
//!   `need`, never retired. It reads the life alone: applied here, life by
//!   life, each changed life saved.
//! - A4 rebalance (`upkeep_ambition::row_verdict`, plan marker
//!   `run/ambition_rebalance_a4c.json`): a bot applies its row of the
//!   fleet plan once. Giving a row's kind draws an ambition from the
//!   fleet, the market and the live levels (`assign_rebalanced`): it cannot
//!   run offline. The command only REPORTS the rows still pending in the
//!   marker (account, and what the row would do: nothing, kept, or give).
//!   The loop no longer applies it: the plan was applied once (marker
//!   `a4c`, every row applied), a missing marker never replans the fleet.
//!
//! Offline: refuses while the daemon runs (the daemon saves the lives; a
//! write here would race it). `--check` lists what would change and
//! writes nothing. Exit 0 when nothing is left to do or everything was
//! applied, 1 on a refusal or an unreadable life.

#![forbid(unsafe_code)]

use lifescape_planner::upkeep_ambition::{self as ambition, RebalanceRow, RowVerdict};
use lifescape_planner::Life;

use crate::brain_rebalance::Marker;

pub use crate::brain_ambition::migrate_life;

/// What a pending A4 row would do to the life that holds it.
pub fn row_effect(life: &Life, marker: &Marker) -> Option<(String, &'static str)> {
    let row = marker.pending_for(&life.account)?;
    let planned = RebalanceRow { from: row.from.clone(), kind: row.kind.clone(), why: row.why.clone(), numbers: row.numbers.clone() };
    let effect = match ambition::row_verdict(life, &planned) {
        RowVerdict::Nothing => "nothing (no ambition held)",
        RowVerdict::Kept(_) => "kept (operator, protected, stale or already on the kind): only the mark would change",
        RowVerdict::Give => "give: draws an ambition, needs the live fleet (cannot run offline)",
    };
    Some((life.account.clone(), effect))
}

/// The files of the lives directory, `*.json` only (temporary files of an
/// interrupted save are never read).
fn life_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(dir).map(|entries| entries.flatten().map(|entry| entry.path()).collect()).unwrap_or_default();
    paths.retain(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"));
    paths.sort();
    paths
}

/// CLI entry; returns the process exit code.
pub fn migrate_lives_cli(args: &[String]) -> i32 {
    let check = args.iter().any(|arg| arg == "--check");
    if let Some(unknown) = args.iter().find(|arg| arg.as_str() != "--check") {
        eprintln!("migrate-lives: unknown argument {unknown} (usage: migrate-lives [--check])");
        return 1;
    }
    let daemon_pid = std::fs::read_to_string(crate::paths::pid_path()).ok().and_then(|raw| raw.trim().parse::<u32>().ok());
    if daemon_pid.is_some_and(crate::worker_owner::pid_alive) {
        eprintln!("migrate-lives: the daemon runs (pid {}); stop it first (shutdown)", daemon_pid.unwrap_or(0));
        return 1;
    }
    let dir = crate::paths::lives_dir();
    let marker = std::fs::read_to_string(crate::brain_rebalance::marker_path()).ok().and_then(|text| lifescape_json::parse(&text).ok()).and_then(|value| Marker::from_value(&value));
    let (mut lives, mut marked, mut unreadable) = (0, Vec::new(), 0);
    let mut pending = Vec::new();
    for path in life_files(&dir) {
        let Some(mut life) = crate::brain::read_life_file(&path) else {
            eprintln!("migrate-lives: {} unreadable, left as is", path.display());
            unreadable += 1;
            continue;
        };
        lives += 1;
        if let Some(marker) = &marker {
            pending.extend(row_effect(&life, marker));
        }
        if let Some(line) = migrate_life(&mut life) {
            println!("{line}");
            if !check {
                if let Err(error) = crate::brain::save_life(&life) {
                    eprintln!("migrate-lives: {error}");
                    return 1;
                }
            }
            marked.push(life.account.clone());
        }
    }
    let verb = if check { "would be marked" } else { "marked" };
    println!("migrate-lives: {lives} lives read, D3 mark: {} {verb} {marked:?}", marked.len());
    match &marker {
        None => println!("migrate-lives: A4 marker {} absent or unreadable: the rebalance is never planned again (the loop no longer runs it)", crate::brain_rebalance::marker_path().display()),
        Some(_) if pending.is_empty() => println!("migrate-lives: A4 rebalance: no row pending in the marker"),
        Some(_) => {
            println!("migrate-lives: A4 rebalance: {} row(s) still pending, not applied (the loop no longer runs it):", pending.len());
            for (account, effect) in &pending {
                println!("  {account}: {effect}");
            }
        }
    }
    i32::from(unreadable > 0)
}

#[cfg(test)]
#[path = "migrate_cmd_tests.rs"]
mod tests;
