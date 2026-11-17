//! Tests of `guide_feasible` (plan rendements D22): every condition a
//! row carries, judged for one fabricated bot by `feasible_for`.

use super::{conditions, feasible_for, BotState, Condition, Feasibility, Pending};
use crate::guide_model::{MarketNeed, Prereqs, SourceGap};

/// A fabricated bot: every skill at `level`, combat `combat`, `quests`,
/// holding `held`, `coins` free, asks `asks` (None: the row's count).
struct Bot {
    level: u8,
    combat: u8,
    quests: Vec<String>,
    held: Vec<(&'static str, f64)>,
    coins: Option<f64>,
    ironman: bool,
    asks: Vec<(&'static str, f64)>,
}

impl Bot {
    fn new(level: u8) -> Self {
        Bot { level, combat: 3, quests: Vec::new(), held: Vec::new(), coins: None, ironman: false, asks: Vec::new() }
    }

    fn judge(&self, prereqs: &Prereqs) -> Feasibility {
        let level_of = |_: &str| self.level;
        let held = |item: &str| self.held.iter().filter(|(name, _)| *name == item).map(|(_, units)| *units).sum::<f64>();
        let asks = |item: &str| self.asks.iter().find(|(name, _)| *name == item).map(|(_, units)| *units);
        let bot = BotState { level_of: &level_of, combat: self.combat, quests_done: &self.quests, held: &held, coins: self.coins, ironman: self.ironman, asks: &asks };
        feasible_for(prereqs, &bot)
    }
}

fn need(item: &str, per_lot: f64, asks: f64, shops: f64, priced: bool) -> MarketNeed {
    MarketNeed { item: item.to_string(), cause: "rune".to_string(), per_6000: per_lot * 10.0, per_lot, asks, recent: 40.0, shops, priced }
}

fn reasons(found: &Feasibility) -> Vec<String> {
    match found {
        Feasibility::Infeasible(reasons, _) => reasons.clone(),
        _ => Vec::new(),
    }
}

fn pending(found: &Feasibility) -> Vec<Pending> {
    match found {
        Feasibility::AfterPrereqs(pending) | Feasibility::Infeasible(_, pending) => pending.clone(),
        Feasibility::Feasible => Vec::new(),
    }
}

/// Levels and combat under the row's: feasible after training them.
#[test]
fn levels_and_combat_are_prerequisites() {
    let prereqs = Prereqs { levels: vec![("smithing".to_string(), 30)], combat: 40, ironman: true, ..Prereqs::default() };
    let mut bot = Bot::new(20);
    bot.combat = 30;
    let found = bot.judge(&prereqs);
    assert_eq!(found.state(), "after_prereqs");
    assert_eq!(pending(&found), vec![Pending::Level { skill: "smithing".to_string(), have: 20, need: 30 }, Pending::Combat { have: 30, need: 40 }]);
    assert!(!found.within_reach(), "a level is no purchase");
    let mut trained = Bot::new(30);
    trained.combat = 40;
    assert_eq!(trained.judge(&prereqs), Feasibility::Feasible);
}

/// No site within bank reach (`danger::NEVER`): impossible today for
/// every bot, whatever its combat.
#[test]
fn no_site_within_bank_reach_is_impossible_for_every_bot() {
    let prereqs = Prereqs { levels: vec![("mining".to_string(), 85)], combat: lifescape_gamedata::danger::NEVER, ironman: true, ..Prereqs::default() };
    let listed = conditions(&prereqs);
    assert!(listed.contains(&Condition::NoSite), "{listed:?}");
    assert!(listed.iter().any(|condition| condition.impossible()));
    assert!(!listed.iter().any(|condition| matches!(condition, Condition::Combat { .. })), "never shown as combat 255");
    let mut maxed = Bot::new(99);
    maxed.combat = 126;
    let found = maxed.judge(&prereqs);
    assert_eq!(found.state(), "infeasible");
    assert!(reasons(&found).iter().any(|reason| reason.contains("no site within bank reach")), "{found:?}");
}

/// Quests and quest points: done first.
#[test]
fn quests_and_quest_points_are_prerequisites() {
    let prereqs = Prereqs { quests: vec!["dragon".to_string()], quest_points: 32, ironman: true, ..Prereqs::default() };
    let found = Bot::new(99).judge(&prereqs);
    assert!(matches!(&pending(&found)[..], [Pending::Quest(quest), Pending::QuestPoints { need: 32, .. }] if quest == "dragon"), "{found:?}");
    let mut done = Bot::new(99);
    done.quests = lifescape_gamedata::f2p::F2P_QUESTS.iter().map(|(_, dir)| dir.strip_prefix("quest_").unwrap_or(dir).to_string()).collect();
    assert!(lifescape_gamedata::quest_requirements::quest_points_of(&done.quests) >= 32);
    assert_eq!(done.judge(&prereqs), Feasibility::Feasible);
}

/// A tool held passes; one not held is to buy (a shop or an ask has one
/// now) or to obtain (neither): within reach either way.
#[test]
fn tools_are_held_bought_or_obtained() {
    let prereqs = Prereqs { tools: vec!["no_such_tool".to_string()], ironman: true, ..Prereqs::default() };
    let found = Bot::new(1).judge(&prereqs);
    assert_eq!(pending(&found), vec![Pending::Tool { tool: "no_such_tool".to_string(), buyable: false }]);
    assert!(found.within_reach(), "a tool is obtained by the usual legs");
    let mut offered = Bot::new(1);
    offered.asks.push(("no_such_tool", 2.0));
    assert_eq!(pending(&offered.judge(&prereqs)), vec![Pending::Tool { tool: "no_such_tool".to_string(), buyable: true }]);
    let mut holding = Bot::new(1);
    holding.held.push(("no_such_tool", 1.0));
    assert_eq!(holding.judge(&prereqs), Feasibility::Feasible);
}

/// A recipe's stock (D5): the bot holds one lot, or cannot work it.
#[test]
fn a_recipe_needs_one_lot_of_its_stock() {
    let prereqs = Prereqs { stock: vec![("iron_ore".to_string(), 10.0), ("coal".to_string(), 19.0)], ironman: true, ..Prereqs::default() };
    let mut short = Bot::new(30);
    short.held = vec![("iron_ore", 10.0), ("coal", 5.0)];
    let found = short.judge(&prereqs);
    assert_eq!(found.state(), "infeasible");
    assert!(reasons(&found).iter().any(|reason| reason.contains("coal") && reason.contains("19")), "{found:?}");
    short.held = vec![("iron_ore", 10.0), ("coal", 19.0)];
    assert_eq!(short.judge(&prereqs), Feasibility::Feasible);
}

/// Capital of a lot: coins to earn first; no wallet given, unjudged.
#[test]
fn capital_is_judged_on_the_free_coins() {
    let prereqs = Prereqs { capital: 600.0, ironman: false, ..Prereqs::default() };
    let mut poor = Bot::new(1);
    poor.coins = Some(100.0);
    let found = poor.judge(&prereqs);
    assert_eq!(pending(&found), vec![Pending::Coins { have: 100.0, need: 600.0 }]);
    assert!(found.within_reach(), "a lot is bounded by the wallet");
    assert_eq!(Bot::new(1).judge(&prereqs), Feasibility::Feasible, "no wallet: capital unjudged");
    let mut ironman = Bot::new(1);
    ironman.ironman = true;
    assert!(reasons(&ironman.judge(&prereqs)).iter().any(|reason| reason.contains("ironman")));
}

/// Market availability of an object bought: held units, asks (the build's
/// or the moment's) and shops must cover one lot; an object nothing
/// prices only if the bot holds the lot; the recent fills are shown, never
/// counted.
#[test]
fn market_availability_counts_held_asks_and_shops_per_lot() {
    let prereqs = Prereqs { market: vec![need("naturerune", 28.0, 0.0, 0.0, true)], ironman: true, ..Prereqs::default() };
    let found = Bot::new(60).judge(&prereqs);
    assert_eq!(found.state(), "infeasible");
    assert!(reasons(&found).iter().any(|reason| reason.contains("naturerune") && reason.contains("asks 0")), "{found:?}");
    let mut holder = Bot::new(60);
    holder.held.push(("naturerune", 30.0));
    assert_eq!(holder.judge(&prereqs), Feasibility::Feasible, "its own runes");
    let mut later = Bot::new(60);
    later.held.push(("naturerune", 10.0));
    later.asks.push(("naturerune", 20.0));
    assert_eq!(later.judge(&prereqs), Feasibility::Feasible, "held + the asks of the moment");
    let shops = Prereqs { market: vec![need("naturerune", 28.0, 0.0, 30.0, true)], ironman: true, ..Prereqs::default() };
    assert_eq!(Bot::new(60).judge(&shops), Feasibility::Feasible, "a shop's window");
    let unpriced = Prereqs { market: vec![need("runite_bar", 28.0, 0.0, 0.0, false)], ironman: true, ..Prereqs::default() };
    assert!(reasons(&Bot::new(99).judge(&unpriced)).iter().any(|reason| reason.contains("nothing prices it")));
    let mut stocked = Bot::new(99);
    stocked.held.push(("runite_bar", 28.0));
    assert_eq!(stocked.judge(&unpriced), Feasibility::Feasible, "held, nothing to buy");
}

/// A source gap: impossible today, listed as such.
#[test]
fn a_source_gap_is_impossible() {
    let gap = SourceGap { item: "logs".to_string(), reason: "logs: no autonomous source at level 30".to_string() };
    let prereqs = Prereqs { sources: vec![gap.clone()], ironman: true, ..Prereqs::default() };
    assert!(conditions(&prereqs).iter().any(|condition| *condition == Condition::Source(gap.clone()) && condition.impossible()));
    let found = Bot::new(99).judge(&prereqs);
    assert!(reasons(&found).iter().any(|reason| reason.contains("logs") && reason.contains("impossible")), "{found:?}");
}

/// The conditions list: every requirement, structured, in order.
#[test]
fn conditions_list_every_requirement() {
    let prereqs = Prereqs {
        levels: vec![("smithing".to_string(), 30)],
        combat: 20,
        quests: vec!["doric".to_string()],
        quest_points: 5,
        tools: vec!["hammer".to_string()],
        capital: 500.0,
        ironman: false,
        stock: vec![("coal".to_string(), 19.0)],
        market: vec![need("naturerune", 28.0, 0.0, 0.0, true)],
        sources: Vec::new(),
    };
    let kinds: Vec<&str> = conditions(&prereqs).iter().map(Condition::kind).collect();
    assert_eq!(kinds, vec!["level", "combat", "quest", "quest_points", "tool", "stock", "capital", "ironman", "market"]);
    let market = conditions(&prereqs).into_iter().find(|condition| condition.kind() == "market").expect("market");
    assert_eq!(market.text(), "buy naturerune 28/lot: asks 0, recent 40, shops 0");
}
