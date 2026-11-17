//! Melee training ladder: which F2P spot a fighter trains at, from its
//! levels, worn bonuses and hitpoints (`melee_math`), within a safety
//! rule, spread across spots and clusters by a stable seed. Pure: the
//! script feeds live state in and walks to what comes out.
//!
//! Rules (user decisions 2026-11-13): every F2P melee spot is allowed;
//! food is carried everywhere except cows and chickens. A spot is
//! workable when:
//! - its gate holds: `Toll` needs 10 coins (or the bot already on the
//!   Al Kharid flank); otherwise the ladder takes the next spot, never
//!   a block. The Edgeville dungeon giants need no key: the trapdoor
//!   and the main dungeon reach their hall; an owned brass key only
//!   rides along as the shortcut (`Rung::shortcut_key`);
//! - two NPC max hits from the eat line cannot kill (`burst`), nor two
//!   hits of any hunter met on the way down to an underground cluster
//!   that would attack this combat level (`approach_threat`);
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
/// F2P fish the brain may ask for when the bot owns no food, smallest
/// heal first (`food_to_ask`): all fished and cooked by bots, Karamja's
/// tuna, lobster and swordfish included (boat routed).
pub const FOOD_LADDER: [&str; 9] = ["shrimp", "sardine", "herring", "trout", "pike", "salmon", "tuna", "lobster", "swordfish"];

/// Food the brain obtains when the bot owns none (`Blocked`): the
/// biggest heal of `FOOD_LADDER` up to half the bot's max hitpoints
/// (user 2026-11-15: "privilégier les nourritures qui donnent le plus
/// d'hp en fonction de leur niveau d'hitpoint max"; every melee bot used
/// to ask shrimps, the market's top bid). Shrimps at the least.
pub fn food_to_ask(max_hitpoints: i64) -> &'static str {
    let room = food_room(max_hitpoints);
    FOOD_LADDER.iter().copied().filter(|food| crate::food::heal(food) <= room).max_by_key(|food| crate::food::heal(food)).unwrap_or(FOOD_LADDER[0])
}

/// Biggest heal a food may restore for a bot of `max_hitpoints`: half
/// its health (a 20-HP bot never carries +14 swordfish that waste most
/// of each bite), shrimps at the least.
pub fn food_room(max_hitpoints: i64) -> i64 {
    (max_hitpoints / 2).max(crate::food::heal(FOOD_LADDER[0]))
}

/// The food a foodless fight obtains, the wallet read (user 2026-11-17:
/// "la meilleure nourriture qu'il peut s'offrir"): the biggest heal of
/// `FOOD_LADDER` up to the `asked` one (already fitted to the bot's
/// hitpoints) whose market price (`price_of`: best ask, else current
/// price; `None` unpriced) covers `count` units with `coins`. Nothing
/// affordable and priced: the asked food stays (the obtain produces it
/// or rests a bid). A non-ladder ask stays as asked.
pub fn affordable_food(asked: &str, count: i64, coins: i64, price_of: &dyn Fn(&str) -> Option<i64>) -> String {
    let cap = crate::food::heal(asked);
    if cap <= 0 || !FOOD_LADDER.contains(&asked) {
        return asked.to_string();
    }
    FOOD_LADDER
        .iter()
        .copied()
        .filter(|food| crate::food::heal(food) <= cap)
        .filter(|food| price_of(food).is_some_and(|price| price.max(1).saturating_mul(count.max(1)) <= coins))
        .max_by_key(|food| crate::food::heal(food))
        .unwrap_or(asked)
        .to_string()
}
/// Brass key debugname (`item:983`, the Edgeville dungeon door).
pub const BRASS_KEY: &str = "edgevilledungeonkey";

/// What a spot needs before the ladder considers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Open,
    /// Across the Al Kharid toll fence.
    Toll,
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
    /// Key that opens a shorter way in, carried every trip when owned
    /// (the brass key: the house door west of Varrock drops straight
    /// into the giants' hall). Never required.
    pub shortcut_key: Option<&'static str>,
}

const fn rung(key: &'static str, npc: &'static str, food_exempt: bool, gate: Gate, area: Option<(i64, i64, i64, i64)>) -> Rung {
    Rung { key, npc, food_exempt, gate, area, self_heal_per_tick: 0.0, shortcut_key: None }
}

/// Every F2P melee rung the fight loop can hunt (surface, plus the
/// Edgeville dungeon giants the router reaches by the Edgeville trapdoor,
/// or the brass-key house when the key is owned).
/// Left out: dark wizards (magic attackers, not a melee exchange), rats
/// (mixed 2-10 hp NPCs), imps (teleport away mid-fight, max hit 0),
/// bears, moss giants (their F2P
/// hall east of the Edgeville dungeon zombies sits behind the slashable
/// web `bigweb_slashable` (3146, 9898): no nav edge, `Disconnected`
/// offline), the Asgarnian ice dungeon (ice warriors and giants: no F2P
/// bank within `danger::BANK_TREK_LIMIT` of Mudskipper Point), and
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
    // Rimmington peninsula only (Crandor needs Dragon Slayer; the
    // Edgeville dungeon camp mixes in the armed level-42 ones).
    rung("hobgoblins", "hobgoblin_unarmed", false, Gate::Open, Some((2895, 3270, 2920, 3300))),
    Rung { shortcut_key: Some(BRASS_KEY), ..rung("hill_giants", "giant", false, Gate::Open, Some((3090, 9815, 3130, 9860))) },
];

/// Rung by spot key.
pub fn rung_by_key(key: &str) -> Option<Rung> {
    RUNGS.iter().find(|entry| entry.key == key).copied()
}

/// Share of the bot's base hitpoints under which a spot's NPC max hit
/// is harmless (user 2026-11-17): fought foodless, like cows. One rule
/// for the scripts (`kit::ladder`) and the planner (`food_gate`).
pub const HARMLESS_HIT_SHARE: f64 = 0.05;
/// Foe max hit assumed when the spot's NPCs have no melee block.
pub const UNKNOWN_FOE_MAX_HIT: i64 = 3;

/// Biggest melee max hit among a spot's NPCs (`UNKNOWN_FOE_MAX_HIT`
/// when none has a melee block).
pub fn foe_max_hit(npc_names: &[&str]) -> i64 {
    npc_names.iter().filter_map(|name| crate::melee_math::npc_max_hit(name)).max().unwrap_or(UNKNOWN_FOE_MAX_HIT)
}

/// True when a foe max hit is harmless for a bot of `max_hp` base
/// hitpoints (under `HARMLESS_HIT_SHARE` of them).
pub fn harmless(foe_max_hit: i64, max_hp: i64) -> bool {
    (foe_max_hit as f64) < HARMLESS_HIT_SHARE * max_hp as f64
}

/// True when the spot `spot_key` must not be fought without food by a
/// bot of `max_hp` base hitpoints: every spot but the exempt rungs
/// (cows, chickens) and the harmless ones (men, max hit 1, for a 30-HP
/// bot).
pub fn spot_food_required(spot_key: &str, max_hp: i64) -> bool {
    if rung_by_key(spot_key).is_some_and(|rung| rung.food_exempt) {
        return false;
    }
    let names: &[&str] = crate::combat_spots::by_key(spot_key).map(|spot| spot.npc_names).unwrap_or(&[]);
    !harmless(foe_max_hit(names), max_hp)
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
    /// of `food_to_ask(max hitpoints)` first.
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

/// Wilderness, surface or the dungeons under it (`danger::in_wilderness`,
/// the engine's own line).
pub fn in_wilderness_any(x: i64, z: i64, level: i64) -> bool {
    crate::danger::in_wilderness(x, z, level)
}

/// Box slack around the trapdoor-to-cluster span where an underground
/// approach meets hunters: the Edgeville trapdoor walk to the giants
/// loops up to z 9914 and east to x 3146 (golden `geants_sans_cle`),
/// 46 and 35 tiles past the box of its ends.
pub const APPROACH_MARGIN: i64 = 50;

/// Worst hunter met on the way down to an underground cluster: one
/// that would attack `combat` (server passive rule, the rung's own NPCs
/// excluded: we came to fight them) and whose two max hits reach the
/// eat line. (name, max hit), or `None` (surface cluster, or every
/// hunter on the way passive or survivable). Hunters spawn anywhere in
/// the box spanning the entrance landing (surface + 6400) and the
/// cluster, `APPROACH_MARGIN` wider, Wilderness ones included (the
/// routes may cross it, user 2026-11-16).
pub fn approach_threat(rung: &Rung, cluster: (i64, i64, i64), combat: i64, eat_line: f64) -> Option<(&'static str, i64)> {
    let (x, z, level) = cluster;
    if z < UNDERGROUND_OFFSET {
        return None;
    }
    let spot = crate::combat_spots::by_key(rung.key)?;
    let (landing_x, landing_z) = spot.entrance.map(|entrance| (entrance.surface.0, entrance.surface.1 + UNDERGROUND_OFFSET)).unwrap_or((x, z));
    let (low_x, high_x) = (x.min(landing_x) - APPROACH_MARGIN, x.max(landing_x) + APPROACH_MARGIN);
    let (low_z, high_z) = (z.min(landing_z) - APPROACH_MARGIN, z.max(landing_z) + APPROACH_MARGIN);
    let combat = u8::try_from(combat.clamp(0, i64::from(u8::MAX))).unwrap_or(u8::MAX);
    crate::danger::hunters()
        .iter()
        .filter(|hunter| hunter.level == level && !spot.npc_names.contains(&hunter.name))
        .filter(|hunter| (low_x..=high_x).contains(&hunter.x) && (low_z..=high_z).contains(&hunter.z))
        .filter(|hunter| crate::danger::hunts(hunter, Some(combat)))
        .filter_map(|hunter| {
            let npc = crate::npc_combat::by_name(hunter.name)?;
            Some((hunter.name, crate::melee_math::max_hit(npc.strength + 9, npc.strength_bonus)))
        })
        .filter(|(_, max_hit)| (BURST_HITS * max_hit) as f64 >= eat_line)
        .max_by_key(|(name, max_hit)| (*max_hit, *name))
}

/// F2P spawn clusters of a rung on level 0, Wilderness included (user
/// 2026-11-16) (15-tile groups, like the fight loop's), two spawns at
/// least, sorted.
pub fn rung_clusters(rung: &Rung) -> Vec<(i64, i64, i64, usize)> {
    let inside = |x: i64, z: i64| rung.area.is_none_or(|(x0, z0, x1, z1)| (x0..=x1).contains(&x) && (z0..=z1).contains(&z));
    let mut groups: Vec<Vec<(i64, i64)>> = Vec::new();
    for (x, z, level) in crate::combat_spots::positions(rung.key) {
        if level != 0 || !inside(x, z) {
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
        let safe: Vec<(i64, i64, i64, usize)> = rung_clusters(&rung)
            .into_iter()
            .filter(|(x, z, level, _)| i64::from(cluster_requirement(&rung, (*x, *z, *level))) <= fighter.combat)
            .collect();
        let threats: Vec<Option<(&str, i64)>> = safe.iter().map(|(x, z, level, _)| approach_threat(&rung, (*x, *z, *level), fighter.combat, eat_line)).collect();
        let clusters: Vec<(i64, i64, i64, usize)> = safe.iter().zip(&threats).filter(|(_, threat)| threat.is_none()).map(|(cluster, _)| *cluster).collect();
        let refused = if !gate_open(rung.gate, holdings) {
            Some(format!("gate {:?} closed", rung.gate))
        } else if (BURST_HITS * estimate.npc_max_hit) as f64 >= eat_line {
            Some(format!("burst: {BURST_HITS} x max hit {} >= eat line {eat_line:.1}", estimate.npc_max_hit))
        } else if !rung.food_exempt && pool < MIN_KILLS_PER_LOAD * estimate.damage_per_kill {
            Some(format!("load {pool:.1} hp < {MIN_KILLS_PER_LOAD} kills x {:.1}", estimate.damage_per_kill))
        } else if safe.is_empty() {
            Some("no cluster safe for this combat level".to_string())
        } else if let (true, Some((name, max_hit))) = (clusters.is_empty(), threats.iter().flatten().next()) {
            Some(format!("approach: {name} {BURST_HITS} x max hit {max_hit} >= eat line {eat_line:.1}"))
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
