//! Daemon-side hosting of the pathfinding service: ONE snapshot (world +
//! tables + version) loaded at daemon start, served over
//! `run/pathfinding.sock`. Load is all-or-nothing: any missing piece
//! leaves the service down and every worker plan fails `nav-down`, never
//! a local plan (a half-world server would diverge from the plan the
//! goldens pin). Reload is `reload-nav` + swap.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use lifescape_json::{object, Value};
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
/// zones, tables. Any error = no service: every worker plan fails
/// `nav-down`, never a local plan.
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

/// Daemon-side snapshot for read-only consumers (the market handover
/// ring): the live `Arc`, or `None` when nav is down. Every worker plan
/// still fails `nav-down`; this never plans, it only reads tiles.
pub fn snapshot() -> Option<Arc<Snapshot>> {
    registry().lock().unwrap().clone().map(|service| service.snapshot())
}

/// Operator gauge: counters + queue + RAM in one object, same shape
/// as the service `stats` op, plus one entry per batch op served. None
/// when the service is down: the gauge is empty rather than stale.
pub fn stats_json() -> Option<Value> {
    let service = registry().lock().unwrap().clone()?;
    let stats = service.stats();
    let int = |num: u64| Value::Int(num.min(i64::MAX as u64) as i64);
    let (door_defs, tiles_flags) = service.query_stats();
    let (static_locs_near, in_f2p, toll_fence, wants) = service.query_stats_scripts();
    let (diff, debug, transitions_list, probe_targets, doors_near) = service.query_stats_diag();
    Some(object(vec![
        ("ok", Value::Bool(true)),
        ("version", Value::Str(service.current_version())),
        ("hits", int(stats.hits)),
        ("trunc_hits", int(stats.trunc_hits)),
        ("misses", int(stats.misses)),
        ("joins", int(stats.joins)),
        ("l2_hits", int(stats.l2_hits)),
        ("l2_miss", int(stats.l2_miss)),
        ("evictions", int(stats.evictions)),
        ("l1_len", int(service.l1_len() as u64)),
        ("l1_bytes", int(service.l1_bytes() as u64)),
        (
            "l1_oldest_age_ms",
            match service.l1_oldest_age_ms() {
                Some(age) => int(age.min(i64::MAX as u128) as u64),
                None => Value::Null,
            },
        ),
        ("queue_depth", int(service.queue_depth() as u64)),
        ("inflight", int(service.inflight_total() as u64)),
        ("refused_cap", int(stats.refused_cap)),
        ("refused_full", int(stats.refused_full)),
        ("refused_stale", int(stats.refused_stale)),
        ("unknown", int(stats.unknown)),
        ("submits", int(stats.submits)),
        ("polls", int(stats.polls)),
        ("door_defs", int(door_defs)),
        ("tiles_flags", int(tiles_flags)),
        ("static_locs_near", int(static_locs_near)),
        ("in_f2p", int(in_f2p)),
        ("toll_fence", int(toll_fence)),
        ("wants", int(wants)),
        ("diff", int(diff)),
        ("debug", int(debug)),
        ("transitions_list", int(transitions_list)),
        ("probe_targets", int(probe_targets)),
        ("doors_near", int(doors_near)),
        ("ram_warn", int(stats.ram_warn)),
        ("ram_shed", int(stats.ram_shed)),
        ("rss_bytes", int(Service::rss_bytes())),
        ("painted_squares", int(service.painted_squares() as u64)),
    ]))
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
    /// throwaway socket via the single-round-trip `plan` op (E3, N9: one
    /// connexion per plan, server-side wait, no submit ticket and no
    /// 2 ms poll loop). No live session is touched, nothing under `run/`.
    #[test]
    fn daemon_snapshot_loads_and_serves() {
        let snapshot = super::load_snapshot().expect("daemon snapshot loads like a session");
        assert!(!snapshot.version.is_empty(), "version feeds the skew check");
        assert!(!snapshot.tables.transitions.is_empty(), "tables travel with the world");
        let sock = std::env::temp_dir().join(format!("lifescape-plan-daemon-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&sock);
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
        // N9: the `plan` op answers terminal on the same connexion. The
        // client performs no submit and no poll here: one admission
        // server-side, zero interrogation on the wire.
        let before = service.stats();
        match client.plan_once(from, dest, &options, &service.current_version(), 10_000) {
            Ok((lifescape_nav::service::Poll::Ready(plan), _served)) => {
                assert!(!plan.segments.is_empty(), "daemon snapshot plans");
            }
            Ok((lifescape_nav::service::Poll::Failed(no), _)) => {
                panic!("daemon snapshot failed: {}", no.reason.as_str())
            }
            Ok((lifescape_nav::service::Poll::Pending, _)) => panic!("plan op never answers pending"),
            Ok((lifescape_nav::service::Poll::Unknown, _)) => panic!("plan op never answers unknown"),
            Err(error) => panic!("plan op refused: {error:?}"),
        }
        let after = service.stats();
        assert_eq!(
            after.submits - before.submits,
            1,
            "one plan op = one internal admission, never a resubmit storm"
        );
        let _ = std::fs::remove_file(&sock);
    }
}
