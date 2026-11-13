//! Melee training ladder: which F2P spot a fighter trains at, from its
//! levels, worn bonuses and hitpoints (`melee_math`), within a safety
//! rule, spread across spots and clusters by a stable seed. Pure: the
//! script feeds live state in and walks to what comes out.
//!
//! Rules (user decisions 2026-11-13): every F2P melee spot is allowed;
//! food is carried everywhere except cows and chickens. A spot is
//! workable when:
//! - its gate holds: `Toll` needs 10 coins (or the bot already on the
//!   Al Kharid flank), `BrassKey` the Edgeville dungeon key (pack or
//!   bank); otherwise the ladder takes the next spot, never a block;
//! - two NPC max hits from the eat line cannot kill (`burst`);
//! - the food load (or, foodless, the hitpoints above the eat line)
//!   covers `MIN_KILLS_PER_LOAD` kills;
//! - one cluster at least sits outside every other hunter that would
//!   attack this combat level, its bank too (`danger`, the target's own
//!   NPC excluded: we came to fight it).
//!
//! Value = xp per tick (discounted by the bank trips the food burn
//! forces) + `COIN_XP_WEIGHT` x coins per tick, so coin droppers win
//! when the xp is comparable. Spots within `BAND` of the best are
//! equivalent: the seed picks one weighted by value x spawns, then one
//! of its clusters weighted by spawns (no shared state, D22).

#![forbid(unsafe_code)]

use crate::melee_math::{estimate, Estimate, Fighter};

/// Eat line of the fight loop (mirrors `kit::survival::EAT_FRACTION`).
pub const EAT_FRACTION: f64 = 0.55;
/// Hits from the eat line the bot must survive (the loop checks hp
/// every couple of ticks; an NPC swings every 4 to 6).
pub const BURST_HITS: i64 = 2;
/// Kills one load must cover, food plus hitpoints above the eat line.
pub const MIN_KILLS_PER_LOAD: f64 = 5.0;
/// Ticks of fighting one food load is sized for.
pub const TRIP_TICKS: f64 = 3000.0;
/// Food carried on top of the sized burn (bad luck, the walk out).
pub const FOOD_RESERVE: u32 = 2;
/// Food bounds per load (pack room stays for loot and gear).
pub const MIN_FOOD: u32 = 3;
pub const MAX_FOOD: u32 = 20;
/// Ticks one bank round trip costs (walk out, bank, walk back).
pub const BANK_TRIP_TICKS: f64 = 400.0;
/// Xp-equivalent of one coin: a tie-break, not a goal.
pub const COIN_XP_WEIGHT: f64 = 0.5;
/// Spots worth at least this share of the best are equivalent: the
/// fleet spreads over them by capacity (crowding is what kills the
/// cow-field rate, not the foe).
pub const BAND: f64 = 0.8;
/// A foodless bot asks for food only when the band beats the best
/// exempt rung (cows, chickens) by this factor.
pub const FOOD_ERRAND_GAIN: f64 = 1.3;
/// Coins the Al Kharid toll gate takes.
pub const TOLL_COINS: u32 = 10;
/// Heal assumed when the bot owns no food yet (shrimps, cooked meat).
pub const DEFAULT_HEAL: i64 = 3;
/// Food the brain obtains when the bot owns none (`Blocked`).
pub const FOOD_ITEM: &str = "shrimp";
/// Brass key debugname (`item:983`, the Edgeville dungeon door).
pub const BRASS_KEY: &str = "edgevilledungeonkey";

/// What a spot needs before the ladder considers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Open,
    /// Across the Al Kharid toll fence.
    Toll,
    /// Behind the Edgeville dungeon brass-key door.
    BrassKey,
}

/// One rung: a `combat_spots` key, the NPC whose stats rate it, the
/// food exemption, the gate and an optional area (x0, z0, x1, z1) that
/// keeps only the reachable clusters of an NPC spawned elsewhere too.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rung {
    pub key: &'static str,
    pub npc: &'static str,
    pub food_exempt: bool,
    pub gate: Gate,
    pub area: Option<(i64, i64, i64, i64)>,
    /// Hitpoints the NPC heals back per tick (monks: 2 hp on one swing
    /// in four, one swing per 4 ticks).
    pub self_heal_per_tick: f64,
}

const fn rung(key: &'static str, npc: &'static str, food_exempt: bool, gate: Gate, area: Option<(i64, i64, i64, i64)>) -> Rung {
    Rung { key, npc, food_exempt, gate, area, self_heal_per_tick: 0.0 }
}

/// Every F2P melee rung the fight loop can hunt (surface, plus the
/// Edgeville dungeon giants the router reaches with the brass key).
/// Left out: dark wizards (magic attackers, not a melee exchange), rats
/// (mixed 2-10 hp NPCs), imps (teleport away mid-fight, max hit 0), bears/ice/wilderness (never safe), moss giants
/// (Varrock sewers: no route offline, BudgetExhausted at the web), and
/// Crandor or members spawns (area filters).
pub const RUNGS: [Rung; 11] = [
    rung("chickens", "chicken", true, Gate::Open, None),
    rung("cows", "cow", true, Gate::Open, None),
    rung("goblins", "goblin", false, Gate::Open, None),
    rung("men", "man", false, Gate::Open, None),
    Rung { self_heal_per_tick: 0.125, ..rung("monks", "monk", false, Gate::Open, None) },
    rung("barbarians", "barbarian", false, Gate::Open, None),
    rung("al_kharid_warriors", "al_kharid_warrior", false, Gate::Toll, None),
    // Al Kharid mine only (Karamja needs the boat).
    rung("scorpions", "scorpion", false, Gate::Toll, Some((3270, 3270, 3320, 3320))),
    rung("guards", "guard1", false, Gate::Open, None),
    // Rimmington peninsula only (Crandor needs Dragon Slayer, the
    // dungeon camp the brass key).
    rung("hobgoblins", "hobgoblin_unarmed", false, Gate::Open, Some((2895, 3270, 2920, 3300))),
    rung("hill_giants", "giant", false, Gate::BrassKey, Some((3090, 9815, 3130, 9860))),
];

/// Rung by spot key.
pub fn rung_by_key(key: &str) -> Option<Rung> {
    RUNGS.iter().find(|entry| entry.key == key).copied()
}

/// What the bot owns that the gates and the food rule read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Holdings {
    /// Edible units, pack plus bank.
    pub food_count: u32,
    /// Heal of the best food owned (0: none).
    pub best_heal: i64,
    /// Coins in the pack.
    pub coins: u32,
    pub brass_key: bool,
    /// Already east of the toll fence (Al Kharid side).
    pub east_of_toll: bool,
}

/// One rated rung with its verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct Rated {
    pub rung: Rung,
    pub estimate: Estimate,
    /// Food units one load carries (0 for the exempt spots).
    pub food: u32,
    pub coins_per_kill: f64,
    pub value: f64,
    /// Why the rung is out (`None`: workable).
    pub refused: Option<String>,
    /// Workable clusters (x, z, level, spawns), stable order.
    pub clusters: Vec<(i64, i64, i64, usize)>,
}

/// The ladder's decision.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    /// Hunt this rung, clusters in visiting order (seeded first), with
    /// this many food units per load (`None`: exempt, carry as usual).
    Hunt { rung: Rung, clusters: Vec<(i64, i64, i64, usize)>, food: Option<u32> },
    /// The best rung needs food and the bot owns none: obtain this many
    /// of `FOOD_ITEM` first.
    NeedFood { rung: Rung, count: u32 },
}

/// Expected coins per kill from the drop tables.
pub fn coins_per_kill(npc: &str) -> f64 {
    crate::drops::all_drops().iter().filter(|row| row.npc == npc && row.obj == "coins").map(|row| row.expected_per_kill()).sum()
}

/// Food units one load needs for this exchange: the burn over
/// `TRIP_TICKS` plus the reserve, bounded.
pub fn food_for(estimate: &Estimate, heal: i64) -> u32 {
    let heal = heal.max(1) as f64;
    let burn = (estimate.loss_per_tick.max(0.0) * TRIP_TICKS / heal).ceil() as u32;
    (burn + FOOD_RESERVE).clamp(MIN_FOOD, MAX_FOOD)
}

/// Underground tiles sit 6400 above their surface.
const UNDERGROUND_OFFSET: i64 = 6400;

/// Wilderness, surface (z 3520 up to the underground band) or the
/// dungeons under it (z 9920 up). `danger::in_wilderness` reads every
/// z from 3520 up as wild, the Edgeville dungeon included.
pub fn in_wilderness_any(x: i64, z: i64, level: i64) -> bool {
    let wild_x = level == 0 && (2944..=3392).contains(&x);
    wild_x && ((3520..UNDERGROUND_OFFSET).contains(&z) || z >= 3520 + UNDERGROUND_OFFSET)
}

/// F2P, non-wilderness spawn clusters of a rung on level 0 (15-tile
/// groups, like the fight loop's), two spawns at least, sorted.
pub fn rung_clusters(rung: &Rung) -> Vec<(i64, i64, i64, usize)> {
    let inside = |x: i64, z: i64| rung.area.is_none_or(|(x0, z0, x1, z1)| (x0..=x1).contains(&x) && (z0..=z1).contains(&z));
    let mut groups: Vec<Vec<(i64, i64)>> = Vec::new();
    for (x, z, level) in crate::combat_spots::positions(rung.key) {
        if level != 0 || in_wilderness_any(x, z, level) || !inside(x, z) {
            continue;
        }
        let center = |group: &Vec<(i64, i64)>| {
            let count = group.len() as i64;
            (group.iter().map(|tile| tile.0).sum::<i64>() / count, group.iter().map(|tile| tile.1).sum::<i64>() / count)
        };
        match groups.iter_mut().find(|group| {
            let (center_x, center_z) = center(group);
            (x - center_x).abs().max((z - center_z).abs()) <= 15
        }) {
            Some(group) => group.push((x, z)),
            None => groups.push(vec![(x, z)]),
        }
    }
    let mut out: Vec<(i64, i64, i64, usize)> = groups
        .into_iter()
        .filter(|group| group.len() >= 2)
        .map(|group| {
            let count = group.len() as i64;
            (group.iter().map(|tile| tile.0).sum::<i64>() / count, group.iter().map(|tile| tile.1).sum::<i64>() / count, 0, group.len())
        })
        .collect();
    out.sort();
    out
}

/// Combat level a cluster needs: no other hunter (the rung's own NPCs
/// excluded) covers it or its bank. Underground clusters bank from
/// their surface entrance.
pub fn cluster_requirement(rung: &Rung, cluster: (i64, i64, i64)) -> u8 {
    let names: &[&str] = crate::combat_spots::by_key(rung.key).map(|spot| spot.npc_names).unwrap_or(&[]);
    let required_at = |x: i64, z: i64, level: i64| {
        crate::danger::hunters()
            .iter()
            .filter(|hunter| !names.contains(&hunter.name))
            .filter(|hunter| hunter.level == level && (x - hunter.x).abs().max((z - hunter.z).abs()) <= hunter.radius)
            .map(crate::danger::passive_from)
            .max()
            .unwrap_or(3)
    };
    let (x, z, level) = cluster;
    if in_wilderness_any(x, z, level) {
        return crate::danger::NEVER;
    }
    let surface = if z >= UNDERGROUND_OFFSET {
        match crate::combat_spots::by_key(rung.key).and_then(|spot| spot.entrance) {
            Some(entrance) => entrance.surface,
            None => return crate::danger::NEVER,
        }
    } else {
        (x, z)
    };
    let Some(bank) = crate::danger::site_bank(surface.0, surface.1, level) else {
        return crate::danger::NEVER;
    };
    required_at(x, z, level).max(required_at(bank.x, bank.z, bank.level))
}

/// Rate every rung for this fighter and these holdings.
pub fn rate(fighter: &Fighter, holdings: &Holdings) -> Vec<Rated> {
    let heal = if holdings.best_heal > 0 { holdings.best_heal } else { DEFAULT_HEAL };
    let mut out = Vec::new();
    for rung in RUNGS {
        let Some(npc) = crate::npc_combat::by_name(rung.npc) else { continue };
        let Some(estimate) = estimate(fighter, &npc, rung.self_heal_per_tick) else { continue };
        let coins = coins_per_kill(rung.npc);
        let food = if rung.food_exempt { 0 } else { food_for(&estimate, heal) };
        // Bank trips the food burn forces, as lost fighting time.
        let load_ticks = if rung.food_exempt || estimate.loss_per_tick <= 0.0 { f64::INFINITY } else { f64::from(food) * heal as f64 / estimate.loss_per_tick };
        let fighting_share = if load_ticks.is_finite() { load_ticks / (load_ticks + BANK_TRIP_TICKS) } else { 1.0 };
        let value = estimate.xp_per_tick * fighting_share + COIN_XP_WEIGHT * coins / estimate.cycle_ticks;
        let eat_line = fighter.hitpoints as f64 * EAT_FRACTION;
        let pool = (fighter.hitpoints as f64 - eat_line) + f64::from(food) * heal as f64;
        let clusters: Vec<(i64, i64, i64, usize)> = rung_clusters(&rung)
            .into_iter()
            .filter(|(x, z, level, _)| i64::from(cluster_requirement(&rung, (*x, *z, *level))) <= fighter.combat)
            .collect();
        let refused = if !gate_open(rung.gate, holdings) {
            Some(format!("gate {:?} closed", rung.gate))
        } else if (BURST_HITS * estimate.npc_max_hit) as f64 >= eat_line {
            Some(format!("burst: {BURST_HITS} x max hit {} >= eat line {eat_line:.1}", estimate.npc_max_hit))
        } else if !rung.food_exempt && pool < MIN_KILLS_PER_LOAD * estimate.damage_per_kill {
            Some(format!("load {pool:.1} hp < {MIN_KILLS_PER_LOAD} kills x {:.1}", estimate.damage_per_kill))
        } else if clusters.is_empty() {
            Some("no cluster safe for this combat level".to_string())
        } else {
            None
        };
        out.push(Rated { rung, estimate, food, coins_per_kill: coins, value, refused, clusters });
    }
    out
}

fn gate_open(gate: Gate, holdings: &Holdings) -> bool {
    match gate {
        Gate::Open => true,
        Gate::Toll => holdings.east_of_toll || holdings.coins >= TOLL_COINS,
        Gate::BrassKey => holdings.brass_key,
    }
}

/// Stable 64-bit FNV-1a of an account name (the spread seed).
pub fn seed_of(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3))
}

/// Seeded pick weighted by `weight` (stable for one seed): a fleet
/// lands on each entry in proportion to its weight, so a 43-spawn camp
/// takes more bots than a 6-spawn one.
fn weighted_index(weights: &[f64], seed: u64) -> usize {
    let total: f64 = weights.iter().map(|weight| weight.max(0.0)).sum();
    if total <= 0.0 || weights.is_empty() {
        return 0;
    }
    let mut point = (seed % 1_000_003) as f64 / 1_000_003.0 * total;
    for (index, weight) in weights.iter().enumerate() {
        point -= weight.max(0.0);
        if point < 0.0 {
            return index;
        }
    }
    weights.len() - 1
}

/// Visiting order: one cluster picked by the seed (weighted by its
/// spawns), then the others nearest to it.
pub fn order_clusters(clusters: &[(i64, i64, i64, usize)], seed: u64) -> Vec<(i64, i64, i64, usize)> {
    if clusters.is_empty() {
        return Vec::new();
    }
    let weights: Vec<f64> = clusters.iter().map(|(_, _, _, spawns)| *spawns as f64).collect();
    let first = clusters[weighted_index(&weights, seed)];
    let mut rest: Vec<(i64, i64, i64, usize)> = clusters.iter().copied().filter(|cluster| *cluster != first).collect();
    rest.sort_by_key(|(x, z, _, _)| (x - first.0).abs() + (z - first.1).abs());
    let mut out = vec![first];
    out.extend(rest);
    out
}

/// Spawns a rung offers across its workable clusters (its capacity).
fn capacity(entry: &Rated) -> f64 {
    entry.clusters.iter().map(|(_, _, _, spawns)| *spawns as f64).sum()
}

/// One rung of `band`, weighted by value x capacity.
fn spread<'rated>(band: &[&'rated Rated], seed: u64) -> &'rated Rated {
    let weights: Vec<f64> = band.iter().map(|entry| entry.value * capacity(entry)).collect();
    band[weighted_index(&weights, seed)]
}

/// The ladder's pick. `None` when no rung is workable at all.
pub fn choose(fighter: &Fighter, holdings: &Holdings, seed: u64) -> Option<Pick> {
    choose_excluding(fighter, holdings, seed, &[])
}

/// `choose` without the rungs in `excluded` (the run's killers).
pub fn choose_excluding(fighter: &Fighter, holdings: &Holdings, seed: u64, excluded: &[&str]) -> Option<Pick> {
    let rated = rate(fighter, holdings);
    let workable: Vec<&Rated> = rated.iter().filter(|entry| entry.refused.is_none() && !excluded.contains(&entry.rung.key)).collect();
    let best = workable.iter().map(|entry| entry.value).fold(f64::MIN, f64::max);
    let mut band: Vec<&Rated> = workable.iter().copied().filter(|entry| entry.value >= best * BAND).collect();
    band.sort_by_key(|entry| entry.rung.key);
    if band.is_empty() {
        return None;
    }
    // Foodless: the food errand only pays when the band beats the best
    // exempt rung by `FOOD_ERRAND_GAIN`; otherwise hunt that one as is.
    if holdings.food_count == 0 && band.iter().any(|entry| !entry.rung.food_exempt) {
        let exempt = workable.iter().copied().filter(|entry| entry.rung.food_exempt).max_by(|left, right| left.value.total_cmp(&right.value));
        match exempt {
            Some(entry) if entry.value * FOOD_ERRAND_GAIN >= best => band = vec![entry],
            _ => {
                let wanted = spread(&band.iter().copied().filter(|entry| !entry.rung.food_exempt).collect::<Vec<_>>(), seed);
                return Some(Pick::NeedFood { rung: wanted.rung, count: wanted.food });
            }
        }
    }
    let chosen = spread(&band, seed);
    Some(Pick::Hunt {
        rung: chosen.rung,
        clusters: order_clusters(&chosen.clusters, seed / 1_000_003),
        food: if chosen.rung.food_exempt { None } else { Some(chosen.food) },
    })
}

#[cfg(test)]
#[path = "melee_ladder_tests.rs"]
mod tests;
