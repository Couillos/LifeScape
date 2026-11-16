//! Guide engine on fabricated worlds (plan rendements R4): one line per
//! F2P method, profit = Σ flows × price, D5, D15, D16, D17, derived XP,
//! quest shortcuts. Nothing reads the store, the snapshot or `run/`.

use std::collections::HashMap;

use crate::guide_engine::{round_trip_ticks, Engine};
use crate::guide_model::{Family, GuideFilter, GuideWorld, Quote, RateAnswer, RateAsk, Supply, VARROCK_SQUARE};
use crate::guide_money::money_guide;
use crate::guide_training::training_guide;

fn answer(flows: &[(&str, &str, f64)], units: &[(&str, &str, f64)]) -> RateAnswer {
    RateAnswer {
        flows: flows.iter().map(|(item, cause, qty)| (item.to_string(), cause.to_string(), *qty)).collect(),
        units: units.iter().map(|(kind, subject, qty)| (kind.to_string(), subject.to_string(), *qty)).collect(),
        pause_per_6000: 0.0,
        confidence: crate::guide_model::Confidence { measured: true, runs: 5, read: "Exact".to_string(), ..Default::default() },
    }
}

/// The steel world: coal and iron mined, steel smelted, logs burnt.
fn steel_yields() -> HashMap<&'static str, RateAnswer> {
    let mut yields = HashMap::new();
    yields.insert("gather:mining/coal", answer(&[("coal", "product", 100.0)], &[("product", "coal", 100.0)]));
    yields.insert("gather:mining/iron_ore", answer(&[("iron_ore", "product", 120.0)], &[("product", "iron_ore", 120.0)]));
    yields.insert("smelt:steel_bar", answer(&[("steel_bar", "product", 100.0), ("iron_ore", "input", -100.0), ("coal", "input", -200.0)], &[("product", "steel_bar", 100.0)]));
    yields.insert("fire:logs", answer(&[("logs", "input", -300.0)], &[("input", "logs", 300.0)]));
    yields
}

fn prices() -> HashMap<&'static str, f64> {
    HashMap::from([("coal", 15.0), ("iron_ore", 10.0), ("steel_bar", 50.0), ("logs", 4.0)])
}

/// Resting book of a never-traded object: (bids, asks), best first.
type Book = (Vec<(f64, f64)>, Vec<(f64, f64)>);

struct Fixture {
    yields: HashMap<&'static str, RateAnswer>,
    /// Traded objects (D12 current price).
    prices: HashMap<&'static str, f64>,
    /// Never-traded objects with resting orders.
    books: HashMap<&'static str, Book>,
    /// Whole quotes (a median, its fills, a clamping book).
    quotes: HashMap<&'static str, Quote>,
    npc: HashMap<&'static str, f64>,
    shop: HashMap<&'static str, f64>,
    wait: Option<f64>,
    quests: HashMap<&'static str, f64>,
}

impl Fixture {
    fn new() -> Self {
        Fixture { yields: steel_yields(), prices: prices(), books: HashMap::new(), quotes: HashMap::new(), npc: HashMap::new(), shop: HashMap::new(), wait: Some(100.0), quests: HashMap::new() }
    }

    fn run<T>(&self, body: impl FnOnce(&Engine) -> T) -> T {
        let rate = |ask: &RateAsk| self.yields.get(ask.method.as_str()).cloned().ok_or_else(|| format!("no yield for {}", ask.method));
        let price = |item: &str| match (self.quotes.get(item), self.prices.get(item)) {
            (Some(quote), _) => quote.clone(),
            (None, Some(price)) => Quote { price: *price, traded: true, median: *price, fills: 5, ..Quote::default() },
            (None, None) => {
                let (bids, asks) = self.books.get(item).cloned().unwrap_or_default();
                // The book midpoint a never-traded quote carries: no value.
                Quote { price: 1.0, traded: false, bids, asks, ..Quote::default() }
            }
        };
        let npc = |item: &str| self.npc.get(item).copied();
        let shop = |item: &str| self.shop.get(item).copied();
        let quest = |key: &str| self.quests.get(key).copied();
        let world = GuideWorld { rate: &rate, price: &price, npc_sale: &npc, shop_buy: &shop, market_wait_ticks: self.wait, quest_ticks: &quest, measured: &[] };
        let engine = Engine::new(&world);
        body(&engine)
    }
}

fn level(at: u8) -> GuideFilter {
    GuideFilter { level: Some(at), ..GuideFilter::default() }
}

fn row<'a>(rows: &'a [crate::guide_model::MoneyRow], method: &str, family: Family) -> &'a crate::guide_model::MoneyRow {
    rows.iter().find(|row| row.method == method && row.family == family).unwrap_or_else(|| panic!("{method} {family:?} row"))
}

/// Profit = Σ signed flows × current price; XP = units × XP of the dump.
#[test]
fn farm_profit_is_the_priced_flow_balance() {
    let (rows, _) = Fixture::new().run(|engine| money_guide(engine, &level(30)));
    let coal = row(&rows, "gather:mining/coal", Family::Farm);
    assert!((coal.profit_6000 - 1500.0).abs() < 1e-9, "100 coal × 15: {}", coal.profit_6000);
    let sum: f64 = coal.flows.iter().map(|line| line.value_6000).sum();
    assert!((sum - coal.profit_6000).abs() < 1e-9);
    let mining = coal.xp_6000.iter().find(|(skill, _)| skill == "mining").map(|(_, xp)| *xp).unwrap_or(0.0);
    assert!((mining - 100.0 * 50.0).abs() < 1e-9, "coal pays 50 XP: {mining}");
    let general: Vec<f64> = rows.iter().filter(|row| row.blocked.is_empty()).map(|row| row.profit_6000).collect();
    assert!(general.windows(2).all(|pair| pair[0] >= pair[1]), "best profit first (blocked rows after)");
}

/// D5: a recipe on owned stock prices its inputs at their sale price; a
/// product an F2P shop pays more for sells there (Q3).
#[test]
fn recipe_on_stock_prices_inputs_at_their_sale_price() {
    let mut fixture = Fixture::new();
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let steel = row(&rows, "smelt:steel_bar", Family::Recipe);
    assert!((steel.profit_6000 - (5000.0 - 1000.0 - 3000.0)).abs() < 1e-9, "{}", steel.profit_6000);
    fixture.npc.insert("steel_bar", 70.0);
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let steel = row(&rows, "smelt:steel_bar", Family::Recipe);
    let bar = steel.flows.iter().find(|line| line.item == "steel_bar").expect("bar line");
    assert_eq!(bar.outlet, crate::guide_model::Outlet::Shop);
    assert!((steel.profit_6000 - (7000.0 - 4000.0)).abs() < 1e-9);
}

/// D15: 100 % market buys the inputs (a shop's price when cheaper, Q7),
/// counts a square round trip and the measured wait per lot, ties a
/// lot's capital, excludes Ironmen.
#[test]
fn full_market_counts_trips_waits_and_capital() {
    let mut fixture = Fixture::new();
    fixture.shop.insert("coal", 12.0);
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let market = row(&rows, "smelt:steel_bar", Family::Market);
    let site = crate::method_rate::best_site(&crate::method_rate::PriorQuery { method: "smelt:steel_bar".to_string(), level: 30, tool: String::new(), site: None }).expect("furnace");
    let trip = round_trip_ticks(VARROCK_SQUARE, site).expect("same plane");
    let lots = 300.0 / 28.0;
    let extra = lots * (trip + 100.0);
    assert!((market.extra_ticks_per_6000 - extra).abs() < 1e-6, "{} vs {extra}", market.extra_ticks_per_6000);
    let raw = 5000.0 - 1000.0 - 200.0 * 12.0;
    assert!((market.profit_6000 - raw * 6000.0 / (6000.0 + extra)).abs() < 1e-6, "{}", market.profit_6000);
    assert!((market.prereqs.capital - (1000.0 + 2400.0) / lots).abs() < 1e-6, "one lot: {}", market.prereqs.capital);
    assert!(!market.prereqs.ironman);
}

/// D16: the chain farms its inputs (ticks per unit summed upstream), its
/// line pays the final product only, each stage shows its sell-here
/// profit.
#[test]
fn chain_sums_upstream_ticks_and_compares_every_stage() {
    let (rows, _) = Fixture::new().run(|engine| money_guide(engine, &level(30)));
    let chain = row(&rows, "smelt:steel_bar", Family::Chain);
    let ticks = 60.0 + 50.0 + 2.0 * 60.0;
    assert!((chain.profit_6000 - 50.0 * 6000.0 / ticks).abs() < 1e-6, "{}", chain.profit_6000);
    let stage = |product: &str| chain.stages.iter().find(|stage| stage.product == product).unwrap_or_else(|| panic!("{product} stage"));
    assert!((stage("iron_ore").profit_6000 - 1200.0).abs() < 1e-6);
    assert!((stage("coal").profit_6000 - 1500.0).abs() < 1e-6);
    assert!((stage("steel_bar").ticks_per_unit - ticks).abs() < 1e-6);
    assert_eq!(chain.stages.last().map(|stage| stage.product.as_str()), Some("steel_bar"), "final stage last");
    let mining = chain.xp_6000.iter().find(|(skill, _)| skill == "mining").map(|(_, xp)| *xp).unwrap_or(0.0);
    assert!(mining > 0.0, "upstream XP rides the chain");
}

/// D17: coins a training spends convert to ticks at the best accessible
/// money line; the market mode of a recipe buys, the autonomous mode
/// farms (no input left to buy).
#[test]
fn training_converts_coins_to_time_at_the_best_money_line() {
    let fixture = Fixture::new();
    let filter = GuideFilter { skill: Some("firemaking".to_string()), level: Some(30), ..GuideFilter::default() };
    let (rows, _) = fixture.run(|engine| training_guide(engine, &filter));
    let (money, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let best = money.iter().filter(|row| row.accessible).map(|row| row.profit_6000).fold(0.0, f64::max);
    let market = rows.iter().find(|row| row.method == "fire:logs" && row.supply == Supply::Market).expect("market mode");
    let spent = -market.coins_6000;
    assert!(spent > 0.0);
    assert!((market.converted_ticks_6000 - spent / (best / 6000.0)).abs() < 1e-6, "D17: {} vs {}", market.converted_ticks_6000, spent / (best / 6000.0));
    assert!((market.effective_xp_6000 - market.xp_6000 * 6000.0 / (6000.0 + market.converted_ticks_6000)).abs() < 1e-6);
    assert!((market.coins_per_xp - spent / market.xp_6000).abs() < 1e-9);
    assert!(market.ticks_to_band_end.is_some_and(|ticks| ticks > 0.0));
}

/// XP/6000 is units × XP of the dump: 300 logs burnt × 40 XP.
#[test]
fn training_xp_is_units_times_dump_xp() {
    let mut fixture = Fixture::new();
    fixture.wait = Some(0.0);
    let filter = GuideFilter { skill: Some("firemaking".to_string()), level: Some(30), ..GuideFilter::default() };
    let (rows, _) = fixture.run(|engine| training_guide(engine, &filter));
    let market = rows.iter().find(|row| row.method == "fire:logs" && row.supply == Supply::Market).expect("market mode");
    // Logs burn anywhere: no trip; the wait is zero: XP = 300 × 40.
    assert!((market.xp_6000 - 12000.0).abs() < 1e-6, "{}", market.xp_6000);
    assert!((market.coins_6000 + 1200.0).abs() < 1e-6, "300 logs bought at 4: {}", market.coins_6000);
}

/// D20: a quest's fixed XP over its measured duration rides beside the
/// methods; unmeasured, it is listed without a rate.
#[test]
fn quest_shortcuts_ride_the_training_guide() {
    let mut fixture = Fixture::new();
    fixture.quests.insert("doric", 3000.0);
    let filter = GuideFilter { skill: Some("mining".to_string()), level: Some(30), ..GuideFilter::default() };
    let (rows, _) = fixture.run(|engine| training_guide(engine, &filter));
    let doric = rows.iter().find(|row| row.method == "quest:doric").expect("doric shortcut");
    assert_eq!(doric.supply, Supply::Quest);
    assert!((doric.quest_xp - 1300.0).abs() < 1e-9);
    assert!((doric.xp_6000 - 2600.0).abs() < 1e-9);
    fixture.quests.clear();
    let (rows, _) = fixture.run(|engine| training_guide(engine, &filter));
    let doric = rows.iter().find(|row| row.method == "quest:doric").expect("still listed");
    assert_eq!(doric.quest_ticks, None);
    let done = GuideFilter { quests_done: vec!["doric".to_string()], ..filter };
    let (rows, _) = fixture.run(|engine| training_guide(engine, &done));
    assert!(!rows.iter().any(|row| row.method == "quest:doric"), "a done quest is no shortcut");
}

/// Every F2P method of the planner has a line, estimated from the game
/// prior when nothing is measured; a method no prior can estimate is
/// listed as unrated with its typed reason, never a made-up figure.
#[test]
fn every_f2p_method_has_a_line_or_a_typed_reason() {
    let rate = |ask: &RateAsk| {
        let query = crate::method_rate::PriorQuery { method: ask.method.clone(), level: ask.level, tool: ask.tool.clone(), site: ask.site };
        crate::method_rate::guide_prior(&query)
            .map(|prior| RateAnswer {
                flows: prior.flows.iter().map(|(item, cause, qty)| (item.clone(), cause.to_string(), *qty)).collect(),
                units: prior.units.iter().map(|(kind, subject, qty)| (kind.to_string(), subject.clone(), *qty)).collect(),
                pause_per_6000: 0.0,
                confidence: Default::default(),
            })
            .map_err(|error| error.to_string())
    };
    let price = |_: &str| Quote { price: 1.0, ..Quote::default() };
    let none = |_: &str| None;
    let world = GuideWorld { rate: &rate, price: &price, npc_sale: &none, shop_buy: &none, market_wait_ticks: None, quest_ticks: &none, measured: &[] };
    let engine = Engine::new(&world);
    let (rows, unrated) = money_guide(&engine, &GuideFilter::default());
    let catalogue = crate::guide_methods::catalogue();
    let mut missing = Vec::new();
    for entry in catalogue {
        let listed = rows.iter().any(|row| row.method == entry.method && row.style == entry.style) || unrated.iter().any(|gap| gap.method == entry.method);
        if !listed {
            missing.push(entry.method.clone());
        }
    }
    assert!(missing.is_empty(), "every method listed: {missing:?}");
    assert!(unrated.len() * 10 < catalogue.len(), "most methods estimate: {unrated:?}");
    assert!(unrated.iter().all(|gap| !gap.reason.is_empty()));
    let fights = rows.iter().filter(|row| row.method == "fight:melee/men" && row.family == Family::Farm).count();
    assert_eq!(fights, 4, "one line per melee style");
    let men = rows.iter().find(|row| row.method == "fight:melee/men").expect("men");
    assert!(men.flows.iter().any(|line| line.cause == "loot"), "a fight prior prices its drop table: {:?}", men.flows);
}

/// D5 on stock: a recipe transforms stock the bot HOLDS. Its row names
/// one lot of that stock as a prerequisite; in the general guide (no
/// bot, nothing held) it is blocked, never accessible and ranks after
/// every other row (D17 never reads it); a bot holding the stock works
/// it on that stock only.
#[test]
fn recipe_on_stock_needs_its_stock_and_ranks_after_the_general_lines() {
    let fixture = Fixture::new();
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let steel = row(&rows, "smelt:steel_bar", Family::Recipe);
    // 300 pack slots per window: 300 / 28 lots; one lot = 10 ore, 19 coal.
    let lots = 300.0 / 28.0;
    let stock = |item: &str| steel.prereqs.stock.iter().find(|(name, _)| name == item).map(|(_, units)| *units);
    assert_eq!(stock("iron_ore"), Some((100.0_f64 / lots).ceil()), "{:?}", steel.prereqs.stock);
    assert_eq!(stock("coal"), Some((200.0_f64 / lots).ceil()), "{:?}", steel.prereqs.stock);
    assert!(!steel.accessible, "nobody holds the stock in the general guide");
    assert!(steel.blocked.iter().any(|reason| reason.contains("stock") && reason.contains("iron_ore")), "{:?}", steel.blocked);
    let first_blocked = rows.iter().position(|row| !row.blocked.is_empty()).expect("a blocked row");
    assert!(rows[first_blocked..].iter().all(|row| !row.blocked.is_empty()), "blocked rows rank after every other row");
    assert!(rows.iter().filter(|row| row.accessible).all(|row| row.family != Family::Recipe), "no stock is assumed");
    let best = fixture.run(|engine| crate::guide_money::best_accessible_profit(engine, 30, &[]));
    let chain = row(&rows, "smelt:steel_bar", Family::Chain).profit_6000;
    assert!(best.is_some_and(|best| best >= chain && best < 1500.0 + 1e-9), "D17 reads the general lines: {best:?}");
    // A bot holding half a window of ore and plenty of coal: accessible,
    // half the window's work, half its profit.
    let held = GuideFilter { level: Some(30), held: vec![("iron_ore".to_string(), 50.0), ("coal".to_string(), 500.0)], ..GuideFilter::default() };
    let (rows, _) = fixture.run(|engine| money_guide(engine, &held));
    let steel = row(&rows, "smelt:steel_bar", Family::Recipe);
    assert!(steel.accessible && steel.blocked.is_empty(), "{:?}", steel.blocked);
    assert!((steel.profit_6000 - (5000.0 - 1000.0 - 3000.0) * 0.5).abs() < 1e-9, "{}", steel.profit_6000);
    let short = GuideFilter { held: vec![("iron_ore".to_string(), 5.0), ("coal".to_string(), 500.0)], ..held };
    let (rows, _) = fixture.run(|engine| money_guide(engine, &short));
    assert!(!row(&rows, "smelt:steel_bar", Family::Recipe).accessible, "under one lot of ore");
}

/// D12: an object never traded has no current price. A bid for one unit
/// pays that unit only; the rest of the window's units go to the F2P
/// shop (Q3), or are worth nothing without one; the line and the row
/// say the price rests on the book.
#[test]
fn never_traded_product_sells_the_bid_depth_then_the_shop() {
    let mut fixture = Fixture::new();
    fixture.prices.insert("iron_bar", 20.0);
    fixture.yields.insert("smith:iron_2h_sword", answer(&[("iron_2h_sword", "product", 100.0), ("iron_bar", "input", -300.0)], &[("product", "iron_2h_sword", 100.0)]));
    fixture.books.insert("iron_2h_sword", (vec![(15000.0, 1.0)], Vec::new()));
    fixture.npc.insert("iron_2h_sword", 50.0);
    let sword = |fixture: &Fixture| {
        let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
        let found = row(&rows, "smith:iron_2h_sword", Family::Recipe).clone();
        let line = found.flows.iter().find(|line| line.item == "iron_2h_sword").cloned().expect("sword line");
        (found, line)
    };
    let (found, line) = sword(&fixture);
    assert!((line.unit_price - (15000.0 + 99.0 * 50.0) / 100.0).abs() < 1e-9, "one unit at the bid, 99 at the shop: {}", line.unit_price);
    assert_eq!(line.basis, crate::guide_model::PriceBasis::BookShop);
    assert!(!line.traded);
    assert_eq!(found.price_confidence, "book/npc");
    assert!((found.profit_6000 - (line.value_6000 - 300.0 * 20.0)).abs() < 1e-6);
    fixture.npc.clear();
    let (_, line) = sword(&fixture);
    assert!((line.unit_price - 15000.0 / 100.0).abs() < 1e-9, "no outlet beyond the bid: {}", line.unit_price);
    assert_eq!(line.basis, crate::guide_model::PriceBasis::BookNone);
    // A traded object keeps its current price (D12), every unit.
    let (rows, _) = Fixture::new().run(|engine| money_guide(engine, &level(30)));
    let coal = row(&rows, "gather:mining/coal", Family::Farm);
    assert_eq!(coal.price_confidence, "traded");
    assert_eq!(coal.flows[0].basis, crate::guide_model::PriceBasis::Traded);
}

/// D12 clamp: a traded object's price lifted above its median by a bid
/// for one unit pays that unit only, the rest at the median; a price
/// cut under the median by an ask for one unit buys that unit only. A
/// median of fewer than five fills is flagged "thin".
#[test]
fn a_book_clamp_moves_the_price_of_its_own_units_only() {
    let mut fixture = Fixture::new();
    fixture.yields.insert("smith:iron_2h_sword", answer(&[("iron_2h_sword", "product", 100.0), ("iron_bar", "input", -300.0)], &[("product", "iron_2h_sword", 100.0)]));
    fixture.prices.insert("iron_bar", 20.0);
    fixture.quotes.insert("iron_2h_sword", Quote { price: 5000.0, traded: true, median: 500.0, fills: 5, bids: vec![(5000.0, 1.0)], asks: Vec::new() });
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let sword = row(&rows, "smith:iron_2h_sword", Family::Recipe);
    let line = sword.flows.iter().find(|line| line.item == "iron_2h_sword").expect("sword line");
    assert!((line.unit_price - (5000.0 + 99.0 * 500.0) / 100.0).abs() < 1e-9, "{}", line.unit_price);
    assert_eq!(sword.price_confidence, "traded");
    fixture.quotes.insert("iron_2h_sword", Quote { price: 500.0, traded: true, median: 500.0, fills: 1, ..Quote::default() });
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let sword = row(&rows, "smith:iron_2h_sword", Family::Recipe);
    assert_eq!(sword.flows.iter().find(|line| line.item == "iron_2h_sword").map(|line| line.basis), Some(crate::guide_model::PriceBasis::Thin));
    assert_eq!(sword.price_confidence, "thin");
    // Purchase: coal's median 15, cut to 5 by an ask for one unit.
    fixture.quotes.insert("coal", Quote { price: 5.0, traded: true, median: 15.0, fills: 5, bids: Vec::new(), asks: vec![(5.0, 1.0)] });
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let market = row(&rows, "smelt:steel_bar", Family::Market);
    let coal = market.flows.iter().find(|line| line.item == "coal").expect("coal line");
    let units = -coal.per_6000;
    assert!((coal.unit_price - (5.0 + (units - 1.0) * 15.0) / units).abs() < 1e-9, "{}", coal.unit_price);
}

/// D15/Q7: an ask for one unit sells one unit. Past the asks' depth a
/// purchase goes to the F2P shop; with none the input is not for sale:
/// the 100 % market row is blocked (with the reason), never accessible,
/// and carries no profit.
#[test]
fn purchase_beyond_the_ask_depth_needs_a_shop_or_blocks_the_row() {
    let mut fixture = Fixture::new();
    fixture.prices.remove("coal");
    fixture.books.insert("coal", (Vec::new(), vec![(10.0, 1.0)]));
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let market = row(&rows, "smelt:steel_bar", Family::Market);
    assert!(!market.accessible);
    assert!(market.blocked.iter().any(|reason| reason.contains("coal")), "{:?}", market.blocked);
    assert_eq!(market.profit_6000, 0.0, "no profit without the input");
    // The chain mines its coal: still a general line.
    assert!(row(&rows, "smelt:steel_bar", Family::Chain).blocked.is_empty());
    fixture.shop.insert("coal", 12.0);
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let market = row(&rows, "smelt:steel_bar", Family::Market);
    assert!(market.blocked.is_empty() && market.accessible, "{:?}", market.blocked);
    let coal = market.flows.iter().find(|line| line.item == "coal").expect("coal line");
    let units = -coal.per_6000;
    assert!((coal.unit_price - (10.0 + (units - 1.0) * 12.0) / units).abs() < 1e-9, "one at the ask, the rest at the shop: {}", coal.unit_price);
    assert_eq!(coal.basis, crate::guide_model::PriceBasis::BookShop);
}

/// Training: an input nothing makes at the level, nobody offers and no
/// shop sells blocks both supply modes (with the reason).
#[test]
fn training_input_nobody_makes_or_sells_blocks_the_row() {
    let mut fixture = Fixture::new();
    fixture.prices.remove("logs");
    let filter = GuideFilter { skill: Some("firemaking".to_string()), level: Some(30), ..GuideFilter::default() };
    let (rows, _) = fixture.run(|engine| training_guide(engine, &filter));
    let fire: Vec<_> = rows.iter().filter(|row| row.method == "fire:logs").collect();
    assert!(!fire.is_empty());
    assert!(fire.iter().all(|row| !row.accessible && row.blocked.iter().any(|reason| reason.contains("logs"))), "{fire:?}");
}
