//! Fleet coverage of ambitions (user decision 2026-11-13): at least one
//! bot per F2P skill group (12, melee as one; the merchant role was
//! removed in wave 3, plan marché Q6);
//! beyond that, random long-term ambitions weighted by the NEED alone
//! (decision D3, user 2026-11-19: the scarcity of what a kind produces,
//! never the bot's levels), except for `LEVEL_LED_ACCOUNTS` and the
//! market makers, which keep the level-led draw (demand × √fit). Pure:
//! every bot's levels, profile and current kind come in as plain values;
//! the daemon reads the life files.

use crate::ambition::GENERALIST;
use crate::ambition_catalog::{fit, kind_for_group, primary_group, transform_share, KindDemand, GROUPS, specs};

/// One bot as the fleet assignment sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct FleetBot {
    pub account: String,
    pub levels: Vec<(String, u8)>,
    pub ironman: bool,
    pub market_affinity: f64,
    pub seed: u64,
    /// Current ambition kind (`None`: to assign).
    pub kind: Option<String>,
    /// A market maker (`Archetype::MarketMaker`): keeps the level-led
    /// draw (D3 exception).
    pub market_maker: bool,
    /// An operator's ambition (a pin, the alcher, overall, a market
    /// maker): the A4 rebalance never touches it (`ambition_value`).
    pub operator: bool,
}

/// Accounts that keep the LEVEL-LED draw (demand × √fit × the trait's
/// tilt, as before D3), together with every market maker
/// (`FleetBot::market_maker` or a `market_maker` ambition). Every other
/// bot draws by need alone. Decided by the user on 2026-11-19 (decision
/// D3, docs/plan-equilibre.txt step 6). Matched without case.
pub const LEVEL_LED_ACCOUNTS: [&str; 3] = ["Zezima", "N0valyfe", "Yogosun"];

/// `why` of the overall ambition a protected account is given
/// (`ambition_value::PROTECTED_ACCOUNTS`, user 2026-11-21: they always
/// hold the first place of the overall ranking, whatever the draw).
pub const PROTECTED_WHY: &str = "protected:overall";

/// True when the bot keeps the level-led draw (`LEVEL_LED_ACCOUNTS`, a
/// market maker); false: its ambition follows the need alone.
pub fn level_led(bot: &FleetBot) -> bool {
    bot.market_maker
        || bot.kind.as_deref() == Some(crate::market_maker::MARKET_MAKER_KIND)
        || LEVEL_LED_ACCOUNTS.iter().any(|name| name.eq_ignore_ascii_case(&bot.account))
}

/// Saturation (D3 review 2026-11-19): each kind's need divided by 1 +
/// the number of fleet bots already holding it (`held`: their kinds), so
/// ten crafters lower the crafter need. The level-led `weight` is kept.
pub fn saturate(demand: &[KindDemand], held: &[&str]) -> Vec<KindDemand> {
    demand
        .iter()
        .map(|row| {
            let holders = held.iter().filter(|kind| **kind == row.kind).count();
            KindDemand { need: row.need / (1 + holders) as f64, ..row.clone() }
        })
        .collect()
}

/// Need of one kind (`KindDemand::need`, 0 when unknown).
fn need_of(kind: &str, demand: &[KindDemand]) -> f64 {
    demand.iter().find(|known| known.kind == kind).map(|known| known.need.max(0.0)).unwrap_or(0.0)
}

impl FleetBot {
    pub fn level(&self, skill: &str) -> u8 {
        self.levels.iter().find(|(name, _)| name == skill).map(|(_, level)| *level).unwrap_or(1)
    }
}

/// One assignment: `why` is `coverage:<group>`,
/// `random` or `demand:<item>`.
#[derive(Debug, Clone, PartialEq)]
pub struct Assignment {
    pub account: String,
    pub kind: String,
    pub why: String,
}

/// Seeded draw word: FNV-1a of the salt folded into the seed, then the
/// splitmix64 finalizer (every input bit reaches the low bits: seeds are
/// nanosecond stamps whose low bits barely vary).
fn mix(seed: u64, salt: &str) -> u64 {
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    for byte in salt.bytes() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01B3);
    }
    let mut word = (seed ^ hash).wrapping_add(0x9E37_79B9_7F4A_7C15);
    word = (word ^ (word >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    word = (word ^ (word >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (word ^ (word >> 31)).max(1)
}

/// Groups no current ambition covers as primary group.
pub fn coverage_gaps(kinds: &[&str]) -> Vec<String> {
    let covered: Vec<&str> = kinds.iter().filter_map(|kind| primary_group(kind)).collect();
    GROUPS.iter().filter(|group| !covered.contains(group)).map(|group| group.to_string()).collect()
}

/// Strength of the market_affinity tilt (user lever 2026-11-17): a
/// pure market profile weighs a pure transformation kind ×2^1.5 (≈ 2.8)
/// and a pure harvest or fight kind ÷2.8; an autarkic one the reverse;
/// the neutral 0.5 tilts nothing.
pub const AFFINITY_TILT: f64 = 1.5;

/// Weight of `kind` for a bot of `affinity` (user lever 2026-11-17,
/// « entraînement marché plus fréquent »): 2^(TILT × (2a − 1) × (2s − 1))
/// where s is the kind's transformation share (`transform_share`,
/// computed from the game data: the share of its skill's activities fed
/// by buy-able inputs). A market bot leans on the skills a player trains
/// by buying (smithing, cooking, firemaking, crafting, magic, prayer,
/// runecraft), an autarkic one on harvests and fights; never a hand list.
pub fn affinity_tilt(kind: &str, affinity: f64) -> f64 {
    let lean = 2.0 * affinity.clamp(0.0, 1.0) - 1.0;
    let transforms = 2.0 * transform_share(kind) - 1.0;
    2f64.powf(AFFINITY_TILT * lean * transforms)
}

/// Score of one bot for one gap, × the trait's tilt: a level-led bot's
/// skill fit; any other bot's 1 + the need of the gap's kind (D3: levels
/// play no role).
fn gap_score(bot: &FleetBot, group: &str, demand: &[KindDemand]) -> Option<f64> {
    let kind = kind_for_group(group)?;
    let base = if level_led(bot) { fit(kind, &|skill| bot.level(skill)) } else { 1.0 + need_of(kind, demand) };
    Some(base * affinity_tilt(kind, bot.market_affinity))
}

/// Fleet pass: every bot without an ambition gets one. Coverage first
/// (global greedy: the best-scoring (bot, gap) pair goes first,
/// `gap_score`, seeded tie-break), then `draw_kind` for the rest. Bots
/// that already hold an ambition count as coverage and are never
/// reassigned here.
pub fn assign_fleet(bots: &[FleetBot], demand: &[KindDemand]) -> Vec<Assignment> {
    let held: Vec<&str> = bots.iter().filter_map(|bot| bot.kind.as_deref()).collect();
    let mut gaps = coverage_gaps(&held);
    let mut free: Vec<&FleetBot> = bots.iter().filter(|bot| bot.kind.is_none()).collect();
    free.sort_by(|left, right| left.account.cmp(&right.account));
    let mut out = Vec::new();
    // A protected account never covers a gap: overall, always.
    free.retain(|bot| {
        let keeps_overall = crate::ambition_value::protected(&bot.account);
        if keeps_overall {
            out.push(Assignment { account: bot.account.clone(), kind: crate::ambition::OVERALL.to_string(), why: PROTECTED_WHY.to_string() });
        }
        !keeps_overall
    });
    while !gaps.is_empty() && !free.is_empty() {
        let mut best: Option<(f64, u64, usize, usize)> = None;
        for (bot_at, bot) in free.iter().enumerate() {
            for (gap_at, gap) in gaps.iter().enumerate() {
                let Some(score) = gap_score(bot, gap, demand) else { continue };
                let tie = mix(bot.seed, gap);
                if best.is_none_or(|(known, known_tie, _, _)| score > known + 1e-9 || ((score - known).abs() <= 1e-9 && tie < known_tie)) {
                    best = Some((score, tie, bot_at, gap_at));
                }
            }
        }
        let Some((_, _, bot_at, gap_at)) = best else { break };
        let group = gaps.remove(gap_at);
        let bot = free.remove(bot_at);
        let kind = kind_for_group(&group).unwrap_or(GENERALIST);
        out.push(Assignment { account: bot.account.clone(), kind: kind.to_string(), why: format!("coverage:{group}") });
    }
    // Saturation (D3): the kinds held and drawn so far lower their need.
    let mut held_now: Vec<String> = held.iter().map(|kind| kind.to_string()).collect();
    held_now.extend(out.iter().map(|row| row.kind.clone()));
    for bot in free {
        let refs: Vec<&str> = held_now.iter().map(String::as_str).collect();
        let (kind, why) = draw_kind(bot, &[], &saturate(demand, &refs), &[]);
        held_now.push(kind.clone());
        out.push(Assignment { account: bot.account.clone(), kind, why });
    }
    out
}

/// One bot's draw: an open coverage gap it can hold (best `gap_score`,
/// seeded tie-break) wins; otherwise a seeded random kind (never an
/// operator-only kind), × `affinity_tilt` (user lever 2026-11-17: a
/// market profile leans on transformation kinds):
/// - by NEED alone (decision D3, user 2026-11-19): weight = the kind's
///   `need`; a kind nothing calls for is never drawn while another is;
///   no need anywhere = equal needs (uniform × tilt, `why` `random`).
///   `why`: `need:<item>` (the object carrying at least
///   `NEED_LABEL_FLOOR` of its need), else `need`;
/// - level-led (`level_led`: `LEVEL_LED_ACCOUNTS`, market makers, the
///   draw before D3): demand weight × √fit (√fit = 1 + lead/5; levels
///   tilt it but never fix it, user decision 2026-11-13). `why`:
///   `demand:<item>` when open bids tilted it (weight ≥ 1.5 with a
///   product), else `random`.
/// `exclude` keeps kinds out (the one just blocked).
pub fn draw_kind(bot: &FleetBot, gaps: &[String], demand: &[KindDemand], exclude: &[&str]) -> (String, String) {
    if crate::ambition_value::protected(&bot.account) {
        return (crate::ambition::OVERALL.to_string(), PROTECTED_WHY.to_string());
    }
    let mut best_gap: Option<(f64, u64, &str)> = None;
    for gap in gaps {
        let Some(kind) = kind_for_group(gap) else { continue };
        if exclude.contains(&kind) {
            continue;
        }
        let Some(score) = gap_score(bot, gap, demand) else { continue };
        let tie = mix(bot.seed, gap);
        if best_gap.is_none_or(|(known, known_tie, _)| score > known + 1e-9 || ((score - known).abs() <= 1e-9 && tie < known_tie)) {
            best_gap = Some((score, tie, gap));
        }
    }
    if let Some((_, _, gap)) = best_gap {
        let kind = kind_for_group(gap).unwrap_or(GENERALIST);
        return (kind.to_string(), format!("coverage:{gap}"));
    }
    let operator_only = |kind: &str| crate::ambition::ladderless(kind) || kind == crate::market_maker::MARKET_MAKER_KIND;
    let drawable: Vec<&str> = specs().iter().map(|entry| entry.kind).filter(|kind| !operator_only(kind) && !exclude.contains(kind)).collect();
    let weighted = if level_led(bot) { level_led_weights(bot, &drawable, demand) } else { need_weights(bot, &drawable, demand) };
    let total: f64 = weighted.iter().map(|(_, weight, _)| *weight).sum();
    if weighted.is_empty() || total <= 0.0 {
        return (GENERALIST.to_string(), "random".to_string());
    }
    let roll = (mix(bot.seed, "ambition") % 1_000_000) as f64 / 1_000_000.0 * total;
    let mut seen = 0.0;
    for (kind, weight, why) in &weighted {
        seen += weight;
        if roll < seen {
            return (kind.to_string(), why.clone());
        }
    }
    let (kind, _, why) = weighted.last().cloned().unwrap_or((GENERALIST, 0.0, "random".to_string()));
    (kind.to_string(), why)
}

/// The kind the need draw weighs most for this bot (D3 weights, no
/// roll, operator kinds out): a retired market maker takes the neediest
/// trade of the fleet (user 2026-11-20, « trouve les rôles idéaux pour
/// ces 11 comptes »); `demand` saturated by the kinds already held, so
/// successive retirements spread. `None` when nothing is drawable.
pub fn neediest_kind(bot: &FleetBot, demand: &[KindDemand], exclude: &[&str]) -> Option<(String, String)> {
    if crate::ambition_value::protected(&bot.account) {
        return Some((crate::ambition::OVERALL.to_string(), PROTECTED_WHY.to_string()));
    }
    let operator_only = |kind: &str| crate::ambition::ladderless(kind) || kind == crate::market_maker::MARKET_MAKER_KIND;
    let drawable: Vec<&str> = specs().iter().map(|entry| entry.kind).filter(|kind| !operator_only(kind) && !exclude.contains(kind)).collect();
    need_weights(bot, &drawable, demand).into_iter().max_by(|left, right| left.1.total_cmp(&right.1)).map(|(kind, _, why)| (kind.to_string(), why))
}

/// Level-led weights (the draw before D3): demand weight × √fit × tilt.
fn level_led_weights<'kinds>(bot: &FleetBot, drawable: &[&'kinds str], demand: &[KindDemand]) -> Vec<(&'kinds str, f64, String)> {
    let mut weighted = Vec::new();
    for kind in drawable {
        let known = demand.iter().find(|known| known.kind == *kind);
        let demand_weight = known.map(|known| known.weight).unwrap_or(1.0).max(0.01);
        let weight = demand_weight * fit(kind, &|skill| bot.level(skill)).sqrt() * affinity_tilt(kind, bot.market_affinity);
        let top = known.filter(|known| known.weight >= 1.5).and_then(|known| known.top_item.clone());
        weighted.push((*kind, weight, top.map(|item| format!("demand:{item}")).unwrap_or_else(|| "random".to_string())));
    }
    weighted
}

/// Need weights (D3): the kind's need × tilt; no need anywhere = equal
/// needs (tilt alone, `random`).
fn need_weights<'kinds>(bot: &FleetBot, drawable: &[&'kinds str], demand: &[KindDemand]) -> Vec<(&'kinds str, f64, String)> {
    let any_need = drawable.iter().any(|kind| need_of(kind, demand) > 0.0);
    let mut weighted = Vec::new();
    for kind in drawable {
        let tilt = affinity_tilt(kind, bot.market_affinity);
        if !any_need {
            weighted.push((*kind, tilt, "random".to_string()));
            continue;
        }
        let need = need_of(kind, demand);
        if need <= 0.0 {
            continue;
        }
        let driver = demand.iter().find(|known| known.kind == *kind).and_then(|known| known.need_item.clone());
        weighted.push((*kind, need * tilt, driver.map(|item| format!("need:{item}")).unwrap_or_else(|| "need".to_string())));
    }
    weighted
}

// A2 (user 2026-11-21): no optional change and no coverage repair of a
// running ambition (`optional_change`, `coverage_repair` are gone): an
// ambition ends only when it is achieved; the gaps stay a log.

#[cfg(test)]
#[path = "ambition_market_tests.rs"]
mod market_tests;

#[cfg(test)]
#[path = "ambition_need_tests.rs"]
mod need_tests;

#[cfg(test)]
#[path = "ambition_protected_tests.rs"]
mod protected_tests;
