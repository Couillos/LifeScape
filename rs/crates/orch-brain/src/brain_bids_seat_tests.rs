//! Live 2026-11-20 22:20 -> 23:20 UTC (after ce3dfdf: one seat per
//! mithril and gold rock field): 59 bots of 74 on a bid answer, 7 on
//! mithril ore (123 `full:farm:mithril_ore` cooldowns in the hour), 14 on
//! 51 amulet of power bids with 6 uncut diamonds on sale. Offline sims of
//! the draw, bot after bot, on the real planner, each bot reading the
//! board the previous ones posted (`brain_bids::bid_goal_in`): the lot's
//! claim, its purchases (`market_claims::claim_purchases`) and its seats
//! (`site_crowd::engage`).

use std::collections::HashMap;

use lifescape_json::{object, Value};
use lifescape_planner::bid_response::{answers_now, Opportunity, Seat};
use lifescape_planner::market_fit::{BookView, MIN_COIN_RATE};
use lifescape_planner::{new_ambition, Archetype, BotNow, GoalKind, Life, Profile, Stamp};

use crate::brain_bids::{choose, BotState, MarketReader};

fn live(levels: &[(&str, i64)]) -> Value {
    let skills = levels
        .iter()
        .map(|(skill, level)| object(vec![("id", Value::Int(i64::from(lifescape_sdk::provenance::skill_id(skill).expect("skill")))), ("level", Value::Int(*level))]))
        .collect();
    object(vec![("skills", Value::Arr(skills))])
}

fn life_of(kind: &str, seed: u64) -> Life {
    let now = BotNow { level_of: &|_| 30, coins: 500, held: &|_| 0 };
    Life {
        schema: 1,
        account: format!("seat{seed}"),
        created_at: 0,
        seed,
        profile: Profile::new(false, 0.5, 0.5, seed, Archetype::Producer),
        goal: GoalKind::MaxSkills { skills: vec!["attack".to_string()], level: 20 },
        nodes: Vec::new(),
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: new_ambition(kind, "need", &now, seed, &[], Stamp { tick: 0, secs: 0 }).ok(),
        goal_origin: None,
        memory: Default::default(),
    }
}

fn units(method: &str, item: &str) -> f64 {
    if item.is_empty() {
        f64::NAN
    } else if method.starts_with("farm:") || method.starts_with("gather:") {
        200.0
    } else {
        600.0
    }
}

/// `count` seeds that weigh `item` in window 0 (the spread lets one bot
/// in two weigh it: the live fleet had 74 bots for these 7 and 14).
fn seeds(item: &str, count: usize) -> Vec<u64> {
    (1..10_000u64).filter(|seed| answers_now(*seed, item, 0)).take(count).collect()
}

/// The board the bots before have posted.
#[derive(Default)]
struct Board {
    claims: HashMap<String, i64>,
    purchases: HashMap<String, i64>,
    seats: Vec<Seat>,
}

impl Board {
    fn post(&mut self, found: &Opportunity) {
        *self.claims.entry(found.item.clone()).or_default() += found.count;
        for (item, units) in found.purchases() {
            *self.purchases.entry(item).or_default() += units;
        }
        for (method, site) in crate::brain_bids::engaged_seats(found) {
            self.seats.push((site, method, 1));
        }
    }
}

/// One draw per bot, in turn, each on the board of those before.
fn draw_in_turn(kind: &str, seeds: &[u64], views: &HashMap<String, BookView>, levels: &[(&str, i64)], held: &dyn Fn(&str) -> i64, board: Board) -> (Vec<Opportunity>, Board) {
    draw_own(kind, seeds, views, levels, held, board, &[], &[])
}

/// `draw_in_turn` with each bot's own cooldown keys and shunned objects.
fn draw_own(kind: &str, seeds: &[u64], views: &HashMap<String, BookView>, levels: &[(&str, i64)], held: &dyn Fn(&str) -> i64, mut board: Board, cooled: &[String], shunned: &[String]) -> (Vec<Opportunity>, Board) {
    let snapshot = live(levels);
    let level_of = |skill: &str| crate::brain_succession::live_level(&snapshot, skill);
    let mut answers = Vec::new();
    for seed in seeds {
        let life = life_of(kind, *seed);
        let reader = MarketReader::fixed(views.clone()).with_board(board.claims.clone(), board.purchases.clone());
        let state = BotState {
            level_of: &level_of,
            held,
            avoid: Vec::new(),
            quests_done: Vec::new(),
            units: &units,
            tick: 0,
            combat: 90,
            position: Some((3213, 3429, 0)),
            planned: Vec::new(),
            caster: None,
            rate: &lifescape_planner::work_rate::prior_rate,
            seats: board.seats.clone(),
            cooled: cooled.to_vec(),
            shunned: shunned.to_vec(),
        };
        if let Some((found, _)) = choose(&life, &reader, MIN_COIN_RATE, &state) {
            board.post(&found);
            answers.push(found);
        }
    }
    (answers, board)
}

/// Test 1: seven miners weigh a 93 mithril ore bid; every mithril rock
/// field holds one miner (its respawns feed one, `site_capacity`), one
/// field already has its miner. Only the bots that find a free seat
/// answer, never two on one field, never the occupied one; without the
/// seats the halving shares (47, 23, 12, 6, 3, 1, 1) sent all seven.
#[test]
fn only_the_miners_with_a_free_seat_answer_the_mithril_bid() {
    let mut views = HashMap::new();
    views.insert("mithril_ore".to_string(), BookView { bids: vec![(339, 93)], ..BookView::default() });
    let levels = [("mining", 60), ("attack", 60), ("strength", 60), ("defence", 60), ("hitpoints", 60)];
    let held = |item: &str| i64::from(item == "rune_pickaxe");
    let miners = seeds("mithril_ore", 7);
    // The seats every miner could take, the free board first.
    let (alone, _) = draw_in_turn("miner", &miners[..1], &views, &levels, &held, Board::default());
    let first = alone.first().expect("a lone miner answers");
    let (method, taken) = crate::brain_bids::engaged_seats(first).into_iter().next().expect("the lot takes a mithril seat");
    assert_eq!(lifescape_planner::combat::site_capacity(&method, taken), Some(1), "one miner per mithril field");
    let board = Board { seats: vec![(taken, "gather:mining/mithril_ore".to_string(), 1)], ..Board::default() };
    let (answers, board) = draw_in_turn("miner", &miners, &views, &levels, &held, board);
    let mithril: Vec<&Opportunity> = answers.iter().filter(|found| found.item == "mithril_ore").collect();
    let free = free_fields("farm:mithril_ore", 90, taken);
    assert!(free >= 1, "a free mithril field outside the occupied one");
    assert_eq!(mithril.len() as i64, free.min(miners.len() as i64), "exactly one answer per free field: {} of {} miners", mithril.len(), miners.len());
    let mut fields: Vec<(i64, i64, i64)> = Vec::new();
    for found in &mithril {
        let seats = crate::brain_bids::engaged_seats(found);
        assert_eq!(seats.len(), 1, "{seats:?}");
        let site = seats[0].1;
        assert!(!lifescape_planner::cooldown::same_area(site, taken), "the occupied field is full: {site:?}");
        assert!(!fields.iter().any(|known| lifescape_planner::cooldown::same_area(*known, site)), "two answers on one field: {site:?} in {fields:?}");
        fields.push(site);
    }
    // Every field taken: the next miner finds none.
    let (late, _) = draw_in_turn("miner", &seeds("mithril_ore", 8)[7..], &views, &levels, &held, board);
    assert!(late.iter().all(|found| found.item != "mithril_ore"), "no seat left: {late:?}");
}

/// Free seats of `method` for a bot at `combat` once the field of
/// `taken` is full: its open sites (no gate but the Karamja boat, safe at
/// that combat level) grouped by working area, each area's capacity.
fn free_fields(method: &str, combat: u8, taken: (i64, i64, i64)) -> i64 {
    use lifescape_gamedata::access::Gate;
    let table = lifescape_planner::combat::method_site_table(method).expect("mithril sites");
    let mut areas: Vec<(i64, i64, i64)> = Vec::new();
    for row in table.iter().filter(|row| row.open_ever && row.required <= combat && row.gates.iter().all(|gate| matches!(gate, Gate::Boat { area: "karamja", .. }))) {
        if !areas.iter().any(|known| lifescape_planner::cooldown::same_area(*known, row.site)) {
            areas.push(row.site);
        }
    }
    areas.iter().filter(|site| !lifescape_planner::cooldown::same_area(**site, taken)).map(|site| lifescape_planner::combat::site_capacity(method, *site).unwrap_or(0)).sum()
}

/// Review point 2: a bot's own cooled sites (other players on the rocks,
/// `contended:`) are no site for an answer, and an object whose answer it
/// gave back is left alone for `BID_SHUN_TICKS`: no slow loop of
/// answer, wait, give back on a block the fleet's seats do not show.
#[test]
fn cooled_sites_and_a_recent_give_back_keep_the_answer_away() {
    let mut views = HashMap::new();
    views.insert("mithril_ore".to_string(), BookView { bids: vec![(339, 93)], ..BookView::default() });
    let levels = [("mining", 60), ("attack", 60), ("strength", 60), ("defence", 60), ("hitpoints", 60)];
    let held = |item: &str| i64::from(item == "rune_pickaxe");
    let miner = seeds("mithril_ore", 1);
    let (alone, _) = draw_in_turn("miner", &miner, &views, &levels, &held, Board::default());
    let (_, first) = crate::brain_bids::engaged_seats(alone.first().expect("a lone miner answers")).into_iter().next().expect("a seat");
    // Its first field cooled for it (players there): another field.
    let cool_first = vec![lifescape_planner::cooldown::cool_key("farm:mithril_ore", Some(first))];
    let (moved, _) = draw_own("miner", &miner, &views, &levels, &held, Board::default(), &cool_first, &[]);
    let (_, second) = crate::brain_bids::engaged_seats(moved.first().expect("another field")).into_iter().next().expect("a seat");
    assert!(!lifescape_planner::cooldown::same_area(first, second), "the cooled field is skipped: {first:?} / {second:?}");
    // Every free field cooled or taken: no answer.
    let cool_both = vec![cool_first[0].clone(), lifescape_planner::cooldown::cool_key("farm:mithril_ore", Some(second))];
    let others = Board { seats: free_seats_but(first, second), ..Board::default() };
    let (none, _) = draw_own("miner", &miner, &views, &levels, &held, others, &cool_both, &[]);
    assert!(none.iter().all(|found| found.item != "mithril_ore"), "{none:?}");
    // Given back lately: left alone, whatever the seats.
    let (shunned, _) = draw_own("miner", &miner, &views, &levels, &held, Board::default(), &[], &["mithril_ore".to_string()]);
    assert!(shunned.iter().all(|found| found.item != "mithril_ore"), "{shunned:?}");
}

/// Every open mithril field but `first` and `second`, one miner each.
fn free_seats_but(first: (i64, i64, i64), second: (i64, i64, i64)) -> Vec<Seat> {
    use lifescape_gamedata::access::Gate;
    let table = lifescape_planner::combat::method_site_table("farm:mithril_ore").expect("mithril sites");
    table
        .iter()
        .filter(|row| row.open_ever && row.gates.iter().all(|gate| matches!(gate, Gate::Boat { area: "karamja", .. })))
        .filter(|row| !lifescape_planner::cooldown::same_area(row.site, first) && !lifescape_planner::cooldown::same_area(row.site, second))
        .map(|row| (row.site, "gather:mining/mithril_ore".to_string(), 1))
        .collect()
}

/// The live input asks (2026-11-19): 6 uncut diamonds, plenty of the
/// rest (`bid_response_reach_tests`).
fn amulet_views() -> HashMap<String, BookView> {
    let asks = |price: i64, qty: i64| BookView { asks: vec![(price, qty)], ..BookView::default() };
    let mut views = HashMap::new();
    let prices = [
        1_042_900, 320_103, 269_511, 206_433, 185_596, 180_980, 170_740, 162_353, 160_174, 156_645, 155_175, 154_906, 138_071, 119_242, 118_777, 116_957, 112_489, 104_773,
        103_618, 101_610, 97_563, 84_946, 83_120, 82_456, 80_463, 75_153, 74_801, 74_438, 72_301, 57_894, 55_766, 50_494, 47_324, 37_850, 35_881, 31_164, 31_066, 30_178,
        26_422, 26_131, 23_750, 23_323, 19_690, 17_374, 13_319, 12_580, 9_187, 8_402, 4_206, 3_688, 3_557,
    ];
    views.insert("amulet_of_power".to_string(), BookView { bids: prices.iter().map(|price| (*price, 1)).collect(), ..BookView::default() });
    views.insert("uncut_diamond".to_string(), BookView { asks: vec![(1, 1), (140, 1), (146, 1), (170, 3)], ..BookView::default() });
    views.insert("gold_bar".to_string(), asks(35, 369));
    views.insert("ball_of_wool".to_string(), asks(3, 229));
    views.insert("cosmicrune".to_string(), asks(7, 1075));
    views.insert("earthrune".to_string(), asks(1, 42_241));
    views
}

/// Test 3: fourteen mages (magic enchants the amulet: their trade,
/// `bid_trade`) weigh the 51 amulet of power bids; 6 uncut
/// diamonds rest on the market. Each answer reserves the diamonds it
/// buys: at most 6 amulets are answered in all, by at most 6 bots.
#[test]
fn the_amulet_answers_stop_at_the_diamonds_on_sale() {
    let levels = [("crafting", 99), ("magic", 99), ("attack", 40), ("strength", 40), ("defence", 40), ("hitpoints", 40)];
    let held = |item: &str| i64::from(matches!(item, "amulet_mould" | "chisel" | "hammer"));
    let mages = seeds("amulet_of_power", 14);
    let (answers, board) = draw_in_turn("mage", &mages, &amulet_views(), &levels, &held, Board::default());
    let amulets: Vec<&Opportunity> = answers.iter().filter(|found| found.item == "amulet_of_power").collect();
    let made: i64 = amulets.iter().map(|found| found.count).sum();
    assert!(!amulets.is_empty(), "the diamonds on sale are answered");
    assert!(made <= 6 && amulets.len() <= 6, "{} bots for {made} amulets with 6 diamonds: {:?}", amulets.len(), amulets.iter().map(|found| found.count).collect::<Vec<_>>());
    assert_eq!(board.purchases.get("uncut_diamond").copied().unwrap_or(0), made, "each amulet reserved its diamond");
}

/// `bid_trade` (live 2026-11-20 00:15 -> 01:15 UTC: priests on mithril
/// battleaxes, runecrafters on mithril bars): the same mithril ore bid,
/// the same levels and free fields, drawn by kinds whose primary skill
/// makes no ore: none answers; the miners do.
#[test]
fn only_the_trade_that_makes_the_object_answers_it() {
    let mut views = HashMap::new();
    views.insert("mithril_ore".to_string(), BookView { bids: vec![(339, 93)], ..BookView::default() });
    let levels = [("mining", 60), ("smithing", 60), ("runecraft", 60), ("attack", 60), ("strength", 60), ("defence", 60), ("hitpoints", 60)];
    let held = |item: &str| i64::from(item == "rune_pickaxe");
    let bots = seeds("mithril_ore", 3);
    for kind in ["priest", "runecrafter", "smith", "firemaker", "cook"] {
        let (answers, _) = draw_in_turn(kind, &bots, &views, &levels, &held, Board::default());
        assert!(answers.iter().all(|found| found.item != "mithril_ore"), "a {kind} answers no ore: {answers:?}");
    }
    let (miners, _) = draw_in_turn("miner", &bots, &views, &levels, &held, Board::default());
    assert!(miners.iter().any(|found| found.item == "mithril_ore"), "the miners do");
}
