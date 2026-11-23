//! Test isolation tripwire: a test of this crate must never resolve state
//! into the live `<repo>/run/` (orch-core compiled without cfg(test) used
//! to hand the real paths out; fixed by the `test-isolation` feature).

use std::path::{Path, PathBuf};

fn live_run() -> PathBuf {
    let mut dir = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    while !dir.join("orchestration").is_dir() {
        assert!(dir.pop(), "repo root not found");
    }
    dir.join("run")
}

#[test]
fn state_paths_are_not_under_the_live_run_dir() {
    let live = live_run();
    let temp = std::env::temp_dir();
    for path in [
        lifescape_orch_core::paths::store_db(),
        lifescape_orch_core::paths::run_dir(),
        lifescape_orch_core::paths::users_dir(),
        lifescape_orch_core::paths::logs_dir(),
        lifescape_orch_core::paths::lives_dir(),
        lifescape_orch_core::paths::life_path("t01"),
        lifescape_orch_core::paths::socket_path(),
        lifescape_orch_core::paths::push_socket_path(),
        lifescape_orch_core::paths::pid_path(),
        lifescape_orch_core::paths::daemon_log(),
        lifescape_orch_core::paths::session_paths("t01").0,
    ] {
        assert!(!path.starts_with(&live), "{} is inside the live run/", path.display());
        assert!(path.starts_with(&temp), "{} is not under the temp root", path.display());
    }
}

#[test]
#[should_panic(expected = "LIVE data directory")]
fn write_json_refuses_the_live_run_dir() {
    let _ = lifescape_orch_core::paths::write_json(&live_run().join("tripwire-probe.json"), &lifescape_json::Value::Null);
}
