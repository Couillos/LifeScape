//! Offline failure replay (§6.5): replan a dossier, print a verdict.
//!
//! No daemon, no session, no login. The dossier carries the inputs
//! (from, dest, options) plus what the bot saw (plan, failing segment,
//! static/live diff, messages); replay rebuilds the world from disk,
//! replans, and classifies:
//!   A plan faux — the replayed walk crosses a static obstacle: fixable
//!     in lifescape-nav, add the route to the golden routes first.
//!   B monde different de la carte — the recorded static/live diff hits
//!     the path: missing data (override, transition, door) or dynamics.
//!   C execution — coherent plan, nothing on the path: the SDK executor
//!     (send, wait, postcondition) is where to look.

use std::collections::HashSet;

use lifescape_json::Value;
use lifescape_nav::bfs::can_step;
use lifescape_nav::plan::{self, EdgeId, NavOptions, Plan, Segment, Tile};
use lifescape_nav::static_world::StaticWorld;

/// Replay verdict: which layer the next debug step reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    A,
    B,
    C,
}

impl Verdict {
    fn as_str(self) -> &'static str {
        match self {
            Verdict::A => "A plan faux",
            Verdict::B => "B monde != carte",
            Verdict::C => "C execution",
        }
    }
}

/// One dossier, parsed: replay inputs plus what the bot saw.
struct Dossier {
    account: String,
    tick: i64,
    from: Tile,
    dest: Tile,
    options: NavOptions,
    /// Walk-step pairs of the plan that failed live, in order.
    pairs: Vec<(Tile, Tile)>,
    /// Every tile the failed plan touched (walk, door, stand, expect).
    path: HashSet<(i32, i32, i32)>,
    plan_segments: usize,
    plan_cost: i64,
    failed_segment: Option<usize>,
    kind: String,
    expected: String,
    observed: String,
    diff: Vec<(i32, i32, i32)>,
}

fn tile_of(value: &Value) -> Option<Tile> {
    Some(Tile {
        x: value.get("x").as_i64()? as i32,
        z: value.get("z").as_i64()? as i32,
        level: value.get("level").as_i64()? as i32,
    })
}

fn edge_of(value: &Value) -> Option<EdgeId> {
    match value.get("kind").as_str()? {
        "door" => Some(EdgeId::Door {
            loc: value.get("loc").as_i64()? as u16,
            x: value.get("x").as_i64()? as i32,
            z: value.get("z").as_i64()? as i32,
            level: value.get("level").as_i64()? as i32,
        }),
        "ride" => Some(EdgeId::Ride(value.get("name").as_str()?.to_string())),
        "custom" => Some(EdgeId::Custom(value.get("name").as_str()?.to_string())),
        _ => None,
    }
}

fn parse_options(value: &Value) -> NavOptions {
    let mut options = NavOptions::default();
    if let Some(paid) = value.get("allow_paid").as_bool() {
        options.allow_paid = paid;
    }
    if let Some(f2p) = value.get("f2p_only").as_bool() {
        options.f2p_only = f2p;
    }
    for tag in value.get("satisfied").as_arr().cloned().unwrap_or_default() {
        if let Some(tag) = tag.as_str() {
            options.satisfied.insert(tag.to_string());
        }
    }
    for edge in value.get("excluded_edges").as_arr().cloned().unwrap_or_default() {
        if let Some(edge) = edge_of(&edge) {
            options.excluded_edges.insert(edge);
        }
    }
    options
}

/// Walk pairs plus touched tiles of a dossier plan. Door and ride jumps
/// are path tiles but never step-checked: the closed static view blocks
/// them by design, and teleports are not steps at all.
fn dossier_path(plan: &Value, from: Tile) -> (Vec<(Tile, Tile)>, HashSet<(i32, i32, i32)>, usize, i64) {
    let mut pairs = Vec::new();
    let mut path = HashSet::new();
    path.insert((from.x, from.z, from.level));
    let mut cursor = from;
    let mut segments = 0;
    let cost = plan.get("cost").as_i64().unwrap_or(0);
    for seg in plan.get("segments").as_arr().cloned().unwrap_or_default() {
        segments += 1;
        match seg.get("kind").as_str().unwrap_or("") {
            "walk" => {
                for tile in seg.get("tiles").as_arr().cloned().unwrap_or_default() {
                    if let Some(next) = tile_of(&tile) {
                        path.insert((next.x, next.z, next.level));
                        pairs.push((cursor, next));
                        cursor = next;
                    }
                }
            }
            "door" => {
                for key in ["stand", "cross_to"] {
                    if let Some(tile) = tile_of(seg.get(key)) {
                        path.insert((tile.x, tile.z, tile.level));
                    }
                }
                if let Some(door) = tile_of(&seg.get("door")) {
                    path.insert((door.x, door.z, door.level));
                }
                if let Some(cross) = tile_of(seg.get("cross_to")) {
                    cursor = cross;
                }
            }
            _ => {
                for key in ["stand", "expect"] {
                    if let Some(tile) = tile_of(seg.get(key)) {
                        path.insert((tile.x, tile.z, tile.level));
                    }
                }
                if let Some(expect) = tile_of(seg.get("expect")) {
                    cursor = expect;
                }
            }
        }
    }
    (pairs, path, segments, cost)
}

fn parse_dossier(value: &Value) -> Option<Dossier> {
    let from = tile_of(value.get("from"))?;
    let dest = tile_of(value.get("dest"))?;
    let (pairs, path, plan_segments, plan_cost) = match value.get("plan").as_obj() {
        Some(_) => {
            let (pairs, path, segments, cost) = dossier_path(value.get("plan"), from);
            (pairs, path, segments, cost)
        }
        None => (Vec::new(), HashSet::new(), 0, 0),
    };
    let mut diff = Vec::new();
    for entry in value.get("static_live_diff").as_arr().cloned().unwrap_or_default() {
        if let (Some(x), Some(z), Some(level)) = (entry.get("x").as_i64(), entry.get("z").as_i64(), entry.get("level").as_i64()) {
            diff.push((x as i32, z as i32, level as i32));
        }
    }
    Some(Dossier {
        account: value.get("account").as_str().unwrap_or("?").to_string(),
        tick: value.get("tick").as_i64().unwrap_or(0),
        from,
        dest,
        options: parse_options(value.get("options")),
        pairs,
        path,
        plan_segments,
        plan_cost,
        failed_segment: value.get("failed_segment").as_i64().map(|index| index as usize),
        kind: value.get("error").get("kind").as_str().unwrap_or("?").to_string(),
        expected: value.get("error").get("expected").as_str().unwrap_or("").to_string(),
        observed: value.get("error").get("observed").as_str().unwrap_or("").to_string(),
        diff,
    })
}

/// Walk pairs of a live `Plan`, with the cursor tracked across segments
/// (ride landings and door crossings included): exactly what the
/// executor walks, step by step.
fn plan_pairs(plan: &Plan, from: Tile) -> Vec<(Tile, Tile)> {
    let mut pairs = Vec::new();
    let mut cursor = from;
    for seg in &plan.segments {
        match seg {
            Segment::Walk { tiles } => {
                for next in tiles {
                    pairs.push((cursor, *next));
                    cursor = *next;
                }
            }
            Segment::OpenDoor { cross_to, .. } => cursor = *cross_to,
            Segment::Ride { expect, .. } | Segment::Custom { expect, .. } => cursor = *expect,
        }
    }
    pairs
}

/// First walk step the static world refuses. Pairs touching a known door
/// tile are the door model's business (checked by B/C), never A: the
/// planner only steps on doors through door edges.
fn find_blocked_step(world: &StaticWorld, pairs: &[(Tile, Tile)]) -> Option<(Tile, Tile)> {
    for (from, to) in pairs {
        if from.level != to.level {
            continue;
        }
        if world.door_at(from.x, from.z, from.level).is_some() || world.door_at(to.x, to.z, to.level).is_some() {
            continue;
        }
        let mut flags = |x: i32, z: i32| world.get(x, z, from.level);
        if !can_step(&mut flags, from.x, from.z, to.x, to.z) {
            return Some((*from, *to));
        }
    }
    None
}

/// Recorded static/live diffs landing on the failed path.
fn path_hits_diff(path: &HashSet<(i32, i32, i32)>, diff: &[(i32, i32, i32)]) -> Vec<(i32, i32, i32)> {
    let mut hits: Vec<(i32, i32, i32)> = diff.iter().copied().filter(|tile| path.contains(tile)).collect();
    hits.sort();
    hits.dedup();
    hits
}

fn classify(blocked: Option<(Tile, Tile)>, hits: &[(i32, i32, i32)]) -> Verdict {
    if blocked.is_some() {
        Verdict::A
    } else if hits.is_empty() {
        Verdict::C
    } else {
        Verdict::B
    }
}

pub fn replay_nav_cmd(args: &[String]) -> i32 {
    let usage = "usage: orchestrator replay-nav <dossier>\nRejoue un dossier run/failures/<compte>-<tick>.json hors session et classe l'echec :\n  A plan faux (bug lifescape-nav, corrigeable hors ligne : ajouter la route en or)\n  B monde != carte (la diff statique/live croise le chemin : donnee manquante)\n  C execution (plan coherent, rien sur le chemin : bug SDK).\nTout debug navigation commence ici.";
    if args.len() != 1 {
        eprintln!("{usage}");
        return 2;
    }
    let text = match std::fs::read_to_string(&args[0]) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("replay-nav: cannot read {}: {err}", args[0]);
            return 1;
        }
    };
    let parsed = match lifescape_json::parse(&text) {
        Ok(parsed) => parsed,
        Err(err) => {
            eprintln!("replay-nav: bad dossier {}: {err}", args[0]);
            return 1;
        }
    };
    let Some(dossier) = parse_dossier(&parsed) else {
        eprintln!("replay-nav: dossier {} has no from/dest tiles", args[0]);
        return 1;
    };
    println!(
        "dossier {} (compte {}, tick {}): {} seg={:?}\n  expected: {}\n  observed: {}",
        args[0], dossier.account, dossier.tick, dossier.kind, dossier.failed_segment, dossier.expected, dossier.observed,
    );
    let (world, tables) = match crate::routecmd::load_offline() {
        Ok(loaded) => loaded,
        Err(err) => {
            eprintln!("replay-nav: {err}");
            return 1;
        }
    };
    match plan::plan(&world, &tables, dossier.from, dossier.dest, &dossier.options) {
        Ok(replayed) => {
            println!("replay: {} segments cost {} (dossier: {} segments cost {})", replayed.segments.len(), replayed.cost, dossier.plan_segments, dossier.plan_cost);
            if replayed.segments.len() != dossier.plan_segments || replayed.cost != u32::try_from(dossier.plan_cost).unwrap_or(u32::MAX) {
                println!("replay: plan DIFFERE du dossier (donnees ou options ont change depuis l'echec)");
            }
            let pairs = plan_pairs(&replayed, dossier.from);
            let blocked = find_blocked_step(&world, &pairs);
            let hits = path_hits_diff(&dossier.path, &dossier.diff);
            match classify(blocked, &hits) {
                Verdict::A => {
                    let (from, to) = blocked.unwrap();
                    println!(
                        "verdict: {} — le plan rejoue marche dans un mur statique ({},{},{})->({},{},{}) flags={:#x}/{:#x}",
                        Verdict::A.as_str(),
                        from.x,
                        from.z,
                        from.level,
                        to.x,
                        to.z,
                        to.level,
                        world.get(from.x, from.z, from.level),
                        world.get(to.x, to.z, to.level),
                    );
                }
                Verdict::B => {
                    println!("verdict: {} — {} tuile(s) du chemin different entre statique et live :", Verdict::B.as_str(), hits.len());
                    for (x, z, level) in hits.iter().take(12) {
                        println!("  ({x},{z},{level}) static={:#x} live={:#x}", world.get(*x, *z, *level), live_of(&parsed, *x, *z, *level));
                    }
                }
                Verdict::C => {
                    println!("verdict: {} — plan coherent ({} pas rejoues, aucun mur statique), rien sur le chemin dans la diff", Verdict::C.as_str(), pairs.len());
                }
            }
            0
        }
        Err(no) => {
            println!("replay: pas de route ({}) closest=({},{},{})", no.reason.as_str(), no.closest_reachable.x, no.closest_reachable.z, no.closest_reachable.level);
            for (edge, tags) in &no.blocked_by_requirements {
                println!("  blocked by {edge:?} missing {tags:?}");
            }
            if dossier.plan_segments == 0 {
                println!("dossier sans plan (echec NoRoute) : la replanification redonne la meme impasse — coherente.");
            } else {
                println!("le dossier avait un plan mais la replanification echoue : les donnees ont change (override, verdict, exclusion).");
            }
            0
        }
    }
}

fn live_of(parsed: &Value, x: i32, z: i32, level: i32) -> u32 {
    for entry in parsed.get("static_live_diff").as_arr().cloned().unwrap_or_default() {
        if entry.get("x").as_i64() == Some(i64::from(x)) && entry.get("z").as_i64() == Some(i64::from(z)) && entry.get("level").as_i64() == Some(i64::from(level)) {
            return entry.get("live").as_i64().unwrap_or(0) as u32;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use lifescape_nav::collision::{tile_walkable, CollisionMap};
    use lifescape_nav::collision::{ANGLE_SOUTH, WALL_STRAIGHT};

    fn walled_map() -> CollisionMap {
        let mut map = CollisionMap::new();
        // Straight fence facing south on (10,10): WALL_SOUTH here,
        // WALL_NORTH on (10,9). East-west along the row stays legal.
        map.change_wall(10, 10, 0, ANGLE_SOUTH, WALL_STRAIGHT, false, true);
        map
    }

    fn step_blocked(map: &CollisionMap, from: (i32, i32), to: (i32, i32)) -> bool {
        let mut flags = |x: i32, z: i32| map.get(x, z, 0);
        !can_step(&mut flags, from.0, from.1, to.0, to.1)
    }

    #[test]
    fn crossing_a_fence_row_is_a_blocked_step() {
        let map = walled_map();
        assert!(tile_walkable(map.get(10, 10, 0)), "fence tile stays enterable from the open sides");
        assert!(!step_blocked(&map, (9, 10), (10, 10)), "along the row is legal");
        assert!(step_blocked(&map, (10, 9), (10, 10)), "crossing the south wall is refused");
        assert!(step_blocked(&map, (10, 10), (10, 9)), "leaving through the south wall is refused");
    }

    #[test]
    fn classify_a_needs_a_blocked_step() {
        let blocked = Some((Tile { x: 10, z: 9, level: 0 }, Tile { x: 10, z: 10, level: 0 }));
        assert_eq!(classify(blocked, &[(10, 10, 0)]), Verdict::A, "a static refusal wins over a diff hit");
        assert_eq!(classify(blocked, &[]), Verdict::A);
    }

    #[test]
    fn classify_b_needs_a_diff_hit_on_the_path() {
        let mut path = HashSet::new();
        path.insert((5, 5, 0));
        assert_eq!(path_hits_diff(&path, &[(5, 5, 0), (9, 9, 0)]), vec![(5, 5, 0)]);
        assert_eq!(classify(None, &[(5, 5, 0)]), Verdict::B);
    }

    #[test]
    fn classify_c_needs_no_refusal_and_no_hit() {
        assert!(path_hits_diff(&HashSet::from([(5, 5, 0)]), &[(9, 9, 0)]).is_empty());
        assert_eq!(classify(None, &[]), Verdict::C);
    }

    #[test]
    fn dossier_options_round_trip_through_json() {
        let raw = lifescape_json::parse(
            r#"{"allow_paid":true,"f2p_only":false,"satisfied":["coins:10"],"excluded_edges":[{"kind":"ride","name":"stairs"}]}"#,
        )
        .unwrap();
        let options = parse_options(&raw);
        assert!(options.allow_paid && !options.f2p_only);
        assert!(options.satisfied.contains("coins:10"));
        assert!(options.excluded_edges.contains(&EdgeId::Ride("stairs".to_string())));
        assert!(edge_of(&lifescape_json::parse(r#"{"kind":"door","loc":1530,"x":1,"z":2,"level":0}"#).unwrap())
            == Some(EdgeId::Door { loc: 1530, x: 1, z: 2, level: 0 }));
    }
}
