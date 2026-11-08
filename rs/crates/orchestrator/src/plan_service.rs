//! Daemon-side hosting of the lot 2 pathfinding service: ONE snapshot
//! (world + tables + version) loaded at daemon start, served over
//! `run/pathfinding.sock`. Load is all-or-nothing: any missing piece
//! leaves the service down and every worker falls back to its local sync
//! plan (a half-world server would diverge from the sync path, which the
//! lot 2 goldens forbid). Reload is lot 3 (`reload-nav` + swap).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use lifescape_nav::service::{self, Service, Snapshot};
use lifescape_nav::tables::Tables;

use crate::paths;

/// Socket workers dial for plans (mirrors `session-<account>.sock` layout).
pub fn socket_path() -> PathBuf {
    paths::run_dir().join("pathfinding.sock")
}

/// Version inputs: every file that conditions a plan.
pub fn version_for(data_dir: &Path, zip: &Path, f2p: &Path) -> String {
    service::data_version(&[
        data_dir.join("transitions.json"),
        data_dir.join("nav_overrides.json"),
        data_dir.join("transitions_verified.json"),
        data_dir.join("gametypes.json"),
        f2p.to_path_buf(),
        zip.to_path_buf(),
    ])
}

/// Load the daemon snapshot exactly like a session worker does (same
/// loaders, same strictness minus the logs): index + lazy squares, f2p
/// zones, tables. Any error = no service (workers stay sync).
pub fn load_snapshot() -> Result<Arc<Snapshot>, String> {
    let data_dir = paths::root().join("gamedata").join("data");
    let engine = PathBuf::from(crate::session::engine_dir());
    let zip = engine
        .join("data")
        .join("pack")
        .join(".cache")
        .join("maps-server.zip");
    let f2p = engine.join("../content").join("maps").join("free2play.csv");
    let defs = crate::staticmap::load_loctypes(&data_dir.join("gametypes.json"))?;
    let world = lifescape_nav::static_world::StaticWorld::open(&zip, defs)
        .map_err(|error| error.to_string())?;
    let zones = lifescape_nav::f2p::load_zones(&f2p).map_err(|error| error.to_string())?;
    world.set_f2p(zones);
    let tables = Tables::load(&data_dir).map_err(|error| error.to_string())?;
    world.set_sky_floors(tables.sky_floors.clone());
    Ok(Snapshot::new(
        world,
        Arc::new(tables),
        version_for(&data_dir, &zip, &f2p),
    ))
}

/// Spawn the pool + acceptor on the daemon socket.
pub fn start(snapshot: Arc<Snapshot>) -> Result<Arc<Service>, String> {
    let service = Service::start(snapshot, &socket_path(), service::PLAN_THREADS)?;
    *registry().lock().unwrap() = Some(service.clone());
    Ok(service)
}

/// Operator `reload-nav` (lot 3): rebuild the snapshot beside the live
/// one, then swap it atomically. In-flight requests keep the old `Arc`;
/// the version bump makes every old L1 key and old vol unreachable, so
/// zero stale plans are served after the swap.
pub fn reload() -> Result<String, String> {
    let service = registry()
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "pathfinding service not running".to_string())?;
    let snapshot = load_snapshot()?;
    let (old, new) = service.reload(snapshot);
    Ok(format!("{old} -> {new}"))
}

fn registry() -> &'static Mutex<Option<Arc<Service>>> {
    static SERVICE: OnceLock<Mutex<Option<Arc<Service>>>> = OnceLock::new();
    SERVICE.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
mod tests {
    use super::socket_path;

    #[test]
    fn plan_socket_lives_next_to_session_sockets() {
        assert_eq!(
            socket_path().file_name().and_then(|name| name.to_str()),
            Some("pathfinding.sock")
        );
    }

    /// Daemon path, read-only: the snapshot loads exactly like a session
    /// worker's (same loaders, strict), then serves one plan over a
    /// throwaway socket. No live session is touched, nothing under `run/`.
    #[test]
    fn daemon_snapshot_loads_and_serves() {
        let snapshot = super::load_snapshot().expect("daemon snapshot loads like a session");
        assert!(!snapshot.version.is_empty(), "version feeds the skew check");
        assert!(!snapshot.tables.transitions.is_empty(), "tables travel with the world");
        let sock = std::env::temp_dir().join(format!("lifescape-plan-daemon-{}.sock", std::process::id()));
        let service = lifescape_nav::service::Service::start(snapshot, &sock, 1).expect("serves on a temp socket");
        let client = lifescape_nav::service::PlanClient::new(service.socket_path(), "daemon-test");
        let from = lifescape_nav::plan::Tile { x: 3228, z: 3214, level: 0 };
        let dest = lifescape_nav::plan::Tile { x: 3224, z: 3214, level: 0 };
        let options = lifescape_nav::plan::NavOptions {
            allow_paid: false,
            f2p_only: true,
            satisfied: std::collections::HashSet::new(),
            excluded_edges: std::collections::HashSet::new(),
        };
        let id = match client.submit(from, dest, &options, "").expect("submit") {
            lifescape_nav::service::Submit::Accepted { id, .. } => id,
            lifescape_nav::service::Submit::Refused { reason, .. } => panic!("refused: {reason}"),
        };
        let start = std::time::Instant::now();
        loop {
            match client.poll(id).expect("poll").0 {
                lifescape_nav::service::Poll::Ready(plan) => {
                    assert!(!plan.segments.is_empty(), "daemon snapshot plans");
                    break;
                }
                lifescape_nav::service::Poll::Failed(no) => panic!("daemon snapshot failed: {}", no.reason.as_str()),
                lifescape_nav::service::Poll::Pending => {}
                lifescape_nav::service::Poll::Unknown => panic!("lost ticket"),
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(120), "daemon snapshot answers");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}
