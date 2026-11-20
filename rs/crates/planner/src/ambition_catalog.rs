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
        ("alcher", "alchimiste : natures et objets achetés au marché, alchés du plus rentable au moins rentable", &[], Empty),
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

/// What a kind's own skills produce in one step (a leg no other way
/// dominates, `dominated`) for an open bid beyond its ladder (user 2026-11-15: "motive les bots a produire les
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
    undominated_legs(item).into_iter().find_map(|leg| {
        let (skill, level) = leg.skill.clone()?;
        let own = skills.contains(&skill.as_str()) && leg.inputs.iter().all(|(input, _)| gathered(input));
        own.then(|| (skill, u8::try_from(level.clamp(1, 99)).unwrap_or(99)))
    })
}

/// True when the kind's ladder lists `item` as a production quota.
pub fn ladder_offers(kind: &str, item: &str) -> bool {
    spec(kind).is_some_and(|found| found.ladder.iter().any(|rung| matches!(rung.then, Then::Produce(items, _, _) if items.contains(&item))))
}

/// True when a skill of the kind's production rungs makes `item` at any
/// level (`ambition_ladder::products_of`): the reach of a demand quota
/// (`ambition::insert_demand_milestone` draws among these products). The
/// catalogue sync keeps what the draw may insert: live 2026-11-20
/// noah_hill (crafter) had a strung diamond amulet quota inserted then
/// dropped every pass, 103 stringing launches in 30 minutes.
pub fn ladder_skill_makes(kind: &str, item: &str) -> bool {
    let Some(found) = spec(kind) else { return false };
    let mut skills: Vec<&str> = Vec::new();
    for rung in found.ladder {
        if let Then::Produce(_, _, skill) = rung.then {
            if !skills.contains(&skill) {
                skills.push(skill);
            }
        }
    }
    skills.into_iter().any(|skill| crate::ambition_ladder::products_of(skill).iter().any(|product| product.item == item))
}

/// Skill-fit of a kind for a bot (read only by the level-led draw and
/// coverage of the D3 exceptions, `ambition_fleet::level_led`): (1 + lead / 5)², where lead is how far
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
    /// A resting sell offer of the object exists on the public book
    /// (`market_demand`): wanted AND missing from the asks weighs more in
    /// the need (D3: scarcity = unserved bids, missing sell offers).
    pub asked: bool,
}

/// Demand weight of one kind, with the product that drives it.
#[derive(Debug, Clone, PartialEq)]
pub struct KindDemand {
    pub kind: String,
    /// The level-led draw's demand factor (as before D3, never below 1):
    /// 1 + share of the open-bid VALUE it serves + share of the best guide
    /// profit.
    pub weight: f64,
    /// The NEED alone (decision D3, user 2026-11-19) = (share of the
    /// open-bid MASS it serves + share of the surplus its training
    /// consumes) × (1 + its share of the fleet's guide gain)
    /// (`kind_weights_with`); 0 when nothing calls for it.
    /// `ambition_fleet::saturate` divides it by 1 + the kind's holders.
    pub need: f64,
    /// The object of the level-led `weight` (heaviest served bid share).
    pub top_item: Option<String>,
    /// The object that carries the need, when it weighs at least
    /// `NEED_LABEL_FLOOR` of it (`why` `need:<item>`).
    pub need_item: Option<String>,
}

/// An object without any resting sell offer weighs this many times more
/// in the need (D3: scarcity = unserved bids AND missing sell offers).
pub const UNASKED_FACTOR: f64 = 2.0;

/// Share of THE KIND's need one object must carry to name it in the
/// draw's `why` (`need:<item>`; below, `need`): the label names a real
/// driver even among hundreds of bids, never one of many equal crumbs.
pub const NEED_LABEL_FLOOR: f64 = 0.05;

/// Mass of one wanted object in the need (lot 6 review 2026-11-19: one
/// amulet_of_power wish at 1.3 M a unit was 72 % of all bid value):
/// ln(1 + ln(1 + weight)), so a bid a million times heavier weighs about
/// 1.5 × an ordinary one (weight 1 400 → 2.1, 4.7 G → 3.1, 1 → 0.5);
/// × `UNASKED_FACTOR` when no sell offer rests for it. A plain
/// ln(1 + weight) still gave a 78 M bid ~2.6 ordinary bids.
pub fn need_mass(item: &DemandItem) -> f64 {
    let compressed = compress(item.weight);
    if item.asked {
        compressed
    } else {
        compressed * UNASKED_FACTOR
    }
}

/// ln(1 + ln(1 + value)) of a non-negative value (a bid's weight, a
/// kind's guide profit): a million times more weighs ~1.5 times more.
pub fn compress(value: f64) -> f64 {
    (1.0 + (1.0 + value.max(0.0)).ln()).ln()
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

/// How much a kind answers the demand for `item` (only through legs no
/// other way dominates, `dominated`: superheat never makes a mage answer
/// the bar demand, user 2026-11-19): 1 when its ladder lists
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

/// A production leg DOMINATED by another way to the same object (user
/// decision 2026-11-19, « pourtant ça coûte rien de faire fondre aux
/// fours »): another leg of `legs` that a SKILL makes needs a subset of
/// its inputs, each in a quantity at most its own, and it needs strictly
/// more (an extra input or a larger quantity). A ground spawn or a drop
/// farm (no skill, no input) dominates nothing: it is no way to MAKE the
/// object (it would otherwise strip every rune, smithed piece and bar). Superheat (ore + coal + nature rune) is
/// dominated by the furnace (ore + coal): its skill does not serve the
/// bar. Pure game data (inputs only, no price); equal input lists never
/// dominate each other.
pub fn dominated(leg: &crate::obtain_legs::ProductionLeg, legs: &[crate::obtain_legs::ProductionLeg]) -> bool {
    let quantity = |inputs: &[(String, i64)], item: &str| inputs.iter().filter(|(name, _)| name == item).map(|(_, count)| *count).sum::<i64>();
    legs.iter().filter(|other| other.skill.is_some()).any(|other| {
        let within = other.inputs.iter().all(|(item, _)| quantity(&other.inputs, item) <= quantity(&leg.inputs, item));
        let more = leg.inputs.iter().any(|(item, _)| quantity(&leg.inputs, item) > quantity(&other.inputs, item));
        within && more
    })
}

/// The production legs of `item` that no other leg dominates (`dominated`).
fn undominated_legs(item: &str) -> Vec<crate::obtain_legs::ProductionLeg> {
    let legs = crate::obtain_legs::production_legs(item);
    legs.iter().filter(|leg| !dominated(leg, &legs)).cloned().collect()
}

fn serves_at(skills: &[&str], item: &str, depth: u8) -> f64 {
    let mut best: f64 = 0.0;
    for leg in undominated_legs(item) {
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

/// Demand weight per drawable kind (no surplus known): `kind_weights_with`.
pub fn kind_weights(items: &[DemandItem], kind_profits: &[(String, f64)]) -> Vec<KindDemand> {
    kind_weights_with(items, kind_profits, &[])
}

/// What a kind's training consumes, with how fast: (object, units per
/// 6000 ticks of the kind's FASTEST method burning it), read from the
/// guide catalogue: every training method whose band skill falls to the
/// kind (`activity::band_skill`, `kind_for_skill`) and that no other way
/// dominates (`dominated_method`: superheat), its game prior's flows
/// at its gate level (`work_rate::prior_rate`) consumed under
/// `work_rate::CONSUMED_CAUSES` (inputs, runes, ammunition, bait,
/// supplies): logs for the firemaker, bones for the priest, runes and
/// alched objects for the mage, arrows for the ranger, raw fish for the
/// cook, ore and bars for the smith, essence for the runecrafter, feathers
/// and bait for the fisher. Never a hand list; memoised (static game
/// data), sorted by object.
pub fn consumption_of(kind: &str) -> Vec<(String, f64)> {
    static MEMO: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Vec<(String, f64)>>>> = std::sync::OnceLock::new();
    crate::guide_methods::memo(&MEMO, kind.to_string(), || {
        let mut out: Vec<(String, f64)> = Vec::new();
        for entry in crate::guide_methods::catalogue() {
            let Some(skill) = crate::activity::band_skill(&entry.method, &entry.style) else { continue };
            if kind_for_skill(&skill) != Some(kind) || dominated_method(&entry.method) {
                continue;
            }
            let Some(rate) = crate::work_rate::prior_rate(&entry.method, crate::guide_methods::gate_level(&entry.method)) else { continue };
            for (item, cause, quantity) in &rate.flows_6000 {
                if *quantity >= 0.0 || item == "coins" || !crate::work_rate::CONSUMED_CAUSES.contains(&cause.as_str()) {
                    continue;
                }
                match out.iter_mut().find(|(known, _)| known == item) {
                    Some(known) => known.1 = known.1.max(-quantity),
                    None => out.push((item.clone(), -quantity)),
                }
            }
        }
        out.sort_by(|left, right| left.0.cmp(&right.0));
        out
    })
}

/// True when a training method is a production leg another way to the
/// same object dominates (`dominated`: superheat by the furnace, user
/// 2026-11-19 « ça coûte rien de faire fondre aux fours »): its inputs are
/// no outlet of its kind (the ore piling up calls for smiths, never mages).
fn dominated_method(method: &str) -> bool {
    let Some((product, _)) = crate::method_way::method_way(method) else { return false };
    let legs = crate::obtain_legs::production_legs(&product);
    legs.iter().find(|leg| leg.method == method).is_some_and(|leg| dominated(leg, &legs))
}

/// Objects a kind's training consumes (`consumption_of`, names only).
pub fn consumed_by(kind: &str) -> Vec<String> {
    consumption_of(kind).into_iter().map(|(item, _)| item).collect()
}

/// Training windows of surplus a kind can burn: Σ over the objects its
/// training consumes of (units resting on the asks ÷ units its fastest
/// method burns per 6000 ticks). Windows of different objects add up (a
/// window is a window of work); raw units never do (relecture tour 2:
/// feathers and plates summed as units made no sense).
pub fn outlet_windows(kind: &str, surplus: &[(String, f64)]) -> f64 {
    consumption_of(kind)
        .iter()
        .filter(|(_, per_6000)| *per_6000 > 0.0)
        .map(|(item, per_6000)| surplus.iter().filter(|(known, _)| known == item).map(|(_, units)| units.max(0.0)).sum::<f64>() / per_6000)
        .sum()
}

/// Demand weight per drawable kind.
///
/// `weight` (the level-led draw of the D3 exceptions, unchanged): 1 +
/// share of the open-bid value it answers (`serves`: its own products at
/// any tier in full, the inputs its skill makes at half per step; 0..n) +
/// its best profit relative to the fleet's best (0..1).
///
/// `need` (decision D3) = (shortfall + outlet) × (1 + gain share):
/// - shortfall: the share of the open-bid MASS it serves (`need_mass`,
///   robust to one outlier, doubled without asks; a loss pulls nobody);
/// - outlet (user 2026-11-19, « la prière, l'allumage de feux, la magie,
///   le tir à distance font disparaître des objets »): the surplus it can
///   burn in TRAINING WINDOWS (`outlet_windows`: per object, units on the
///   asks ÷ what its fastest method burns per 6000 ticks, summed),
///   ln(1 + windows), over the sum of every kind's (`surplus`: (object,
///   units on the asks), market makers' quotes left out);
/// - gain share: its best guide profit over the fleet's summed profits,
///   linear, read once (the profits come bounded by what can really be
///   bought, `brain_d17::kind_profits`: no outlier left to compress).
/// Shortfall and outlet each weigh 1 over the whole fleet; the gain only
/// TILTS them (×1..×2): a kind nobody calls for and whose inputs do not
/// pile up has no need whatever it earns (review 2026-11-19: the gain
/// added on its own gave 0.6..1 to every kind against bids of at most
/// 0.14, a flat need). Keeps the draw random, tilted toward what buyers
/// wait for, what piles up unused, and what pays.
pub fn kind_weights_with(items: &[DemandItem], kind_profits: &[(String, f64)], surplus: &[(String, f64)]) -> Vec<KindDemand> {
    let wanted_items: Vec<&DemandItem> = items.iter().filter(|wanted| wanted.weight > 0.0 && wanted.margin.is_none_or(|margin| margin > 0.0)).collect();
    let top_weight = wanted_items.iter().map(|entry| entry.weight).fold(0.0, f64::max);
    let top_profit = kind_profits.iter().map(|(_, profit)| *profit).fold(0.0, f64::max);
    let total_mass: f64 = wanted_items.iter().map(|wanted| need_mass(wanted)).sum();
    let kinds: Vec<&'static str> = specs().iter().filter(|entry| !crate::ambition::ladderless(entry.kind)).map(|entry| entry.kind).collect();
    let profit_of = |kind: &str| kind_profits.iter().filter(|(known, _)| known == kind).map(|(_, profit)| *profit).fold(0.0, f64::max);
    // ONE compression, ln(1 + windows): live the backlogs span four
    // orders of magnitude (1 256 bones under one window of burying,
    // 639 000 feathers ~hundreds of fly fishing): linear, one object would
    // own the whole outlet signal; ln keeps the order (more windows, more
    // need) while every pile still counts.
    let outlet_of = |kind: &str| if surplus.is_empty() { 0.0 } else { outlet_windows(kind, surplus).ln_1p() };
    let gains: Vec<f64> = kinds.iter().map(|kind| profit_of(kind).max(0.0)).collect();
    let outlets: Vec<f64> = kinds.iter().map(|kind| outlet_of(kind)).collect();
    let (total_gain, total_outlet) = (gains.iter().sum::<f64>(), outlets.iter().sum::<f64>());
    let mut out = Vec::new();
    for (index, kind) in kinds.iter().enumerate() {
        let mut demand = 0.0;
        let mut top: Option<(&DemandItem, f64)> = None;
        let mut offer = 0.0;
        let mut driver: Option<(&DemandItem, f64)> = None;
        for wanted in &wanted_items {
            let answer = serves(kind, &wanted.item);
            if answer <= 0.0 {
                continue;
            }
            let share = if top_weight > 0.0 { answer * wanted.weight / top_weight } else { 0.0 };
            demand += share;
            if top.is_none_or(|(_, known)| share > known) {
                top = Some((wanted, share));
            }
            let part = if total_mass > 0.0 { answer * need_mass(wanted) / total_mass } else { 0.0 };
            offer += part;
            if driver.is_none_or(|(_, known)| part > known) {
                driver = Some((wanted, part));
            }
        }
        let profit = profit_of(kind);
        let profit_share = if top_profit > 0.0 { profit.max(0.0) / top_profit } else { 0.0 };
        let gain = if total_gain > 0.0 { gains[index] / total_gain } else { 0.0 };
        let outlet = if total_outlet > 0.0 { outlets[index] / total_outlet } else { 0.0 };
        let need = (offer + outlet) * (1.0 + gain);
        let need_item = driver.filter(|(_, part)| need > 0.0 && *part >= NEED_LABEL_FLOOR * need).map(|(wanted, _)| wanted.item.clone());
        out.push(KindDemand { kind: kind.to_string(), weight: 1.0 + demand + profit_share, need, top_item: top.map(|(wanted, _)| wanted.item.clone()), need_item });
    }
    out
}

/// Product of one rung: the candidate with the heaviest open demand,
/// else a seeded pick (bots of one kind spread over the candidates).
pub fn pick_product(items: &[&'static str], seed: u64, demand: &[DemandItem]) -> &'static str {
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
