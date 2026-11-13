//! Fleet coverage of ambitions (user decision 2026-11-13): at least one
//! bot per F2P skill group (12, melee as one) and at least one merchant;
//! beyond that, random long-term ambitions weighted by demand and by the
//! bot's own levels. Pure: every bot's levels, profile and current kind
//! come in as plain values; the daemon reads the life files.

use crate::ambition::{may_change, Ambition, GENERALIST};
use crate::ambition_catalog::{fit, kind_for_group, primary_group, KindDemand, GROUPS, MERCHANT, SPECS};

/// One bot as the fleet assignment sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct FleetBot {
    pub account: String,
    pub levels: Vec<(String, u8)>,
    pub ironman: bool,
    /// Drawn merchant archetype (D18): favoured for the merchant role.
    pub merchant_archetype: bool,
    pub market_affinity: f64,
    pub seed: u64,
    /// Current ambition kind (`None`: to assign).
    pub kind: Option<String>,
}

impl FleetBot {
    pub fn level(&self, skill: &str) -> u8 {
        self.levels.iter().find(|(name, _)| name == skill).map(|(_, level)| *level).unwrap_or(1)
    }
}

/// One assignment: `why` is `coverage:<group>`, `levels:<group>`,
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

/// Groups (and `merchant`) no current ambition covers as primary group.
pub fn coverage_gaps(kinds: &[&str]) -> Vec<String> {
    let covered: Vec<&str> = kinds.iter().filter_map(|kind| primary_group(kind)).collect();
    GROUPS.iter().chain(std::iter::once(&MERCHANT)).filter(|group| !covered.contains(group)).map(|group| group.to_string()).collect()
}

/// Score of one bot for one gap: skill fit; the merchant role goes to the
/// merchant archetype first, then to the most market-minded. Iron never
/// merchants (`None`).
fn gap_score(bot: &FleetBot, group: &str) -> Option<f64> {
    if group == MERCHANT {
        if bot.ironman {
            return None;
        }
        return Some(if bot.merchant_archetype { 50.0 } else { 1.0 + bot.market_affinity });
    }
    let kind = kind_for_group(group)?;
    Some(fit(kind, &|skill| bot.level(skill)))
}

/// Fleet pass: every bot without an ambition gets one. Coverage first
/// (global greedy: the best-fitting (bot, gap) pair goes first, seeded
/// tie-break), then a demand- and level-weighted random draw for the
/// rest. Bots that already hold an ambition count as coverage and are
/// never reassigned here.
pub fn assign_fleet(bots: &[FleetBot], demand: &[KindDemand]) -> Vec<Assignment> {
    let held: Vec<&str> = bots.iter().filter_map(|bot| bot.kind.as_deref()).collect();
    let mut gaps = coverage_gaps(&held);
    let mut free: Vec<&FleetBot> = bots.iter().filter(|bot| bot.kind.is_none()).collect();
    free.sort_by(|left, right| left.account.cmp(&right.account));
    let mut out = Vec::new();
    while !gaps.is_empty() && !free.is_empty() {
        let mut best: Option<(f64, u64, usize, usize)> = None;
        for (bot_at, bot) in free.iter().enumerate() {
            for (gap_at, gap) in gaps.iter().enumerate() {
                let Some(score) = gap_score(bot, gap) else { continue };
                let tie = mix(bot.seed, gap);
                if best.is_none_or(|(known, known_tie, _, _)| score > known + 1e-9 || ((score - known).abs() <= 1e-9 && tie < known_tie)) {
                    best = Some((score, tie, bot_at, gap_at));
                }
            }
        }
        let Some((_, _, bot_at, gap_at)) = best else { break };
        let group = gaps.remove(gap_at);
        let bot = free.remove(bot_at);
        let kind = if group == MERCHANT { MERCHANT } else { kind_for_group(&group).unwrap_or(GENERALIST) };
        out.push(Assignment { account: bot.account.clone(), kind: kind.to_string(), why: format!("coverage:{group}") });
    }
    for bot in free {
        let (kind, why) = draw_kind(bot, &[], demand, &[]);
        out.push(Assignment { account: bot.account.clone(), kind, why });
    }
    out
}

/// Fit from which a bot's levels choose its ambition (best skill of the
/// group ≥ 15 levels above its mean): existing accounts are read from
/// their levels, not drawn by lot (frank_lane woodcutting 64 → woodcutter).
pub const SPECIALIST_FIT: f64 = 16.0;

/// One bot's draw: an open coverage gap it can hold (best fit, seeded
/// tie-break) wins; then a specialist's best-fitting kind (`levels:
/// <group>`, fit ≥ `SPECIALIST_FIT`); otherwise a random kind weighted by
/// demand × skill fit (merchant archetype ×5 on the merchant, never for
/// Iron). `exclude` keeps kinds out (the one just blocked). `why`:
/// coverage, levels, `demand:<item>` when open bids tilted the draw
/// (weight ≥ 1.5 with a product), else `random`.
pub fn draw_kind(bot: &FleetBot, gaps: &[String], demand: &[KindDemand], exclude: &[&str]) -> (String, String) {
    let mut best_gap: Option<(f64, u64, &str)> = None;
    for gap in gaps {
        let Some(kind) = (if gap == MERCHANT { Some(MERCHANT) } else { kind_for_group(gap) }) else { continue };
        if exclude.contains(&kind) {
            continue;
        }
        let Some(score) = gap_score(bot, gap) else { continue };
        let tie = mix(bot.seed, gap);
        if best_gap.is_none_or(|(known, known_tie, _)| score > known + 1e-9 || ((score - known).abs() <= 1e-9 && tie < known_tie)) {
            best_gap = Some((score, tie, gap));
        }
    }
    if let Some((_, _, gap)) = best_gap {
        let kind = if gap == MERCHANT { MERCHANT } else { kind_for_group(gap).unwrap_or(GENERALIST) };
        return (kind.to_string(), format!("coverage:{gap}"));
    }
    let specialist = SPECS
        .iter()
        .filter(|entry| entry.kind != GENERALIST && entry.kind != MERCHANT && !exclude.contains(&entry.kind))
        .map(|entry| (entry, fit(entry.kind, &|skill| bot.level(skill))))
        .filter(|(_, score)| *score >= SPECIALIST_FIT)
        .max_by(|left, right| left.1.total_cmp(&right.1));
    if let Some((entry, _)) = specialist {
        return (entry.kind.to_string(), format!("levels:{}", entry.groups.first().copied().unwrap_or("")));
    }
    let mut weighted: Vec<(&str, f64, Option<String>)> = Vec::new();
    for entry in SPECS.iter().filter(|entry| entry.kind != GENERALIST && !exclude.contains(&entry.kind)) {
        if entry.kind == MERCHANT && bot.ironman {
            continue;
        }
        let tilt = demand.iter().find(|known| known.kind == entry.kind);
        let demand_weight = tilt.map(|known| known.weight).unwrap_or(1.0).max(0.01);
        let archetype = if entry.kind == MERCHANT && bot.merchant_archetype { 5.0 } else { 1.0 };
        let weight = demand_weight * fit(entry.kind, &|skill| bot.level(skill)) * archetype;
        let top = tilt.filter(|known| known.weight >= 1.5).and_then(|known| known.top_item.clone());
        weighted.push((entry.kind, weight, top));
    }
    let total: f64 = weighted.iter().map(|(_, weight, _)| *weight).sum();
    if weighted.is_empty() || total <= 0.0 {
        return (GENERALIST.to_string(), "random".to_string());
    }
    let roll = (mix(bot.seed, "ambition") % 1_000_000) as f64 / 1_000_000.0 * total;
    let mut seen = 0.0;
    for (kind, weight, top) in &weighted {
        seen += weight;
        if roll < seen {
            let why = top.as_ref().map(|item| format!("demand:{item}")).unwrap_or_else(|| "random".to_string());
            return (kind.to_string(), why);
        }
    }
    let (kind, _, top) = weighted.last().cloned().unwrap_or((GENERALIST, 0.0, None));
    (kind.to_string(), top.map(|item| format!("demand:{item}")).unwrap_or_else(|| "random".to_string()))
}

/// Optional change after a completed milestone (never mandatory): only
/// when `AMBITION_CHANGE_TICKS` passed since the last change. A bot whose
/// primary group another bot also covers (`redundant`) fills an open gap
/// first; otherwise one chance in four (seeded per completed milestone)
/// of a fresh demand-weighted draw. The sole holder of a group never
/// leaves it. Returns (kind, why) when the ambition should change.
pub fn optional_change(ambition: &Ambition, bot: &FleetBot, gaps: &[String], redundant: bool, demand: &[KindDemand], ticks_since: &dyn Fn(i64) -> u64) -> Option<(String, String)> {
    if ambition.kind == GENERALIST || !may_change(ambition, ticks_since) || !redundant {
        return None;
    }
    let current = ambition.kind.as_str();
    if !gaps.is_empty() {
        let (kind, why) = draw_kind(bot, gaps, demand, &[current]);
        if why.starts_with("coverage:") {
            return Some((kind, why));
        }
    }
    let done = ambition.milestones.iter().filter(|stone| stone.status == crate::ambition::MilestoneStatus::Done).count();
    if mix(bot.seed, &format!("change{done}")) % 4 != 0 {
        return None;
    }
    let (kind, why) = draw_kind(bot, &[], demand, &[current]);
    Some((kind, why))
}
