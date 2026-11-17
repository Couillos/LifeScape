//! Ambition catalogue: one entry per F2P skill group of rev 274 (melee
//! counts as ONE group: attack, strength, defence, hitpoints), plus the
//! operator-only kinds: the generalist, the overall highscore and the
//! market maker's coin ladder (plan marché Q6; the drawn merchant and its
//! fleet coverage role were removed in wave 3). Each entry is a ladder of
//! rungs (a level to reach, then a production quota, an equipment tier
//! or a coin target) COMPUTED from the game data by `ambition_ladder`
//! (user 2026-11-15): no product list is written by hand.
//!
//! Also here: skill fit (existing levels map to a sensible ambition) and
//! the demand weights per kind (open bids, chain and method profits).

/// The 12 F2P skill groups of rev 274 (fletching is members-only).
pub const GROUPS: [&str; 12] =
    ["melee", "ranged", "prayer", "magic", "cooking", "woodcutting", "fishing", "firemaking", "crafting", "smithing", "mining", "runecraft"];

/// What a rung asks once its level is reached.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Then {
    Nothing,
    /// Candidate products, quota, skill the production trains.
    Produce(&'static [&'static str], i64, &'static str),
    Equip(&'static str),
    Coins(i64),
}

/// One ladder step: `level` of `skill` (0 = no level milestone), then.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rung {
    pub skill: &'static str,
    pub level: u8,
    pub then: Then,
}

/// One catalogue entry. `groups[0]` is the primary group (coverage).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spec {
    pub kind: &'static str,
    pub label: &'static str,
    pub groups: &'static [&'static str],
    pub ladder: &'static [Rung],
}

/// One catalogue row: kind, label, groups (`groups[0]` primary), and
/// the rule its ladder is computed by (`ambition_ladder`).
type Meta = (&'static str, &'static str, &'static [&'static str], crate::ambition_ladder::Rule);

const META: &[Meta] = {
    use crate::ambition_ladder::Rule::{Climber, Coins, Empty, Melee, Producer};
    &[
        ("warrior", "guerrier : mêlée et armures", &["melee"], Melee),
        ("ranger", "archer : distance et équipement", &["ranged", "melee"], Climber("ranged", "ranged")),
        ("priest", "prêtre : prière, os enterrés", &["prayer", "melee"], Climber("prayer", "melee")),
        ("mage", "mage : magie, sorts et alchimie", &["magic"], Climber("magic", "magic")),
        ("cook", "cuisinier : poissons et viandes cuits", &["cooking", "fishing"], Producer("cooking")),
        ("woodcutter", "bûcheron : fournisseur de bûches", &["woodcutting"], Producer("woodcutting")),
        ("fisher", "pêcheur : fournisseur de poisson cru", &["fishing"], Producer("fishing")),
        ("firemaker", "allumeur : feux, bûches achetées ou coupées", &["firemaking", "woodcutting"], Climber("firemaking", "")),
        ("crafter", "artisan : cuir, poterie, bijoux, laine", &["crafting"], Producer("crafting")),
        ("smith", "forgeron : armes et armures de métal", &["smithing", "mining"], Producer("smithing")),
        ("miner", "mineur : fournisseur de minerai", &["mining"], Producer("mining")),
        ("runecrafter", "runier : essence et runes", &["runecraft", "mining"], Producer("runecraft")),
        ("market_maker", "teneur de marché : capital d'abord (argent), puis cotation à la fontaine", &[], Coins),
        ("generalist", "généraliste : compétence la plus basse", &[], Empty),
        ("overall", "premier au classement overall : total level puis XP", &[], Empty),
    ]
};

/// Every ambition the fleet draws from, plus the generalist and the
/// operator-only overall highscore (never drawn): ladders computed once
/// from the game data (`ambition_ladder`), never written by hand.
pub fn specs() -> &'static [Spec] {
    static ALL: std::sync::OnceLock<Vec<Spec>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        META.iter()
            .map(|(kind, label, groups, rule)| Spec { kind, label, groups, ladder: Box::leak(crate::ambition_ladder::build(*rule).into_boxed_slice()) })
            .collect()
    })
}

pub fn spec(kind: &str) -> Option<&'static Spec> {
    specs().iter().find(|entry| entry.kind == kind)
}

/// The ambition covering a group (primary group match).
pub fn kind_for_group(group: &str) -> Option<&'static str> {
    specs().iter().find(|entry| entry.groups.first() == Some(&group)).map(|entry| entry.kind)
}

/// Primary group of a kind (`None` for the operator-only kinds or an
/// unknown kind).
pub fn primary_group(kind: &str) -> Option<&'static str> {
    spec(kind).and_then(|entry| entry.groups.first().copied())
}

/// Skill group of one skill (melee stats are ONE group).
pub fn group_of_skill(skill: &str) -> Option<&'static str> {
    match skill {
        "attack" | "strength" | "defence" | "hitpoints" => Some("melee"),
        other => GROUPS.iter().find(|group| **group == other).copied(),
    }
}

/// Skills of a group (melee: the three trained stats and hitpoints).
pub fn group_skills(group: &str) -> Vec<&'static str> {
    match group {
        "melee" => vec!["attack", "strength", "defence", "hitpoints"],
        other => GROUPS.iter().filter(|known| **known == other).copied().collect(),
    }
}

/// What a kind's own skills produce in one step for an open bid beyond
/// its ladder (user 2026-11-15: "motive les bots a produire les
/// ressources manquantes sur le marché"): a production leg trained by one
/// of the kind's group skills whose inputs are none or gathered by those
/// same skills (a smith's steel bars from mined ore and coal). Returns
/// the skill and level of the leg; `None` when the item needs a chain
/// the kind does not climb (an amulet of magic needs crafted amulets).
pub fn servable(kind: &str, item: &str) -> Option<(String, u8)> {
    let found = spec(kind)?;
    let skills: Vec<&str> = found.groups.iter().flat_map(|group| group_skills(group)).collect();
    let gathered = |input: &str| {
        crate::obtain_legs::production_legs(input)
            .iter()
            .any(|leg| leg.inputs.is_empty() && leg.skill.as_ref().is_some_and(|(skill, _)| skills.contains(&skill.as_str())))
    };
    crate::obtain_legs::production_legs(item).into_iter().find_map(|leg| {
        let (skill, level) = leg.skill.clone()?;
        let own = skills.contains(&skill.as_str()) && leg.inputs.iter().all(|(input, _)| gathered(input));
        own.then(|| (skill, u8::try_from(level.clamp(1, 99)).unwrap_or(99)))
    })
}

/// True when the kind's ladder lists `item` as a production quota.
pub fn ladder_offers(kind: &str, item: &str) -> bool {
    spec(kind).is_some_and(|found| found.ladder.iter().any(|rung| matches!(rung.then, Then::Produce(items, _, _) if items.contains(&item))))
}

/// Skill-fit of a kind for a bot: (1 + lead / 5)², where lead is how far
/// the best skill of its primary group stands above the bot's mean over
/// the groups (frank_lane woodcutting 64 among levels ~10: ~120 for the
/// woodcutter, 1 elsewhere). Fresh bots fit everything at 1.
pub fn fit(kind: &str, level_of: &dyn Fn(&str) -> u8) -> f64 {
    let Some(group) = primary_group(kind) else { return 0.0 };
    // Hitpoints start at 10 and rise with any fight: never a lead.
    let best_in = |name: &str| {
        group_skills(name).iter().filter(|skill| **skill != "hitpoints").map(|skill| f64::from(level_of(skill).max(1))).fold(1.0, f64::max)
    };
    let mean = GROUPS.iter().map(|name| best_in(name)).sum::<f64>() / GROUPS.len() as f64;
    let lead = (best_in(group) - mean).max(0.0);
    (1.0 + lead / 5.0).powi(2)
}

/// One open demand the ambitions answer (from the market's open bids).
#[derive(Debug, Clone, PartialEq)]
pub struct DemandItem {
    pub item: String,
    /// Bid value weighted by age (`market_demand`): relative only.
    pub weight: f64,
    pub unmet: i64,
    /// Per-unit margin of making it at the best bid against its inputs at
    /// their market reference (`profit::chain_profit`); `None` when no
    /// production way makes it or no price is known. Only a positive
    /// margin makes a demand milestone.
    pub margin: Option<f64>,
}

/// Demand weight of one kind, with the product that drives it.
#[derive(Debug, Clone, PartialEq)]
pub struct KindDemand {
    pub kind: String,
    pub weight: f64,
    pub top_item: Option<String>,
}

/// Every product a kind's ladder can make.
pub fn products(kind: &str) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for rung in spec(kind).map(|entry| entry.ladder).unwrap_or(&[]) {
        if let Then::Produce(items, _, _) = rung.then {
            for item in items {
                if !out.contains(item) {
                    out.push(item);
                }
            }
        }
    }
    out
}

/// Kind that a skill's work serves (chain and method profits).
pub fn kind_for_skill(skill: &str) -> Option<&'static str> {
    kind_for_group(group_of_skill(skill)?)
}

/// Skill a method trains (`smith:x` smithing, `gather:mining/x` mining).
pub fn method_skill(method: &str) -> Option<&'static str> {
    let (head, tail) = method.split_once([':', '/']).unwrap_or((method, ""));
    Some(match head {
        "gather" => match tail.split('/').next().unwrap_or("") {
            "fishing" => "fishing",
            "woodcut" => "woodcutting",
            "mining" => "mining",
            _ => "crafting",
        },
        "smith" | "smelt" => "smithing",
        "cook" | "bake" => "cooking",
        "fire" => "firemaking",
        "spin" | "jewelry" | "string" | "pottery" | "craft" | "tan" | "cut" => "crafting",
        "runecraft" | "essence" => "runecraft",
        "enchant" | "alch" | "superheat" | "telegrab" | "teleport" | "curse" | "bananas" => "magic",
        "bury" => "prayer",
        "fight" => match tail.split('/').next().unwrap_or("") {
            "ranged" => "ranged",
            "magic" => "magic",
            _ => "attack",
        },
        _ => return None,
    })
}

/// Share (0..1) of the guide catalogue's activities training `skill`
/// (the skill of their game-data way: `method_way`, else `spell_way`)
/// that BUY-ABLE inputs feed: a transformation of stock
/// (`guide_methods::transforms`: recipe, alchemy, burning, burying,
/// runecraft) or a spell burning its runes. 0 for a skill no such way
/// trains (fights, harvests). Computed from the game data, memoised
/// (user lever 2026-11-17: the ambitions of a market profile lean on the
/// skills a player trains by buying).
pub fn transform_share_of_skill(skill: &str) -> f64 {
    static MEMO: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, f64>>> = std::sync::OnceLock::new();
    crate::guide_methods::memo(&MEMO, skill.to_string(), || {
        let mut trained = 0usize;
        let mut fed = 0usize;
        for entry in crate::guide_methods::catalogue() {
            let way = crate::method_way::method_way(&entry.method).or_else(|| crate::spell_methods::spell_way(&entry.method));
            let Some((_, way)) = way else { continue };
            if way.skill.as_ref().is_none_or(|(name, _)| name != skill) {
                continue;
            }
            trained += 1;
            let spell = way.kind == lifescape_gamedata::obtain::WayKind::Spell && !way.inputs.is_empty();
            if crate::guide_methods::transforms(&entry.method) || spell {
                fed += 1;
            }
        }
        if trained == 0 {
            0.0
        } else {
            fed as f64 / trained as f64
        }
    })
}

/// A kind's transformation share: the highest of its primary group's
/// skills (`transform_share_of_skill`; melee, ranged, harvests 0; smith,
/// cook, firemaker, crafter, priest, mage, runecrafter high).
pub fn transform_share(kind: &str) -> f64 {
    primary_group(kind).map(|group| group_skills(group).into_iter().map(transform_share_of_skill).fold(0.0, f64::max)).unwrap_or(0.0)
}

/// Highest level a kind's ladder climbs `skill` to (0: never). A bot
/// whose ambition climbs a skill values that XP: training it for a lot is
/// no cost of the lot (`market_value::training_share`).
pub fn ladder_level(kind: &str, skill: &str) -> u8 {
    spec(kind).map(|entry| entry.ladder.iter().filter(|rung| rung.skill == skill).map(|rung| rung.level).max().unwrap_or(0)).unwrap_or(0)
}

/// How much a kind answers the demand for `item`: 1 when its ladder lists
/// the object or its primary skill makes it at any tier (adamant armour
/// out of every bot's reach still calls for smiths: « devenir forgeron
/// parce que l'adamant se vend », 2026-11-14); half per recipe step when
/// its skill makes an input (the ores and coal of a bar: miners); else 0.
pub fn serves(kind: &str, item: &str) -> f64 {
    if products(kind).contains(&item) {
        return 1.0;
    }
    let Some(group) = primary_group(kind) else { return 0.0 };
    let skills = group_skills(group);
    serves_at(&skills, item, 0)
}

fn serves_at(skills: &[&str], item: &str, depth: u8) -> f64 {
    let mut best: f64 = 0.0;
    for leg in crate::obtain_legs::production_legs(item) {
        if leg.skill.as_ref().is_some_and(|(skill, _)| skills.contains(&skill.as_str())) {
            return 0.5f64.powi(i32::from(depth));
        }
        if depth < 2 {
            for (input, _) in &leg.inputs {
                best = best.max(serves_at(skills, input, depth + 1));
            }
        }
    }
    best
}

/// Demand weight per drawable kind: 1 + share of the open-bid value it
/// answers (`serves`: its own products at any tier in full, the inputs
/// its skill makes at half per step; 0..n) + its best profit relative to
/// the fleet's best (0..1, chain evals and measured methods, `(kind,
/// profit)`). A bid whose known margin is a loss pulls nobody. Keeps the
/// draw random, tilted toward what buyers wait for and what pays.
pub fn kind_weights(items: &[DemandItem], kind_profits: &[(String, f64)]) -> Vec<KindDemand> {
    let wanted_items: Vec<&DemandItem> = items.iter().filter(|wanted| wanted.weight > 0.0 && wanted.margin.is_none_or(|margin| margin > 0.0)).collect();
    let top_weight = wanted_items.iter().map(|entry| entry.weight).fold(0.0, f64::max);
    let top_profit = kind_profits.iter().map(|(_, profit)| *profit).fold(0.0, f64::max);
    let mut out = Vec::new();
    for entry in specs().iter().filter(|entry| !crate::ambition::ladderless(entry.kind)) {
        let mut demand = 0.0;
        let mut top: Option<(&DemandItem, f64)> = None;
        for wanted in &wanted_items {
            let answer = serves(entry.kind, &wanted.item);
            if answer <= 0.0 {
                continue;
            }
            let share = if top_weight > 0.0 { answer * wanted.weight / top_weight } else { 0.0 };
            demand += share;
            if top.is_none_or(|(_, known)| share > known) {
                top = Some((wanted, share));
            }
        }
        let profit = kind_profits.iter().filter(|(kind, _)| kind == entry.kind).map(|(_, profit)| *profit).fold(0.0, f64::max);
        let profit_share = if top_profit > 0.0 { profit.max(0.0) / top_profit } else { 0.0 };
        out.push(KindDemand { kind: entry.kind.to_string(), weight: 1.0 + demand + profit_share, top_item: top.map(|(wanted, _)| wanted.item.clone()) });
    }
    out
}

/// Product of one rung: the candidate with the heaviest open demand,
/// else a seeded pick (bots of one kind spread over the candidates).
pub fn pick_product(items: &'static [&'static str], seed: u64, demand: &[DemandItem]) -> &'static str {
    let wanted = items
        .iter()
        .filter_map(|item| demand.iter().find(|entry| entry.item == *item && entry.weight > 0.0).map(|entry| (*item, entry.weight)))
        .max_by(|left, right| left.1.total_cmp(&right.1));
    if let Some((item, _)) = wanted {
        return item;
    }
    let mut word = seed.max(1);
    word ^= word << 13;
    word ^= word >> 7;
    word ^= word << 17;
    items.get((word % items.len().max(1) as u64) as usize).copied().unwrap_or("")
}
