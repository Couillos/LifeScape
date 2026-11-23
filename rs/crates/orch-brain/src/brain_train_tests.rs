//! The brain's training pick read from a precomputed guide set (R6): a
//! set built in memory (no store, no engine, no rate read: a pick is a
//! READ of the published views), fabricated lines, profiles of every
//! trait; the resolver then locks the pick and blocks on what it lacks.

use std::collections::HashMap;
use std::time::Instant;

use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::guide_model::{Confidence, Family, MoneyRow, Prereqs, SourceGap, Supply, TrainingRow};
use lifescape_planner::guide_read::{MoneyView, TrainingView};
use lifescape_planner::{resolve_ready, Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};

use super::{pick_from_guide, Climb, GuidePick};
use lifescape_planner::guide_feasible::BotState;
use crate::guides_store::GuideSet;

fn life_with(market_affinity: f64, thrift: f64) -> Life {
    Life {
        schema: 1,
        account: "maker01".to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(false, market_affinity, 0.5, 7, Archetype::Producer).with_thrift(thrift),
        goal: GoalKind::MaxSkills { skills: vec!["firemaking".to_string()], level: 65 },
        nodes: Vec::new(),
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
        memory: Default::default(),
    }
}

fn line(skill: &str, method: &str, supply: Supply, xp_6000: f64, coins_6000: f64) -> TrainingRow {
    TrainingRow {
        skill: skill.to_string(),
        band: 56,
        method: method.to_string(),
        style: String::new(),
        site: None,
        tool: String::new(),
        supply,
        prereqs: Prereqs::default(),
        xp_6000,
        coins_6000,
        coins_per_xp: -coins_6000 / xp_6000,
        converted_ticks_6000: 0.0,
        effective_xp_6000: xp_6000,
        ticks_to_band_end: None,
        other_xp_6000: Vec::new(),
        quest_xp: 0.0,
        quest_ticks: None,
        flows: Vec::new(),
        routes: Vec::new(),
        confidence: Confidence { runs: 9, ..Confidence::default() },
        price_confidence: "traded",
        priced: true,
    }
}

/// Coins per tick of the money line (`money_line`): the time value the
/// climbs of these tests price their coins at (D1: the bot's measured
/// gain, here the same 5 coins per tick).
const MONEY_LINE_RATE: f64 = 30_000.0 / 6000.0;

/// The bot's best money line (D17): 30 000 coins per 6000 ticks.
fn money_line() -> MoneyRow {
    MoneyRow {
        family: Family::Farm,
        method: "gather:woodcut/logs".to_string(),
        style: String::new(),
        site: None,
        tool: String::new(),
        band: 56,
        prereqs: Prereqs::default(),
        flows: Vec::new(),
        other_drops: Default::default(),
        revenue_6000: 30_000.0,
        cost_6000: 0.0,
        profit_6000: 30_000.0,
        xp_6000: Vec::new(),
        extra_ticks_per_6000: 0.0,
        stages: Vec::new(),
        routes: Vec::new(),
        confidence: Confidence::default(),
        price_confidence: "traded",
        priced: true,
    }
}

/// A published set whose band 56-60 holds `rows` (built at 60).
fn set_with(rows: Vec<TrainingRow>) -> GuideSet {
    let money = MoneyView { level: Some(60), rows: vec![money_line()], ..MoneyView::default() };
    GuideSet {
        built_unix_ms: 0,
        built: Instant::now(),
        cycle_ms: 0,
        cycle_cpu_ms: None,
        market_wait_ticks: None,
        money_catalog: MoneyView::default(),
        training_catalog: TrainingView::default(),
        money_bands: vec![(60, money)],
        training_bands: vec![(60, TrainingView { level: Some(60), rows, gaps: Vec::new() })],
        quotes: HashMap::new(),
        stats: lifescape_json::Value::Null,
    }
}

/// Firemaking yew from the guide (R6 example): the market bot buys its
/// logs (the faster line once its coins are converted), the autarkic
/// one cuts them; a quest shortcut and another skill's line never
/// launch a firemaking climb.
#[test]
fn the_brain_picks_its_line_from_the_published_guide() {
    let mut quest = line("firemaking", "quest:druidic_ritual", Supply::Quest, 1_000_000.0, 0.0);
    quest.quest_xp = 250.0;
    let set = set_with(vec![
        quest,
        line("woodcutting", "gather:woodcut/yew_logs", Supply::Autonomous, 90_000.0, 30_000.0),
        line("firemaking", "fire:yew_logs", Supply::Autonomous, 30_000.0, 0.0),
        line("firemaking", "fire:yew_logs", Supply::Market, 60_000.0, -30_000.0),
    ]);
    let none = |_: &str| 0;
    let market = pick(&set, "firemaking", 60, 65, &life_with(1.0, 0.5), &[], &none).expect("a line");
    assert_eq!((market.pick.method.as_str(), market.supply), ("fire:yew_logs", Supply::Market));
    assert_eq!(market.best_profit, Some(30_000.0), "D17 read in the money view");
    assert_eq!(market.scored, 2, "the quest and the woodcutting line are out");
    assert_eq!(market.pick.missing, vec!["yew_logs".to_string()], "the logs to obtain first");
    let autarkic = pick(&set, "firemaking", 60, 65, &life_with(0.0, 0.5), &[], &none).expect("a line");
    assert_eq!((autarkic.pick.method.as_str(), autarkic.supply), ("fire:yew_logs", Supply::Autonomous));
    assert!((autarkic.pick.xp_per_tick - 5.0).abs() < 1e-9, "the line's effective rate");
}

/// Rapide ↔ économe on two magic lines: the fast bot teleports (dear,
/// quick), the thrifty one curses (cheap, slow). Inaccessible or
/// blocked lines never win, and with none left the pick is a typed
/// error (the static tranche then runs).
#[test]
fn the_thrift_trait_splits_a_fast_line_from_a_cheap_one() {
    let mut blocked = line("magic", "fight:magic/fire_strike/cows", Supply::Autonomous, 900_000.0, 0.0);
    blocked.prereqs.sources = vec![SourceGap { item: "firerune".to_string(), reason: "no autonomous source".to_string() }];
    let set = set_with(vec![
        blocked,
        line("magic", "teleport:varrock_teleport", Supply::Market, 120_000.0, -120_000.0),
        line("magic", "curse:confuse/chickens", Supply::Market, 30_000.0, -3000.0),
    ]);
    let none = |_: &str| 0;
    let fast = pick(&set, "magic", 58, 60, &life_with(0.5, 0.0), &[], &none).expect("a line");
    assert_eq!(fast.pick.method, "teleport:varrock_teleport");
    let thrifty = pick(&set, "magic", 58, 60, &life_with(0.5, 1.0), &[], &none).expect("a line");
    assert_eq!(thrifty.pick.method, "curse:confuse/chickens");
    let empty = set_with(Vec::new());
    assert!(pick(&empty, "magic", 58, 60, &life_with(0.5, 0.5), &[], &none).is_err());
}

/// The pick locks like any priced tranche: the resolver blocks on the
/// logs it lacks, then locks the method once they are held.
#[test]
fn the_resolver_locks_the_guide_pick() {
    let set = set_with(vec![line("firemaking", "fire:yew_logs", Supply::Market, 60_000.0, -30_000.0)]);
    let none = |_: &str| 0;
    let found = pick(&set, "firemaking", 60, 65, &life_with(1.0, 0.5), &[], &none).expect("a line");
    let mut node = Node::new("n4", NodeKind::ReachLevel { skill: "firemaking".to_string(), level: 65 }, Vec::new(), 65);
    node.status = NodeStatus::Ready;
    let mut ctx = ReadyCtx::blind(1.0);
    ctx.bank_known = true;
    ctx.tools = vec!["tinderbox".to_string()];
    ctx.training = Some(found.pick.clone());
    resolve_ready(&mut node, &ctx);
    assert_eq!(node.method.as_deref(), Some("fire:yew_logs"));
    assert_eq!(node.status, NodeStatus::Blocked { reason: "missing:yew_logs".to_string() });
    let logs = |item: &str| if item == "yew_logs" { 27 } else { 0 };
    let held = pick(&set, "firemaking", 60, 65, &life_with(1.0, 0.5), &[], &logs).expect("a line");
    assert!(held.pick.missing.is_empty(), "a load in hand");
}

/// The pick of a bot at `level` in every skill, its quests and held units
/// (no wallet judged, the build's asks), seed 1, nothing quarantined.
fn pick(set: &GuideSet, skill: &str, level: u8, target: u8, life: &Life, quests: &[String], held: &dyn Fn(&str) -> i64) -> Result<GuidePick, String> {
    pick_seeded(set, skill, level, target, life, quests, held, &[], 1, None)
}

#[allow(clippy::too_many_arguments)]
fn pick_seeded(set: &GuideSet, skill: &str, level: u8, target: u8, life: &Life, quests: &[String], held: &dyn Fn(&str) -> i64, stock: &[(String, f64)], seed: u64, previous: Option<&str>) -> Result<GuidePick, String> {
    let level_of = |_: &str| level;
    let held_units = |item: &str| held(item) as f64;
    let asks = |_: &str| None;
    let bot = BotState { level_of: &level_of, combat: 126, quests_done: quests, held: &held_units, coins: None, ironman: life.profile.ironman, asks: &asks };
    let never = |_: &str| false;
    pick_from_guide(set, &Climb { skill, level, target, previous, food: None, coin_rate: MONEY_LINE_RATE }, life, &bot, stock, seed, &never, &lifescape_planner::work_rate::prior_rate)
}

/// D1 (plan équilibre step 1): the coins of a line convert at the bot's
/// MEASURED time value (`Climb::coin_rate`), never at its best guide line
/// (D17). The market bot of the firemaking example buys its logs while a
/// tick earns it 5 coins; at the 0.1 floor the logs cost it 10 ticks a
/// coin and it cuts them itself, the D17 line unchanged.
#[test]
fn the_climb_converts_coins_at_the_measured_time_value() {
    let set = set_with(vec![
        line("firemaking", "fire:yew_logs", Supply::Autonomous, 30_000.0, 0.0),
        line("firemaking", "fire:yew_logs", Supply::Market, 60_000.0, -30_000.0),
    ]);
    let life = life_with(1.0, 0.5);
    let level_of = |_: &str| 60u8;
    let none = |_: &str| 0.0;
    let asks = |_: &str| None;
    let bot = BotState { level_of: &level_of, combat: 126, quests_done: &[], held: &none, coins: None, ironman: false, asks: &asks };
    let never = |_: &str| false;
    let at = |coin_rate: f64| {
        let climb = Climb { skill: "firemaking", level: 60, target: 65, previous: None, food: None, coin_rate };
        pick_from_guide(&set, &climb, &life, &bot, &[], 1, &never, &lifescape_planner::work_rate::prior_rate).expect("a line")
    };
    assert_eq!(at(MONEY_LINE_RATE).supply, Supply::Market, "5 coins a tick: the logs are bought");
    let floor = at(lifescape_planner::market_fit::MIN_COIN_RATE);
    assert_eq!(floor.supply, Supply::Autonomous, "0.1 coin a tick: the logs are cut");
    assert_eq!(floor.best_profit, Some(30_000.0), "the D17 line stays the guide's, for display");
}

/// R7: two lines within 15 % of each other are both drawn across seeds
/// (D10), the same seed always the same; a line beyond the band only by
/// exploration (D14) while it is unmeasured.
#[test]
fn the_climb_draws_among_close_lines() {
    let set = set_with(vec![
        line("woodcutting", "gather:woodcut/willow_logs", Supply::Autonomous, 60_000.0, 0.0),
        line("woodcutting", "gather:woodcut/oak_logs", Supply::Autonomous, 56_000.0, 0.0),
        line("woodcutting", "gather:woodcut/logs", Supply::Autonomous, 20_000.0, 0.0),
    ]);
    let life = life_with(0.5, 0.5);
    let none = |_: &str| 0;
    let drawn: std::collections::BTreeMap<String, usize> = (0..300u64).fold(Default::default(), |mut tally, seed| {
        let found = pick_seeded(&set, "woodcutting", 60, 65, &life, &[], &none, &[], seed, None).expect("a line");
        assert_eq!(found.choice.drawn.band, 2, "willow and oak in the band");
        *tally.entry(found.pick.method).or_insert(0) += 1;
        tally
    });
    assert!(drawn.get("gather:woodcut/willow_logs").is_some_and(|count| *count > 100) && drawn.get("gather:woodcut/oak_logs").is_some_and(|count| *count > 80), "{drawn:?}");
    assert_eq!(drawn.get("gather:woodcut/logs"), None, "measured and out of the band: never");
    let again = pick_seeded(&set, "woodcutting", 60, 65, &life, &[], &none, &[], 41, None).expect("a line").pick.method;
    assert_eq!(pick_seeded(&set, "woodcutting", 60, 65, &life, &[], &none, &[], 41, None).expect("a line").pick.method, again, "same seed, same line");
}

/// R7 (c): harvest and combat choose through the guide too; a melee line
/// trains its own style's skill only (an attack climb never takes a
/// strength-style fight).
#[test]
fn combat_climbs_take_their_own_style_from_the_guide() {
    let mut strength = line("attack", "fight:melee/cows", Supply::Autonomous, 90_000.0, 500.0);
    strength.style = "melee:strength".to_string();
    let mut attack = line("attack", "fight:melee/chickens", Supply::Autonomous, 40_000.0, 100.0);
    attack.style = "melee:attack".to_string();
    let set = set_with(vec![strength, attack]);
    let none = |_: &str| 0;
    let found = pick(&set, "attack", 58, 60, &life_with(0.5, 0.5), &[], &none).expect("a line");
    assert_eq!(found.pick.method, "fight:melee/chickens");
    assert_eq!(found.scored, 1, "the strength-style row never trains an attack climb");
}

/// The bot works what it can NOW first while that line stays within the
/// band of the best one; a line far ahead whose tool it must obtain is
/// drawn instead (`train_score::drawn_class`, live 2026-11-20: leather a
/// class behind the bowls). Once it blocked on that tool for its previous
/// pick, the previous pick comes back as soon as it is feasible and
/// within the band of the best line (D10).
#[test]
fn feasible_lines_draw_first_and_a_blocked_pick_comes_back() {
    let mut harpoon = line("fishing", "gather:fishing/raw_swordfish", Supply::Autonomous, 90_000.0, 0.0);
    harpoon.prereqs.tools = vec!["harpoon".to_string()];
    let close = set_with(vec![harpoon.clone(), line("fishing", "gather:fishing/raw_trout", Supply::Autonomous, 85_000.0, 0.0)]);
    let set = set_with(vec![harpoon, line("fishing", "gather:fishing/raw_trout", Supply::Autonomous, 50_000.0, 0.0)]);
    let life = life_with(0.5, 0.5);
    let none = |_: &str| 0;
    for seed in 0..40 {
        let found = pick_seeded(&close, "fishing", 58, 60, &life, &[], &none, &[], seed, None).expect("a line");
        assert_eq!((found.pick.method.as_str(), found.choice.class), ("gather:fishing/raw_trout", 0), "feasible now first within the band");
        let far = pick_seeded(&set, "fishing", 58, 60, &life, &[], &none, &[], seed, None).expect("a line");
        assert_eq!((far.pick.method.as_str(), far.choice.class), ("gather:fishing/raw_swordfish", 2), "far ahead: the harpoon first");
    }
    let harpoon_held = |item: &str| i64::from(item == "harpoon");
    let back = pick_seeded(&set, "fishing", 58, 60, &life, &[], &harpoon_held, &[], 3, Some("gather:fishing/raw_swordfish")).expect("a line");
    assert!(back.choice.sticky && back.pick.method == "gather:fishing/raw_swordfish", "the previous pick, feasible, comes back");
    // A previous pick out of the band of the best line now (2026-11-20:
    // bronze bars smelted for smithing 59 -> 66 kept by every re-resolve
    // under the old score) is drawn again among the lines of the band.
    let stale = pick_seeded(&set, "fishing", 58, 60, &life, &[], &harpoon_held, &[], 3, Some("gather:fishing/raw_trout")).expect("a line");
    assert!(!stale.choice.sticky && stale.pick.method == "gather:fishing/raw_swordfish", "a previous pick out of the band is no longer taken again: {:?}", stale.choice);
    let only_tool = set_with(vec![{
        let mut row = line("fishing", "gather:fishing/raw_swordfish", Supply::Autonomous, 90_000.0, 0.0);
        row.prereqs.tools = vec!["harpoon".to_string()];
        row
    }]);
    let found = pick(&only_tool, "fishing", 58, 60, &life, &[], &none).expect("after a tool");
    assert_eq!(found.choice.class, 2, "after a tool: drawn, the resolver blocks on it");
}

/// R7 (a): the training guide reads the bot's own stock: the alchemy of
/// the platebodies it holds is a `stock` line (its batch the stock), a
/// bot without the stock has none.
#[test]
fn training_reads_the_bots_own_stock() {
    let mut recipe = money_line();
    recipe.family = Family::Recipe;
    recipe.method = "alch:iron_platebody".to_string();
    recipe.profit_6000 = 12_000.0;
    recipe.xp_6000 = vec![("magic".to_string(), 78_000.0)];
    recipe.prereqs.stock = vec![("iron_platebody".to_string(), 28.0)];
    recipe.confidence = Confidence { runs: 9, ..Confidence::default() };
    let mut set = set_with(vec![line("magic", "teleport:varrock_teleport", Supply::Market, 30_000.0, -60_000.0)]);
    set.money_bands[0].1.rows.push(recipe);
    set.money_bands[0].1.recipes.push(lifescape_planner::guide_read::RecipePlace { row: 1, ask: Default::default(), answer: Default::default() });
    let life = life_with(0.5, 0.5);
    let plates = |item: &str| if item == "iron_platebody" { 40 } else { 0 };
    let stock = vec![("iron_platebody".to_string(), 40.0)];
    let found = pick_seeded(&set, "magic", 58, 60, &life, &[], &plates, &stock, 1, None).expect("a line");
    assert_eq!((found.pick.method.as_str(), found.supply), ("alch:iron_platebody", Supply::Stock));
    assert_eq!(found.pick.batch, 40, "one cast a platebody, the stock bounds the batch");
    let none = |_: &str| 0;
    let without = pick_seeded(&set, "magic", 58, 60, &life, &[], &none, &[], 1, None).expect("a line");
    assert_eq!(without.pick.method, "teleport:varrock_teleport", "no stock, no stock line");
}

/// A fight on a spot that bites a bot without food works after its food:
/// a line feasible now within the band comes first; alone or far ahead,
/// the biting line is drawn with its food to obtain first (`food_gate`).
#[test]
fn a_biting_spot_waits_for_its_food() {
    let mut men = line("attack", "fight:melee/men", Supply::Autonomous, 90_000.0, 800.0);
    men.style = "melee:attack".to_string();
    let mut cows = line("attack", "fight:melee/cows", Supply::Autonomous, 85_000.0, 100.0);
    cows.style = "melee:attack".to_string();
    let life = life_with(0.5, 0.5);
    let level_of = |_: &str| 58u8;
    let none = |_: &str| 0.0;
    let asks = |_: &str| None;
    let bot = BotState { level_of: &level_of, combat: 126, quests_done: &[], held: &none, coins: None, ironman: false, asks: &asks };
    let never = |_: &str| false;
    let food = |method: &str| (method == "fight:melee/men").then(|| "herring".to_string());
    let climb = Climb { skill: "attack", level: 58, target: 60, previous: None, food: Some(&food), coin_rate: MONEY_LINE_RATE };
    let both = set_with(vec![men.clone(), cows]);
    let found = pick_from_guide(&both, &climb, &life, &bot, &[], 1, &never, &lifescape_planner::work_rate::prior_rate).expect("a line");
    assert_eq!((found.pick.method.as_str(), found.choice.class), ("fight:melee/cows", 0), "fed lines first within the band");
    let mut slow = line("attack", "fight:melee/cows", Supply::Autonomous, 30_000.0, 100.0);
    slow.style = "melee:attack".to_string();
    let far = set_with(vec![men.clone(), slow]);
    let found = pick_from_guide(&far, &climb, &life, &bot, &[], 1, &never, &lifescape_planner::work_rate::prior_rate).expect("a line");
    assert_eq!((found.pick.method.as_str(), found.choice.class), ("fight:melee/men", 1), "far ahead: the food first");
    let alone = set_with(vec![men]);
    let found = pick_from_guide(&alone, &climb, &life, &bot, &[], 1, &never, &lifescape_planner::work_rate::prior_rate).expect("a line");
    assert_eq!((found.pick.method.as_str(), found.choice.class), ("fight:melee/men", 1));
    assert!(found.pick.missing.contains(&"herring".to_string()), "the food obtained first: {:?}", found.pick.missing);
}

/// User lever 2026-11-17 (« entraînement marché plus fréquent »), the
/// user's test: smithing 56->60 from the published guide, bars bought
/// (market) or ore mined and smelted (autonomous). A market profile (0.8)
/// takes the market line, the node buys its inputs: the block files the
/// bars as a `market:buy` obtain at the tranche's volume (a need bid);
/// an autarkic profile (0.1) works them up, every production leg kept.
#[test]
fn a_market_profile_climbs_smithing_on_bought_bars() {
    let set = set_with(vec![
        line("smithing", "smith:steel_med_helm", Supply::Autonomous, 1700.0, 510.0),
        line("smithing", "smith:steel_med_helm", Supply::Market, 4400.0, -2640.0),
    ]);
    let none = |_: &str| 0;
    let climb = |life: &Life| {
        let found = pick(&set, "smithing", 56, 60, life, &[], &none).expect("a line");
        let mut node = Node::new("n4", NodeKind::ReachLevel { skill: "smithing".to_string(), level: 60 }, Vec::new(), 60);
        node.status = NodeStatus::Ready;
        let mut ctx = ReadyCtx::blind(life.profile.market_affinity);
        ctx.tools = vec!["hammer".to_string()];
        ctx.training = Some(found.pick.clone());
        resolve_ready(&mut node, &ctx);
        assert_eq!(node.status, NodeStatus::Blocked { reason: "missing:steel_bar".to_string() });
        let fresh = lifescape_planner::replan::replan_node_from(&mut node, &lifescape_planner::replan::ReplanCause::Blocked, &["steel_bar".to_string()], &mut 4, Some(56));
        (found, fresh)
    };
    let (market, fresh) = climb(&life_with(0.8, 0.5));
    assert_eq!(market.supply, Supply::Market);
    assert!(market.pick.buys_inputs);
    // One work session at a time: the tranche's bars, capped by what one
    // 6000-tick session burns (user decision 2026-11-22).
    let tranche = lifescape_planner::sizing::tranche_need("smith:steel_med_helm", 56, 60, "steel_bar").expect("bars");
    let session = lifescape_planner::sizing::session_need_with(&lifescape_planner::work_rate::prior_rate, "smith:steel_med_helm", 56, "steel_bar").expect("session");
    let bought = tranche.min(session);
    assert!(matches!(&fresh[0].kind, NodeKind::ObtainItems { item, count } if item == "steel_bar" && *count == bought), "{:?}", fresh[0].kind);
    assert_eq!(fresh[0].alternatives.iter().map(|alt| alt.method.as_str()).collect::<Vec<_>>(), vec!["market:buy"], "bars bought, never smelted");
    let (autarkic, fresh) = climb(&life_with(0.1, 0.5));
    assert_eq!(autarkic.supply, Supply::Autonomous);
    assert!(!autarkic.pick.buys_inputs);
    assert!(fresh[0].alternatives.iter().any(|alt| alt.method.starts_with("smelt:")), "{:?}", fresh[0].alternatives);
}

/// R7 suite: a `reach_combat` node climbs its style's skill on the guide
/// (ranged, magic), melee its lowest of attack, strength and defence.
#[test]
fn a_combat_level_climbs_its_styles_lowest_skill() {
    let levels = |skill: &str| match skill {
        "attack" => 40,
        "strength" => 32,
        "defence" => 35,
        _ => 10,
    };
    assert_eq!(super::combat_climb_skill("fight:melee/chickens", &levels), "strength");
    assert_eq!(super::combat_climb_skill("fight:ranged/chickens", &levels), "ranged");
    assert_eq!(super::combat_climb_skill("fight:magic/chickens", &levels), "magic");
    assert_eq!(super::combat_climb_skill("fight:melee/chickens", &|_: &str| 20), "attack", "a tie trains attack");
}

/// 2026-11-23, oscar_cole: 5913 iron arrows held, no bronze ones. The
/// obtain node counted the family ("done without launch"); the pick read
/// `bronze_arrow` alone, found it missing and blocked: a loop of 507
/// cycles in 34 min. The pick reads the family too.
#[test]
fn iron_arrows_cover_the_bronze_arrows_of_a_ranged_pick() {
    let set = set_with(vec![line("ranged", "fight:ranged/cows", Supply::Autonomous, 90_000.0, 800.0)]);
    let life = life_with(0.5, 0.5);
    let in_pack = |name: &str| if name == "iron_arrow" { 5913 } else { 0 };
    let family = |item: &str| super::family_units(item, &in_pack);
    let found = pick(&set, "ranged", 58, 60, &life, &[], &family).expect("a line");
    assert_eq!(found.pick.method, "fight:ranged/cows");
    assert!(found.pick.missing.is_empty(), "{:?}", found.pick.missing);
    let exact = |item: &str| in_pack(item);
    let alone = pick(&set, "ranged", 58, 60, &life, &[], &exact).expect("a line");
    assert!(alone.pick.missing.iter().any(|item| item == "bronze_arrow"), "the exact read is the bug: {:?}", alone.pick.missing);
    let staff = |name: &str| if name == "staff_of_air" { 1 } else { 0 };
    assert_eq!(super::family_units("airrune", &staff), 0, "a staff is not counted as units");
}
