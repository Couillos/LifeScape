//! `orchestrator purge-measures-v1 [--confirm]` (plan rendements D8,
//! lot R1): removes the measure-1 yields (net-diff reports) so the rates
//! restart from the contract-2 ledger measures and the game priors.
//!
//! Destructive and confirmed by the user only: without `--confirm` it
//! prints what it would delete and touches nothing. It refuses while the
//! daemon runs (the daemon holds the snapshot in memory and would write
//! it back, and an older daemon cannot open a schema-5 file): stop the
//! daemon, purge, start the new build.
//! Deletes: the `task_reports` rows of measure version 1 with their
//! `task_yields` lines, and `run/rates-snapshot.json`.

#![forbid(unsafe_code)]

/// CLI entry; returns the process exit code.
pub fn purge_cli(args: &[String]) -> i32 {
    let confirm = args.iter().any(|arg| arg == "--confirm");
    let daemon_pid = std::fs::read_to_string(crate::paths::pid_path()).ok().and_then(|raw| raw.trim().parse::<u32>().ok());
    if daemon_pid.is_some_and(crate::worker_owner::pid_alive) {
        eprintln!("purge-measures-v1: the daemon runs (pid {}); stop it first (shutdown), it holds the rates snapshot in memory", daemon_pid.unwrap_or(0));
        return 1;
    }
    let store = match lifescape_store::Store::open(&crate::market_service::db_path()) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("purge-measures-v1: store open: {error}");
            return 1;
        }
    };
    let snapshot = crate::rates_service::snapshot_path();
    let counts = match store.v1_measure_counts() {
        Ok(counts) => counts,
        Err(error) => {
            eprintln!("purge-measures-v1: count: {error}");
            return 1;
        }
    };
    println!(
        "measure v1: {} task_reports, {} task_yields; snapshot {} ({})",
        counts.reports,
        counts.yields,
        snapshot.display(),
        if snapshot.exists() { "present" } else { "absent" }
    );
    if !confirm {
        println!("dry run: nothing deleted (add --confirm to purge)");
        return 0;
    }
    match store.purge_v1_measures() {
        Ok(done) => println!("purged {} task_reports, {} task_yields", done.reports, done.yields),
        Err(error) => {
            eprintln!("purge-measures-v1: {error}");
            return 1;
        }
    }
    if snapshot.exists() {
        if let Err(error) = std::fs::remove_file(&snapshot) {
            eprintln!("purge-measures-v1: snapshot {}: {error}", snapshot.display());
            return 1;
        }
        println!("removed {}", snapshot.display());
    }
    0
}
