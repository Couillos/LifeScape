//! Live-like danger squares for the replay benches (test only).
//!
//! Live workers send `kit::danger::arm` squares with EVERY plan and
//! `walk_costs`: every aggressive spawn within 250 tiles of the bot that
//! would attack it (100-250 squares in the F2P towns). The first replay
//! profile sent one square for a tenth of the accounts, so the per-request
//! cost of the danger list (JSON, keys, `DangerMap`, cache option checks)
//! never showed. Same rule as `scripts/src/kit/danger.rs`, read from the
//! same dumps (`npcs.json`, `npc_ranges.json`); the nav crate has no
//! gamedata dependency, so the rule is restated here.

use std::sync::OnceLock;

use crate::plan::{DangerZone, Tile};

/// One hunter spawn: square centre, radius, vislevel, wilderness flag.
pub(crate) struct Hunter {
    pub x: i32,
    pub z: i32,
    pub level: i32,
    pub radius: i32,
    pub vislevel: i64,
    pub wild: bool,
}

const ARM_RADIUS: i32 = 250;
const LETHAL_LEVEL: i64 = 5;
const DANGER_MARGIN: i64 = 1;
const DEFAULT_WANDER: i64 = 5;

fn load_hunters() -> Vec<Hunter> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    let data = root.join("LifeScape/gamedata/data");
    let npcs = lifescape_json::parse(&std::fs::read_to_string(data.join("npcs.json")).expect("npcs.json")).expect("npcs json");
    let ranges = lifescape_json::parse(&std::fs::read_to_string(data.join("npc_ranges.json")).expect("npc_ranges.json")).expect("ranges json");
    let mut hunters = Vec::new();
    for npc in npcs.as_arr().expect("npc list") {
        if npc.get("hunts_players").as_bool() != Some(true) {
            continue;
        }
        let name = npc.get("name").as_str().unwrap_or("?");
        let wander = ranges.get("npcs").get(name).get("wanderrange").as_i64().unwrap_or(DEFAULT_WANDER);
        let radius = npc.get("huntrange").as_i64().unwrap_or(0) + wander + DANGER_MARGIN;
        let vislevel = npc.get("vislevel").as_i64().unwrap_or(0);
        for spawn in npc.get("spawns").as_arr().map(|rows| rows.as_slice()).unwrap_or(&[]) {
            let (x, z, level) = (spawn.get("x").as_i64().unwrap_or(0), spawn.get("z").as_i64().unwrap_or(0), spawn.get("level").as_i64().unwrap_or(0));
            let wild = level == 0 && (2944..=3392).contains(&x) && z >= 3520;
            hunters.push(Hunter { x: x as i32, z: z as i32, level: level as i32, radius: radius as i32, vislevel, wild });
        }
    }
    hunters
}

fn hunters() -> &'static [Hunter] {
    static HUNTERS: OnceLock<Vec<Hunter>> = OnceLock::new();
    HUNTERS.get_or_init(load_hunters)
}

/// `kit::danger::arm` for a bot standing on `at` with combat `combat`.
pub(crate) fn live_danger(at: Tile, combat: u8) -> Vec<DangerZone> {
    let ours = i64::from(combat);
    hunters()
        .iter()
        .filter(|hunter| hunter.level == at.level)
        .filter(|hunter| {
            let threatens = ours <= hunter.vislevel * 2;
            let dangerous = threatens && (hunter.vislevel >= LETHAL_LEVEL || hunter.vislevel >= ours);
            if hunter.wild { hunter.vislevel >= LETHAL_LEVEL } else { dangerous }
        })
        .filter(|hunter| (hunter.x - at.x).abs().max((hunter.z - at.z).abs()) <= ARM_RADIUS)
        .map(|hunter| DangerZone { x: hunter.x, z: hunter.z, level: at.level, radius: hunter.radius })
        .collect()
}

/// Combat level per account (fixed, spread over the live mix).
pub(crate) fn combat_of(hash: u64) -> u8 {
    [3u8, 3, 3, 8, 15, 25, 32][((hash >> 24) % 7) as usize]
}

/// Per-request fixed cost, stage by stage, on the replayed requests (one
/// thread, no cache: every request runs every stage). Ignored: needs the
/// pack and `NAV_REPLAY` (same extract as `service_replay`).
/// `NAV_REPLAY_LIMIT` caps the requests (default 3000).
#[test]
#[ignore]
fn service_replay_breakdown() {
    use std::time::Instant;

    use crate::plan::{self, DangerMap};
    use crate::service::{decode_options, encode_options, encode_plan, tile_value, PlanKey};
    use crate::service_replay_tests::{parse_line, real_snapshot, request_options};

    let Ok(path) = std::env::var("NAV_REPLAY") else { return };
    let limit: usize = std::env::var("NAV_REPLAY_LIMIT").ok().and_then(|raw| raw.parse().ok()).unwrap_or(3000);
    let text = std::fs::read_to_string(&path).expect("replay extract");
    let mut requests: Vec<_> = text.lines().filter_map(parse_line).collect();
    requests.sort_by_key(|request| request.secs);
    requests.truncate(limit);
    let (_snapshot, world, tables) = real_snapshot();
    let shared = plan::prepare_shared(&world, &tables, &plan::NavOptions::default(), &mut || true).expect("edges");
    let memo = crate::pocket::PocketMemo::default();
    // Warm the painted squares and the pocket memo like a running daemon.
    for request in requests.iter().take(500) {
        let _ = plan::plan_memo(&world, &tables, request.from, request.dest, &request_options(request), &shared, &memo, &mut || true);
    }
    const STAGES: [&str; 11] = ["parse", "key", "danger_map", "pocket", "plan", "analyze", "reply", "fast_hop", "walk_costs_cold", "walk_costs_warm", "zones"];
    // [class][stage] = nanos (zones: count); class 0 = short (<= 8, same level), 1 = other.
    let mut sums = [[0u128; 11]; 2];
    let mut counts = [0u64; 2];
    let mut zones_total = 0usize;
    for request in &requests {
        let options = request_options(request);
        zones_total += options.danger.len();
        let short = request.from.level == request.dest.level && request.from.chebyshev(request.dest) <= 8;
        let class = if short { 0 } else { 1 };
        counts[class] += 1;
        let line = lifescape_json::stringify(&lifescape_json::object(vec![
            ("op", lifescape_json::str_value("plan")),
            ("account", lifescape_json::str_value(&request.account)),
            ("from", tile_value(request.from)),
            ("dest", tile_value(request.dest)),
            ("options", encode_options(&options)),
            ("version", lifescape_json::str_value("replay")),
        ]));
        let started = Instant::now();
        let parsed = lifescape_json::parse(&line).expect("frame");
        let decoded = decode_options(parsed.get("options")).expect("options");
        sums[class][0] += started.elapsed().as_nanos();
        let started = Instant::now();
        let key = PlanKey::of(request.from, request.dest, &decoded, "replay");
        sums[class][1] += started.elapsed().as_nanos();
        drop(key);
        let started = Instant::now();
        let map = DangerMap::new(&decoded.danger);
        sums[class][2] += started.elapsed().as_nanos();
        drop(map);
        let started = Instant::now();
        let _ = memo.sealed(&world, &tables, &shared, request.from, request.dest);
        sums[class][3] += started.elapsed().as_nanos();
        let started = Instant::now();
        let outcome = plan::plan_memo(&world, &tables, request.from, request.dest, &decoded, &shared, &memo, &mut || true);
        sums[class][4] += started.elapsed().as_nanos();
        if let Ok(found) = &outcome {
            let started = Instant::now();
            let info = crate::route_path::analyze(&world, &tables, request.from, found, &decoded.danger);
            sums[class][5] += started.elapsed().as_nanos();
            drop(info);
            let started = Instant::now();
            let reply = lifescape_json::stringify(&encode_plan(found));
            sums[class][6] += started.elapsed().as_nanos();
            drop(reply);
        }
        let started = Instant::now();
        let _ = crate::fast_hop::short_hop(&world, &shared, request.from, request.dest, &decoded);
        sums[class][7] += started.elapsed().as_nanos();
        let ring: Vec<Tile> = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)]
            .iter()
            .map(|(step_x, step_z)| Tile { x: request.dest.x + step_x, z: request.dest.z + step_z, level: request.dest.level })
            .filter(|tile| crate::collision::tile_walkable(world.get(tile.x, tile.z, tile.level)))
            .collect();
        let started = Instant::now();
        let _ = crate::costs::plan_costs_with(&world, &tables, request.from, &ring, &decoded, &shared, Some(&memo));
        sums[class][8] += started.elapsed().as_nanos();
        let started = Instant::now();
        let _ = crate::costs::plan_costs_with(&world, &tables, request.from, &ring, &decoded, &shared, Some(&memo));
        sums[class][9] += started.elapsed().as_nanos();
        sums[class][10] += options.danger.len() as u128;
    }
    println!("breakdown requests={} zones/request={:.0}", requests.len(), zones_total as f64 / requests.len().max(1) as f64);
    for (class, name) in ["short<=8", "other"].iter().enumerate() {
        let count = counts[class].max(1) as f64;
        let cells: Vec<String> = STAGES
            .iter()
            .enumerate()
            .map(|(stage, label)| {
                if stage == 10 {
                    format!("{label}={:.0}", sums[class][stage] as f64 / count)
                } else {
                    format!("{label}={:.0}us", sums[class][stage] as f64 / count / 1000.0)
                }
            })
            .collect();
        println!("class {name} n={} {}", counts[class], cells.join(" "));
    }
}
