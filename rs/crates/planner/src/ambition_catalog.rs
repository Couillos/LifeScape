//! Ambition catalogue: one entry per F2P skill group of rev 274 (melee
//! counts as ONE group: attack, strength, defence, hitpoints), plus the
//! merchant and the operator-only generalist. Each entry is a ladder of
//! rungs: a level to reach, then a production quota (several candidate
//! products per rung, so bots of one kind do not all make the same
//! thing), an equipment tier or a coin target. Products were checked
//! against the obtain index (`orchestrator obtain <item>`) and are
//! asserted to map to a production leg in the tests.
//!
//! Also here: skill fit (existing levels map to a sensible ambition) and
//! the demand weights per kind (open bids, chain and method profits).

/// The 12 F2P skill groups of rev 274 (fletching is members-only).
pub const GROUPS: [&str; 12] =
    ["melee", "ranged", "prayer", "magic", "cooking", "woodcutting", "fishing", "firemaking", "crafting", "smithing", "mining", "runecraft"];
/// The trading role every fleet holds at least once.
pub const MERCHANT: &str = "merchant";

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

const fn level_rung(skill: &'static str, level: u8) -> Rung {
    Rung { skill, level, then: Then::Nothing }
}
const fn make(skill: &'static str, level: u8, items: &'static [&'static str], count: i64) -> Rung {
    Rung { skill, level, then: Then::Produce(items, count, skill) }
}
const fn gear(skill: &'static str, level: u8, style: &'static str) -> Rung {
    Rung { skill, level, then: Then::Equip(style) }
}

const WARRIOR: &[Rung] = &[
    level_rung("attack", 20), level_rung("strength", 20), gear("defence", 20, "melee"),
    make("attack", 30, &["cow_hide", "bones"], 50), level_rung("strength", 30), gear("defence", 30, "melee"),
    level_rung("attack", 40), level_rung("strength", 40), gear("defence", 40, "melee"),
    level_rung("attack", 50), level_rung("strength", 50), gear("defence", 50, "melee"),
    level_rung("attack", 60), level_rung("strength", 60), gear("defence", 60, "melee"),
];
const RANGER: &[Rung] = &[
    gear("ranged", 10, "ranged"), level_rung("ranged", 20), gear("ranged", 30, "ranged"), level_rung("defence", 20),
    level_rung("ranged", 40), gear("ranged", 50, "ranged"), level_rung("ranged", 60), gear("ranged", 70, "ranged"),
];
const MAGE: &[Rung] = &[
    gear("magic", 10, "magic"), make("magic", 20, &["amulet_of_magic"], 10), gear("magic", 30, "magic"),
    level_rung("magic", 40), make("magic", 45, &["amulet_of_magic"], 25), gear("magic", 50, "magic"), level_rung("magic", 60),
];
const PRIEST: &[Rung] = &[
    Rung { skill: "prayer", level: 10, then: Then::Produce(&["bones"], 100, "attack") }, level_rung("attack", 20),
    gear("prayer", 20, "melee"), level_rung("defence", 30), level_rung("prayer", 30), gear("prayer", 40, "melee"), level_rung("prayer", 43), level_rung("prayer", 50),
];
// Lobster, tuna and swordfish are Karamja water (`access::BOAT_ROUTED`:
// no nav edge for the sailors) and maples have no F2P tree: no quota names
// them until they are plannable (audit 2026-11-13). Composite food
// (`bake:`: pies, pizza, cake) carries the cook past salmon.
const COOK: &[Rung] = &[
    make("cooking", 10, &["shrimp", "anchovies", "sardine", "cooked_chicken", "cooked_meat"], 50),
    make("cooking", 20, &["trout", "herring", "pike", "bread", "redberry_pie"], 50), level_rung("fishing", 20),
    make("cooking", 30, &["salmon", "pike", "meat_pie", "stew"], 50), make("cooking", 40, &["cake", "plain_pizza", "apple_pie", "salmon"], 50),
    make("cooking", 50, &["cake", "salmon"], 50), make("cooking", 60, &["cake", "salmon"], 100),
];
const FISHER: &[Rung] = &[
    make("fishing", 10, &["raw_shrimp", "raw_anchovies"], 100), make("fishing", 20, &["raw_sardine", "raw_herring", "raw_trout"], 100),
    make("fishing", 30, &["raw_trout", "raw_salmon", "raw_pike"], 100), make("fishing", 40, &["raw_salmon", "raw_pike"], 150),
    make("fishing", 50, &["raw_salmon"], 150), make("fishing", 60, &["raw_salmon", "raw_trout"], 200), level_rung("fishing", 70),
];
const WOODCUTTER: &[Rung] = &[
    make("woodcutting", 15, &["logs"], 100), make("woodcutting", 30, &["oak_logs"], 100), make("woodcutting", 45, &["willow_logs"], 150),
    make("woodcutting", 60, &["yew_logs", "willow_logs"], 150), make("woodcutting", 75, &["yew_logs"], 100), make("woodcutting", 90, &["yew_logs"], 200),
];
const FIREMAKER: &[Rung] = &[
    level_rung("woodcutting", 15), level_rung("firemaking", 15), Rung { skill: "firemaking", level: 30, then: Then::Produce(&["oak_logs", "logs"], 100, "woodcutting") },
    level_rung("woodcutting", 30), level_rung("firemaking", 45), Rung { skill: "firemaking", level: 60, then: Then::Produce(&["willow_logs"], 100, "woodcutting") },
    level_rung("firemaking", 75), level_rung("firemaking", 90),
];
const CRAFTER: &[Rung] = &[
    make("crafting", 5, &["ball_of_wool"], 100), make("crafting", 10, &["leather_gloves", "pot_empty", "gold_ring"], 30),
    make("crafting", 16, &["leather_boots", "leather_cowl", "piedish", "unstrung_gold_amulet"], 30),
    make("crafting", 20, &["leather_vambraces", "leather_armour", "bowl_empty", "strung_gold_amulet"], 30),
    make("crafting", 30, &["leather_chaps", "sapphire_ring"], 20), level_rung("crafting", 40), level_rung("crafting", 50),
];
const SMITH: &[Rung] = &[
    level_rung("mining", 10), make("smithing", 10, &["bronze_dagger", "bronze_sword", "bronze_med_helm"], 20),
    make("smithing", 18, &["bronze_platebody", "bronze_platelegs", "bronze_kiteshield"], 10), level_rung("mining", 20),
    make("smithing", 25, &["iron_dagger", "iron_scimitar", "iron_full_helm"], 20),
    make("smithing", 33, &["iron_platebody", "iron_platelegs", "iron_kiteshield"], 10), level_rung("mining", 30),
    make("smithing", 40, &["steel_dagger", "steel_sword", "steel_full_helm"], 20), make("smithing", 48, &["steel_platebody", "steel_platelegs"], 10),
    level_rung("mining", 55), make("smithing", 58, &["mithril_dagger", "mithril_full_helm"], 10), make("smithing", 68, &["mithril_platebody", "mithril_platelegs"], 5),
];
const MINER: &[Rung] = &[
    make("mining", 10, &["copper_ore", "tin_ore", "clay"], 100), make("mining", 20, &["iron_ore", "tin_ore"], 100),
    make("mining", 30, &["coal", "iron_ore"], 150), make("mining", 40, &["gold_ore", "coal"], 100),
    make("mining", 55, &["mithril_ore", "coal"], 100), level_rung("mining", 70), level_rung("mining", 85),
];
const RUNECRAFTER: &[Rung] = &[
    Rung { skill: "runecraft", level: 0, then: Then::Produce(&["blankrune"], 200, "mining") },
    make("runecraft", 5, &["airrune", "mindrune"], 200), make("runecraft", 10, &["waterrune", "earthrune"], 200),
    make("runecraft", 20, &["firerune", "bodyrune"], 200), make("runecraft", 30, &["bodyrune", "firerune", "earthrune"], 300),
    level_rung("runecraft", 40), level_rung("runecraft", 50),
];
const MERCHANT_LADDER: &[Rung] = &[
    Rung { skill: "", level: 0, then: Then::Coins(5_000) },
    Rung { skill: "", level: 0, then: Then::Coins(20_000) },
    Rung { skill: "", level: 0, then: Then::Coins(50_000) },
    Rung { skill: "", level: 0, then: Then::Coins(100_000) },
    Rung { skill: "", level: 0, then: Then::Coins(250_000) },
    Rung { skill: "", level: 0, then: Then::Coins(500_000) },
    Rung { skill: "", level: 0, then: Then::Coins(1_000_000) },
];

/// Every ambition the fleet draws from, plus the operator-only generalist.
pub static SPECS: &[Spec] = &[
    Spec { kind: "warrior", label: "guerrier : mêlée et armures", groups: &["melee"], ladder: WARRIOR },
    Spec { kind: "ranger", label: "archer : distance et équipement", groups: &["ranged", "melee"], ladder: RANGER },
    Spec { kind: "priest", label: "prêtre : prière, os enterrés", groups: &["prayer", "melee"], ladder: PRIEST },
    Spec { kind: "mage", label: "mage : magie, amulettes enchantées", groups: &["magic"], ladder: MAGE },
    Spec { kind: "cook", label: "cuisinier : poissons et viandes cuits", groups: &["cooking", "fishing"], ladder: COOK },
    Spec { kind: "woodcutter", label: "bûcheron : fournisseur de bûches", groups: &["woodcutting"], ladder: WOODCUTTER },
    Spec { kind: "fisher", label: "pêcheur : fournisseur de poisson cru", groups: &["fishing"], ladder: FISHER },
    Spec { kind: "firemaker", label: "allumeur : feux, bûches achetées ou coupées", groups: &["firemaking", "woodcutting"], ladder: FIREMAKER },
    Spec { kind: "crafter", label: "artisan : cuir, poterie, bijoux, laine", groups: &["crafting"], ladder: CRAFTER },
    Spec { kind: "smith", label: "forgeron : armes et armures de métal", groups: &["smithing", "mining"], ladder: SMITH },
    Spec { kind: "miner", label: "mineur : fournisseur de minerai", groups: &["mining"], ladder: MINER },
    Spec { kind: "runecrafter", label: "runier : essence et runes", groups: &["runecraft", "mining"], ladder: RUNECRAFTER },
    Spec { kind: "merchant", label: "marchand : achète bas, revend haut", groups: &["merchant"], ladder: MERCHANT_LADDER },
    Spec { kind: "generalist", label: "généraliste : compétence la plus basse", groups: &[], ladder: &[] },
];

pub fn spec(kind: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|entry| entry.kind == kind)
}

/// The ambition covering a group (primary group match).
pub fn kind_for_group(group: &str) -> Option<&'static str> {
    SPECS.iter().find(|entry| entry.groups.first() == Some(&group)).map(|entry| entry.kind)
}

/// Primary group of a kind (`merchant` for the merchant, `None` for the
/// generalist or an unknown kind).
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

/// Skill-fit of a kind for a bot: (1 + lead / 5)², where lead is how far
/// the best skill of its primary group stands above the bot's mean over
/// the groups (frank_lane woodcutting 64 among levels ~10: ~120 for the
/// woodcutter, 1 elsewhere). Fresh bots fit everything at 1.
pub fn fit(kind: &str, level_of: &dyn Fn(&str) -> u8) -> f64 {
    let Some(group) = primary_group(kind) else { return 0.0 };
    if group == MERCHANT {
        return 1.0;
    }
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
        "enchant" | "alch" | "superheat" | "telegrab" => "magic",
        "bury" => "prayer",
        "fight" => match tail.split('/').next().unwrap_or("") {
            "ranged" => "ranged",
            "magic" => "magic",
            _ => "attack",
        },
        _ => return None,
    })
}

/// Demand weight per drawable kind: 1 + share of the open-bid value its
/// products answer (0..n) + its best profit relative to the fleet's best
/// (0..1, chain evals and measured methods, `(kind, profit)`). Keeps the
/// draw random, tilted toward what buyers wait for and what pays.
pub fn kind_weights(items: &[DemandItem], kind_profits: &[(String, f64)]) -> Vec<KindDemand> {
    let top_weight = items.iter().map(|entry| entry.weight).fold(0.0, f64::max);
    let top_profit = kind_profits.iter().map(|(_, profit)| *profit).fold(0.0, f64::max);
    let mut out = Vec::new();
    for entry in SPECS.iter().filter(|entry| entry.kind != crate::ambition::GENERALIST) {
        let made = products(entry.kind);
        let mut demand = 0.0;
        let mut top: Option<(&DemandItem, f64)> = None;
        for wanted in items.iter().filter(|wanted| made.contains(&wanted.item.as_str()) && wanted.weight > 0.0) {
            let share = if top_weight > 0.0 { wanted.weight / top_weight } else { 0.0 };
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
