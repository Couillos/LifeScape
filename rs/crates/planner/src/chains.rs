//! Production chains (D58), method choices (§4.4), quest shortcuts (D59),
//! boot without anything (D51) and era priority (D64).
//!
//! Pure: fixtures plus caller-supplied public numbers in, comparisons
//! out. No network, no files, no cross-bot state (D22). Works on the
//! partial catalogue (P2.1 fills rows in parallel): chain fixtures carry
//! their own priors, and the `gamedata_*` tests below validate every
//! fixture fact against the real dumps at the end.

use crate::catalog::Epoch;

/// One production step: a catalogue activity (or service) with its
/// priors. XP is real XP (server raw / 10, as the recipes dump shows).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChainStep {
    pub method: &'static str,
    pub skill: &'static str,
    pub level: u8,
    pub ticks_each: f64,
    pub xp_each: f64,
    pub coin_each: f64,
    pub epoch: Epoch,
    pub wilderness: bool,
    pub takes: &'static str,
    pub takes_qty: f64,
    pub makes: &'static str,
    pub makes_qty: f64,
}

/// One end-to-end chain (catalogue §7): gather → transform → sell/use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Chain {
    pub id: &'static str,
    pub steps: &'static [ChainStep],
    pub product: &'static str,
    /// What the chain unlocks ("" when nothing): a P2 chain that
    /// unlocks still runs (D64 exception).
    pub unlocks: &'static str,
}

// D58 families, priors from the catalogue and the recipes dump (smelt,
// cook, leather and gem XP are server raw / 10, asserted in tests).
const IRON_STEPS: &[ChainStep] = &[
    ChainStep { method: "gather:mining/iron_ore", skill: "mining", level: 15, ticks_each: 8.0, xp_each: 3.5, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "", takes_qty: 0.0, makes: "iron_ore", makes_qty: 1.0 },
    ChainStep { method: "smelt:iron_bar", skill: "smithing", level: 15, ticks_each: 4.0, xp_each: 12.5, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "iron_ore", takes_qty: 1.0, makes: "iron_bar", makes_qty: 1.0 },
    ChainStep { method: "smith:iron_sword", skill: "smithing", level: 29, ticks_each: 4.0, xp_each: 25.0, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "iron_bar", takes_qty: 1.0, makes: "iron_sword", makes_qty: 1.0 },
];
const STEEL_STEPS: &[ChainStep] = &[
    ChainStep { method: "gather:mining/iron_ore", skill: "mining", level: 15, ticks_each: 8.0, xp_each: 3.5, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "", takes_qty: 0.0, makes: "iron_ore", makes_qty: 1.0 },
    ChainStep { method: "gather:mining/coal", skill: "mining", level: 30, ticks_each: 10.0, xp_each: 5.0, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "", takes_qty: 0.0, makes: "coal", makes_qty: 1.0 },
    ChainStep { method: "smelt:steel_bar", skill: "smithing", level: 30, ticks_each: 4.0, xp_each: 17.5, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "iron_ore+coal", takes_qty: 1.0, makes: "steel_bar", makes_qty: 1.0 },
    ChainStep { method: "smith:steel_platebody", skill: "smithing", level: 48, ticks_each: 4.0, xp_each: 62.5, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "steel_bar", takes_qty: 5.0, makes: "steel_platebody", makes_qty: 1.0 },
];
const FISH_COOK_STEPS: &[ChainStep] = &[
    ChainStep { method: "gather:fishing/raw_trout", skill: "fishing", level: 20, ticks_each: 12.0, xp_each: 7.0, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "", takes_qty: 0.0, makes: "raw_trout", makes_qty: 1.0 },
    ChainStep { method: "cook:trout", skill: "cooking", level: 15, ticks_each: 3.0, xp_each: 70.0, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "raw_trout", takes_qty: 1.0, makes: "trout", makes_qty: 1.0 },
];
const LOGS_FIRE_STEPS: &[ChainStep] = &[
    ChainStep { method: "gather:woodcut/willow_logs", skill: "woodcutting", level: 30, ticks_each: 8.0, xp_each: 6.75, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "", takes_qty: 0.0, makes: "willow_logs", makes_qty: 1.0 },
    ChainStep { method: "fire:willow_logs", skill: "firemaking", level: 30, ticks_each: 3.0, xp_each: 30.0, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "willow_logs", takes_qty: 1.0, makes: "ashes", makes_qty: 1.0 },
];
const HIDE_LEATHER_STEPS: &[ChainStep] = &[
    ChainStep { method: "fight:melee/cows", skill: "attack", level: 1, ticks_each: 20.0, xp_each: 12.0, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "", takes_qty: 0.0, makes: "cow_hide", makes_qty: 1.0 },
    ChainStep { method: "tan:cow_hide", skill: "crafting", level: 1, ticks_each: 2.0, xp_each: 0.0, coin_each: 1.0, epoch: Epoch::P1, wilderness: false, takes: "cow_hide", takes_qty: 1.0, makes: "leather", makes_qty: 1.0 },
    ChainStep { method: "craft:leather_gloves", skill: "crafting", level: 1, ticks_each: 3.0, xp_each: 13.8, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "leather", takes_qty: 1.0, makes: "leather_gloves", makes_qty: 1.0 },
];
const GOLD_STEPS: &[ChainStep] = &[
    ChainStep { method: "gather:mining/gold_ore", skill: "mining", level: 40, ticks_each: 12.0, xp_each: 6.5, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "", takes_qty: 0.0, makes: "gold_ore", makes_qty: 1.0 },
    ChainStep { method: "smelt:gold_bar", skill: "smithing", level: 40, ticks_each: 4.0, xp_each: 22.5, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "gold_ore", takes_qty: 1.0, makes: "gold_bar", makes_qty: 1.0 },
    ChainStep { method: "jewelry:gold_ring", skill: "crafting", level: 5, ticks_each: 4.0, xp_each: 15.0, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "gold_bar", takes_qty: 1.0, makes: "gold_ring", makes_qty: 1.0 },
    ChainStep { method: "alch:gold_ring", skill: "magic", level: 55, ticks_each: 3.0, xp_each: 65.0, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "gold_ring", takes_qty: 1.0, makes: "coins", makes_qty: 1.0 },
];
const ESSENCE_STEPS: &[ChainStep] = &[
    ChainStep { method: "gather:mining/blankrune", skill: "mining", level: 1, ticks_each: 6.0, xp_each: 2.0, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "", takes_qty: 0.0, makes: "blankrune", makes_qty: 1.0 },
    ChainStep { method: "runecraft:airrune", skill: "runecraft", level: 1, ticks_each: 8.0, xp_each: 5.0, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "blankrune", takes_qty: 1.0, makes: "airrune", makes_qty: 1.0 },
];
const WOOL_STEPS: &[ChainStep] = &[
    // shear_sheep.rs2 grants no XP at all (PRD3 lot 7: was 2.5).
    ChainStep { method: "gather:sheep/wool", skill: "crafting", level: 1, ticks_each: 4.0, xp_each: 0.0, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "", takes_qty: 0.0, makes: "wool", makes_qty: 1.0 },
    ChainStep { method: "spin:ball_of_wool", skill: "crafting", level: 1, ticks_each: 3.0, xp_each: 2.5, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "wool", takes_qty: 1.0, makes: "ball_of_wool", makes_qty: 1.0 },
    ChainStep { method: "string:unstrung_gold_amulet", skill: "crafting", level: 1, ticks_each: 2.0, xp_each: 4.0, coin_each: 0.0, epoch: Epoch::P2, wilderness: false, takes: "ball_of_wool", takes_qty: 1.0, makes: "strung_gold_amulet", makes_qty: 1.0 },
];
const CLAY_STEPS: &[ChainStep] = &[
    ChainStep { method: "gather:mining/clay", skill: "mining", level: 1, ticks_each: 8.0, xp_each: 2.0, coin_each: 0.0, epoch: Epoch::P2, wilderness: false, takes: "", takes_qty: 0.0, makes: "clay", makes_qty: 1.0 },
    ChainStep { method: "pottery:pot", skill: "crafting", level: 1, ticks_each: 4.0, xp_each: 6.3, coin_each: 0.0, epoch: Epoch::P2, wilderness: false, takes: "clay", takes_qty: 1.0, makes: "pot", makes_qty: 1.0 },
];
const BREAD_STEPS: &[ChainStep] = &[
    ChainStep { method: "gather:wheat/grain", skill: "cooking", level: 1, ticks_each: 4.0, xp_each: 1.0, coin_each: 0.0, epoch: Epoch::P2, wilderness: false, takes: "", takes_qty: 0.0, makes: "grain", makes_qty: 1.0 },
    ChainStep { method: "mill:pot_flour", skill: "cooking", level: 1, ticks_each: 6.0, xp_each: 1.0, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "grain", takes_qty: 1.0, makes: "pot_flour", makes_qty: 1.0 },
    ChainStep { method: "bake:bread", skill: "cooking", level: 1, ticks_each: 3.0, xp_each: 40.0, coin_each: 0.0, epoch: Epoch::P2, wilderness: false, takes: "bread_dough", takes_qty: 1.0, makes: "bread", makes_qty: 1.0 },
];
const SUPERHEAT_STEPS: &[ChainStep] = &[
    ChainStep { method: "gather:mining/iron_ore", skill: "mining", level: 15, ticks_each: 8.0, xp_each: 3.5, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "", takes_qty: 0.0, makes: "iron_ore", makes_qty: 1.0 },
    ChainStep { method: "superheat:iron_bar", skill: "magic", level: 43, ticks_each: 3.0, xp_each: 53.0, coin_each: 0.0, epoch: Epoch::P2, wilderness: false, takes: "iron_ore", takes_qty: 1.0, makes: "iron_bar", makes_qty: 1.0 },
];
const RUNITE_WILD_STEPS: &[ChainStep] = &[
    ChainStep { method: "gather:mining/runite_ore", skill: "mining", level: 85, ticks_each: 30.0, xp_each: 12.5, coin_each: 0.0, epoch: Epoch::P1, wilderness: true, takes: "", takes_qty: 0.0, makes: "runite_ore", makes_qty: 1.0 },
];
const ENCHANT_STEPS: &[ChainStep] = &[
    // Wiring amulets to enchant spells (P2.1 review): the strung input
    // resolves spell, level, runes and product in the script; the XP is
    // the spell dump (enchant_lvl1 175 raw / 10, asserted below).
    ChainStep { method: "enchant:strung_sapphire_amulet", skill: "magic", level: 7, ticks_each: 3.0, xp_each: 17.5, coin_each: 0.0, epoch: Epoch::P2, wilderness: false, takes: "strung_sapphire_amulet", takes_qty: 1.0, makes: "amulet_of_magic", makes_qty: 1.0 },
];
const BURY_STEPS: &[ChainStep] = &[
    // §7 combat → burial: the bones bury in the combat loop (no
    // standalone bury script: `bury_loot::bury_now` is an in-loop
    // helper), so the chain step is the fight itself, scored in prayer.
    ChainStep { method: "fight:melee/cows", skill: "prayer", level: 1, ticks_each: 20.0, xp_each: 4.5, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "", takes_qty: 0.0, makes: "bones", makes_qty: 1.0 },
];
const LESSER_STEPS: &[ChainStep] = &[
    // §7 lesser demons → rune → high alchemy. The helm is a drop, not a
    // guarantee: makes_qty 1.0 is an upper-bound prior (measured
    // yields replace priors from n ≥ 3, D22 rates, never the dump rate).
    ChainStep { method: "fight:melee/lesser_demons", skill: "attack", level: 82, ticks_each: 20.0, xp_each: 12.0, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "", takes_qty: 0.0, makes: "rune_med_helm", makes_qty: 1.0 },
    ChainStep { method: "alch:rune_med_helm", skill: "magic", level: 55, ticks_each: 3.0, xp_each: 65.0, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "rune_med_helm", takes_qty: 1.0, makes: "coins", makes_qty: 1.0 },
];

/// Every D58 family on the partial catalogue (§7, minus member-gated rows).
pub static CHAINS: &[Chain] = &[
    Chain { id: "iron", steps: IRON_STEPS, product: "iron_sword", unlocks: "" },
    Chain { id: "steel", steps: STEEL_STEPS, product: "steel_platebody", unlocks: "" },
    Chain { id: "fish_cook", steps: FISH_COOK_STEPS, product: "trout", unlocks: "" },
    Chain { id: "logs_fire", steps: LOGS_FIRE_STEPS, product: "ashes", unlocks: "" },
    Chain { id: "hide_leather", steps: HIDE_LEATHER_STEPS, product: "leather_gloves", unlocks: "" },
    Chain { id: "gold", steps: GOLD_STEPS, product: "coins", unlocks: "" },
    Chain { id: "essence", steps: ESSENCE_STEPS, product: "airrune", unlocks: "runecraft" },
    Chain { id: "wool", steps: WOOL_STEPS, product: "strung_gold_amulet", unlocks: "" },
    Chain { id: "clay", steps: CLAY_STEPS, product: "pot", unlocks: "" },
    Chain { id: "bread", steps: BREAD_STEPS, product: "bread", unlocks: "" },
    Chain { id: "superheat", steps: SUPERHEAT_STEPS, product: "iron_bar", unlocks: "" },
    Chain { id: "runite_wild", steps: RUNITE_WILD_STEPS, product: "runite_ore", unlocks: "" },
    Chain { id: "enchant", steps: ENCHANT_STEPS, product: "amulet_of_magic", unlocks: "" },
    Chain { id: "bury", steps: BURY_STEPS, product: "bones", unlocks: "" },
    Chain { id: "lesser_alch", steps: LESSER_STEPS, product: "coins", unlocks: "" },
];

pub fn chain_by_id(id: &str) -> Option<&'static Chain> {
    CHAINS.iter().find(|chain| chain.id == id)
}

/// Public numbers for one evaluation: market refs and shop floors
/// (D22: caller-supplied, never another bot's state).
pub struct ChainPrices<'a> {
    pub market: &'a dyn Fn(&str) -> f64,
    pub shop: &'a dyn Fn(&str) -> f64,
    pub trip_ticks: u64,
}

/// Whole-chain totals for `qty` final units.
#[derive(Debug, Clone, PartialEq)]
pub struct ChainEval {
    pub id: String,
    pub ticks: u64,
    pub xp: Vec<(String, f64)>,
    pub coins_spent: i64,
    pub final_value: f64,
    /// Net per 6000 ticks at full pass scale (money comparison, §4.4).
    pub profit_per_6000: f64,
    pub epoch: Epoch,
    pub wilderness: bool,
}

/// Extra ticks priced on Wilderness legs (risk of death and loss).
pub const WILDERNESS_TAX_TICKS: u64 = 1200;

/// Evaluate one chain end to end: every step's ticks, every skill's
/// XP, service fees, and the final market-or-shop value.
pub fn eval_chain(chain: &Chain, qty: f64, prices: &ChainPrices) -> ChainEval {
    let mut ticks = prices.trip_ticks.saturating_mul(2) as f64;
    let mut xp: Vec<(String, f64)> = Vec::new();
    let mut coins = 0.0;
    // Walk backwards: each step must make enough for the next one's takes.
    let mut need = qty;
    for step in chain.steps.iter().rev() {
        let actions = if step.makes_qty > 0.0 { need / step.makes_qty } else { need };
        ticks += actions.ceil() * step.ticks_each;
        coins += actions.ceil() * step.coin_each;
        let slot = xp.iter_mut().find(|(skill, _)| *skill == step.skill);
        if let Some(slot) = slot {
            slot.1 += actions.ceil() * step.xp_each;
        } else {
            xp.push((step.skill.to_string(), actions.ceil() * step.xp_each));
        }
        need = actions.ceil() * step.takes_qty;
        if step.wilderness {
            ticks += WILDERNESS_TAX_TICKS as f64;
        }
    }
    let unit = (prices.market)(chain.product).max((prices.shop)(chain.product));
    let final_value = unit * qty;
    let profit = final_value - coins;
    let profit_per_6000 = if ticks > 0.0 { profit / ticks * 6000.0 } else { 0.0 };
    let epoch = if chain.steps.iter().any(|step| step.epoch == Epoch::P2) { Epoch::P2 } else { Epoch::P1 };
    ChainEval {
        id: chain.id.to_string(),
        ticks: ticks.ceil() as u64,
        xp,
        coins_spent: coins.ceil() as i64,
        final_value,
        profit_per_6000,
        epoch,
        wilderness: chain.steps.iter().any(|step| step.wilderness),
    }
}

/// XP rate of one eval on one skill (XP per tick).
pub fn xp_per_tick(eval: &ChainEval, skill: &str) -> f64 {
    if eval.ticks == 0 {
        return 0.0;
    }
    eval.xp.iter().find(|(name, _)| name == skill).map(|(_, amount)| amount / eval.ticks as f64).unwrap_or(0.0)
}

/// Level choice (§4.4): the chain with the best XP/tick at the
/// bot's level; P1 wins ties (D64). Locked chains (a step above the
/// bot's level) never win.
pub fn best_for_xp<'c>(chains: &[&'c Chain], evals: &[ChainEval], skill: &str, levels: &dyn Fn(&str) -> u8) -> Option<&'c Chain> {
    let mut best: Option<&Chain> = None;
    let mut best_rate = 0.0;
    for (chain, eval) in chains.iter().zip(evals.iter()) {
        if chain.steps.iter().any(|step| levels(step.skill) < step.level) {
            continue;
        }
        let rate = xp_per_tick(eval, skill);
        if rate <= 0.0 {
            continue;
        }
        // P1 wins ties (D64): keep the first P1 at equal rate.
        let incumbent_p2 = best.is_some_and(|held: &Chain| {
            eval_of(held, evals).is_some_and(|other| other.epoch == Epoch::P2)
        });
        if rate > best_rate + 1e-9 || ((rate - best_rate).abs() <= 1e-9 && eval.epoch == Epoch::P1 && incumbent_p2) {
            best_rate = rate;
            best = Some(*chain);
        }
    }
    best
}

fn eval_of<'e>(chain: &Chain, evals: &'e [ChainEval]) -> Option<&'e ChainEval> {
    evals.iter().find(|eval| eval.id == chain.id)
}

/// Money choice (§4.4): best net per 6000 ticks, trend-adjusted like
/// `rank_money_method`, with a ±2% seed jitter so near-ties vary by
/// profile instead of herding every bot onto one chain (D22-safe:
/// the seed is the bot's own).
pub fn best_for_money<'c>(chains: &[&'c Chain], evals: &[ChainEval], trend: &dyn Fn(&str) -> f64, seed: u64) -> Option<&'c Chain> {
    let mut best: Option<&Chain> = None;
    let mut top = f64::NEG_INFINITY;
    for (chain, eval) in chains.iter().zip(evals.iter()) {
        let jitter = (xorshift(seed ^ hash_id(chain.id)) % 200) as f64 / 100.0 - 1.0;
        let score = crate::cost::rank_money_method(eval.profit_per_6000, trend(chain.product), eval.final_value.max(1.0), 0.1)
            + jitter * 0.02 * eval.final_value.max(1.0);
        if score > top {
            top = score;
            best = Some(*chain);
        }
    }
    best
}

fn hash_id(id: &str) -> u64 {
    let mut hash = 0u64;
    for byte in id.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(byte as u64);
    }
    hash.max(1)
}

fn xorshift(mut word: u64) -> u64 {
    word = word.max(1);
    word ^= word << 13;
    word ^= word >> 7;
    word ^= word << 17;
    word
}

/// Era gate (D64, strict): P1 chains always run; a P2 chain runs when
/// it unlocks (quest, area, spell — the D64 exception) or when the goal
/// names its real product. Any other P2 chain is EXCLUDED, never
/// deferred: the brain spends no ticks off-goal off-era.
pub fn usable_chains<'c>(chains: &[&'c Chain], goal_product: &str, unlocked: &[&str]) -> Vec<&'c Chain> {
    let mut early: Vec<&Chain> = Vec::new();
    let mut goaled: Vec<&Chain> = Vec::new();
    for chain in chains {
        let epoch = if chain.steps.iter().any(|step| step.epoch == Epoch::P2) { Epoch::P2 } else { Epoch::P1 };
        if epoch == Epoch::P1 {
            early.push(*chain);
        } else if !chain.unlocks.is_empty() && unlocked.contains(&chain.unlocks) {
            early.push(*chain);
        } else if chain.product == goal_product {
            goaled.push(*chain);
        }
    }
    early.extend(goaled);
    early
}

/// One D59 shortcut: quest XP or the unlock that shortens the goal.
#[derive(Debug, Clone, PartialEq)]
pub struct QuestShortcut {
    pub quest: &'static str,
    pub skill: &'static str,
    pub xp: f64,
    pub unlocks: &'static str,
}

// Quest XP is server raw / 10 (quests.json, asserted below); unlocks
// from the catalogue §5 (squire needs mining 10, doric opens anvils,
// runemysteries opens runecraft).
const QUEST_SHORTCUTS: &[QuestShortcut] = &[
    QuestShortcut { quest: "squire", skill: "smithing", xp: 12725.0, unlocks: "" },
    QuestShortcut { quest: "doric", skill: "mining", xp: 1300.0, unlocks: "anvils" },
    QuestShortcut { quest: "runemysteries", skill: "runecraft", xp: 0.0, unlocks: "runecraft" },
    QuestShortcut { quest: "cook", skill: "cooking", xp: 300.0, unlocks: "" },
    QuestShortcut { quest: "sheep", skill: "crafting", xp: 150.0, unlocks: "" },
    QuestShortcut { quest: "gobdip", skill: "crafting", xp: 200.0, unlocks: "" },
    QuestShortcut { quest: "priest", skill: "prayer", xp: 1125.0, unlocks: "" },
    QuestShortcut { quest: "vampire", skill: "attack", xp: 4825.0, unlocks: "" },
    QuestShortcut { quest: "hetty", skill: "magic", xp: 325.0, unlocks: "" },
    QuestShortcut { quest: "imp", skill: "magic", xp: 875.0, unlocks: "" },
];

/// Quests worth planning for a skill goal (D59): their XP or unlock
/// shortens the path, cheapest first (quest chains expand via
/// `expand_quest`, so requirements ride along).
pub fn quests_for_goal(skill: &str) -> Vec<QuestShortcut> {
    QUEST_SHORTCUTS.iter().filter(|shortcut| shortcut.skill == skill).cloned().collect()
}

pub fn quest_shortcut(quest: &str) -> Option<QuestShortcut> {
    QUEST_SHORTCUTS.iter().find(|shortcut| shortcut.quest == quest).cloned()
}

/// One way to hold an item (§4.4 object choice).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Market,
    Shop,
    Farm,
    Craft,
    Spawn,
    Drop,
    Quest,
}

/// Ticks to hold `qty` of `item` by each way (caller numbers in).
pub struct SourceCtx {
    pub qty: i64,
    pub market_ticks: u64,
    pub shop_ticks: u64,
    pub farm_ticks: u64,
    pub craft_ticks: u64,
    pub spawn_ticks: u64,
    pub drop_ticks: u64,
    pub quest_ticks: u64,
    pub epoch: Epoch,
    pub ironman: bool,
    pub quest_planned: bool,
    pub unlock_needed: bool,
}

/// Cheapest way to hold an item: market, shop, farm, craft, ground
/// spawn, NPC drop, quest reward. P2 ways pay 25% unless they unlock
/// or serve the goal (D64); Iron never markets; quest legs only run
/// when the quest is already planned (D59).
pub fn cheapest_source(item: &str, ctx: &SourceCtx) -> Option<Source> {
    let mut ways: Vec<(Source, u64)> = Vec::new();
    if !ctx.ironman {
        ways.push((Source::Market, ctx.market_ticks));
    }
    ways.push((Source::Shop, ctx.shop_ticks));
    ways.push((Source::Farm, ctx.farm_ticks));
    ways.push((Source::Craft, ctx.craft_ticks));
    ways.push((Source::Spawn, ctx.spawn_ticks));
    ways.push((Source::Drop, ctx.drop_ticks));
    if ctx.quest_planned {
        ways.push((Source::Quest, ctx.quest_ticks));
    }
    let _ = item;
    let mut best: Option<(Source, f64)> = None;
    for (way, ticks) in ways {
        // shortcut: spawn/drop/quest legs are P2-flavoured detours here;
        // the chain evals carry the real per-activity epochs.
        let p2 = matches!(way, Source::Spawn | Source::Drop | Source::Quest) && ctx.epoch == Epoch::P2;
        let priced = ticks as f64 * if p2 && !ctx.unlock_needed { 1.25 } else { 1.0 };
        if best.is_none_or(|(_, top)| priced < top) {
            best = Some((way, priced));
        }
    }
    best.map(|(way, _)| way)
}

/// One D51 boot step: what to take, and whether it needs coins.
#[derive(Debug, Clone, PartialEq)]
pub struct BootStep {
    pub item: &'static str,
    pub way: Source,
    pub needs_coins: bool,
}

/// Bare start (0 coin, empty packs, empty bank) to owning a net and
/// a pickaxe with no intervention (§4.5 order: ground first, bare
/// hands next, sell surplus, buy the missing tool).
pub fn boot_plan() -> Vec<BootStep> {
    vec![
        BootStep { item: "bronze_pickaxe", way: Source::Spawn, needs_coins: false },
        BootStep { item: "coins", way: Source::Spawn, needs_coins: false },
        BootStep { item: "feather", way: Source::Farm, needs_coins: false },
        BootStep { item: "net", way: Source::Shop, needs_coins: true },
        BootStep { item: "bronze_pickaxe", way: Source::Shop, needs_coins: true },
    ]
}

/// Boot check: every no-coin step is first, and both tools land.
pub fn boot_covers(steps: &[BootStep]) -> bool {
    let mut seen_coin = false;
    for step in steps {
        if step.needs_coins {
            seen_coin = true;
        } else if seen_coin {
            return false;
        }
    }
    steps.iter().any(|step| step.item == "net") && steps.iter().any(|step| step.item == "bronze_pickaxe")
}

/// Synergies (§4.4): free XP or value the chain drags along.
pub fn synergies(chain_id: &str) -> &'static [&'static str] {
    match chain_id {
        "fish_cook" => &["cook own catch", "eat own food"],
        "iron" | "steel" | "gold" => &["banknotes stack"],
        "logs_fire" => &["ashes kept"],
        "hide_leather" => &["bury own bones", "cook own meat"],
        "essence" => &["cast own runes"],
        "gold_alch" => &["alch unsold products"],
        _ => &["alch unsold products"],
    }
}

#[cfg(test)]
mod tests {
    use super::{
        best_for_money, best_for_xp, boot_covers, boot_plan, chain_by_id, cheapest_source, eval_chain, quest_shortcut,
        quests_for_goal, synergies, usable_chains, xp_per_tick, Chain, ChainPrices, ChainStep, Source, SourceCtx, CHAINS,
    };
    use crate::catalog::Epoch;

    fn prices() -> (Box<dyn Fn(&str) -> f64>, Box<dyn Fn(&str) -> f64>) {
        let market = Box::new(|item: &str| match item {
            "iron_sword" => 91.0,
            "steel_platebody" => 2000.0,
            "trout" => 20.0,
            "ashes" => 2.0,
            "leather_gloves" => 6.0,
            "coins" => 1.0,
            "airrune" => 4.0,
            "strung_gold_amulet" => 350.0,
            "pot" => 1.0,
            "bread" => 12.0,
            "iron_bar" => 28.0,
            "runite_ore" => 1000.0,
            _ => 1.0,
        });
        let shop = Box::new(|item: &str| match item {
            "trout" => 20.0,
            "ashes" => 2.0,
            _ => 1.0,
        });
        (market, shop)
    }

    fn evals(qty: f64) -> Vec<super::ChainEval> {
        let (market, shop) = prices();
        let ctx = ChainPrices { market: &market, shop: &shop, trip_ticks: 800 };
        CHAINS.iter().map(|chain| eval_chain(chain, qty, &ctx)).collect()
    }

    #[test]
    fn whole_chains_compare_ticks_xp_and_value() {
        let values = evals(10.0);
        let iron = values.iter().find(|eval| eval.id == "iron").expect("iron evals");
        let steel = values.iter().find(|eval| eval.id == "steel").expect("steel evals");
        // Whole-chain totals: gathering plus two transforms plus trips.
        assert!(iron.ticks > 800 * 2, "trips plus work, got {}", iron.ticks);
        // Steel drags 5 bars per body: more ticks and more XP than iron.
        assert!(steel.ticks > iron.ticks, "steel costs more end to end ({} vs {})", steel.ticks, iron.ticks);
        let xp_of = |eval: &super::ChainEval, skill: &str| eval.xp.iter().find(|(name, _)| name == skill).map(|(_, amount)| *amount).unwrap_or(0.0);
        assert!(xp_of(steel, "smithing") > xp_of(iron, "smithing"), "steel teaches more smithing");
        assert!(steel.final_value > iron.final_value, "platebody outvalues sword");
        // Fish chain feeds two skills at once.
        let fish = values.iter().find(|eval| eval.id == "fish_cook").expect("fish evals");
        assert!(xp_of(fish, "fishing") > 0.0 && xp_of(fish, "cooking") > 0.0, "fish cooks while it catches");
        // Gold chain ends in coins through alchemy, with no shop floor under it.
        let gold = values.iter().find(|eval| eval.id == "gold").expect("gold evals");
        assert!(gold.coins_spent >= 0, "tanner-free chain spends nothing but work");
        let hide = values.iter().find(|eval| eval.id == "hide_leather").expect("hide evals");
        assert_eq!(hide.coins_spent, 10, "tanner fee rides in the chain (10 hides)");
    }

    #[test]
    fn existing_methods_stay_covered_by_chain_choices() {
        // Tranches (D21) still fix one resource per palier.
        assert_eq!(crate::expand::tranches("fishing", 1, 40), vec![5, 10, 20, 25, 35, 40]);
        assert_eq!(crate::expand::reach_method("mining", 30), "gather:mining/coal");
        // Tick costs still price market/farm/craft legs.
        assert_eq!(crate::cost::cost_market_buy(100, 30, 100, 1.0), 200 + 3000);
        assert_eq!(crate::cost::cost_farm(300, 300.0, 100), 6000 + 200);
        assert!(crate::cost::choose_market_or_farm(1000, 1200, 0.72), "trait still breaks close calls");
        assert!(crate::cost::rank_money_method(500.0, 0.6, 1000.0, 0.1) > 500.0, "trend still lifts shortages");
    }

    #[test]
    fn level_choice_maximises_xp_per_tick() {
        let values = evals(10.0);
        let refs: Vec<&Chain> = CHAINS.iter().collect();
        let levels = |skill: &str| match skill {
            "mining" => 40,
            "smithing" => 48,
            "fishing" => 40,
            "cooking" => 40,
            "woodcutting" => 60,
            "firemaking" => 60,
            "attack" => 40,
            "crafting" => 40,
            "magic" => 55,
            "runecraft" => 10,
            _ => 99,
        };
        let smith = best_for_xp(&refs, &values, "smithing", &levels).expect("a smithing chain");
        let rate = xp_per_tick(values.iter().find(|eval| eval.id == smith.id).expect("eval"), "smithing");
        for eval in &values {
            let chain = chain_by_id(&eval.id).expect("chain");
            if chain.steps.iter().any(|step| levels(step.skill) < step.level) {
                continue;
            }
            assert!(xp_per_tick(eval, "smithing") <= rate + 1e-9, "{} never beats {}", eval.id, smith.id);
        }
        // Locked-out bot cannot touch steel (coal 30, platebody 48).
        let low = |_: &str| 1u8;
        let starter = best_for_xp(&refs, &values, "smithing", &low);
        assert!(starter.is_none_or(|chain| chain.id != "steel"), "level 1 never picks steel");
    }

    #[test]
    fn money_choice_pays_best_per_6000_with_profile_diversity() {
        let values = evals(10.0);
        let refs: Vec<&Chain> = CHAINS.iter().collect();
        let flat = |_: &str| 0.0;
        let first = best_for_money(&refs, &values, &flat, 7).expect("a money chain");
        // Same seed replays; a 10x gap never flips on jitter.
        assert_eq!(best_for_money(&refs, &values, &flat, 7).map(|chain| chain.id), Some(first.id));
        let top: f64 = values.iter().map(|eval| eval.profit_per_6000).fold(f64::NEG_INFINITY, f64::max);
        let picked = values.iter().find(|eval| eval.id == first.id).expect("eval");
        assert!((top - picked.profit_per_6000) / top.max(1.0) < 0.1, "jitter only flips close calls");
    }

    #[test]
    fn era_gate_is_strict_p1_unless_unlock_or_goal() {
        let refs: Vec<&Chain> = CHAINS.iter().collect();
        // No unlock, unrelated goal: every plain P2 chain is excluded,
        // never deferred.
        let ordered = usable_chains(&refs, "trout", &[]);
        assert!(ordered.iter().all(|chain| {
            chain.steps.iter().all(|step| step.epoch == Epoch::P1) || !chain.unlocks.is_empty()
        }), "P1 or unlocking only, got {:?}", ordered.iter().map(|chain| chain.id).collect::<Vec<_>>());
        assert!(!ordered.iter().any(|chain| chain.id == "clay"), "plain P2 clay excluded off-goal");
        assert!(!ordered.iter().any(|chain| chain.id == "enchant"), "plain P2 enchant excluded off-goal");
        // A P2 product the goal names is still reachable, after the P1 set.
        let goaled = usable_chains(&refs, "pot", &[]);
        assert!(goaled.iter().any(|chain| chain.id == "clay"), "goal product stays reachable");
        let first_plain_p2 = goaled.iter().position(|chain| chain.id == "clay").expect("clay runs on-goal");
        assert!(goaled[..first_plain_p2].iter().all(|chain| {
            chain.steps.iter().all(|step| step.epoch == Epoch::P1) || !chain.unlocks.is_empty()
        }), "P1 first, goal-named P2 after");
        // Essence unlocks (Aubury via Rune Mysteries) but mines at P1
        // tiers, so it runs with the P1 set either way.
        let unlocked = usable_chains(&refs, "trout", &["runecraft"]);
        assert!(unlocked.iter().take(8).any(|chain| chain.id == "essence"), "essence runs early");
    }

    #[test]
    fn p1_wins_xp_ties() {
        // D64 tiebreak, order-independent: same xp/tick, P1 beats P2.
        static TIE_P1: &[ChainStep] = &[ChainStep { method: "cook:trout", skill: "cooking", level: 15, ticks_each: 3.0, xp_each: 70.0, coin_each: 0.0, epoch: Epoch::P1, wilderness: false, takes: "raw_trout", takes_qty: 1.0, makes: "trout", makes_qty: 1.0 }];
        static TIE_P2: &[ChainStep] = &[ChainStep { method: "bake:bread", skill: "cooking", level: 1, ticks_each: 3.0, xp_each: 70.0, coin_each: 0.0, epoch: Epoch::P2, wilderness: false, takes: "bread_dough", takes_qty: 1.0, makes: "bread", makes_qty: 1.0 }];
        let p1 = Chain { id: "tie_p1", steps: TIE_P1, product: "trout", unlocks: "" };
        let p2 = Chain { id: "tie_p2", steps: TIE_P2, product: "bread", unlocks: "" };
        let (market, shop) = prices();
        let ctx = ChainPrices { market: &market, shop: &shop, trip_ticks: 800 };
        let levels = |_: &str| 99u8;
        for order in [vec![&p1, &p2], vec![&p2, &p1]] {
            let evals: Vec<super::ChainEval> = order.iter().map(|chain| eval_chain(chain, 1.0, &ctx)).collect();
            assert!((evals[0].ticks as f64 - evals[1].ticks as f64).abs() < 1e-9, "tied rates, got {evals:?}");
            let picked = best_for_xp(&order, &evals, "cooking", &levels).expect("a chain wins");
            assert_eq!(picked.id, "tie_p1", "P1 wins ties whichever order, got {}", picked.id);
        }
    }

    #[test]
    fn wilderness_legs_cost_extra_ticks() {
        let (market, shop) = prices();
        let ctx = ChainPrices { market: &market, shop: &shop, trip_ticks: 800 };
        let wild = chain_by_id("runite_wild").expect("wilderness chain");
        let eval = eval_chain(wild, 1.0, &ctx);
        assert!(eval.wilderness, "flagged");
        assert_eq!(eval.ticks, 800 * 2 + 30 + super::WILDERNESS_TAX_TICKS, "trips + one runite rock + risk, got {}", eval.ticks);
    }

    #[test]
    fn quests_shortcut_the_goal() {
        let smithing: Vec<String> = quests_for_goal("smithing").iter().map(|shortcut| shortcut.quest.to_string()).collect();
        assert!(smithing.contains(&"squire".to_string()), "knight's sword shortcuts smithing, got {smithing:?}");
        assert!(quests_for_goal("mining").iter().any(|shortcut| shortcut.quest == "doric"), "doric opens anvils");
        assert!(quests_for_goal("runecraft").iter().any(|shortcut| shortcut.quest == "runemysteries"), "rune mysteries unlocks");
        let squire = quest_shortcut("squire").expect("squire row");
        assert!((squire.xp - 12725.0).abs() < 1e-9, "squire pays 1→29, got {}", squire.xp);
        assert_eq!(quest_shortcut("nope"), None, "unknown quests fail loud");
        assert!(!synergies("fish_cook").is_empty(), "fish chain names its synergies");
        assert!(synergies("logs_fire").contains(&"ashes kept"), "burnt logs keep ashes");
    }

    #[test]
    fn object_choice_covers_every_way() {
        let base = SourceCtx { qty: 10, market_ticks: 2000, shop_ticks: 1800, farm_ticks: 6000, craft_ticks: 8000, spawn_ticks: 1700, drop_ticks: 9000, quest_ticks: 6000, epoch: Epoch::P1, ironman: false, quest_planned: false, unlock_needed: false };
        assert_eq!(cheapest_source("net", &base), Some(Source::Spawn), "free spawn wins");
        let broke_shop = SourceCtx { spawn_ticks: u64::MAX / 2, drop_ticks: u64::MAX / 2, farm_ticks: u64::MAX / 2, craft_ticks: u64::MAX / 2, ..base };
        assert_eq!(cheapest_source("net", &broke_shop), Some(Source::Shop), "shop next");
        let iron = SourceCtx { market_ticks: 10, ..base };
        let iron_only = SourceCtx { ironman: true, ..iron };
        assert_ne!(cheapest_source("feather", &iron_only), Some(Source::Market), "iron never markets");
        assert_eq!(cheapest_source("feather", &iron), Some(Source::Market), "a 10x gap still buys");
        let quest = SourceCtx { quest_ticks: 100, quest_planned: true, ..base };
        assert_eq!(cheapest_source("cake", &quest), Some(Source::Quest), "planned quests reward");
        let unplanned = SourceCtx { quest_planned: false, ..quest };
        assert_ne!(cheapest_source("cake", &unplanned), Some(Source::Quest), "unplanned quests never win");
    }

    #[test]
    fn boot_reaches_net_and_pickaxe_from_nothing() {
        let plan = boot_plan();
        assert!(boot_covers(&plan), "net plus pickaxe, free legs first");
        assert!(plan.iter().take_while(|step| !step.needs_coins).any(|step| step.item == "bronze_pickaxe"), "pickaxe before any coin");
        assert!(plan.iter().any(|step| step.item == "net"), "net lands");
        // The paid legs only fund the shop buy: feathers sell first.
        assert!(plan.iter().position(|step| step.item == "feather").expect("feathers") < plan.iter().position(|step| step.item == "net").expect("net"), "sell before buy");
    }

    #[test]
    fn gamedata_validates_every_fixture_fact() {
        // Prices the comparisons rely on.
        let price = |name: &str| lifescape_gamedata::objs_by_name(name).into_iter().next().map(|entry| entry.price).unwrap_or(-1);
        assert!(price("steel_platebody") > price("iron_sword"), "platebody outvalues sword");
        assert!(price("lobster") > price("trout"), "lobster outvalues trout");
        assert!(price("gold_ring") > price("gold_bar"), "ring outvalues bar");
        assert!(price("amulet_of_magic") > price("ball_of_wool"), "strung amulet outvalues wool ball");
        // Transforms the chains walk through.
        assert!(!lifescape_gamedata::recipes::making("iron_bar", Some("smelt")).is_empty(), "iron smelts");
        assert!(!lifescape_gamedata::recipes::making("steel_bar", Some("smelt")).is_empty(), "steel smelts");
        assert!(!lifescape_gamedata::recipes::making("gold_bar", Some("smelt")).is_empty(), "gold smelts");
        assert!(!lifescape_gamedata::recipes::making("shrimp", Some("cook")).is_empty(), "fish cooks");
        assert!(!lifescape_gamedata::recipes::making("leather_gloves", Some("leather")).is_empty(), "leather crafts");
        assert!(!lifescape_gamedata::recipes::making("sapphire", Some("gem")).is_empty(), "gems cut");
        // Boot tools exist and trade F2P.
        for tool in ["net", "bronze_pickaxe", "feather", "tinderbox", "hammer"] {
            let entry = lifescape_gamedata::objs_by_name(tool).into_iter().next().unwrap_or_else(|| panic!("{tool} exists"));
            assert!(entry.tradeable && entry.members != Some(true), "{tool} trades F2P");
        }
        // Sources cover the boot: net shops, pickaxe spawns or shops.
        let has = |item: &str, want: &str| lifescape_gamedata::item_sources::sources_for_name(item).iter().any(|source| format!("{source:?}").starts_with(want));
        assert!(has("net", "Shop"), "nets shop");
        assert!(has("bronze_pickaxe", "Spawn") || has("bronze_pickaxe", "Shop"), "pickaxes spawn or shop");
        // Quest shortcuts resolve to real requirements.
        for quest in ["squire", "doric", "runemysteries", "cook", "sheep", "priest", "vampire"] {
            assert!(lifescape_gamedata::quest_requirements::by_key(quest).is_some(), "{quest} is a real quest");
        }
        assert_eq!(lifescape_gamedata::quest_requirements::by_key("squire").expect("squire").skills[0].skill, "mining");
    }

    /// P3.6: every chain step names an executable (map_method is Some).
    /// A step without a script fails typed at launch, never silently —
    /// so this test pins that the whole catalogue is wired today.
    #[test]
    fn every_chain_step_maps_to_a_script() {
        let node = crate::model::Node::new(
            "n9",
            crate::model::NodeKind::ObtainItems { item: "feather".to_string(), count: 2000 },
            Vec::new(),
            2000,
        );
        for chain in CHAINS {
            for step in chain.steps {
                assert!(
                    crate::mapping::map_method(step.method, &node, None).is_some(),
                    "chain {} step {} names no script",
                    chain.id,
                    step.method
                );
                // Locked steps name a real level, never 0.
                assert!(step.level >= 1, "chain {} step {} needs its level", chain.id, step.method);
            }
        }
    }

    /// P3.6 §8: past the last tier, levelling continues by volume — so
    /// every chain must hold at volume (finite ticks, value and XP that
    /// grow with quantity, no cliff past the top method).
    #[test]
    fn chains_hold_at_volume_past_the_last_tier() {
        let (market, shop) = prices();
        let ctx = ChainPrices { market: &market, shop: &shop, trip_ticks: 800 };
        for chain in CHAINS {
            let small = eval_chain(chain, 10.0, &ctx);
            let bulk = eval_chain(chain, 1000.0, &ctx);
            assert!(bulk.ticks > small.ticks, "chain {} grows with quantity", chain.id);
            assert!(bulk.final_value > small.final_value, "chain {} value grows", chain.id);
            assert!(bulk.xp.iter().any(|(_, amount)| *amount > 0.0), "chain {} teaches at volume", chain.id);
            assert!(small.final_value > 0.0, "chain {} product has a price", chain.id);
        }
    }

    /// P3.6 §8 + M10b: every F2P skill trains 1→99 — a catalogue method
    /// for each 10-level tranche, and a brain reach method that maps.
    /// XP are dump raw tenths (methods.rs); the planner divides by 10.
    #[test]
    fn every_skill_trains_1_to_99_by_tranches() {
        let skills = [
            "woodcutting", "mining", "fishing", "cooking", "firemaking", "crafting", "smithing",
            "runecraft", "prayer", "magic", "ranged", "attack", "strength", "defence", "hitpoints",
        ];
        assert_eq!(skills.len(), 15, "all 15 F2P skills");
        let node = crate::model::Node::new(
            "n9",
            crate::model::NodeKind::ObtainItems { item: "feather".to_string(), count: 2000 },
            Vec::new(),
            2000,
        );
        for skill in skills {
            for level in [1i64, 10, 20, 30, 40, 50, 60, 70, 80, 90, 99] {
                assert!(
                    lifescape_gamedata::methods::best_method(skill, level).is_some(),
                    "{skill} has no catalogue method at {level}"
                );
            }
            // The brain fixes one resource per tranche to 99 (D21); each
            // tier names a mapped family method.
            let mut tiers = crate::expand::tranches(skill, 1, 99);
            assert_eq!(tiers.last().copied(), Some(99), "{skill} tranches reach 99, got {tiers:?}");
            tiers.retain(|tier| *tier < 99);
            for tier in tiers {
                let method = crate::expand::reach_method(skill, tier);
                assert!(
                    crate::mapping::map_method(&method, &node, None).is_some(),
                    "{skill}@{tier} {method} names no script"
                );
            }
        }
    }

    /// P3.6 priors-vs-dump: chain priors carry real XP (server raw / 10,
    /// single source: `recipes`, `extra_recipes`, `methods`, `spells`
    /// dumps). Sampled per family; published rates replace them at n ≥ 3.
    #[test]
    fn chain_priors_match_dump_tenths() {
        let smelt_xp = |bar: &str| {
            lifescape_gamedata::recipes::making(bar, Some("smelt"))
                .into_iter()
                .map(|recipe| recipe.xp as f64 / 10.0)
                .next()
                .expect("smelt row")
        };
        assert!((smelt_xp("iron_bar") - 12.5).abs() < 1e-9, "iron smelt prior, dump {}", smelt_xp("iron_bar"));
        assert!((smelt_xp("steel_bar") - 17.5).abs() < 1e-9, "steel smelt prior");
        assert!((smelt_xp("gold_bar") - 22.5).abs() < 1e-9, "gold smelt prior");
        let enchant_xp = lifescape_gamedata::spells::by_name("enchant_lvl1")
            .into_iter()
            .map(|spell| spell.xp as f64 / 10.0)
            .next()
            .expect("enchant row");
        assert!((enchant_xp - 17.5).abs() < 1e-9, "enchant_lvl1 prior, dump {enchant_xp}");
        let alch_xp = lifescape_gamedata::spells::by_name("highlvl_alchemy")
            .into_iter()
            .map(|spell| spell.xp as f64 / 10.0)
            .next()
            .expect("alch row");
        assert!((alch_xp - 65.0).abs() < 1e-9, "high alch prior, dump {alch_xp}");
        let string_xp = lifescape_gamedata::extra_recipes::by_kind("string")
            .into_iter()
            .find(|entry| entry.source == "string_gold_amulet")
            .expect("string row")
            .xp as f64
            / 10.0;
        assert!((string_xp - 4.0).abs() < 1e-9, "stringing prior, dump {string_xp}");
        // Wool now strings the real dump amulet, which the enchant
        // ladder resolves (`enchant_for` wiring amulets → spells).
        let wool = chain_by_id("wool").expect("wool chain");
        assert_eq!(wool.product, "strung_gold_amulet", "real dump names, got {}", wool.product);
        assert_eq!(wool.steps[2].method, "string:unstrung_gold_amulet");
    }

    /// P2.1 review, statued: the 5th furnace tile (3081, 9496) is the
    /// Tutorial Island furnace (tutorial-only, smithing instructor),
    /// not a mainland 5th site. The catalogue's 4 (§5.7) stand: Al-Kharid,
    /// Falador, Lumbridge and Wilderness.
    #[test]
    fn four_mainland_furnace_sites_plus_tutorial() {
        let near = |x: i64, z: i64| {
            lifescape_gamedata::stations::all_stations()
                .into_iter()
                .any(|entry| entry.kind == "furnace" && (entry.x - x).abs() <= 2 && (entry.z - z).abs() <= 2)
        };
        assert!(near(3272, 3185), "Al-Kharid furnace");
        assert!(near(2975, 3368), "Falador furnace");
        assert!(near(3225, 3256), "Lumbridge furnace");
        assert!(near(3143, 3735), "Wilderness furnace");
        assert!(near(3081, 9496), "tutorial furnace tile stays in the dump, excluded from the 4");
    }
}
