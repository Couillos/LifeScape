//! Tree pool: tiers, spawns, liveness, toll fence and hunter
//! filters, regrow prediction. Pure selection (no walking): the tree
//! driver walks to whatever this returns. Split from the old woodcut
//! script (file-size rule); the generic filters live in `kit::spots`.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::observe::Observe;
use crate::kit::spots::{drop_occupied, f2p_inside, hunter_covers, hunter_zones, refine_by_walk, OCCUPIED_RADIUS};
use crate::kit::bans::{across_toll, BanSet};
use crate::ScriptError;

const NEAR_BANK_TILES: i64 = 150;

/// Server regrow delay in ticks (1 bot tick = 1 server cycle) per tree
/// table (`woodcutting_trees.dbrow` respawnrate; `loc_change` counts
/// server cycles): oak/willow 30, maple 120, yew 200, magic 400. Rate-0
/// tables (normal/burnt/achey logs) regrow at 120+random(80), midpoint
/// 160. Memory is per tier, not per loc: the rate is defined per tree
/// table, identical for every spot of the tier at fixed playercount
/// (`scale_by_playercount` ~= 1.0 solo), so one exact wait calibrates
/// the whole tier — per-loc memory would need an observation per tree
/// for the same number. Observed waits replace the default.
pub(crate) fn default_respawn_ticks(product: &str) -> u64 {
    match product {
        "oak_logs" | "willow_logs" => 30,
        "maple_logs" => 120,
        "yew_logs" => 200,
        "magic_logs" => 400,
        _ => 160,
    }
}

/// Highest F2P tier whose level requirement is reached: members tiers
/// (`product_members`, magic) are never picked — travel and spots are
/// F2P-only, so selecting them just burns every repick on a
/// guaranteed-empty pool before degrading.
pub(crate) fn pick_tier(tiers: &[knowledge::trees::TreeTier], level: i64) -> Option<knowledge::trees::TreeTier> {
    let mut best: Option<&knowledge::trees::TreeTier> = None;
    for tier in tiers {
        if tier.product_members == Some(true) {
            continue;
        }
        if tier.level <= level && best.as_ref().is_none_or(|known| tier.level > known.level) {
            best = Some(tier);
        }
    }
    best.cloned()
}

/// Tree spawns for the tier's loc names within range of the bank.
fn tier_spawns(tier: &knowledge::trees::TreeTier, bank: &knowledge::banks::Bank) -> Vec<(i32, i32)> {
    let mut found = Vec::new();
    for entry in knowledge::all_locs() {
        let name = entry.name.clone().unwrap_or_default().to_lowercase();
        if tier.trees.iter().any(|tree| *tree == name) {
            for spawn in entry.spawns {
                if spawn.level == bank.level
                    && (spawn.x - bank.x).abs() + (spawn.z - bank.z).abs() <= NEAR_BANK_TILES
                {
                    found.push((spawn.x as i32, spawn.z as i32));
                }
            }
        }
    }
    found
}

/// Every spawn of the tier's loc names on this level, any bank: the
/// cluster-hop pool when nothing qualifies near the current bank.
fn tier_spawns_wide(tier: &knowledge::trees::TreeTier, level: i64) -> Vec<(i32, i32)> {
    let mut found = Vec::new();
    for entry in knowledge::all_locs() {
        let name = entry.name.clone().unwrap_or_default().to_lowercase();
        if tier.trees.iter().any(|tree| *tree == name) {
            for spawn in entry.spawns {
                if spawn.level == level {
                    found.push((spawn.x as i32, spawn.z as i32));
                }
            }
        }
    }
    found
}

/// False when the server cleared this tile or confirmed something else
/// here. A tile that has not streamed yet counts as standing (the static
/// template is the truth until the server says otherwise).
fn tree_live(bot: &Bot, spot_x: i32, spot_z: i32, tier_names: &[String]) -> bool {
    if bot.world.loc_removed(spot_x, spot_z, bot.world.level) {
        return false;
    }
    match bot.world.loc_at(spot_x, spot_z, bot.world.level) {
        Some((loc, _, _)) => knowledge::loc_by_id(usize::from(loc))
            .is_none_or(|entry| entry.name.is_some_and(|name| tier_names.iter().any(|tree| *tree == name.to_lowercase()))),
        None => true,
    }
}



/// Nearest live same-tier spawn within hop range. Pure scan: no logging,
/// no bans, no mutation — the caller logs and decides. `fence = None`
/// skips the toll check (cross-fence relocation when stranded).
#[allow(clippy::too_many_arguments)]
fn nearest_hop_spot(
    bot: &mut Bot,
    tier: &knowledge::trees::TreeTier,
    fence: Option<i32>,
    banned: &BanSet,
    except: Option<(i32, i32)>,
    hunters: &[(i64, i64, i64)],
    saw_paid_only: &mut bool,
    saw_f2p_only: &mut bool,
    observe: &mut Observe,
) -> Option<(i64, (i32, i32))> {
    // Local filters first (no service), then ONE f2p batch for the
    // survivors — never one IPC per spot in a tight loop.
    let mut spots: Vec<(i32, i32)> = Vec::new();
    for (spot_x, spot_z) in tier_spawns_wide(tier, i64::from(bot.world.level)) {
        if Some((spot_x, spot_z)) == except {
            continue;
        }
        if banned.contains((spot_x, spot_z)) {
            continue;
        }
        // Stale stump whose regrow ETA passed reads as live: the bot
        // wasn't there to see it regrow. Arrival re-streams and proves
        // it (live chop or fresh re-ban + re-stamp).
        let wait = observe.respawn.get(&tier.product).copied().unwrap_or_else(|| default_respawn_ticks(&tier.product));
        if !tree_live(bot, spot_x, spot_z, &tier.trees)
            && !observe.regrown_due(bot.world.ticks, (spot_x, spot_z), wait, is_stump(bot, spot_x, spot_z))
        {
            continue;
        }
        if across_toll(bot.world.x, spot_x, fence) {
            *saw_paid_only = true;
            continue;
        }
        if hunter_covers(&hunters, spot_x, spot_z) {
            continue;
        }
        spots.push((spot_x, spot_z));
    }
    let inside = f2p_inside(bot, &spots)?;
    let mut cands: Vec<(i64, (i32, i32))> = Vec::new();
    for ((spot_x, spot_z), live) in spots.into_iter().zip(inside) {
        if !live {
            *saw_f2p_only = true;
            continue;
        }
        let dist = i64::from((spot_x - bot.world.x).abs() + (spot_z - bot.world.z).abs());
        cands.push((dist, (spot_x, spot_z)));
    }
    let cands = drop_occupied(bot, cands, OCCUPIED_RADIUS);
    refine_by_walk(bot, cands)
}

pub(crate) fn pick_tree_spot(
    bot: &mut Bot,
    tiers: &[knowledge::trees::TreeTier],
    bank: &knowledge::banks::Bank,
    level: i64,
    banned: &mut BanSet,
    except: Option<(i32, i32)>,
    observe: &mut Observe,
) -> Option<(knowledge::trees::TreeTier, i32, i32)> {
    // Toll-fence aware: banks on the far side are last resort, never the
    // default — same getter as `nearest_bank`, one line per side.
    let fence = if bot.allow_paid { None } else { crate::kit::bank::toll_gate_x(&mut *bot) };
    // Hunter filter unless opted out: spots a threat could reach are
    // skipped (the hurt-driven flee below still handles wanderers).
    let hunters: Vec<(i64, i64, i64)> = if bot.allow_wild { Vec::new() } else { hunter_zones(bot) };
    let mut saw_paid_only = false;
    let mut saw_f2p_only = false;
    // F2P only, same rule as `pick_tier`: members tiers (magic) never
    // enter the pool, or every repick burns on a guaranteed-empty tier.
    let mut affordable: Vec<&knowledge::trees::TreeTier> = tiers
        .iter()
        .filter(|tier| tier.level <= level && tier.product_members != Some(true))
        .collect();
    affordable.sort_by_key(|tier| -tier.level);
    let best = affordable.first().copied();
    let top_level = best.map(|tier| tier.level);
    // First degraded-tier candidates, held back: the same-tier hop below
    // preempts them, and only a failed hop falls through to them, logged.
    let mut fallback: Option<(knowledge::trees::TreeTier, Vec<(i64, (i32, i32))>)> = None;
    for tier in affordable {
        // Local filters first (no service), then ONE f2p batch for the
        // survivors — never one IPC per spot in a tight loop.
        let mut spots: Vec<(i32, i32)> = Vec::new();
        for (spot_x, spot_z) in tier_spawns(tier, bank) {
            if Some((spot_x, spot_z)) == except {
                continue;
            }
            if banned.contains((spot_x, spot_z)) {
                continue;
            }
            let wait = observe.respawn.get(&tier.product).copied().unwrap_or_else(|| default_respawn_ticks(&tier.product));
            if !tree_live(bot, spot_x, spot_z, &tier.trees)
                && !observe.regrown_due(bot.world.ticks, (spot_x, spot_z), wait, is_stump(bot, spot_x, spot_z))
            {
                continue;
            }
            if across_toll(bot.world.x, spot_x, fence) {
                saw_paid_only = true;
                continue;
            }
            if hunter_covers(&hunters, spot_x, spot_z) {
                continue;
            }
            spots.push((spot_x, spot_z));
        }
        // Travel always plans F2P-only: an out-of-F2P spot is a
        // guaranteed NoRoute dossier (live t01-4572), never a pick.
        let Some(inside) = f2p_inside(bot, &spots) else {
            continue;
        };
        let mut cands: Vec<(i64, (i32, i32))> = Vec::new();
        for ((spot_x, spot_z), live) in spots.into_iter().zip(inside) {
            if !live {
                saw_f2p_only = true;
                continue;
            }
            let dist = i64::from((spot_x - bot.world.x).abs() + (spot_z - bot.world.z).abs());
            cands.push((dist, (spot_x, spot_z)));
        }
        if cands.is_empty() {
            continue;
        }
        if Some(tier.level) == top_level {
            let cands = drop_occupied(bot, cands, OCCUPIED_RADIUS);
            if let Some((_, (spot_x, spot_z))) = refine_by_walk(&mut *bot, cands) {
                return Some((tier.clone(), spot_x, spot_z));
            }
        } else if fallback.is_none() {
            fallback = Some((tier.clone(), cands));
        }
    }
    // Best tier fully stumped/banned near this bank: hop to the next live
    // cluster of the SAME tier before degrading. A hop that fails to walk
    // rides the short ban (Gone, 250t) so the next repick tries the
    // following cluster, then the tier below — no travel loop.
    if let Some(top) = best {
        let hop = nearest_hop_spot(
            &mut *bot,
            top,
            fence,
            banned,
            except,
            &hunters,
            &mut saw_paid_only,
            &mut saw_f2p_only,
            observe,
        );
        if let Some((dist, (spot_x, spot_z))) = hop {
            bot.log(&format!(
                "no {} near bank ({},{}), hopping same tier to ({spot_x},{spot_z}) +{dist} tiles",
                top.product, bank.x, bank.z
            ));
            return Some((top.clone(), spot_x, spot_z));
        }
        // Stranded on the wrong side of a toll fence: every in-range spot
        // is across it, so waiting for respawns is futile — nothing will
        // cross the fence on its own. Re-pick the best tier ignoring the
        // toll filter and let travel_to route the free detour around the
        // fence end (or pay it if allow_paid). The filter re-applies from
        // the new side, so this relocates once instead of ping-ponging.
        if fence.is_some() && saw_paid_only {
            if let Some((dist, (spot_x, spot_z))) = nearest_hop_spot(
                &mut *bot,
                top,
                None,
                banned,
                except,
                &hunters,
                &mut saw_paid_only,
                &mut saw_f2p_only,
                observe,
            ) {
                bot.log(&format!(
                    "no {} on this side of the toll fence, relocating across to ({spot_x},{spot_z}) +{dist} tiles",
                    top.product
                ));
                return Some((top.clone(), spot_x, spot_z));
            }
        }
        {
            // One debug line per failed best-tier repick: every in-range
            // spot with its disqualification reason (banned-remaining,
            // stump, gone, toll, hunter). The in-range pool is small (a
            // dozen spots); far ones collapse to a count. A "hop-missed"
            // tag would flag a filter/hop disagreement bug.
            let now = bot.world.ticks;
            let mut wide = tier_spawns_wide(top, i64::from(bot.world.level));
            wide.sort();
            wide.dedup();
            // One batch for the note tags below, like the pick filters.
            let wide_inside = f2p_inside(bot, &wide);
            let mut notes: Vec<String> = Vec::new();
            let mut banned_notes: Vec<((i32, i32), u64)> = Vec::new();
            for (index, &(spot_x, spot_z)) in wide.iter().enumerate() {
                let dist = i64::from((spot_x - bot.world.x).abs() + (spot_z - bot.world.z).abs());
                let why = if Some((spot_x, spot_z)) == except {
                    "noroute".to_string()
                } else if let Some(until) = banned.until((spot_x, spot_z)) {
                    banned_notes.push(((spot_x, spot_z), until));
                    format!("banned-{}t", until.saturating_sub(now))
                } else if !tree_live(bot, spot_x, spot_z, &top.trees) {
                    if is_stump(bot, spot_x, spot_z) {
                        // Regrow clock, read-only here (the pick filter
                        // owns stamping): due stumps read as live to the
                        // picker, so only the toll/f2p/hunter stages below
                        // can still exclude them.
                        let wait = observe.respawn.get(&top.product).copied().unwrap_or_else(|| default_respawn_ticks(&top.product));
                        let elapsed = observe.stumped.get(&(spot_x, spot_z)).map(|at| now.saturating_sub(*at));
                        match elapsed {
                            Some(past) if past >= wait => "stump-due".to_string(),
                            Some(past) => format!("stump~{}t", wait.saturating_sub(past)),
                            None => "stump".to_string(),
                        }
                    } else {
                        "gone".to_string()
                    }
                } else if across_toll(bot.world.x, spot_x, fence) {
                    "toll".to_string()
                } else if wide_inside.as_ref().is_some_and(|verdicts| !verdicts[index]) {
                    "f2p".to_string()
                } else if wide_inside.is_none() {
                    // Down batch: name it, so the famine release stays
                    // shut (no verified spot to retry) and the note tells
                    // the outage apart from a genuinely filtered pool.
                    "nav-down".to_string()
                } else if hunter_covers(&hunters, spot_x, spot_z) {
                    "hunter".to_string()
                } else {
                    "hop-missed".to_string()
                };
                notes.push(format!("({spot_x},{spot_z}){why}+{dist}"));
            }
            let degrading = fallback.is_some();
            bot.log(&format!(
                "no {} reachable anywhere{}: {} disqualified [{}]",
                top.product,
                if degrading { ", degrading a tier" } else { "" },
                notes.len(),
                notes.join(" "),
            ));
            // Anti-famine: every in-range spot banned (none stumped, gated
            // or hunted — the tier is not depleted, just blacklisted) means
            // a transient storm poisoned the clocks. Retry the oldest ban
            // instead of degrading a tier dumbly; a still-dead spot re-bans
            // on the next attempt, one release per repick so no hot loop.
            // A planner-proven dead spot (`except`, NoRoute from here)
            // breaks unanimity above ("noroute", never released): retrying
            // it from the same tile is provably futile until the bot moves.
            if let Some(oldest) = banned.release_oldest_if_starved(notes.len(), &banned_notes) {
                bot.log(&format!(
                    "tier starved by bans: {} {} banned in range, releasing oldest ({},{})",
                    banned_notes.len(),
                    top.product,
                    oldest.0,
                    oldest.1,
                ));
                return pick_tree_spot(&mut *bot, tiers, bank, level, &mut *banned, except, observe);
            }
        }
    }
    if saw_paid_only {
        bot.log("tree spots only across toll fence");
    }
    if saw_f2p_only {
        bot.log("tree spots only outside F2P");
    }
    // Explicit downgrade: name both tiers so a willow interlude on a yew
    // account reads as a decision with a reason (the disqualified line
    // above), never a silent slide. `fallback` is the highest degraded
    // tier with live spots, refined by walking cost like the top tier.
    let (down, cands) = fallback?;
    if let Some(top) = best {
        if down.level < top.level {
            bot.log(&format!("degrading {}->{} for this repick (best tier unreachable, reasons above)", top.product, down.product));
        }
    }
    let cands = drop_occupied(bot, cands, OCCUPIED_RADIUS);
    refine_by_walk(&mut *bot, cands).map(|(_, (spot_x, spot_z))| (down, spot_x, spot_z))
}

/// Live loc id at a tile (live stream first, static template as fallback).
/// The worker paints nothing: the fallback is one `tiles_flags` batch to
/// the nav service, which reads its snapshot. Live still wins (a stump
/// streams live), so `is_stump`/`tree_still_there` keep deciding below.
/// A down service stays loud (`nav-down` via `ScriptError::Sdk`); only a
/// tile the service has no template for keeps the `Usage` error.
pub(crate) fn resolve_loc(bot: &mut Bot, tile_x: i32, tile_z: i32) -> Result<(u16, u8), ScriptError> {    if let Some((loc, _, angle)) = bot.world.loc_at(tile_x, tile_z, bot.world.level) {
        return Ok((loc, angle));
    }
    let level = bot.world.level;
    let rows = bot.tiles_flags(&[(tile_x, tile_z, level)]).map_err(ScriptError::Sdk)?;
    if let Some((loc, _, angle)) = rows.into_iter().next().and_then(|row| row.loc) {
        return Ok((loc, angle));
    }
    Err(ScriptError::Usage(format!("no loc at {tile_x},{tile_z} from {:?}", bot.position())))
}

/// Live loc id at a tile (live stream first, static template as fallback).
pub(crate) fn live_loc_id(bot: &mut Bot, tree_x: i32, tree_z: i32) -> Result<u16, ScriptError> {
    resolve_loc(bot, tree_x, tree_z).map(|(loc, _)| loc)
}

/// True only when the server confirms a stump here. Unknown means keep
/// waiting, never assume depletion.
pub(crate) fn is_stump(bot: &Bot, tree_x: i32, tree_z: i32) -> bool {
    match bot.world.locs.get(&(tree_x, tree_z, bot.world.level)) {
        Some(Some((loc, _, _))) => knowledge::loc_by_id(usize::from(*loc))
            .is_some_and(|entry| entry.name.unwrap_or_default().contains("stump")),
        _ => false,
    }
}

pub(crate) fn tree_still_there(bot: &mut Bot, spot_x: i32, spot_z: i32, tier_names: &[String]) -> bool {
    if bot.world.loc_removed(spot_x, spot_z, bot.world.level) {
        return false;
    }
    let Ok(loc) = live_loc_id(bot, spot_x, spot_z) else {
        return true;
    };
    match knowledge::loc_by_id(usize::from(loc)) {
        Some(entry) => entry.name.is_some_and(|name| tier_names.iter().any(|tree| *tree == name.to_lowercase())),
        None => true,
    }
}





#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::observe::Observe;
    use crate::kit::spots::refine_by_walk;
    use crate::kit::bank::toll_gate_x;
    use crate::kit::bans::{same_side, starved_oldest, BanSet};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    /// Every yew spawn banned except `keep`: the hop/degrade/starvation
    /// paths must decide on the whole tier, never on a 200-tile window.
    /// Returns the bans and the full spawn list (for len assertions).
    fn ban_all_yews_except(tiers: &[knowledge::trees::TreeTier], keep: &[(i32, i32)]) -> (BanSet, Vec<(i32, i32)>) {
        let yew = tiers.iter().find(|tier| tier.product == "yew_logs").expect("yew tier in dump");
        let mut wide = tier_spawns_wide(yew, 0);
        wide.sort();
        wide.dedup();
        let mut bans = BanSet::new();
        for pos in &wide {
            if !keep.contains(pos) {
                bans.ban_until(*pos, u64::MAX);
            }
        }
        (bans, wide)
    }

    /// F2P only: magic (75, members) never wins, even far above its
    /// level — the spot pool would be 100% out-of-F2P and every repick
    /// would burn on a guaranteed-empty tier before degrading.
    #[test]
    fn pick_tier_never_selects_members() {
        let tiers = lifescape_gamedata::trees::all_tiers();
        assert_eq!(pick_tier(&tiers, 99).expect("tier").product, "yew_logs");
        assert_eq!(pick_tier(&tiers, 75).expect("tier").product, "yew_logs");
        assert_eq!(pick_tier(&tiers, 60).expect("tier").product, "yew_logs");
    }

    /// Mapless worker against a real nav service (repo dumps): the bot
    /// carries no `nav_world`, only the plan handle. Each test binds the
    /// service so the socket outlives the bot. Hunter filter off unless
    /// the test says otherwise; the fence stays live unless the test
    /// opts into paid crossings.
    fn served_spot_bot(
        spot_x: i32,
        spot_z: i32,
        socket_tag: &str,
    ) -> (Bot, std::sync::Arc<lifescape_nav::service::Service>) {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = spot_x;
        bot.world.z = spot_z;
        bot.allow_wild = true;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
        let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
        let data = root.join("LifeScape/gamedata/data");
        let gametypes = std::fs::read_to_string(data.join("gametypes.json")).expect("gametypes");
        let overrides = std::fs::read_to_string(data.join("nav_overrides.json")).expect("overrides");
        let defs = lifescape_nav::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
        let world = lifescape_nav::static_world::StaticWorld::open(&zip, defs).expect("static world");
        let zones = lifescape_nav::f2p::load_zones(&root.join("lostcity/content/maps/free2play.csv")).expect("f2p");
        world.set_f2p(zones);
        let tables = std::sync::Arc::new(lifescape_nav::tables::Tables::load(&data).expect("tables"));
        let version = format!("test-spot-{socket_tag}");
        let service = lifescape_nav::service::Service::start(
            lifescape_nav::service::Snapshot::new(world, tables, version.clone()),
            &lifescape_nav::service::test_socket(socket_tag),
            1,
        )
        .expect("service starts");
        bot.plan_client = Some(lifescape_nav::service::PlanClient::new(service.socket_path(), socket_tag));
        bot.plan_data_version = Some(version);
        bot.last_keepalive = Some(std::time::Instant::now());
        (bot, service)
    }

    /// Stump clock: a remembered stump whose regrow ETA passed reads as
    /// live — the bot wasn't there to see it regrow. Arrival re-streams
    /// and proves it (or re-bans with a fresh clock). Paid crossings on:
    /// these tests prove the stump clock, not the fence, so the real
    /// fence (if any) stays out of the way like the old empty paint.
    fn stump_bot_at(spot: (i32, i32), now: u64, socket_tag: &str) -> (Bot, std::sync::Arc<lifescape_nav::service::Service>) {
        let (mut bot, service) = served_spot_bot(3053, 3271, socket_tag);
        bot.world.ticks = now;
        bot.allow_paid = true;
        // Static yew stump on the tile: the stream still shows it.
        bot.world.locs.insert((spot.0, spot.1, 0), Some((1341, 0, 0)));
        (bot, service)
    }

    fn draynor_bank() -> lifescape_gamedata::banks::Bank {
        lifescape_gamedata::banks::Bank {
            x: 3091,
            z: 3242,
            level: 0,
            loc_id: 0,
            name: "draynor".to_string(),
            members: false,
        }
    }

    #[test]
    fn due_stump_reads_live_and_gets_picked() {
        let tiers = lifescape_gamedata::trees::all_tiers();
        let bank = draynor_bank();
        let spot = (3053, 3271);
        let (mut banned, _) = ban_all_yews_except(&tiers, &[spot]);
        let mut observe = Observe::fresh();
        observe.stumped.insert(spot, 0);
        let (mut bot, _service) = stump_bot_at(spot, 200, "spot-due-stump");
        let picked = pick_tree_spot(&mut bot, &tiers, &bank, 63, &mut banned, None, &mut observe);
        let Some((tier, spot_x, spot_z)) = picked else {
            panic!("due stump should read live, got None");
        };
        assert_eq!(tier.product, "yew_logs", "stays on tier, got: {}", tier.product);
        assert_eq!((spot_x, spot_z), spot, "walks back to verify, got: ({spot_x},{spot_z})");
    }

    #[test]
    fn fresh_stump_stays_down_and_degrades() {
        let tiers = lifescape_gamedata::trees::all_tiers();
        let bank = draynor_bank();
        let spot = (3053, 3271);
        let (mut banned, _) = ban_all_yews_except(&tiers, &[spot]);
        // Stumped "now": the 200-tick yew regrow hasn't elapsed.
        let mut observe = Observe::fresh();
        observe.stumped.insert(spot, 1000);
        let (mut bot, _service) = stump_bot_at(spot, 1000, "spot-fresh-stump");
        let picked = pick_tree_spot(&mut bot, &tiers, &bank, 63, &mut banned, None, &mut observe);
        let Some((tier, _, _)) = picked else {
            panic!("fresh stump should degrade, got None");
        };
        assert_eq!(tier.product, "willow_logs", "degrades while the stump is down, got: {}", tier.product);
    }

    #[test]
    fn first_sight_stamps_and_second_repick_verifies() {
        let tiers = lifescape_gamedata::trees::all_tiers();
        let bank = draynor_bank();
        let spot = (3053, 3271);
        let (mut banned, _) = ban_all_yews_except(&tiers, &[spot]);
        let mut observe = Observe::fresh();
        // First sight stamps the clock: still down, degrades.
        let (mut bot, _service) = stump_bot_at(spot, 10000, "spot-first-sight");
        let picked = pick_tree_spot(&mut bot, &tiers, &bank, 63, &mut banned, None, &mut observe);
        let product = picked.map(|(tier, _, _)| tier.product);
        assert!(product.as_deref() == Some("willow_logs"), "first sight degrades, got: {product:?}");
        assert_eq!(observe.stumped.get(&spot), Some(&10000), "first sight stamps the clock");
        // A regrow window later the same stump reads live again.
        bot.world.ticks = 10250;
        let picked = pick_tree_spot(&mut bot, &tiers, &bank, 63, &mut banned, None, &mut observe);
        let Some((tier, spot_x, spot_z)) = picked else {
            panic!("due stump should read live, got None");
        };
        assert_eq!((tier.product.clone(), (spot_x, spot_z)), ("yew_logs".to_string(), spot), "re-probe after ETA, got: {} at ({spot_x},{spot_z})", tier.product);
    }

    use super::default_respawn_ticks;

    /// Defaults mirror the server tree table (`woodcutting_trees.dbrow`
    /// respawnrate): oak/willow 30, maple 120, yew 200, magic 400, and
    /// 160 for normal logs (120+random(80), midpoint). Unknown products
    /// fall back to the normal-tree mean, never a round guess.
    #[test]
    fn respawn_defaults_match_server_table() {
        assert_eq!(default_respawn_ticks("oak_logs"), 30, "oak regrows in 30");
        assert_eq!(default_respawn_ticks("willow_logs"), 30, "willow regrows in 30");
        assert_eq!(default_respawn_ticks("maple_logs"), 120, "maple regrows in 120");
        assert_eq!(default_respawn_ticks("yew_logs"), 200, "yew regrows in 200");
        assert_eq!(default_respawn_ticks("magic_logs"), 400, "magic regrows in 400");
        assert_eq!(default_respawn_ticks("logs"), 160, "normal trees average 160");
        assert_eq!(default_respawn_ticks("something_new"), 160, "unknown falls back to the normal mean");
    }

    /// Threat rule mirrors the server: covered by the kit brick's own
    /// test (`kit::spots`), kept here as the call-site witness that the
    /// tree picker filters through it.
    #[test]
    fn hunter_filter_covers_threatened_tiles() {
        assert!(hunter_covers(&[(0, 0, 5)], 3, 2), "inside the zone");
        assert!(!hunter_covers(&[(0, 0, 5)], 4, 2), "outside the zone");
        assert!(!hunter_covers(&[], 0, 0), "no hunters, nothing covered");
    }

    /// Toll fence reporting: the Al-Kharid gate line comes from the
    /// service, not painted state — x=3268 near the corridor, nothing
    /// near Draynor. Pure side math stays covered in `kit::bans`.
    #[test]
    fn toll_fence_reports_the_gate_line() {
        let (mut bot, _service) = served_spot_bot(3275, 3227, "spot-fence-line");
        bot.allow_paid = false;
        assert_eq!(toll_gate_x(&mut bot), Some(3268), "gate line from the service");
        assert!(same_side(3268, 3275, 3300), "east flank together");
        assert!(!same_side(3268, 3275, 3249), "across never targets");
        assert!(across_toll(3275, 3249, Some(3268)), "west spot is across");
        assert!(!across_toll(3275, 3300, Some(3268)), "east spot is same side");
        assert!(!across_toll(3275, 3249, None), "no fence means no filtering");
        // No fence near Draynor: the getter stays quiet there.
        let (mut draynor, _service) = served_spot_bot(3091, 3242, "spot-fence-far");
        draynor.allow_paid = false;
        assert_eq!(toll_gate_x(&mut draynor), None, "no fence near Draynor");
    }

    /// Toll fence: the only live yew sits across the real Al-Kharid
    /// fence → relocate to the same tier instead of degrading to willow.
    /// Every other yew spawn is banned; (3249,3201) sits west of the
    /// line: relocation (not the anti-famine release) resolves the pick.
    #[test]
    fn toll_stranded_relocates_across_fence_to_same_tier() {
        let (mut bot, _service) = served_spot_bot(3275, 3227, "spot-relocate");
        bot.allow_paid = false;
        assert_eq!(toll_gate_x(&mut bot), Some(3268), "east of the real fence");
        let bank = draynor_bank();
        let tiers = lifescape_gamedata::trees::all_tiers();
        // Every yew spawn banned except (3249,3201), across the fence.
        let (mut banned, wide) = ban_all_yews_except(&tiers, &[(3249, 3201)]);
        let picked = pick_tree_spot(&mut bot, &tiers, &bank, 63, &mut banned, None, &mut Observe::fresh());
        let Some((tier, spot_x, spot_z)) = picked else {
            panic!("stranded bot should relocate across the fence, got None");
        };
        assert_eq!(tier.product, "yew_logs", "same tier, not willow, got: {}", tier.product);
        assert_eq!((spot_x, spot_z), (3249, 3201), "cross-fence yew, got: ({spot_x},{spot_z})");
        assert!(spot_x < 3268, "relocated west of the fence, got x={spot_x}");
        assert_eq!(banned.len(), wide.len() - 1, "relocation resolves without releasing bans, got: {}", banned.len());
    }

    /// Same-side filter: the nearer yew across the real fence loses to
    /// the farther same-side one — the pick never crosses while unpaid.
    /// (3270,3470) east-far beats (3249,3201) west-near from (3275,3227).
    #[test]
    fn toll_filter_prefers_same_side_over_nearer_across() {
        let (mut bot, _service) = served_spot_bot(3275, 3227, "spot-sameside");
        bot.allow_paid = false;
        assert_eq!(toll_gate_x(&mut bot), Some(3268), "east of the real fence");
        let bank = draynor_bank();
        let tiers = lifescape_gamedata::trees::all_tiers();
        let (mut banned, _) = ban_all_yews_except(&tiers, &[(3270, 3470), (3249, 3201)]);
        let picked = pick_tree_spot(&mut bot, &tiers, &bank, 63, &mut banned, None, &mut Observe::fresh());
        let Some((tier, spot_x, spot_z)) = picked else {
            panic!("same-side yew should win, got None");
        };
        assert_eq!(tier.product, "yew_logs", "same tier, not degraded, got: {}", tier.product);
        assert_eq!((spot_x, spot_z), (3270, 3470), "far same-side beats near across, got: ({spot_x},{spot_z})");
    }

    /// Same-tier hop: every other yew is banned, so the pick walks to
    /// the next live yew cluster ((2996,3311), 97 tiles off) instead of
    /// settling for the live willow next door.
    #[test]
    fn hop_same_tier_before_degrading() {
        let (mut bot, _service) = served_spot_bot(3053, 3271, "spot-hop");
        // Paid crossings on: this test proves the hop, not the fence.
        bot.allow_paid = true;
        let bank = draynor_bank();
        let tiers = lifescape_gamedata::trees::all_tiers();
        let (mut banned, _) = ban_all_yews_except(&tiers, &[(2996, 3311)]);
        let picked = pick_tree_spot(&mut bot, &tiers, &bank, 63, &mut banned, None, &mut Observe::fresh());
        let Some((tier, spot_x, spot_z)) = picked else {
            panic!("hop should find the next yew cluster, got None");
        };
        assert_eq!(tier.product, "yew_logs", "same tier, not willow");
        assert_eq!((spot_x, spot_z), (2996, 3311), "nearest live yew cluster, got: ({spot_x},{spot_z})");
    }

    /// Starvation: every yew spawn banned (a transient storm poisoned
    /// the clocks) retries the oldest ban instead of degrading to willow.
    /// Staggered clocks prove the oldest goes first, and its entry leaves
    /// the map; a still-dead spot re-bans on the next attempt.
    #[test]
    fn starved_tier_releases_oldest_ban() {
        let (mut bot, _service) = served_spot_bot(3053, 3271, "spot-starved");
        // Paid crossings on: this test proves the famine release, not
        // the fence.
        bot.allow_paid = true;
        let bank = lifescape_gamedata::banks::Bank {
            x: 3091,
            z: 3242,
            level: 0,
            loc_id: 0,
            name: "draynor".to_string(),
            members: false,
        };
        let tiers = lifescape_gamedata::trees::all_tiers();
        // The whole tier banned: the oldest clock ((3053,3271)@1000)
        // goes first, everything else ticks later so unanimity holds.
        let yew = tiers.iter().find(|tier| tier.product == "yew_logs").expect("yew tier in dump");
        let mut wide = tier_spawns_wide(yew, 0);
        wide.sort();
        wide.dedup();
        assert!(wide.contains(&(3053, 3271)), "oldest spot is a yew spawn, wide={wide:?}");
        let mut banned = BanSet::new();
        banned.ban_until((3053, 3271), 1000);
        let mut clock = 2000u64;
        for pos in &wide {
            if *pos == (3053, 3271) {
                continue;
            }
            clock += 1;
            banned.ban_until(*pos, clock);
        }
        let before = banned.len();
        assert_eq!(before, wide.len(), "whole tier banned, got: {before} of {}", wide.len());
        let picked = pick_tree_spot(&mut bot, &tiers, &bank, 63, &mut banned, None, &mut Observe::fresh());
        let Some((tier, spot_x, spot_z)) = picked else {
            panic!("starved tier should retry the oldest ban, got None");
        };
        assert_eq!(tier.product, "yew_logs", "same tier retried, not willow, got: {}", tier.product);
        assert_eq!((spot_x, spot_z), (3053, 3271), "oldest ban retried first, got: ({spot_x},{spot_z})");
        assert!(!banned.contains((3053, 3271)), "released entry leaves the map");
        assert_eq!(banned.len(), before - 1, "one release per repick, got: {}", banned.len());
    }

    /// Release rule: only a full ban sweep (100% of spots seen) releases
    /// the oldest; one stump, gate or hunter means genuinely unavailable.
    #[test]
    fn starved_oldest_needs_full_ban_sweep() {
        assert_eq!(starved_oldest(0, &[]), None, "nothing seen, nothing released");
        assert_eq!(starved_oldest(2, &[((1, 1), 50)]), None, "mixed stump+ban never releases");
        assert_eq!(
            starved_oldest(2, &[((1, 1), 50), ((2, 2), 30)]),
            Some((2, 2)),
            "oldest first, got: {:?}",
            starved_oldest(2, &[((1, 1), 50), ((2, 2), 30)])
        );
    }

    /// F2P filter: travel plans F2P-only, so an out-of-F2P spot is never a
    /// pick (live t01-4572: NoRoute OutsideF2p to a willow at (2906,3385)).
    /// Verdicts come from one service batch per pool; a down service
    /// drops the pool instead of guessing (the wait loop retries).
    #[test]
    fn f2p_filter_skips_outside_spots() {
        let (mut bot, _service) = served_spot_bot(2862, 3384, "spot-f2p");
        bot.allow_paid = true;
        // The service verdicts match the old direct reads: the dossier
        // tiles sit outside F2P, the Draynor bank booth inside.
        assert_eq!(bot.in_f2p_batch(&[(2862, 3385, 0)]).expect("f2p answers"), vec![false], "dossier spot is outside F2P");
        assert_eq!(bot.in_f2p_batch(&[(2906, 3385, 0)]).expect("f2p answers"), vec![false], "dossier leg dest is outside F2P");
        assert_eq!(bot.in_f2p_batch(&[(3091, 3242, 0)]).expect("f2p answers"), vec![true], "Draynor bank booth is inside F2P");
        let bank = lifescape_gamedata::banks::Bank {
            x: 2860,
            z: 3380,
            level: 0,
            loc_id: 0,
            name: "test".to_string(),
            members: false,
        };
        let tiers = lifescape_gamedata::trees::all_tiers();
        // Every tree spawn in both pools (near-bank 150 and same-tier
        // hop) banned except the out-of-F2P one: the pick must refuse
        // it instead of walking into a guaranteed NoRoute dossier.
        // Tier-agnostic: the dossier tile's tier is whatever it is.
        let mut tree_names = std::collections::HashSet::new();
        for tier in &tiers {
            for tree in &tier.trees {
                tree_names.insert(tree.clone());
            }
        }
        let mut banned = BanSet::new();
        for entry in lifescape_gamedata::all_locs() {
            let name = entry.name.clone().unwrap_or_default().to_lowercase();
            if !tree_names.contains(&name) {
                continue;
            }
            for spawn in entry.spawns {
                if spawn.level == 0 && (spawn.x - 2862).abs() + (spawn.z - 3384).abs() <= 200 {
                    let pos = (spawn.x as i32, spawn.z as i32);
                    if pos != (2862, 3385) {
                        banned.ban_until(pos, u64::MAX);
                    }
                }
            }
        }
        let filtered = pick_tree_spot(&mut bot, &tiers, &bank, 30, &mut banned, None, &mut Observe::fresh());
        let outside = filtered.as_ref().is_some_and(|(_, found_x, found_z)| (*found_x, *found_z) == (2862, 3385));
        assert!(!outside, "filtered pick never returns the F2P-excluded spot, got: {filtered:?}");
    }

    /// Planner-proven dead spot (`except`): never picked, never
    /// starvation-released, breaks unanimity so the repick degrades to the
    /// highest CUTTABLE tier instead of hot-looping the dead yew. No maple
    /// spawns near the bank (dump), so yew->willow with oak/logs untouched
    /// when every other yew is banned: tiers are walked in order, never
    /// skipped when available, and the downgrade is logged.
    #[test]
    fn except_dead_spot_degrades_one_tier() {
        let (mut bot, _service) = served_spot_bot(3053, 3271, "spot-except");
        // Paid crossings on: this test proves the dead-spot degrade, not
        // the fence.
        bot.allow_paid = true;
        let bank = lifescape_gamedata::banks::Bank {
            x: 3091,
            z: 3242,
            level: 0,
            loc_id: 0,
            name: "draynor".to_string(),
            members: false,
        };
        let tiers = lifescape_gamedata::trees::all_tiers();
        // Every yew spawn banned except the planner-dead one, which is
        // excluded instead of released: the repick must degrade to willow
        // (no maple spawns near this bank, so willow is next).
        let (mut banned, wide) = ban_all_yews_except(&tiers, &[(2996, 3311)]);
        let before = banned.len();
        assert_eq!(before, wide.len() - 1, "all but the dead spot banned, got: {before} of {}", wide.len());
        let picked = pick_tree_spot(&mut bot, &tiers, &bank, 63, &mut banned, Some((2996, 3311)), &mut Observe::fresh());
        let Some((tier, _, _)) = picked else {
            panic!("dead-spot repick should degrade one tier, got None");
        };
        assert_eq!(tier.product, "willow_logs", "highest cuttable tier below yew, got: {}", tier.product);
        assert!(tier.product != "oak_logs" && tier.product != "logs", "never crashes to the bottom, got: {}", tier.product);
        assert_eq!(banned.len(), before, "no release without unanimity, got: {}", banned.len());
    }

    /// Walking-cost selection: the Manhattan-nearest oak (3252,3237, +26)
    /// sits behind houses/doors from (3230,3241) while (3203,3246, +32)
    /// walks clean — the lure loses. Measured walk: 50 vs 36. One remote
    /// batch, L1-cached on repeat.
    #[test]
    fn walk_cost_beats_manhattan_lure() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        crate::wire_nav(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3230;
        bot.world.z = 3241;
        let world = bot.nav_world.clone().expect("wired");
        let tables = bot.nav_tables.clone().expect("wired");
        let service = lifescape_nav::service::Service::start(
            lifescape_nav::service::Snapshot::new(world, tables, "test-spot".to_string()),
            &lifescape_nav::service::test_socket("spot-walk"),
            1,
        )
        .expect("service starts");
        bot.plan_client = Some(lifescape_nav::service::PlanClient::new(service.socket_path(), "spot"));
        bot.plan_data_version = Some("test-spot".to_string());
        bot.last_keepalive = Some(std::time::Instant::now());
        let cands = vec![(26, (3252, 3237)), (32, (3203, 3246))];
        let picked = refine_by_walk(&mut bot, cands).expect("non-empty pool picks");
        assert_eq!(picked.1, (3203, 3246), "walk-cheap wins over the Manhattan lure, got: {picked:?}");
        // L1 hit: the same cands cost no new compute.
        let before = service.stats();
        let cands = vec![(26, (3252, 3237)), (32, (3203, 3246))];
        let again = refine_by_walk(&mut bot, cands).expect("non-empty pool picks");
        assert_eq!(again.1, (3203, 3246), "repeat ranks identically");
        let after = service.stats();
        assert_eq!(after.misses - before.misses, 0, "second ranking hits L1, never recomputes");
        assert!(after.hits + after.trunc_hits > before.hits + before.trunc_hits, "L1 served the repeat");
    }

    /// No planner wired (old offline tests, golden paths): the pick keeps
    /// the Manhattan order, never empties — ranking changes, picking never.
    #[test]
    fn unwired_keeps_manhattan_order() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3230;
        bot.world.z = 3241;
        let cands = vec![(26, (3252, 3237)), (32, (3203, 3246))];
        let picked = refine_by_walk(&mut bot, cands).expect("non-empty pool picks");
        assert_eq!(picked.1, (3252, 3237), "unwired keeps Manhattan order, got: {picked:?}");
    }

    /// Unstreamed tree tile: the service template answers the loc instead
    /// of ending the run with `Usage` (live t01: `no loc at (3053,3271)`).
    /// Live still wins once streamed; a tile with no template stays `Usage`.
    #[test]
    fn live_loc_id_falls_back_to_the_service_template() {
        let (mut bot, _service) = served_spot_bot(3053, 3271, "spot-loc-fallback");
        assert!(bot.world.loc_at(3053, 3271, 0).is_none(), "nothing streamed yet");
        let loc = live_loc_id(&mut bot, 3053, 3271).expect("static template answers");
        let name =
            knowledge::loc_by_id(usize::from(loc)).and_then(|entry| entry.name).unwrap_or_default().to_lowercase();
        assert!(name.contains("yew"), "template names the yew, got: {name}");
        // Live wins once streamed: a stump id reports the stump, not the tree.
        bot.world.locs.insert((3053, 3271, 0), Some((1341, 0, 0)));
        assert_eq!(live_loc_id(&mut bot, 3053, 3271).expect("live answers"), 1341, "live beats the template");
        // Footprint follows the fallback too: yews are 3x3, never 1x1.
        bot.world.locs.remove(&(3053, 3271, 0));
        assert_eq!(crate::kit::harvest::spot_footprint(&mut bot, 3053, 3271), (3, 3), "fallback footprint");
    }

    /// A tile the service has no template for keeps the `Usage` error
    /// (a spot with genuinely no tree), never a guessed id.
    #[test]
    fn live_loc_id_without_template_stays_usage() {
        let (mut bot, _service) = served_spot_bot(3053, 3271, "spot-loc-empty");
        // One batch over open grass: the first tile without a template
        // proves the error path.
        let cands: Vec<(i32, i32, i32)> = (0..32).map(|index| (3000 + index, 3300, 0)).collect();
        let rows = bot.tiles_flags(&cands).expect("flags answer");
        let empty = cands
            .iter()
            .zip(rows)
            .find(|(_, row)| row.loc.is_none())
            .map(|(tile, _)| *tile)
            .expect("open grass somewhere");
        let err = live_loc_id(&mut bot, empty.0, empty.1).expect_err("no template is Usage");
        assert!(matches!(err, ScriptError::Usage(_)), "Usage, got: {err:?}");
    }
}
