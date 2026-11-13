//! Generic spot pool: the filters every gather skill shares, whatever the
//! resource. Pure selection (no walking): the gather loop walks to
//! whatever the caller picks from the survivors.
//!
//! Occupation perçue (new): a spot where the bot *sees* another player
//! working is skipped while free spots exist — like a player would, and
//! indispensable with hundreds of bots. Only own vision ever decides
//! (D22): `bot.world.players` is what this bot sees, nothing shared.
//! When every candidate is occupied the pool is kept whole (never idle
//! while a resource stands).

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::{bans::{GONE_BAN_TICKS, UNREACHABLE_BAN_TICKS, BanSet}, observe::STRUCTURAL_BAN_TICKS, pack::obj_price};
use crate::ScriptError;

/// Bank trek bounds and live-spot radius for the rock/fish pools.
const NEAR_BANK_TILES: i64 = 150;

const LIVE_RADIUS: i32 = 40;

const BANK_TREK_LIMIT: i64 = 150;

/// Chebyshev radius around a spot that reads as worked when another
/// player stands inside it (stands ring the spot at distance 1-2; 3
/// covers a neighbour tree's cutter without shadowing a whole forest).
pub const OCCUPIED_RADIUS: i32 = 3;

/// Visible players on this level: only what this bot sees, never shared.
pub fn visible_workers(bot: &Bot) -> Vec<(i32, i32)> {
    bot.world
        .players
        .values()
        .filter(|player| player.level == bot.world.level)
        .map(|player| (player.x, player.z))
        .collect()
}

/// Drop candidates another visible player stands on (Chebyshev `radius`
/// or under), unless that empties the pool — then keep it whole. Logs
/// the dodge so a crowded hop reads as a decision with a reason.
pub fn drop_occupied(
    bot: &mut Bot,
    cands: Vec<(i64, (i32, i32))>,
    radius: i32,
) -> Vec<(i64, (i32, i32))> {
    if cands.len() <= 1 {
        return cands;
    }
    let workers = visible_workers(bot);
    if workers.is_empty() {
        return cands;
    }
    let free: Vec<(i64, (i32, i32))> = cands
        .iter()
        .filter(|(_, (spot_x, spot_z))| {
            !workers.iter().any(|(worker_x, worker_z)| {
                (worker_x - spot_x).abs().max((worker_z - spot_z).abs()) <= radius
            })
        })
        .copied()
        .collect();
    if free.is_empty() {
        return cands;
    }
    if free.len() != cands.len() {
        bot.log(&format!("{} of {} spots look worked, skipping them", cands.len() - free.len(), cands.len()));
    }
    free
}

/// Shortlist bound for the walking-cost refinement: Manhattan ranks the
/// filtered pool, only the top-K get a pure plan, the cheapest verified
/// walk wins. K=4: live plans cost ~30-500 ms, so <=4 plans per pick
/// (amortized over minutes of gathering); a true-nearest pushed past rank
/// 4 needs a detour the toll filter already excludes.
pub(crate) const WALK_SHORTLIST: usize = 4;

/// Cheapest-to-walk spot among pre-filtered candidates (toll, hunter, F2P,
/// bans and liveness ran BEFORE, so no plan is wasted on an excluded
/// spot). One remote batch (no local Dijkstra, no walk, no send). The
/// ranking lives in `lifescape_nav::walk_rank`; this only bounds the
/// pool, maps the winner back to a walk-or-Manhattan metric and logs the
/// upset. Unwired (no service) or all-disconnected keeps the Manhattan
/// order, never drops the pick.
pub(crate) fn refine_by_walk(bot: &mut Bot, mut cands: Vec<(i64, (i32, i32))>) -> Option<(i64, (i32, i32))> {
    cands.sort();
    let manhattan = *cands.first()?;
    let level = bot.world.level;
    let (cost, (_, pos)) = crate::skill_walk::refine_walking(bot, cands, WALK_SHORTLIST, "spot", |(_, pos)| lifescape_nav::plan::Tile { x: pos.0, z: pos.1, level }).into_iter().next().expect("non-empty pool ranks");
    Some(match cost {
        Some(walk) => (i64::from(walk), pos),
        None => manhattan,
    })
}

/// Radius around the brain's site where a picker may choose (Chebyshev):
/// a site is one spawn, the resource around it is the working area.
pub const SITE_RADIUS: i32 = 20;

/// True when the tile sits in the run's assigned site (always true
/// without one: manual runs keep choosing freely).
pub fn in_assigned_site(bot: &Bot, x: i32, z: i32) -> bool {
    bot.assigned_site.is_none_or(|(site_x, site_z, _)| (x - site_x).abs().max((z - site_z).abs()) <= SITE_RADIUS)
}

/// One F2P verdict per candidate spot, in order, in a single service
/// batch. `None` when the service is down (logged): callers drop every
/// candidate instead of guessing — the pick returns `None` and the loop
/// waits, while any real walk still fails loud with `nav-down`.
pub(crate) fn f2p_inside(bot: &mut Bot, spots: &[(i32, i32)]) -> Option<Vec<bool>> {
    if spots.is_empty() {
        return Some(Vec::new());
    }
    let level = bot.world.level;
    let tiles: Vec<(i32, i32, i32)> = spots.iter().map(|(spot_x, spot_z)| (*spot_x, *spot_z, level)).collect();
    match bot.in_f2p_batch(&tiles) {
        Ok(verdicts) => Some(verdicts),
        Err(error) => {
            bot.log(&format!("f2p batch unavailable ({error}), dropping {} spots", spots.len()));
            None
        }
    }
}

/// World ticks a "can't reach" goblin tile stays banned before retry.
pub const BLOCKED_BAN_TICKS: u64 = 200;

/// Ban for a foe behind a door the server proved uncrossable (door opens,
/// the step through is refused): the tile stays out for the whole fight.
/// A spot that fails in a loop is worse than a spot skipped: each new
/// indoor goblin burns one approach, never a campaign.
pub const DOOR_BAN_TICKS: u64 = u64::MAX;

// -- melee target acquisition lives in `kit::foes` (split by role, rule 4) --
pub use crate::kit::foes::{attack_nearest, is_shop_staff, rank_foes, ranked_clusters};

// -- rock pool (mining): tiers, spawns, liveness --

pub(crate) fn usable_rock_tiers() -> Vec<knowledge::rocks::RockTier> {
    knowledge::rocks::all_rock_tiers()
        .into_iter()
        .filter(|tier| !tier.product.is_empty() && tier.product_members == Some(false) && tier.product != "blankrune")
        .collect()
}

pub(crate) fn rock_tier_price(tier: &knowledge::rocks::RockTier) -> i64 {
    obj_price(&tier.product)
}

pub(crate) fn pick_rock_tier(tiers: &[knowledge::rocks::RockTier], level: i64) -> Option<knowledge::rocks::RockTier> {
    let mut best: Option<&knowledge::rocks::RockTier> = None;
    for tier in tiers {
        if tier.level <= level && best.as_ref().is_none_or(|known| (tier.level, rock_tier_price(tier)) > (known.level, rock_tier_price(known))) {
            best = Some(tier);
        }
    }
    best.cloned()
}

/// Rock spawns for the tier's loc names within range of the bank.
fn rock_tier_spawns(tier: &knowledge::rocks::RockTier, bank: &knowledge::banks::Bank) -> Vec<(i32, i32)> {
    let mut found = Vec::new();
    for entry in knowledge::all_locs() {
        let name = entry.name.clone().unwrap_or_default().to_lowercase();
        if tier.rocks.iter().any(|rock| *rock == name) {
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

pub(crate) fn pick_rock_spot(
    bot: &mut Bot,
    tiers: &[knowledge::rocks::RockTier],
    bank: &knowledge::banks::Bank,
    level: i64,
    banned: &BanSet,
    now: u64,
) -> Option<(knowledge::rocks::RockTier, i32, i32)> {
    let mut affordable: Vec<&knowledge::rocks::RockTier> =
        tiers.iter().filter(|tier| tier.level <= level).collect();
    affordable.sort_by_key(|tier| (-tier.level, -rock_tier_price(tier)));
    let threats = crate::kit::danger::threats(bot);
    for tier in affordable {
        let mut cands: Vec<(i64, (i32, i32))> = Vec::new();
        for (spot_x, spot_z) in rock_tier_spawns(tier, bank) {
            if banned.is_banned((spot_x, spot_z), now) || !in_assigned_site(bot, spot_x, spot_z) {
                continue;
            }
            if crate::kit::danger::covering(&threats, spot_x, spot_z).is_some() {
                continue;
            }
            let dist = i64::from((spot_x - bot.world.x).abs() + (spot_z - bot.world.z).abs());
            cands.push((dist, (spot_x, spot_z)));
        }
        // Occupation perçue: dodge rocks another visible player works
        // while free ones exist; a full house keeps the pool.
        let cands = drop_occupied(bot, cands, OCCUPIED_RADIUS);
        if let Some((_, (spot_x, spot_z))) = cands.into_iter().min_by_key(|(dist, _)| *dist) {
            return Some((tier.clone(), spot_x, spot_z));
        }
    }
    None
}

/// Live loc id at a tile (live stream first, static template as fallback).
/// The worker paints nothing: the fallback is one `tiles_flags` batch to
/// the nav service, which reads its snapshot. Live still wins (a depleted
/// rock streams its change), so `rock_still_there` keeps deciding below.
/// A down service stays loud (`nav-down` via `ScriptError::Sdk`); only a
/// tile the service has no template for keeps the `Usage` error.
pub(crate) fn rock_live_loc_id(bot: &mut Bot, rock_x: i32, rock_z: i32) -> Result<u16, ScriptError> {
    if let Some((loc, _, _)) = bot.world.loc_at(rock_x, rock_z, bot.world.level) {
        return Ok(loc);
    }
    let level = bot.world.level;
    let rows = bot.tiles_flags(&[(rock_x, rock_z, level)]).map_err(ScriptError::Sdk)?;
    if let Some((loc, _, _)) = rows.into_iter().next().and_then(|row| row.loc) {
        return Ok(loc);
    }
    Err(ScriptError::Failed(format!("no loc at {rock_x},{rock_z} from {:?}", bot.position())))
}

/// True unless the server confirmed a depleted rock here (unknown = assume
/// standing, never assume depletion).
pub(crate) fn rock_still_there(bot: &mut Bot, spot_x: i32, spot_z: i32, tier: &knowledge::rocks::RockTier) -> bool {
    let Ok(loc) = rock_live_loc_id(bot, spot_x, spot_z) else {
        return true;
    };
    match knowledge::loc_by_id(usize::from(loc)) {
        Some(entry) => {
            let debug = entry.name.unwrap_or_default().to_lowercase();
            tier.rocks.iter().any(|rock| *rock == debug)
        }
        None => true,
    }
}

/// Shared rank: best pickaxe first, level gates pass through `usable`
/// (`ToolSpec` decides, the tables carry no params). One const, reused by
/// every caller (never rebuilt per call).

// -- fish pool (fishing): tiers, live/stale spots, pinned bank --

/// Live NPC id whose debugname ends with the tier category tail.
pub(crate) fn tier_npc_matches(bot: &Bot, npc_type: u16, category: &str) -> bool {
    let tail = format!("_{category}");
    (bot.tables.npc_name)(npc_type).unwrap_or_default().ends_with(&tail)
}

/// Nearest live spot NPC for a tier, relative to a reference tile.
pub(crate) fn nearest_live(bot: &Bot, category: &str, near_x: i32, near_z: i32) -> Option<(u16, i32, i32)> {
    let mut best: Option<(i64, (u16, i32, i32))> = None;
    for (slot, npc) in &bot.world.npcs {
        if !tier_npc_matches(bot, npc.npc_type, category) {
            continue;
        }
        if (npc.x - bot.world.x).abs() + (npc.z - bot.world.z).abs() > LIVE_RADIUS {
            continue;
        }
        let dist = i64::from((npc.x - near_x).abs() + (npc.z - near_z).abs());
        if best.as_ref().is_none_or(|(known, _)| dist < *known) {
            best = Some((dist, (*slot, npc.x, npc.z)));
        }
    }
    best.map(|(_, found)| found)
}

/// Static spawns of one fish category plus every live matching spot
/// NPC the bot sees (level carried along for the same-level filter).
fn fish_candidates(bot: &Bot, category: &str) -> Vec<(i32, i32, i32)> {
    let mut cands: Vec<(i32, i32, i32)> = Vec::new();
    for npc_id in knowledge::fishing::spot_npc_ids(category) {
        if let Some(entry) = knowledge::npc_by_id(npc_id) {
            for spawn in entry.spawns {
                cands.push((spawn.level as i32, spawn.x as i32, spawn.z as i32));
            }
        }
    }
    for (_, npc) in &bot.world.npcs {
        if tier_npc_matches(bot, npc.npc_type, category) {
            cands.push((npc.level, npc.x, npc.z));
        }
    }
    cands
}

/// Near-bank pool for one tier: the woodcut shape (local filters, one
/// F2P batch, occupation dodge inside the surviving pool, walk-cost
/// refinement). Occupation applies AFTER the pin — dodging the global
/// pool first discards the only locally-viable spawns whenever a
/// stranger stands near home water.
pub(crate) fn pick_near_fish_spot(
    bot: &mut Bot,
    tier: &knowledge::fishing::FishTier,
    fence: Option<i32>,
    banned: &BanSet,
    now: u64,
    pinned: &knowledge::banks::Bank,
) -> Option<(i32, i32)> {
    // Aggressive NPCs that would attack this bot (spot or bank walk).
    let threats = crate::kit::danger::threats(bot);
    let mut spots: Vec<(i32, i32)> = Vec::new();
    for (spot_level, spot_x, spot_z) in fish_candidates(bot, tier.category) {
        if spot_level != bot.world.level || banned.is_banned((spot_x, spot_z), now) || !in_assigned_site(bot, spot_x, spot_z) {
            continue;
        }
        if crate::kit::bans::across_toll(bot.world.x, spot_x, fence) {
            continue;
        }
        let bank = match nearest_bank_at(spot_x, spot_z, spot_level) {
            Some(bank) => bank,
            None => continue,
        };
        if (i64::from(bank.x) - pinned.x).abs() + (i64::from(bank.z) - pinned.z).abs() > 12 {
            continue;
        }
        let dist = (i64::from(spot_x) - bank.x).abs() + (i64::from(spot_z) - bank.z).abs();
        if dist > BANK_TREK_LIMIT {
            continue;
        }
        if (spot_x - bot.world.x).abs() + (spot_z - bot.world.z).abs() > 400 {
            continue;
        }
        // The spot itself must sit outside every aggro square; the walk
        // there goes around them (danger-aware routes).
        if crate::kit::danger::covering(&threats, spot_x, spot_z).is_some() {
            continue;
        }
        spots.push((spot_x, spot_z));
    }
    // Travel always plans F2P-only: an out-of-F2P spot is a guaranteed
    // NoRoute dossier, never a pick.
    let Some(inside) = f2p_inside(bot, &spots) else {
        return None;
    };
    let mut cands: Vec<(i64, (i32, i32))> = Vec::new();
    for ((spot_x, spot_z), live) in spots.into_iter().zip(inside) {
        if !live {
            continue;
        }
        let dist = i64::from((spot_x - bot.world.x).abs() + (spot_z - bot.world.z).abs());
        cands.push((dist, (spot_x, spot_z)));
    }
    if cands.is_empty() {
        return None;
    }
    let cands = drop_occupied(bot, cands, OCCUPIED_RADIUS);
    refine_by_walk(bot, cands).map(|(_, pos)| pos)
}

/// Same-tier hop for one tier: every static spawn of the category, any
/// bank, when nothing qualifies near the pinned bank (woodcut
/// `nearest_hop_spot` parity). Without it a bot pinned at a waterless
/// bank — Varrock — waits at the bank forever.
pub(crate) fn pick_hop_fish_spot(
    bot: &mut Bot,
    tier: &knowledge::fishing::FishTier,
    fence: Option<i32>,
    banned: &BanSet,
    now: u64,
) -> Option<(i32, i32)> {
    // Aggressive NPCs that would attack this bot (spot or bank walk).
    let threats = crate::kit::danger::threats(bot);
    let mut spots: Vec<(i32, i32)> = Vec::new();
    for (spot_level, spot_x, spot_z) in fish_candidates(bot, tier.category) {
        if spot_level != bot.world.level || banned.is_banned((spot_x, spot_z), now) || !in_assigned_site(bot, spot_x, spot_z) {
            continue;
        }
        if crate::kit::bans::across_toll(bot.world.x, spot_x, fence) {
            continue;
        }
        let bank = match nearest_bank_at(spot_x, spot_z, spot_level) {
            Some(bank) => bank,
            None => continue,
        };
        let trek = (i64::from(spot_x) - bank.x).abs() + (i64::from(spot_z) - bank.z).abs();
        if trek > BANK_TREK_LIMIT {
            continue;
        }
        if (spot_x - bot.world.x).abs() + (spot_z - bot.world.z).abs() > 400 {
            continue;
        }
        // The spot itself must sit outside every aggro square; the walk
        // there goes around them (danger-aware routes).
        if crate::kit::danger::covering(&threats, spot_x, spot_z).is_some() {
            continue;
        }
        spots.push((spot_x, spot_z));
    }
    let Some(inside) = f2p_inside(bot, &spots) else {
        return None;
    };
    let mut cands: Vec<(i64, (i32, i32))> = Vec::new();
    for ((spot_x, spot_z), live) in spots.into_iter().zip(inside) {
        if !live {
            continue;
        }
        let dist = i64::from((spot_x - bot.world.x).abs() + (spot_z - bot.world.z).abs());
        cands.push((dist, (spot_x, spot_z)));
    }
    if cands.is_empty() {
        return None;
    }
    let cands = drop_occupied(bot, cands, OCCUPIED_RADIUS);
    refine_by_walk(bot, cands).map(|(_, pos)| pos)
}

/// Best affordable tier with a live or static spot: near the pinned
/// bank first, then a same-tier hop to real water (never an idle wait
/// at a waterless bank).
pub(crate) fn pick_fish_spot(
    bot: &mut Bot,
    level: i64,
    banned: &BanSet,
    now: u64,
    pinned: &knowledge::banks::Bank,
) -> Option<(knowledge::fishing::FishTier, i32, i32, knowledge::banks::Bank)> {
    // Toll-fence aware: banks on the far side are last resort, never the
    // default — same discipline as `nearest_bank` and the tree picker.
    let fence = if bot.allow_paid { None } else { crate::kit::bank::toll_gate_x(&mut *bot) };
    let mut tiers = knowledge::fishing::all_fish_tiers();
    tiers.sort_by_key(|tier| -tier.level);
    for tier in tiers.into_iter().filter(|tier| tier.level <= level) {
        if let Some((spot_x, spot_z)) = pick_near_fish_spot(bot, &tier, fence, banned, now, pinned) {
            let bank = nearest_bank_at(spot_x, spot_z, bot.world.level).expect("bank just found");
            return Some((tier, spot_x, spot_z, bank));
        }
        if let Some((spot_x, spot_z)) = pick_hop_fish_spot(bot, &tier, fence, banned, now) {
            bot.log(&format!(
                "no {} near bank ({},{}), hopping same tier to ({spot_x},{spot_z})",
                tier.products.first().unwrap_or(&"?"),
                pinned.x,
                pinned.z
            ));
            let bank = nearest_bank_at(spot_x, spot_z, bot.world.level).expect("bank just found");
            return Some((tier, spot_x, spot_z, bank));
        }
    }
    None
}

pub(crate) fn nearest_bank_at(x: i32, z: i32, level: i32) -> Option<knowledge::banks::Bank> {
    knowledge::banks::all_banks()
        .into_iter()
        .filter(|bank| !bank.members && bank.level == i64::from(level))
        .min_by_key(|bank| (bank.x - i64::from(x)).abs() + (bank.z - i64::from(z)).abs())
}

    /// Mapless worker against a real nav service (repo dumps): no painted
    /// world, only the plan handle — the same shape as a live worker.

/// Ban for a failed walk to a stand. Planner-proven `NoRoute`
/// (`Error::Travel`, disconnected from here) bans a short re-probe
/// delay (UNREACHABLE_BAN_TICKS): re-planning the same walk from the
/// same tile is provably futile, so the spot sits out instead of
/// burning a full plan on every repick. Every other walk
/// failure (step refused mid-leg, leg timeout, door mid-swing) rides the
/// short ban (250t) and is re-tested at expiry.
/// Operable direct refusals on a standing tree before it is condemned: the
/// first refusal rides the short ban (a transient storm or a gate mid-swing
/// refuses once), the repeat proves permanence (wall/fence). A walk-leg
/// failure never counts as a strike, however close the walk died. Geometry
/// is footprint-orthogonal (`operable_stand`), never Chebyshev: a refused
/// diagonal-to-anchor corner is a bad stand, not a walled tree.
pub(crate) const DIRECT_REFUSALS_TO_CONDEMN: u8 = 2;

pub(crate) fn walk_fail_ban(planner_proved: bool) -> u64 {
    if planner_proved {
        UNREACHABLE_BAN_TICKS
    } else {
        GONE_BAN_TICKS
    }
}

/// Ban for a failed tree stand. `direct` is true only when the server
/// refused the chop click itself ("can't reach that") from a
/// footprint-operable stand; walk-leg failures (never arrived, leg timeout,
/// step refused far away, inoperable stand) pass false and can never return
/// the structural ban (STRUCTURAL_BAN_TICKS: ~7 min at dev tick, ~72 min
/// official — a proven wall, not a session exile). Structural needs a
/// repeated operable direct
/// refusal of a standing tree; everything else is transient (a crowd, a
/// gate mid-swing, a walk that fell short) and rides the short ban (Gone,
/// 250t) so the spot is re-tested at expiry instead of sitting out half the
/// session. A stump is depletion (Gone).
pub(crate) fn refusal_ban_ticks(operable: bool, stump: bool, direct: bool, prior_strikes: u8) -> u64 {
    if stump {
        return GONE_BAN_TICKS;
    }
    if direct && operable && prior_strikes + 1 >= DIRECT_REFUSALS_TO_CONDEMN {
        return STRUCTURAL_BAN_TICKS;
    }
    GONE_BAN_TICKS
}

#[cfg(test)]
mod tests {
    use super::{drop_occupied, refusal_ban_ticks, rock_live_loc_id, visible_workers, walk_fail_ban, OCCUPIED_RADIUS};
    use crate::kit::{bans::{GONE_BAN_TICKS, UNREACHABLE_BAN_TICKS}, observe::STRUCTURAL_BAN_TICKS};
    use crate::tables_live::attach_live_tables;
    use crate::ScriptError;
    use lifescape_gamedata as knowledge;
    use lifescape_sdk::Bot;

    /// Occupation is own vision only: a stranger on the spot dodges it,
    /// nobody around changes nothing, and a full house keeps the pool
    /// (never idle while a resource stands).
    #[test]
    fn occupation_dodges_visible_workers_only() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 0;
        bot.world.z = 0;
        let cands = vec![(10i64, (100, 100)), (20, (200, 200)), (30, (300, 300))];
        assert_eq!(drop_occupied(&mut bot, cands.clone(), OCCUPIED_RADIUS).len(), 3, "nobody around changes nothing");
        assert!(visible_workers(&bot).is_empty(), "nobody seen");
        bot.world.players.insert(7u16, lifescape_sdk::world::PlayerInfo { x: 101, z: 100, level: 0, jump: false, name: None });
        let free = drop_occupied(&mut bot, cands.clone(), OCCUPIED_RADIUS);
        assert_eq!(free, vec![(20i64, (200, 200)), (30, (300, 300))], "worked spot dodged, got: {free:?}");
        bot.world.players.insert(8u16, lifescape_sdk::world::PlayerInfo { x: 200, z: 200, level: 1, jump: false, name: None });
        let still = drop_occupied(&mut bot, cands.clone(), OCCUPIED_RADIUS);
        assert_eq!(still.len(), 2, "other-level players don't count, got: {still:?}");
        bot.world.players.insert(9u16, lifescape_sdk::world::PlayerInfo { x: 201, z: 199, level: 0, jump: false, name: None });
        bot.world.players.insert(10u16, lifescape_sdk::world::PlayerInfo { x: 300, z: 301, level: 0, jump: false, name: None });
        assert_eq!(drop_occupied(&mut bot, cands.clone(), OCCUPIED_RADIUS), cands, "full house keeps the pool");
        assert_eq!(drop_occupied(&mut bot, vec![(5i64, (1, 1))], OCCUPIED_RADIUS).len(), 1, "single candidate never dropped");
    }

    /// Mapless worker against a real nav service (repo dumps): no painted
    /// world, only the plan handle — the same shape as a live worker.
    fn served_spot_bot(socket_tag: &str, x: i32, z: i32) -> (Bot, std::sync::Arc<lifescape_nav::service::Service>) {
        let (mut bot, service) = served_mining_bot(socket_tag);
        bot.world.x = x;
        bot.world.z = z;
        (bot, service)
    }

    /// Varrock-banked bots must hop to real water instead of waiting at
    /// the bank forever: no saltfish water sits near a Varrock bank, so
    /// the near-bank pool is empty and only a same-tier hop (woodcut
    /// parity) saves the pick.
    #[test]
    fn varrock_bank_hops_to_water() {
        let (mut bot, _service) = served_spot_bot("fish-hop-varrock", 3185, 3436);
        let banned = crate::kit::bans::BanSet::new();
        let pinned = super::nearest_bank_at(3185, 3436, 0).expect("a Varrock bank");
        let found = super::pick_fish_spot(&mut bot, 1, &banned, 0, &pinned);
        let (tier, spot_x, spot_z, bank) = found.expect("hop to water from a Varrock bank");
        assert_eq!(tier.category, "saltfish");
        // Really at water: the hop leaves the pinned-bank neighbourhood.
        assert!(
            (i64::from(spot_x) - pinned.x).abs() + (i64::from(spot_z) - pinned.z).abs() > 12,
            "hopped away from the bank, got ({spot_x},{spot_z})"
        );
        // Sane trek from its own bank, and F2P like every tree hop.
        assert!(
            (i64::from(spot_x) - bank.x).abs() + (i64::from(spot_z) - bank.z).abs() <= super::BANK_TREK_LIMIT,
            "trek sane from ({},{}) to ({spot_x},{spot_z})",
            bank.x,
            bank.z
        );
        assert_eq!(bot.in_f2p_batch(&[(spot_x, spot_z, 0)]).expect("f2p answers"), vec![true], "hop stays F2P");
    }

    /// Crowded home water still fishes: occupation dodges inside the
    /// near-bank pool, never the global one — a stranger on the Draynor
    /// spawns must not push the pick out to pin-killed far water.
    #[test]
    fn crowded_home_water_still_picks() {
        let (mut bot, _service) = served_spot_bot("fish-crowd-draynor", 3092, 3242);
        // Occupation only: the danger map (Draynor wizards) is off here,
        // `fresh_bot_never_picks_draynor` covers it.
        bot.allow_wild = true;
        bot.world.players.insert(
            7u16,
            lifescape_sdk::world::PlayerInfo { x: 3085, z: 3230, level: 0, jump: false, name: None },
        );
        let banned = crate::kit::bans::BanSet::new();
        let pinned = super::nearest_bank_at(3092, 3242, 0).expect("a Draynor bank");
        let found = super::pick_fish_spot(&mut bot, 1, &banned, 0, &pinned);
        let (_, spot_x, spot_z, _) = found.expect("crowded home water still picks");
        assert!(
            (spot_x - 3092).abs() + (spot_z - 3242).abs() <= 60,
            "stays near home water, got ({spot_x},{spot_z})"
        );
    }

    /// The north Draynor spot sits in the young dark wizards' aggro square
    /// (bots died there ~100 times): a fresh bot only picks water outside
    /// every square.
    #[test]
    fn fresh_bot_never_picks_covered_water() {
        let (mut bot, _service) = served_spot_bot("fish-danger-draynor", 3092, 3242);
        let banned = crate::kit::bans::BanSet::new();
        let pinned = super::nearest_bank_at(3092, 3242, 0).expect("a Draynor bank");
        let (_, spot_x, spot_z, _) = super::pick_fish_spot(&mut bot, 1, &banned, 0, &pinned).expect("safe water exists");
        let threats = crate::kit::danger::threats(&bot);
        assert!(crate::kit::danger::covering(&threats, spot_x, spot_z).is_none(), "picked a covered spot ({spot_x},{spot_z})");
    }
    /// Mapless worker against a real nav service (repo dumps): no painted
    /// world, only the plan handle — the same shape as a live worker.
    fn served_mining_bot(socket_tag: &str) -> (Bot, std::sync::Arc<lifescape_nav::service::Service>) {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        crate::tables_live::attach_live_tables(&mut bot);
        bot.world.level = 0;
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
        let version = format!("test-mine-{socket_tag}");
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

    /// First dump rock spawn the service has a template for, with its loc:
    /// the fallback target, found in one batch.
    fn templated_rock_spawn(bot: &mut Bot) -> (i32, i32, u16) {
        let mut spawns: Vec<(i32, i32)> = Vec::new();
        for tier in super::usable_rock_tiers() {
            for entry in knowledge::all_locs() {
                let name = entry.name.clone().unwrap_or_default().to_lowercase();
                if tier.rocks.iter().any(|rock| *rock == name) {
                    for spawn in entry.spawns {
                        if spawn.level == 0 {
                            spawns.push((spawn.x as i32, spawn.z as i32));
                        }
                    }
                }
            }
        }
        spawns.sort();
        spawns.dedup();
        let tiles: Vec<(i32, i32, i32)> = spawns.iter().take(64).map(|(x, z)| (*x, *z, 0)).collect();
        let rows = bot.tiles_flags(&tiles).expect("flags answer");
        spawns
            .into_iter()
            .take(64)
            .zip(rows)
            .find_map(|((x, z), row)| row.loc.map(|(loc, _, _)| (x, z, loc)))
            .expect("a templated rock spawn")
    }

    /// Unstreamed rock tile: the service template answers the loc instead
    /// of ending the run with `Usage` (same hole as the woodcut twin).
    /// Live still wins once streamed.
    #[test]
    fn rock_live_loc_id_falls_back_to_the_service_template() {
        let (mut bot, _service) = served_mining_bot("mine-loc-fallback");
        let (rock_x, rock_z, template) = templated_rock_spawn(&mut bot);
        bot.world.locs.remove(&(rock_x, rock_z, 0));
        assert!(bot.world.loc_at(rock_x, rock_z, 0).is_none(), "nothing streamed yet");
        assert_eq!(rock_live_loc_id(&mut bot, rock_x, rock_z).expect("static template answers"), template);
        // Live wins once streamed.
        bot.world.locs.insert((rock_x, rock_z, 0), Some((template.wrapping_add(1), 0, 0)));
        assert_eq!(
            rock_live_loc_id(&mut bot, rock_x, rock_z).expect("live answers"),
            template.wrapping_add(1),
            "live beats the template"
        );
    }

    /// A tile the service has no template for keeps the `Usage` error
    /// (a spot with genuinely no rock), never a guessed id.
    #[test]
    fn rock_live_loc_id_without_template_is_runtime_failure() {
        let (mut bot, _service) = served_mining_bot("mine-loc-empty");
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
        let err = rock_live_loc_id(&mut bot, empty.0, empty.1).expect_err("no template is a runtime failure");
        assert!(matches!(err, ScriptError::Failed(_)), "Failed, got: {err:?}");
    }

    /// Walk-failure bans: planner-proven NoRoute sits out 30k (re-planning
    /// from the same tile is futile), every transient walk failure rides
    /// the short ban and is re-tested at expiry.
    #[test]
    fn walk_fail_ban_is_proven_or_short() {
        assert_eq!(walk_fail_ban(true), UNREACHABLE_BAN_TICKS, "planner proof bans long");
        assert_eq!(walk_fail_ban(false), GONE_BAN_TICKS, "transient walk retries short");
    }

    /// Strike rule: only a refusal from a footprint-operable stand counts
    /// toward condemning (a corner refusal is a bad stand, retried short).
    #[test]
    fn direct_refusal_needs_a_repeat_to_condemn() {
        assert_eq!(refusal_ban_ticks(false, false, false, 5), GONE_BAN_TICKS, "far walk retries short");
        assert_eq!(refusal_ban_ticks(true, false, false, 5), GONE_BAN_TICKS, "walk failure retries short");
        assert_eq!(refusal_ban_ticks(true, false, true, 0), GONE_BAN_TICKS, "first refusal retries short");
        assert_eq!(refusal_ban_ticks(true, false, true, 1), STRUCTURAL_BAN_TICKS, "repeat condemns");
        assert_eq!(
            refusal_ban_ticks(false, false, true, 1),
            GONE_BAN_TICKS,
            "afar click is a short walk, not a walled stand"
        );
        assert_eq!(
            refusal_ban_ticks(true, true, true, 1),
            GONE_BAN_TICKS,
            "stump is depletion even when adjacent"
        );
    }
}

