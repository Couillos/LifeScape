use super::{choose, coin_weight, market_weight, rank_rows, score_row, ticks_per_coin};
use crate::guide_model::{Confidence, Prereqs, Supply, TrainingRow};
use crate::model::{Archetype, Profile};

/// A fabricated training line: `xp_6000` of the row's own time, `coins_6000`
/// (− spent, + earned).
fn line(method: &str, supply: Supply, xp_6000: f64, coins_6000: f64) -> TrainingRow {
    TrainingRow {
        skill: "firemaking".to_string(),
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
        confidence: Confidence::default(),
        price_confidence: "traded",
        blocked: Vec::new(),
        accessible: true,
    }
}

fn bot(market_affinity: f64, thrift: f64) -> Profile {
    Profile::new(false, market_affinity, 0.5, 7, Archetype::Producer).with_thrift(thrift)
}

/// D17 of the test bots: the best money line earns 30 000 per 6000
/// ticks, a coin costs 0.2 tick.
const BEST_PROFIT: f64 = 30_000.0;

/// Firemaking yew (R6 example): cutting the logs yourself (autonomous,
/// 30 000 XP per 6000 ticks of cutting and burning) against buying them
/// (market, 60 000 XP per 6000 ticks, 0.5 coin per XP). A market bot
/// buys the yew, an autarkic one cuts it.
#[test]
fn a_market_bot_buys_its_yew_an_autarkic_one_cuts_it() {
    let rows = vec![line("fire:yew_logs", Supply::Autonomous, 30_000.0, 0.0), line("fire:yew_logs", Supply::Market, 60_000.0, -30_000.0)];
    let per_coin = ticks_per_coin(Some(BEST_PROFIT));
    for thrift in [0.0, 0.5, 1.0] {
        let market = choose(&rank_rows(&rows, &bot(1.0, thrift), per_coin)).expect("a line");
        assert_eq!(market.supply, Supply::Market, "market bot, thrift {thrift}");
        let autarkic = choose(&rank_rows(&rows, &bot(0.0, thrift), per_coin)).expect("a line");
        assert_eq!(autarkic.supply, Supply::Autonomous, "autarkic bot, thrift {thrift}");
    }
}

/// A rune pickaxe nobody can make at the level (R6 example): the
/// autonomous line is blocked, so every profile buys.
#[test]
fn every_bot_buys_what_it_cannot_farm() {
    let mut farm = line("gather:mining/runite_ore", Supply::Autonomous, 30_000.0, 0.0);
    farm.blocked = vec!["rune_pickaxe: no autonomous source".to_string()];
    let rows = vec![farm, line("gather:mining/runite_ore", Supply::Market, 30_000.0, -3000.0)];
    for (affinity, thrift) in [(0.0, 0.0), (0.0, 1.0), (1.0, 0.0), (1.0, 1.0), (0.5, 0.5)] {
        let picked = choose(&rank_rows(&rows, &bot(affinity, thrift), ticks_per_coin(Some(BEST_PROFIT)))).expect("the market line");
        assert_eq!(picked.supply, Supply::Market, "affinity {affinity}, thrift {thrift}");
    }
}

/// Rapide ↔ économe: two market lines, one fast and dear (0.05 tick and
/// 1 coin per XP), one slow and cheap (0.2 tick and 0.1 coin per XP). A
/// fast bot takes the dear one, a thrifty bot the cheap one.
#[test]
fn a_fast_bot_spends_a_thrifty_one_saves() {
    let rows = vec![line("teleport:varrock_teleport", Supply::Market, 120_000.0, -120_000.0), line("curse:confuse/chickens", Supply::Market, 30_000.0, -3000.0)];
    let per_coin = ticks_per_coin(Some(BEST_PROFIT));
    let fast = choose(&rank_rows(&rows, &bot(0.5, 0.0), per_coin)).expect("a line");
    assert_eq!(fast.method, "teleport:varrock_teleport");
    let thrifty = choose(&rank_rows(&rows, &bot(0.5, 1.0), per_coin)).expect("a line");
    assert_eq!(thrifty.method, "curse:confuse/chickens");
}

/// The terms: own ticks per XP, coins per XP at D17, weights 0.5 + thrift
/// and 1.5 − affinity; an earning line keeps its order under the market
/// weight; no earning money line = time alone.
#[test]
fn the_score_weighs_time_coins_and_supply() {
    assert_eq!((coin_weight(0.0), coin_weight(1.0), coin_weight(7.0)), (0.5, 1.5, 1.5));
    assert_eq!((market_weight(0.0), market_weight(1.0)), (1.5, 0.5));
    let per_coin = ticks_per_coin(Some(BEST_PROFIT));
    let bought = line("fire:yew_logs", Supply::Market, 60_000.0, -30_000.0);
    let neutral = score_row(&bought, &bot(0.5, 0.5), per_coin).expect("scored");
    assert!((neutral.own_ticks_per_xp - 0.1).abs() < 1e-12 && (neutral.coin_ticks_per_xp - 0.1).abs() < 1e-12, "{neutral:?}");
    assert!((neutral.score - 0.2).abs() < 1e-12, "neutral weights: the D17 total");
    let earning = line("smith:bronze_dagger", Supply::Market, 6000.0, 600_000.0);
    let fond = score_row(&earning, &bot(1.0, 0.5), per_coin).expect("scored").score;
    let wary = score_row(&earning, &bot(0.0, 0.5), per_coin).expect("scored").score;
    assert!(fond < 0.0 && fond < wary, "a market bot still prefers the earning market line: {fond} vs {wary}");
    let blind = score_row(&bought, &bot(0.5, 1.0), None).expect("scored");
    assert!((blind.score - 0.1).abs() < 1e-12, "no money line: time alone");
    assert!(ticks_per_coin(Some(-5.0)).is_none() && ticks_per_coin(None).is_none());
}

/// Lines a bot cannot run never score: blocked, inaccessible, no XP, a
/// market line for an Ironman.
#[test]
fn unrunnable_lines_never_score() {
    let per_coin = ticks_per_coin(Some(BEST_PROFIT));
    let mut blocked = line("fire:yew_logs", Supply::Autonomous, 24_000.0, 0.0);
    blocked.blocked = vec!["yew_logs".to_string()];
    let mut closed = line("fire:yew_logs", Supply::Autonomous, 24_000.0, 0.0);
    closed.accessible = false;
    let idle = line("fire:yew_logs", Supply::Autonomous, 0.0, 0.0);
    for row in [&blocked, &closed, &idle] {
        assert!(score_row(row, &bot(0.5, 0.5), per_coin).is_none(), "{row:?}");
    }
    let iron = Profile::new(true, 0.5, 0.5, 7, Archetype::Producer);
    assert!(score_row(&line("fire:yew_logs", Supply::Market, 60_000.0, -30_000.0), &iron, per_coin).is_none(), "an Ironman never buys");
    assert!(choose(&[]).is_none());
}
