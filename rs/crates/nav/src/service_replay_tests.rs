//! Replay bench of the real plan traffic (ignored: needs the engine pack
//! and an extract of worker `nav plan ... from (...) to (...)` lines).
//!
//! `NAV_REPLAY=<extract> [NAV_REPLAY_LIMIT=N] [NAV_REPLAY_WARM=W]
//! [NAV_REPLAY_QUALITY=K] [NAV_REPLAY_MISSES=1] cargo test -p
//! lifescape-nav --lib service_replay -- --ignored --nocapture`
//! (the last N requests measured after W warm-up ones; `MISSES` prints
//! every computed long request, cost > 50).
//!
//! Extract: `<account> [HH:MM:SS] ... nav plan N segments cost C from
//! (x,z,l) to (x,z,l)` per line (worker logs prefixed by the account).
//! The replay is sequential in log time order through a REAL `Service`
//! on a test socket (`plan` op like the workers, plus one synthetic
//! `walk_costs` ring per third short plan, the live 0.4 batch/plan
//! ratio). Each account gets a fixed option profile (paid, item tags)
//! plus the live danger list of each request's start
//! (`service_replay_live`, ~180 squares; `NAV_REPLAY_DANGER=0` keeps
//! the first profile's single square for a tenth of the accounts), so
//! the key fragmentation and per-request cost of live bots are present.
//! `service_replay_breakdown` (same extract) times each stage alone.
//! Reports the CPU of the service threads (`plan-*`, schedstat ns), the
//! cache counters, and the route quality: every K-th served plan is
//! re-planned fresh off the cache and the cost excess is measured.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use crate::plan::{self, DangerZone, NavOptions, Tile};
use crate::service::{PlanClient, Poll, Service, Snapshot};
use crate::static_world::StaticWorld;
use crate::tables::Tables;

pub(crate) fn real_snapshot() -> (Arc<Snapshot>, Arc<StaticWorld>, Arc<Tables>) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
    let gametypes = std::fs::read_to_string(root.join("LifeScape/gamedata/data/gametypes.json")).expect("gametypes");
    let overrides = std::fs::read_to_string(root.join("LifeScape/gamedata/data/nav_overrides.json")).expect("overrides");
    let defs = crate::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
    let world = StaticWorld::open(&zip, defs).expect("static world");
    let zones = crate::f2p::load_zones(&root.join("lostcity/content/maps/free2play.csv")).expect("f2p");
    world.set_f2p(zones);
    let tables = Arc::new(Tables::load(&root.join("LifeScape/gamedata/data")).expect("tables"));
    world.set_sky_floors(tables.sky_floors.clone());
    let snapshot = Snapshot::new(world.clone(), tables.clone(), "replay".to_string());
    (snapshot, world, tables)
}

/// One replayed request: account, log second, from, dest, logged cost.
pub(crate) struct Request {
    pub account: String,
    pub secs: u32,
    pub from: Tile,
    pub dest: Tile,
    pub cost: u32,
}

fn parse_tile(text: &str) -> Option<Tile> {
    let inner = text.strip_prefix('(')?.split(')').next()?;
    let mut parts = inner.split(',').map(|part| part.trim().parse::<i32>());
    Some(Tile { x: parts.next()?.ok()?, z: parts.next()?.ok()?, level: parts.next()?.ok()? })
}

pub(crate) fn parse_line(line: &str) -> Option<Request> {
    let account = line.split(' ').next()?.to_string();
    let clock = line.split('[').nth(1)?.split(']').next()?;
    let mut fields = clock.split(':').map(|part| part.parse::<u32>());
    let secs = fields.next()?.ok()? * 3600 + fields.next()?.ok()? * 60 + fields.next()?.ok()?;
    let cost = line.split(" cost ").nth(1)?.split(' ').next()?.parse().ok()?;
    let from = parse_tile(line.split(" from ").nth(1)?)?;
    let dest = parse_tile(line.split(" to ").nth(1)?)?;
    Some(Request { account, secs, from, dest, cost })
}

pub(crate) fn account_hash(account: &str) -> u64 {
    account.bytes().fold(0xcbf29ce484222325u64, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3))
}

/// Per-request options: the account profile, with the live danger list
/// (`kit::danger::arm` around the requesting tile, `service_replay_live`)
/// unless `NAV_REPLAY_DANGER=0` (the first replay's single square).
pub(crate) fn request_options(request: &Request) -> NavOptions {
    let mut options = profile(&request.account);
    if std::env::var("NAV_REPLAY_DANGER").as_deref() != Ok("0") {
        let combat = crate::service_replay_live::combat_of(account_hash(&request.account));
        options.danger = crate::service_replay_live::live_danger(request.from, combat);
    }
    options
}

/// Fixed per-account options: paid for an eighth, the live `wants` tags
/// (`coins:30` for half, `item:983` for some: they vary with the pack), a danger
/// square for a tenth (combat bots).
pub(crate) fn profile(account: &str) -> NavOptions {
    let hash = account_hash(account);
    let mut options = NavOptions { f2p_only: true, ..NavOptions::default() };
    options.allow_paid = hash % 8 == 0;
    if hash % 2 == 0 {
        options.satisfied = HashSet::from(["coins:30".to_string()]);
        if (hash >> 8) % 7 == 0 {
            options.satisfied.insert("item:983".to_string());
        }
    }
    if hash % 10 == 0 {
        options.danger = vec![DangerZone { x: 3245, z: 3400 + ((hash >> 16) % 20) as i32, level: 0, radius: 3 }];
    }
    options
}

/// CPU nanoseconds per service thread group (`plan-pool`, `plan-wait`,
/// `plan-query`), from `/proc/self/task/*/schedstat`.
pub(crate) fn thread_cpu() -> HashMap<String, u64> {
    let mut groups: HashMap<String, u64> = HashMap::new();
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else { return groups };
    for task in tasks.flatten() {
        let comm = std::fs::read_to_string(task.path().join("comm")).unwrap_or_default();
        let name = comm.trim();
        if !name.starts_with("plan-") {
            continue;
        }
        let group = name.trim_end_matches(|ch: char| ch.is_ascii_digit() || ch == '-').to_string();
        let nanos = std::fs::read_to_string(task.path().join("schedstat"))
            .ok()
            .and_then(|text| text.split_whitespace().next().and_then(|field| field.parse::<u64>().ok()))
            .unwrap_or(0);
        *groups.entry(group).or_default() += nanos;
    }
    groups
}

#[test]
#[ignore]
fn service_replay() {
    let Ok(path) = std::env::var("NAV_REPLAY") else { return };
    let limit: usize = std::env::var("NAV_REPLAY_LIMIT").ok().and_then(|raw| raw.parse().ok()).unwrap_or(usize::MAX);
    let warm: usize = std::env::var("NAV_REPLAY_WARM").ok().and_then(|raw| raw.parse().ok()).unwrap_or(5000);
    let quality_every: usize = std::env::var("NAV_REPLAY_QUALITY").ok().and_then(|raw| raw.parse().ok()).unwrap_or(20);
    let text = std::fs::read_to_string(&path).expect("replay extract");
    let mut requests: Vec<Request> = text.lines().filter_map(parse_line).collect();
    requests.sort_by_key(|request| request.secs);
    if requests.len() > limit.saturating_add(warm) {
        requests.drain(..requests.len() - limit - warm);
    }
    let (snapshot, world, tables) = real_snapshot();
    let sock = crate::service::test_socket("nav-replay");
    let service = Service::start(snapshot, &sock, crate::service::PLAN_THREADS).expect("service");
    let version = service.current_version();
    let shared = plan::prepare_shared(&world, &tables, &NavOptions::default(), &mut || true).expect("edges");
    let mut before = service.stats();
    let mut cpu_before = thread_cpu();
    let mut started = Instant::now();
    let (mut served, mut failed, mut costs_batches) = (0u64, 0u64, 0u64);
    let mut fresh_wall = std::time::Duration::ZERO;
    let (mut checked, mut excess_sum, mut excess_max, mut over_eps, mut verdict_mismatch) = (0u64, 0u64, 0u32, 0u64, 0u64);
    let warm = warm.min(requests.len());
    // (requests, misses, wall ms) per logged cost class: <=5, <=50, >50.
    let mut by_class = [(0u64, 0u64, 0f64); 3];
    let mut costs_wall: Vec<f64> = Vec::new();
    // Byte-identity proof of the navigation lots: FNV-1a over the Debug
    // text of every plan outcome and every `walk_costs` answer, in
    // replay order (warm-up included). Same extract, same digests.
    let (mut plan_digest, mut costs_digest) = (0xcbf29ce484222325u64, 0xcbf29ce484222325u64);
    let fold = |digest: &mut u64, text: &str| {
        for byte in text.bytes() {
            *digest = (*digest ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    };
    for (index, request) in requests.iter().enumerate() {
        if index == warm {
            // Warm L1 like the live daemon: measure from here on.
            before = service.stats();
            cpu_before = thread_cpu();
            started = Instant::now();
            (served, failed, costs_batches) = (0, 0, 0);
        }
        let options = request_options(request);
        let client = PlanClient::new(&sock, &request.account);
        let misses_before = service.stats().misses;
        let plan_start = Instant::now();
        let outcome = client.plan_once(request.from, request.dest, &options, &version, 30_000);
        if index >= warm {
            let class = if request.cost <= 5 { 0 } else if request.cost <= 50 { 1 } else { 2 };
            by_class[class].0 += 1;
            let missed = service.stats().misses - misses_before;
            by_class[class].1 += missed;
            if missed > 0 && class == 2 && std::env::var("NAV_REPLAY_MISSES").is_ok() {
                println!("long-miss {} {} {} {} {} {} {}", request.account, request.from.x, request.from.z, request.from.level, request.dest.x, request.dest.z, request.dest.level);
            }
            by_class[class].2 += plan_start.elapsed().as_secs_f64() * 1000.0;
        }
        fold(&mut plan_digest, &format!("{:?}", outcome));
        match &outcome {
            Ok((Poll::Ready(_), _)) => served += 1,
            _ => failed += 1,
        }
        if request.cost <= 5 && index % 3 == 0 {
            let ring: Vec<Tile> = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)]
                .iter()
                .map(|(step_x, step_z)| Tile { x: request.dest.x + step_x, z: request.dest.z + step_z, level: request.dest.level })
                .filter(|tile| crate::collision::tile_walkable(world.get(tile.x, tile.z, tile.level)))
                .collect();
            if !ring.is_empty() {
                let costs_start = Instant::now();
                let answer = client.walk_costs(request.from, &ring, &options, &version);
                fold(&mut costs_digest, &format!("{:?}", answer));
                if index >= warm {
                    costs_wall.push(costs_start.elapsed().as_secs_f64() * 1000.0);
                }
                costs_batches += 1;
            }
        }
        if quality_every > 0 && index >= warm && index % quality_every == 0 {
            let fresh_start = Instant::now();
            let fresh = plan::plan_yielding_with(&world, &tables, request.from, request.dest, &options, &shared, &mut || true);
            fresh_wall += fresh_start.elapsed();
            match (&outcome, &fresh) {
                (Ok((Poll::Ready(cached), _)), Ok(fresh)) => {
                    checked += 1;
                    let excess = cached.cost.saturating_sub(fresh.cost);
                    excess_sum += u64::from(excess);
                    excess_max = excess_max.max(excess);
                    if excess > crate::route_splice::splice_epsilon(fresh.cost) {
                        over_eps += 1;
                    }
                    if cached.cost < fresh.cost {
                        eprintln!("cheaper than fresh?! {:?}->{:?} {} < {}", request.from, request.dest, cached.cost, fresh.cost);
                    }
                }
                (Ok((Poll::Failed(_), _)), Err(_)) => checked += 1,
                _ => verdict_mismatch += 1,
            }
        }
    }
    let wall = started.elapsed().saturating_sub(fresh_wall);
    let cpu_after = thread_cpu();
    let after = service.stats();
    let mut groups: Vec<(String, f64)> = cpu_after
        .iter()
        .map(|(group, nanos)| (group.clone(), nanos.saturating_sub(*cpu_before.get(group).unwrap_or(&0)) as f64 / 1e6))
        .collect();
    groups.sort_by(|left, right| left.0.cmp(&right.0));
    let total: f64 = groups.iter().map(|(_, millis)| millis).sum();
    println!("replay requests={} (+{warm} warm-up) served={served} failed={failed} walk_costs={costs_batches} wall={:.1}s", requests.len() - warm, wall.as_secs_f64());
    println!("cpu_ms total={total:.0} {}", groups.iter().map(|(group, millis)| format!("{group}={millis:.0}")).collect::<Vec<_>>().join(" "));
    println!(
        "cache hits={} trunc={} suffix={} miss={} joins={} fast={} fast_zero={} fast_declined={}",
        after.hits - before.hits,
        after.trunc_hits - before.trunc_hits,
        after.suffix_hits - before.suffix_hits,
        after.misses - before.misses,
        after.joins - before.joins,
        after.fast_hits - before.fast_hits,
        after.fast_zero - before.fast_zero,
        after.fast_declined - before.fast_declined,
    );
    for (name, (count, misses, millis)) in ["short<=5", "mid<=50", "long>50"].iter().zip(by_class.iter()) {
        println!("class {name}: requests={count} misses={misses} wall_ms={millis:.0}");
    }
    costs_wall.sort_by(|left, right| left.total_cmp(right));
    if !costs_wall.is_empty() {
        let at = |share: f64| costs_wall[((costs_wall.len() - 1) as f64 * share) as usize];
        println!(
            "walk_costs wall_ms sum={:.0} p50={:.2} p99={:.1} max={:.1} over20ms={}",
            costs_wall.iter().sum::<f64>(),
            at(0.5),
            at(0.99),
            at(1.0),
            costs_wall.iter().filter(|millis| **millis > 20.0).count()
        );
    }
    println!("routes {:?} pockets={}", service.route_stats(), service.pocket_verdicts());
    println!("ops {}", crate::nav_meter::ops_segment(&before.ops, &after.ops));
    let total_visits: u64 = after.ops.iter().zip(before.ops.iter()).map(|(now, was)| now.visits - was.visits).sum();
    let unreached = |slot: usize| after.ops[slot].unreached_visits - before.ops[slot].unreached_visits;
    println!(
        "visits total={total_visits} walk_costs_unreached={} ({:.1}%) plan_unreached={} ({:.1}%)",
        unreached(0),
        100.0 * unreached(0) as f64 / total_visits.max(1) as f64,
        unreached(3),
        100.0 * unreached(3) as f64 / total_visits.max(1) as f64
    );
    println!("digest plans={plan_digest:016x} walk_costs={costs_digest:016x}");
    println!(
        "quality checked={checked} excess_sum={excess_sum} excess_max={excess_max} over_epsilon={over_eps} verdict_mismatch={verdict_mismatch}"
    );
    let _ = std::fs::remove_file(&sock);
}
