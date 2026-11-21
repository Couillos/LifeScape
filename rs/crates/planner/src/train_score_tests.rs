use super::{best as choose, buy_weight, coin_weight, market_weight, rank_rows, score_row, ticks_per_coin};
use crate::guide_model::{Confidence, Prereqs, SourceGap, Supply, TrainingRow};
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
        priced: true,
    }
}

/// The test bot's judgement (D22): a level-56 bot without quests.
fn reach(row: &TrainingRow) -> bool {
    let filter = crate::guide_model::GuideFilter { level: Some(56), ..crate::guide_model::GuideFilter::default() };
    crate::guide_feasible::with_filter_bot(&filter, |judge| crate::guide_feasible::feasible_for(row, judge).within_reach())
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
        let market = choose(&rank_rows(&rows, &bot(1.0, thrift), per_coin, &reach)).expect("a line");
        assert_eq!(market.supply, Supply::Market, "market bot, thrift {thrift}");
        let autarkic = choose(&rank_rows(&rows, &bot(0.0, thrift), per_coin, &reach)).expect("a line");
        assert_eq!(autarkic.supply, Supply::Autonomous, "autarkic bot, thrift {thrift}");
    }
}

/// A rune pickaxe nobody can make at the level (R6 example): the
/// autonomous line is blocked, so every profile buys.
#[test]
fn every_bot_buys_what_it_cannot_farm() {
    let mut farm = line("gather:mining/runite_ore", Supply::Autonomous, 30_000.0, 0.0);
    farm.prereqs.sources = vec![SourceGap { item: "rune_pickaxe".to_string(), reason: "no autonomous source".to_string() }];
    let rows = vec![farm, line("gather:mining/runite_ore", Supply::Market, 30_000.0, -3000.0)];
    for (affinity, thrift) in [(0.0, 0.0), (0.0, 1.0), (1.0, 0.0), (1.0, 1.0), (0.5, 0.5)] {
        let picked = choose(&rank_rows(&rows, &bot(affinity, thrift), ticks_per_coin(Some(BEST_PROFIT)), &reach)).expect("the market line");
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
    let fast = choose(&rank_rows(&rows, &bot(0.5, 0.0), per_coin, &reach)).expect("a line");
    assert_eq!(fast.method, "teleport:varrock_teleport");
    let thrifty = choose(&rank_rows(&rows, &bot(0.5, 1.0), per_coin, &reach)).expect("a line");
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
    assert!((fond - 0.25).abs() < 1e-12 && fond < wary, "earnings count zero, a market bot still prefers the market line: {fond} vs {wary}");
    let blind = score_row(&bought, &bot(0.5, 1.0), None).expect("scored");
    assert!((blind.score - 0.1).abs() < 1e-12, "no money line: time alone");
    assert!(ticks_per_coin(Some(-5.0)).is_none() && ticks_per_coin(None).is_none());
}

/// Lines without XP or a market line for an Ironman never score; lines
/// out of the bot's reach (D22: a missing source, a level above its own)
/// never rank.
#[test]
fn unrunnable_lines_never_score() {
    let per_coin = ticks_per_coin(Some(BEST_PROFIT));
    let idle = line("fire:yew_logs", Supply::Autonomous, 0.0, 0.0);
    assert!(score_row(&idle, &bot(0.5, 0.5), per_coin).is_none(), "{idle:?}");
    let mut sourceless = line("fire:yew_logs", Supply::Autonomous, 24_000.0, 0.0);
    sourceless.prereqs.sources = vec![SourceGap { item: "yew_logs".to_string(), reason: "no autonomous source".to_string() }];
    let mut above = line("fire:yew_logs", Supply::Autonomous, 24_000.0, 0.0);
    above.prereqs.levels = vec![("firemaking".to_string(), 60)];
    let rows = vec![sourceless, above];
    assert!(rank_rows(&rows, &bot(0.5, 0.5), per_coin, &reach).is_empty(), "out of reach: never ranked");
    let iron = Profile::new(true, 0.5, 0.5, 7, Archetype::Producer);
    assert!(score_row(&line("fire:yew_logs", Supply::Market, 60_000.0, -30_000.0), &iron, per_coin).is_none(), "an Ironman never buys");
    assert!(choose(&[]).is_none());
}

/// The market weight of a training line is a continuous, decreasing
/// curve of market_affinity (user lever 2026-11-17, « entraînement
/// marché plus fréquent »): neutral at 0.5, a market profile (≥ 0.6)
/// discounts buying, an autarkic one (≤ 0.4) taxes it.
#[test]
fn the_buy_weight_follows_the_trait_continuously() {
    assert!((buy_weight(0.5) - 1.0).abs() < 1e-12);
    assert!(buy_weight(0.6) < 1.0 && buy_weight(0.4) > 1.0);
    assert!(buy_weight(0.8) < 0.5 && buy_weight(0.1) > 2.5, "{} {}", buy_weight(0.8), buy_weight(0.1));
    let mut previous = f64::INFINITY;
    for step in 0..=100 {
        let affinity = f64::from(step) / 100.0;
        let weight = buy_weight(affinity);
        assert!(weight < previous, "decreasing at {affinity}");
        assert!(previous.is_infinite() || previous - weight < 0.12, "continuous at {affinity}: {previous} -> {weight}");
        previous = weight;
    }
    assert_eq!(buy_weight(-3.0), buy_weight(0.0));
    assert_eq!(buy_weight(7.0), buy_weight(1.0));
}

/// Smithing 40-45 (user test 2026-11-17): bars bought (market: 4400 XP
/// per 6000 ticks, 0.6 coin per XP) against ore mined, smelted and
/// smithed (autonomous: 1700 XP per 6000 ticks, earning 0.3 coin per
/// XP). At the fleet's D17 (30 000 coins per 6000 ticks) a market
/// profile (0.8) buys its bars, an autarkic one (0.1) works them up; a
/// bot whose best money line earns little (D17 high: 300 coins per 6000
/// ticks, 20 ticks a coin) works them up unless it is fast (thrift 0:
/// its coins weigh half, the bars it buys go 2.6 times faster; the
/// autonomous line's earnings count zero since 2026-11-20).
#[test]
fn a_market_profile_buys_its_bars_while_its_coins_allow_it() {
    let mut bought = line("smith:steel_platebody", Supply::Market, 4400.0, -2640.0);
    bought.skill = "smithing".to_string();
    let mut worked = line("smith:steel_platebody", Supply::Autonomous, 1700.0, 510.0);
    worked.skill = "smithing".to_string();
    let rows = vec![worked, bought];
    let fleet = ticks_per_coin(Some(BEST_PROFIT));
    for thrift in [0.0, 0.5, 1.0] {
        let market = choose(&rank_rows(&rows, &bot(0.8, thrift), fleet, &reach)).expect("a line");
        assert_eq!(market.supply, Supply::Market, "market profile 0.8, thrift {thrift}");
        let autarkic = choose(&rank_rows(&rows, &bot(0.1, thrift), fleet, &reach)).expect("a line");
        assert_eq!(autarkic.supply, Supply::Autonomous, "autarkic profile 0.1, thrift {thrift}");
        let poor = choose(&rank_rows(&rows, &bot(0.8, thrift), ticks_per_coin(Some(300.0)), &reach)).expect("a line");
        let expected = if thrift == 0.0 { Supply::Market } else { Supply::Autonomous };
        assert_eq!(poor.supply, expected, "D17 high: the autonomous line again unless fast, thrift {thrift}");
    }
}

/// Smithing 60-68 (live 2026-11-20, lara_lane n1628, bellarhodes n3350):
/// copper and tin mined and smelted into bronze bars (autonomous, 0.09 XP
/// per tick, the bars selling for 1.59 coins per tick) against steel
/// platebodies of bought bars (market, 3.7 XP per tick, the plates
/// earning a little too), at the bot's measured time value (D1: 0.67 coin
/// per tick). The old score credited the bronze bars' coins at 1.49 ticks
/// each: −5.5 per XP against −1.3, the climb took the line 40 times
/// slower (40 060 copper ore filed for 59 -> 66). A climb is chosen for
/// its XP: what a line earns makes it free at best, never better than a
/// faster line; the fast line wins for every profile.
#[test]
fn earned_coins_never_make_a_slow_climb_win() {
    let mut smelted = line("smelt:bronze_bar", Supply::Autonomous, 538.0, 9540.0);
    smelted.skill = "smithing".to_string();
    let mut plated = line("smith:steel_platebody", Supply::Market, 22_200.0, 2400.0);
    plated.skill = "smithing".to_string();
    let rows = vec![smelted, plated];
    let per_coin = Some(1.0 / 0.67);
    for (affinity, thrift) in [(0.52, 0.13), (0.68, 0.93), (0.1, 0.5), (0.0, 1.0), (1.0, 0.0)] {
        let ranked = rank_rows(&rows, &bot(affinity, thrift), per_coin, &|_: &TrainingRow| true);
        let picked = choose(&ranked).expect("a line");
        assert_eq!(picked.method, "smith:steel_platebody", "affinity {affinity}, thrift {thrift}: {ranked:?}");
        assert!(ranked.iter().all(|(score, _)| score.score >= 0.0), "earnings never push a score under zero: {ranked:?}");
    }
}

/// A fight line's flow of `value_6000` coins a window (negative: spent).
fn flow(item: &str, cause: &str, value_6000: f64) -> crate::guide_model::FlowLine {
    crate::guide_model::FlowLine {
        item: item.to_string(),
        cause: cause.to_string(),
        per_6000: value_6000,
        unit_price: 1.0,
        value_6000,
        outlet: crate::guide_model::Outlet::Market,
        traded: true,
        basis: crate::guide_model::PriceBasis::Traded,
    }
}

/// User 2026-11-20 (« pour la magie, si le bot veut s'entraîner alors il
/// faut privilégier l'xp/coût »): the runes a training fight casts are
/// its cost, whatever its drops bring (the loot earns, counted zero like
/// any earning, never paying the runes). Crumble undead whose skeletons
/// drop more than its chaos runes cost no longer trains for free ahead
/// of the earth strike on cows; a ranged fight's arrows the same.
#[test]
fn a_training_fight_pays_its_runes_whatever_its_drops() {
    let mut crumble = line("fight:magic/crumble_undead/wild_skeletons", Supply::Market, 20_000.0, 10_000.0);
    crumble.skill = "magic".to_string();
    crumble.flows = vec![flow("chaosrune", "rune", -18_000.0), flow("airrune", "rune", -2000.0), flow("steel_platebody", "loot", 30_000.0)];
    let mut strike = line("fight:magic/earth_strike/cows", Supply::Market, 15_000.0, -1000.0);
    strike.skill = "magic".to_string();
    strike.flows = vec![flow("earthrune", "rune", -1000.0)];
    let rows = vec![crumble, strike];
    let ranked = rank_rows(&rows, &bot(0.5, 0.5), Some(0.5), &|_: &TrainingRow| true);
    assert_eq!(choose(&ranked).map(|row| row.method.as_str()), Some("fight:magic/earth_strike/cows"), "{ranked:?}");
    let crumble_score = &ranked.iter().find(|(_, row)| row.method.contains("crumble")).expect("crumble scored").0;
    assert!((crumble_score.coin_ticks_per_xp - 0.5).abs() < 1e-9, "20 000 coins of runes over 20 000 XP at 0.5 tick a coin: {crumble_score:?}");
    let mut shot = line("fight:ranged/hill_giants", Supply::Market, 20_000.0, 10_000.0);
    shot.flows = vec![flow("rune_arrow", "ammo", -20_000.0), flow("big_bones", "loot", 30_000.0)];
    let mut cheap = line("fight:ranged/cows", Supply::Market, 15_000.0, -500.0);
    cheap.flows = vec![flow("bronze_arrow", "ammo", -500.0)];
    let rows = vec![shot, cheap];
    let ranked = rank_rows(&rows, &bot(0.5, 0.5), Some(0.5), &|_: &TrainingRow| true);
    assert_eq!(choose(&ranked).map(|row| row.method.as_str()), Some("fight:ranged/cows"), "{ranked:?}");
    // Alchemy keeps its balance: the coins it makes are its product.
    let mut alch = line("alch:iron_platebody", Supply::Market, 20_000.0, 5000.0);
    alch.flows = vec![flow("naturerune", "rune", -18_000.0), flow("coins", "product", 30_000.0)];
    let scored = score_row(&alch, &bot(0.5, 0.5), Some(0.5)).expect("alchemy scored");
    assert_eq!(scored.coin_ticks_per_xp, 0.0, "alchemy earns: free at best: {scored:?}");
}

fn classed(row: &TrainingRow, score: f64, class: u8) -> super::Classed<'_> {
    super::Classed { score: super::LineScore { own_ticks_per_xp: score, coin_ticks_per_xp: 0.0, score }, row, class }
}

/// Live 2026-11-20 22:09 -> 23:09 UTC: every crafter without a needle
/// shaped bowls (1.1 XP/tick) while leather (6 to 8.5 XP/tick) waited a
/// class behind, 162 000 cow hides banked. A line far ahead that needs a
/// tool first is drawn; one only a little ahead leaves the feasible line.
#[test]
fn a_line_far_ahead_wins_over_its_tool_a_close_one_does_not() {
    let bowls = line("pottery:bowl_empty", Supply::Market, 6_600.0, -100.0);
    let leather = line("craft:leather_chaps", Supply::Market, 51_000.0, 1_500.0);
    let far = vec![classed(&leather, 0.13, 2), classed(&bowls, 0.9, 0)];
    assert_eq!(super::drawn_class(&far), Some(2));
    let (index, _) = super::pick(&far, 7).expect("a pick");
    assert_eq!(far[index].row.method, "craft:leather_chaps");
    let close = vec![classed(&leather, 0.13, 2), classed(&bowls, 0.14, 0)];
    assert_eq!(super::drawn_class(&close), Some(0), "within the band the feasible line keeps its place");
    assert_eq!(super::drawn_class(&[]), None);
}

/// A1 (a), user 2026-11-21: a lucrative target pulls the climb. While a
/// climb that opens a lucrative way is pending, a tick of the bot's time
/// is worth that way's gain per tick (`investment::time_value`): the
/// paid fast line that wins by the hours it saves beats the free slow one
/// a poor bot would take.
#[test]
fn a_lucrative_target_makes_the_bot_pay_for_a_faster_climb() {
    let rows = vec![line("fire:logs", Supply::Autonomous, 10_000.0, 0.0), line("fire:logs", Supply::Market, 30_000.0, -6_000.0)];
    let profile = bot(0.5, 0.5);
    let pick = |coin_rate: f64| choose(&rank_rows(&rows, &profile, Some(1.0 / coin_rate), &reach)).expect("a line").supply;
    let poor = 0.2;
    assert_eq!(pick(poor), Supply::Autonomous, "a poor bot works its own logs");
    let pending = vec![crate::investment::Investment {
        item: "amulet_of_power".to_string(),
        method: "enchant:strung_diamond_amulet".to_string(),
        gates: Vec::new(),
        climb_ticks: 20_000.0,
        gain_per_tick: 40.0,
        margin: 900_000.0,
        capacity: 66.0,
        units_per_tick: 0.3,
        value_6000: 30_000.0,
    }];
    let pulled = crate::investment::time_value(poor, &pending);
    assert!((pulled - 5.0).abs() < 1e-9, "30 000 per 6000 ticks = 5 coins a tick: {pulled}");
    assert_eq!(pick(pulled), Supply::Market, "the target makes time worth more than the coins");
}
