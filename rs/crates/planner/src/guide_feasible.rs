//! Conditions of a guide row and their feasibility for one bot (plan
//! rendements D22, user 2026-11-17): the guides never block a row. Each
//! row keeps its economy at the current price and carries its
//! requirements as INFORMATION (`Prereqs`, flattened by `conditions`):
//! levels, combat, a site within bank reach, quests and quest points,
//! tools, the stock a recipe transforms, the capital of a lot, Ironman,
//! the market and shop availability of every object it buys, and the
//! objects nothing makes nor prices. The same rule for every family
//! (farm, recipe, chain, market, training autonomous and market).
//!
//! `feasible_for(row, bot)` is the ONE judgement of a row for a bot at
//! decision time (its levels, quests, tools held or buyable, free stock,
//! free coins, the market of the moment):
//! - `Feasible`: every condition met;
//! - `AfterPrereqs(pending)`: what the bot can still do first — train a
//!   level or its combat, a quest, quest points, a tool (held never; buy
//!   it when a shop or an ask has one, else obtain it), coins for a lot;
//! - `Infeasible(reasons, pending)`: something no prerequisite fixes now
//!   — no site within bank reach (`danger::NEVER`, impossible today for
//!   every bot), an object nothing makes nor prices, a recipe's stock
//!   the bot does not hold (one lot), an object bought whose held units,
//!   asks and shops do not cover one lot, an Ironman on a market row.
//!
//! `Feasibility::within_reach` is what the readers that used the old
//! "accessible" flag keep (D17's best line, the trade offers): feasible,
//! or feasible once a tool is obtained or coins for a lot are earned
//! (what a lot and the planner's obtain legs already handle).

use crate::guide_model::{GuideFilter, MarketNeed, MoneyRow, Prereqs, SourceGap, TrainingRow, WINDOW_TICKS};

/// A row the feasibility reads.
pub trait GuideRow {
    fn prereqs(&self) -> &Prereqs;
}

impl GuideRow for MoneyRow {
    fn prereqs(&self) -> &Prereqs {
        &self.prereqs
    }
}

impl GuideRow for TrainingRow {
    fn prereqs(&self) -> &Prereqs {
        &self.prereqs
    }
}

impl GuideRow for Prereqs {
    fn prereqs(&self) -> &Prereqs {
        self
    }
}

/// One requirement of a row (D22), the structured list the API serves.
#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    Level { skill: String, level: u8 },
    Combat { level: u8 },
    /// No site of the method has a bank in reach (`danger::NEVER`):
    /// impossible today for every bot.
    NoSite,
    Quest { quest: String },
    QuestPoints { points: i64 },
    /// A carried tool; `shop`: an F2P shop sells it.
    Tool { tool: String, shop: bool },
    /// One lot of a recipe's stock (D5).
    Stock { item: String, units: f64 },
    /// Coins one lot ties up (100 % market).
    Capital { coins: f64 },
    NotIronman,
    Market(MarketNeed),
    Source(SourceGap),
}

impl Condition {
    pub fn kind(&self) -> &'static str {
        match self {
            Condition::Level { .. } => "level",
            Condition::Combat { .. } => "combat",
            Condition::NoSite => "site",
            Condition::Quest { .. } => "quest",
            Condition::QuestPoints { .. } => "quest_points",
            Condition::Tool { .. } => "tool",
            Condition::Stock { .. } => "stock",
            Condition::Capital { .. } => "capital",
            Condition::NotIronman => "ironman",
            Condition::Market(_) => "market",
            Condition::Source(_) => "source",
        }
    }

    /// True for a condition no bot meets today, whatever it does first.
    pub fn impossible(&self) -> bool {
        matches!(self, Condition::NoSite | Condition::Source(_))
    }

    /// Plain text (the front's column, the logs).
    pub fn text(&self) -> String {
        match self {
            Condition::Level { skill, level } => format!("{skill} {level}"),
            Condition::Combat { level } => format!("combat {level}"),
            Condition::NoSite => "no site within bank reach (impossible today)".to_string(),
            Condition::Quest { quest } => format!("quest {quest}"),
            Condition::QuestPoints { points } => format!("{points} quest points"),
            Condition::Tool { tool, shop } => format!("{tool}{}", if *shop { " (shop)" } else { "" }),
            Condition::Stock { item, units } => format!("stock {units:.0} {item}"),
            Condition::Capital { coins } => format!("capital {coins:.0}"),
            Condition::NotIronman => "no ironman".to_string(),
            Condition::Market(need) => market_text(need),
            Condition::Source(gap) => format!("{} (impossible today)", gap.reason),
        }
    }
}

/// "naturerune 28/lot: asks 0, recent 40, shops 0" (or "no price").
fn market_text(need: &MarketNeed) -> String {
    let price = if need.priced { String::new() } else { ", no price".to_string() };
    format!("buy {} {:.0}/lot: asks {:.0}, recent {:.0}, shops {:.0}{price}", need.item, need.per_lot, need.asks, need.recent, need.shops)
}

/// True when an F2P shop sells `tool` (memo per process: static data).
pub fn shop_sells(tool: &str) -> bool {
    static MEMO: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, bool>>> = std::sync::OnceLock::new();
    crate::guide_methods::memo(&MEMO, tool.to_string(), || !lifescape_gamedata::shop_buy::purchase_steps(tool, WINDOW_TICKS as i64).is_empty())
}

/// The structured conditions of a row, in a fixed order.
pub fn conditions(prereqs: &Prereqs) -> Vec<Condition> {
    let mut out: Vec<Condition> = Vec::new();
    for (skill, level) in &prereqs.levels {
        out.push(Condition::Level { skill: skill.clone(), level: *level });
    }
    if prereqs.combat == lifescape_gamedata::danger::NEVER {
        out.push(Condition::NoSite);
    } else if prereqs.combat > 3 {
        out.push(Condition::Combat { level: prereqs.combat });
    }
    out.extend(prereqs.quests.iter().map(|quest| Condition::Quest { quest: quest.clone() }));
    if prereqs.quest_points > 0 {
        out.push(Condition::QuestPoints { points: prereqs.quest_points });
    }
    out.extend(prereqs.tools.iter().map(|tool| Condition::Tool { tool: tool.clone(), shop: shop_sells(tool) }));
    out.extend(prereqs.stock.iter().map(|(item, units)| Condition::Stock { item: item.clone(), units: *units }));
    if prereqs.capital > 0.0 {
        out.push(Condition::Capital { coins: prereqs.capital });
    }
    if !prereqs.ironman {
        out.push(Condition::NotIronman);
    }
    out.extend(prereqs.market.iter().cloned().map(Condition::Market));
    out.extend(prereqs.sources.iter().cloned().map(Condition::Source));
    out
}

/// What the judgement reads of one bot at decision time.
pub struct BotState<'a> {
    /// Its level in a skill.
    pub level_of: &'a dyn Fn(&str) -> u8,
    pub combat: u8,
    pub quests_done: &'a [String],
    /// Units of an object it holds and may use (free stock, tools).
    pub held: &'a dyn Fn(&str) -> f64,
    /// Free coins; `None`: unknown, a lot's capital is not judged.
    pub coins: Option<f64>,
    pub ironman: bool,
    /// Units resting on the asks now; `None`: the row's count at the
    /// build (the guide set's own cycle).
    pub asks: &'a dyn Fn(&str) -> Option<f64>,
}

/// What a bot can still do first.
#[derive(Debug, Clone, PartialEq)]
pub enum Pending {
    Level { skill: String, have: u8, need: u8 },
    Combat { have: u8, need: u8 },
    Quest(String),
    QuestPoints { have: i64, need: i64 },
    /// A tool it does not hold; `buyable`: a shop or an ask has one now.
    Tool { tool: String, buyable: bool },
    /// Coins for one lot.
    Coins { have: f64, need: f64 },
}

impl Pending {
    pub fn text(&self) -> String {
        match self {
            Pending::Level { skill, have, need } => format!("{skill} {have} -> {need}"),
            Pending::Combat { have, need } => format!("combat {have} -> {need}"),
            Pending::Quest(quest) => format!("quest {quest}"),
            Pending::QuestPoints { have, need } => format!("quest points {have} -> {need}"),
            Pending::Tool { tool, buyable } => format!("{} {tool}", if *buyable { "buy" } else { "obtain" }),
            Pending::Coins { have, need } => format!("coins {have:.0} -> {need:.0}"),
        }
    }

    /// A tool to obtain or coins to earn: what a lot and the planner's
    /// obtain legs handle on their own.
    pub fn is_supply(&self) -> bool {
        matches!(self, Pending::Tool { .. } | Pending::Coins { .. })
    }
}

/// The judgement of one row for one bot (module docs).
#[derive(Debug, Clone, PartialEq)]
pub enum Feasibility {
    Feasible,
    AfterPrereqs(Vec<Pending>),
    /// Why not now (every reason), and what else would be pending.
    Infeasible(Vec<String>, Vec<Pending>),
}

impl Feasibility {
    pub fn state(&self) -> &'static str {
        match self {
            Feasibility::Feasible => "feasible",
            Feasibility::AfterPrereqs(_) => "after_prereqs",
            Feasibility::Infeasible(..) => "infeasible",
        }
    }

    /// Feasible, or once a tool is obtained or coins for a lot earned
    /// (module docs: the old "accessible" readers keep this).
    pub fn within_reach(&self) -> bool {
        match self {
            Feasibility::Feasible => true,
            Feasibility::AfterPrereqs(pending) => pending.iter().all(Pending::is_supply),
            Feasibility::Infeasible(..) => false,
        }
    }
}

/// The feasibility of `row` for `bot` (module docs).
pub fn feasible_for(row: &impl GuideRow, bot: &BotState) -> Feasibility {
    let prereqs = row.prereqs();
    let mut reasons: Vec<String> = Vec::new();
    let mut pending: Vec<Pending> = Vec::new();
    for (skill, need) in &prereqs.levels {
        let have = (bot.level_of)(skill);
        if have < *need {
            pending.push(Pending::Level { skill: skill.clone(), have, need: *need });
        }
    }
    if prereqs.combat == lifescape_gamedata::danger::NEVER {
        reasons.push(Condition::NoSite.text());
    } else if prereqs.combat > bot.combat {
        pending.push(Pending::Combat { have: bot.combat, need: prereqs.combat });
    }
    for quest in prereqs.quests.iter().filter(|quest| !bot.quests_done.contains(quest)) {
        pending.push(Pending::Quest(quest.clone()));
    }
    if prereqs.quest_points > 0 {
        let have = lifescape_gamedata::quest_requirements::quest_points_of(bot.quests_done);
        if have < prereqs.quest_points {
            pending.push(Pending::QuestPoints { have, need: prereqs.quest_points });
        }
    }
    for tool in prereqs.tools.iter().filter(|tool| (bot.held)(tool) < 1.0) {
        let buyable = shop_sells(tool) || (bot.asks)(tool).unwrap_or(0.0) >= 1.0;
        pending.push(Pending::Tool { tool: tool.clone(), buyable });
    }
    for (item, units) in &prereqs.stock {
        let have = (bot.held)(item);
        if have + 1e-9 < *units {
            reasons.push(format!("stock: holds {have:.0} of one lot of {units:.0} {item}"));
        }
    }
    if let Some(coins) = bot.coins.filter(|coins| *coins + 1e-9 < prereqs.capital) {
        pending.push(Pending::Coins { have: coins, need: prereqs.capital });
    }
    if !prereqs.ironman && bot.ironman {
        reasons.push("ironman: the row trades on the market".to_string());
    }
    for need in &prereqs.market {
        let have = (bot.held)(&need.item);
        if have + 1e-9 >= need.per_lot {
            continue;
        }
        let asks = (bot.asks)(&need.item).unwrap_or(need.asks);
        if !need.priced {
            reasons.push(format!("{}: nothing prices it (never traded, no ask, no shop), holds {have:.0} of {:.0} per lot", need.item, need.per_lot));
        } else if have + asks + need.shops + 1e-9 < need.per_lot {
            reasons.push(format!("{}: holds {have:.0}, asks {asks:.0}, shops {:.0} of {:.0} per lot", need.item, need.shops, need.per_lot));
        }
    }
    reasons.extend(prereqs.sources.iter().map(|gap| format!("{} (impossible today)", gap.reason)));
    match (reasons.is_empty(), pending.is_empty()) {
        (true, true) => Feasibility::Feasible,
        (true, false) => Feasibility::AfterPrereqs(pending),
        (false, _) => Feasibility::Infeasible(reasons, pending),
    }
}

/// Order rank of a judgement (D22 Q1): feasible now, after
/// prerequisites, infeasible.
pub fn state_rank(found: &Feasibility) -> u8 {
    match found {
        Feasibility::Feasible => 0,
        Feasibility::AfterPrereqs(_) => 1,
        Feasibility::Infeasible(..) => 2,
    }
}

/// The rows a bot can work first (D22 Q1, user 2026-11-17, R7): feasible
/// now, then after prerequisites, then infeasible; the guide's order
/// (priced first, then by value) kept within each (stable).
pub fn feasible_first<T: GuideRow>(rows: &mut [T], bot: &BotState) {
    rows.sort_by_cached_key(|row| state_rank(&feasible_for(row, bot)));
}

/// Runs `body` with the bot a guide filter describes: every skill at its
/// `level` (`levels` overriding a skill), the catalogue (no level) at 99
/// (each row is read at its own gate: levels and combat pass, a site out
/// of reach still does not), its quests, its stock (`held`), its coins
/// (`None`: capital unjudged), no Ironman, the asks of the build.
pub fn with_filter_bot<T>(filter: &GuideFilter, body: impl FnOnce(&BotState) -> T) -> T {
    let base = filter.level.unwrap_or(99);
    let level_of = |skill: &str| filter.levels.iter().find(|(name, _)| name == skill).map(|(_, level)| *level).unwrap_or(base);
    let combat = crate::combat::combat_level(&crate::combat::BaseLevels {
        attack: level_of("attack"),
        strength: level_of("strength"),
        defence: level_of("defence"),
        hitpoints: level_of("hitpoints").max(10),
        ranged: level_of("ranged"),
        prayer: level_of("prayer"),
        magic: level_of("magic"),
    });
    let held = |item: &str| filter.held.iter().filter(|(name, _)| name == item).fold(0.0, |total, (_, units)| total + units);
    let asks = |_: &str| None;
    let bot = BotState { level_of: &level_of, combat, quests_done: &filter.quests_done, held: &held, coins: filter.coins, ironman: false, asks: &asks };
    body(&bot)
}

#[cfg(test)]
#[path = "guide_feasible_tests.rs"]
mod tests;
