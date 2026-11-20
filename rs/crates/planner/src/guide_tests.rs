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
    /// A shop selling without limit at one price.
    shop: HashMap<&'static str, f64>,
    /// A shop's finite window: (price, units) steps, cheapest first.
    shop_ladders: HashMap<&'static str, Vec<(f64, f64)>>,
    /// The condition of the shops that buy or sell an object (Q7).
    shop_needs: HashMap<&'static str, crate::guide_model::ShopNeed>,
    wait: Option<f64>,
    quests: HashMap<&'static str, f64>,
    measured: Vec<crate::guide_model::MeasuredKey>,
    /// Which key the estimator read for an ask at a site (else "Exact").
    site_reads: HashMap<(i64, i64, i64), &'static str>,
}

impl Fixture {
    fn new() -> Self {
        Fixture {
            yields: steel_yields(),
            prices: prices(),
            books: HashMap::new(),
            quotes: HashMap::new(),
            npc: HashMap::new(),
            shop: HashMap::new(),
            shop_ladders: HashMap::new(),
            shop_needs: HashMap::new(),
            wait: Some(100.0),
            quests: HashMap::new(),
            measured: Vec::new(),
            site_reads: HashMap::new(),
        }
    }

    fn run<T>(&self, body: impl FnOnce(&Engine) -> T) -> T {
        let rate = |ask: &RateAsk| {
            let mut found = self.yields.get(ask.method.as_str()).cloned().ok_or_else(|| format!("no yield for {}", ask.method))?;
            if let Some(site) = ask.site {
                found.confidence.read = self.site_reads.get(&site).copied().unwrap_or("Exact").to_string();
            }
            Ok(found)
        };
        let price = |item: &str| match (self.quotes.get(item), self.prices.get(item)) {
            (Some(quote), _) => quote.clone(),
            // A traded object's market makers offer it at its price; its
            // fills moved more than any window sells past those asks (a
            // deep history: the asks at the price sell first, step 2).
            (None, Some(price)) => Quote { price: *price, traded: true, median: *price, fills: 5, fill_units: 2e9, asks: vec![(*price, 1e9)], ..Quote::default() },
            (None, None) => {
                let (bids, asks) = self.books.get(item).cloned().unwrap_or_default();
                // The book midpoint a never-traded quote carries: no value.
                Quote { price: 1.0, traded: false, bids, asks, ..Quote::default() }
            }
        };
        let need = |item: &str| self.shop_needs.get(item).copied().unwrap_or_default();
        let npc = |item: &str| self.npc.get(item).map(|price| (*price, need(item)));
        let shop = |item: &str| match (self.shop_ladders.get(item), self.shop.get(item)) {
            (Some(ladder), _) => ladder.iter().map(|(price, units)| (*price, *units, need(item))).collect(),
            (None, Some(price)) => vec![(*price, f64::INFINITY, need(item))],
            (None, None) => Vec::new(),
        };
        let quest = |key: &str| self.quests.get(key).copied();
        let world = GuideWorld { rate: &rate, price: &price, npc_sale: &npc, shop_buy: &shop, market_wait_ticks: self.wait, quest_ticks: &quest, measured: &self.measured };
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

/// The feasibility of a row for the bot a filter describes (D22).
fn judged(row: &impl crate::guide_feasible::GuideRow, filter: &GuideFilter) -> crate::guide_feasible::Feasibility {
    crate::guide_feasible::with_filter_bot(filter, |bot| crate::guide_feasible::feasible_for(row, bot))
}

/// Within the reach of the filter's bot (what "accessible" was).
fn reachable(row: &impl crate::guide_feasible::GuideRow, filter: &GuideFilter) -> bool {
    judged(row, filter).within_reach()
}

/// The reasons a row is infeasible for the filter's bot, joined.
fn why_not(row: &impl crate::guide_feasible::GuideRow, filter: &GuideFilter) -> String {
    match judged(row, filter) {
        crate::guide_feasible::Feasibility::Infeasible(reasons, _) => reasons.join("; "),
        _ => String::new(),
    }
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
    let general: Vec<f64> = rows.iter().filter(|row| row.priced).map(|row| row.profit_6000).collect();
    assert!(general.windows(2).all(|pair| pair[0] >= pair[1]), "best profit first (unpriced rows after)");
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
/// counts a square round trip and, when the asks do not hold a lot (iron
/// ore 5 units deep), the measured wait per lot; a lot the asks and the
/// shops hold pays the trip only (live 2026-11-20). Ties a lot's
/// capital, excludes Ironmen.
#[test]
fn full_market_counts_trips_waits_and_capital() {
    let mut fixture = Fixture::new();
    fixture.shop.insert("coal", 12.0);
    let site = crate::method_rate::best_site(&crate::method_rate::PriorQuery { method: "smelt:steel_bar".to_string(), level: 30, tool: String::new(), site: None }).expect("furnace");
    let trip = round_trip_ticks(VARROCK_SQUARE, site).expect("same plane");
    let lots = 300.0 / 28.0;
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let covered = row(&rows, "smelt:steel_bar", Family::Market).extra_ticks_per_6000;
    assert!((covered - lots * trip).abs() < 1e-6, "covered: {covered} vs {}", lots * trip);
    fixture.quotes.insert("iron_ore", Quote { price: 10.0, traded: true, median: 10.0, fills: 5, fill_units: 1e9, bids: Vec::new(), asks: vec![(10.0, 5.0)], claimed: 0.0 });
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let market = row(&rows, "smelt:steel_bar", Family::Market);
    let extra = lots * (trip + 100.0);
    assert!((market.extra_ticks_per_6000 - extra).abs() < 1e-6, "{} vs {extra}", market.extra_ticks_per_6000);
    let raw = 5000.0 - 1000.0 - 200.0 * 12.0;
    assert!((market.profit_6000 - raw * 6000.0 / (6000.0 + extra)).abs() < 1e-6, "{}", market.profit_6000);
    assert!((market.prereqs.capital - (1000.0 + 2400.0) / lots).abs() < 1e-6, "one lot: {}", market.prereqs.capital);
    assert!(!market.prereqs.ironman);
}

/// D16: the chain farms its inputs (ticks per unit summed upstream), its
/// line pays the final product only, each stage shows its sell-here
/// profit. Buying ore costs 100 here: farming is the cheaper way (§3).
#[test]
fn chain_sums_upstream_ticks_and_compares_every_stage() {
    let mut fixture = Fixture::new();
    for (item, price) in [("iron_ore", 10.0), ("coal", 15.0)] {
        fixture.quotes.insert(item, Quote { price, traded: true, median: price, fills: 5, fill_units: 500.0, bids: Vec::new(), asks: vec![(100.0, 1e9)], claimed: 0.0 });
    }
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
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
    let best = money.iter().filter(|row| row.priced && reachable(*row, &level(30))).map(|row| row.profit_6000).fold(0.0, f64::max);
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
    let no_sale = |_: &str| None;
    let no_shop = |_: &str| Vec::new();
    let world = GuideWorld { rate: &rate, price: &price, npc_sale: &no_sale, shop_buy: &no_shop, market_wait_ticks: None, quest_ticks: &none, measured: &[] };
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
/// one lot of that stock as a condition; in the general guide (no bot,
/// nothing held) it keeps its economy over a full window (D22: never
/// blocked) but no bot of the filter can work it (D17 never reads it); a
/// bot holding the stock works it on that stock only.
#[test]
fn recipe_on_stock_needs_its_stock_and_is_judged_on_it() {
    let fixture = Fixture::new();
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let steel = row(&rows, "smelt:steel_bar", Family::Recipe);
    // 300 pack slots per window: 300 / 28 lots; one lot = 10 ore, 19 coal.
    let lots = 300.0 / 28.0;
    let stock = |item: &str| steel.prereqs.stock.iter().find(|(name, _)| name == item).map(|(_, units)| *units);
    assert_eq!(stock("iron_ore"), Some((100.0_f64 / lots).ceil()), "{:?}", steel.prereqs.stock);
    assert_eq!(stock("coal"), Some((200.0_f64 / lots).ceil()), "{:?}", steel.prereqs.stock);
    assert!(steel.priced && (steel.profit_6000 - (5000.0 - 1000.0 - 3000.0)).abs() < 1e-9, "its economy stays: {}", steel.profit_6000);
    assert!(why_not(steel, &level(30)).contains("stock") && why_not(steel, &level(30)).contains("iron_ore"), "nobody holds the stock in the general guide");
    assert!(rows.iter().filter(|row| reachable(*row, &level(30))).all(|row| row.family != Family::Recipe), "no stock is assumed");
    let best = fixture.run(|engine| crate::guide_money::best_accessible_profit(engine, 30, &[]));
    let chain = row(&rows, "smelt:steel_bar", Family::Chain).profit_6000;
    assert!(best.is_some_and(|best| best >= chain && best < 1500.0 + 1e-9), "D17 reads the general lines: {best:?}");
    // A bot holding half a window of ore and plenty of coal: feasible,
    // half the window's work, half its profit.
    let held = GuideFilter { level: Some(30), held: vec![("iron_ore".to_string(), 50.0), ("coal".to_string(), 500.0)], ..GuideFilter::default() };
    let (rows, _) = fixture.run(|engine| money_guide(engine, &held));
    let steel = row(&rows, "smelt:steel_bar", Family::Recipe);
    assert_eq!(judged(steel, &held), crate::guide_feasible::Feasibility::Feasible);
    assert!((steel.profit_6000 - (5000.0 - 1000.0 - 3000.0) * 0.5).abs() < 1e-9, "{}", steel.profit_6000);
    let short = GuideFilter { held: vec![("iron_ore".to_string(), 5.0), ("coal".to_string(), 500.0)], ..held };
    let (rows, _) = fixture.run(|engine| money_guide(engine, &short));
    assert!(why_not(row(&rows, "smelt:steel_bar", Family::Recipe), &short).contains("iron_ore"), "under one lot of ore");
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
    fixture.quotes.insert("iron_2h_sword", Quote { price: 5000.0, traded: true, median: 500.0, fills: 5, fill_units: 500.0, bids: vec![(5000.0, 1.0)], asks: Vec::new(), claimed: 0.0 });
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let sword = row(&rows, "smith:iron_2h_sword", Family::Recipe);
    let line = sword.flows.iter().find(|line| line.item == "iron_2h_sword").expect("sword line");
    assert!((line.unit_price - (5000.0 + 99.0 * 500.0) / 100.0).abs() < 1e-9, "{}", line.unit_price);
    assert_eq!(sword.price_confidence, "traded");
    fixture.quotes.insert("iron_2h_sword", Quote { price: 500.0, traded: true, median: 500.0, fills: 1, fill_units: 100.0, ..Quote::default() });
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let sword = row(&rows, "smith:iron_2h_sword", Family::Recipe);
    assert_eq!(sword.flows.iter().find(|line| line.item == "iron_2h_sword").map(|line| line.basis), Some(crate::guide_model::PriceBasis::Thin));
    assert_eq!(sword.price_confidence, "thin");
    // Purchase: coal's median 15, cut to 5 by an ask for one unit: that
    // unit at 5, the rest up the asks (2026-11-16: never under an ask).
    fixture.quotes.insert("coal", Quote { price: 5.0, traded: true, median: 15.0, fills: 5, fill_units: 500.0, bids: Vec::new(), asks: vec![(5.0, 1.0), (20.0, 1000.0)], claimed: 0.0 });
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let market = row(&rows, "smelt:steel_bar", Family::Market);
    let coal = market.flows.iter().find(|line| line.item == "coal").expect("coal line");
    let units = -coal.per_6000;
    assert!((coal.unit_price - (5.0 + (units - 1.0) * 20.0) / units).abs() < 1e-9, "{}", coal.unit_price);
}

/// Thin price (user 2026-11-16, extended 2026-11-17 to any number of
/// fills, `volume::current_price_holds_for_the_recent_fill_volume_only`):
/// a current price holds for the units its fills moved only; past them a
/// sale hits the bids for their depth, then the F2P shop (Q3), else is
/// worth nothing, as for a never-traded object. One fill of one unit at
/// 15 411 never prices 300 units at 15 411.
#[test]
fn thin_price_holds_for_the_traded_volume_only() {
    let mut fixture = Fixture::new();
    fixture.quotes.insert("leather_chaps", Quote { price: 15411.0, traded: true, median: 15411.0, fills: 1, fill_units: 1.0, bids: vec![(400.0, 20.0)], asks: Vec::new(), claimed: 0.0 });
    fixture.npc.insert("leather_chaps", 50.0);
    let (one, many) = fixture.run(|engine| (engine.sale_at("leather_chaps", 1.0), engine.sale_at("leather_chaps", 300.0)));
    assert!((one.price - 15411.0).abs() < 1e-9, "the traded unit: {}", one.price);
    assert_eq!(one.basis, crate::guide_model::PriceBasis::Thin);
    let expected = (15411.0 + 20.0 * 400.0 + 279.0 * 50.0) / 300.0;
    assert!((many.price - expected).abs() < 1e-9, "1 at the thin price, 20 at the bid, 279 at the shop: {} vs {expected}", many.price);
    assert_eq!(many.basis, crate::guide_model::PriceBasis::BookShop);
    assert_eq!(many.outlet, crate::guide_model::Outlet::Shop);
    fixture.npc.clear();
    let many = fixture.run(|engine| engine.sale_at("leather_chaps", 300.0));
    assert!((many.price - (15411.0 + 20.0 * 400.0) / 300.0).abs() < 1e-9, "no outlet past the bids: {}", many.price);
    assert_eq!(many.basis, crate::guide_model::PriceBasis::BookNone);
    assert!(many.available, "a sale is always possible, at 0 past every outlet");
    // Five fills that moved 300 units: the current price for every unit
    // (D12, no saturation; 2026-11-17: the volume decides, not the count).
    fixture.quotes.insert("leather_chaps", Quote { price: 15411.0, traded: true, median: 15411.0, fills: 5, fill_units: 300.0, ..Quote::default() });
    let many = fixture.run(|engine| engine.sale_at("leather_chaps", 300.0));
    assert!((many.price - 15411.0).abs() < 1e-9 && many.basis == crate::guide_model::PriceBasis::Traded, "{many:?}");
}

/// Purchase price (user 2026-11-16): an input costs at least the best
/// resting ask: max(current price, asks walked for the volume); past the
/// asks' depth an F2P shop (Q7), else (D22) the current price, never
/// under the last ask walked: the row keeps its economy and names what
/// the market holds (a condition: the general bot cannot buy a lot). A
/// median of 7 under a cheapest ask of 35 buys at 35.
#[test]
fn purchase_costs_at_least_the_best_ask() {
    let mut fixture = Fixture::new();
    fixture.quotes.insert("coal", Quote { price: 7.0, traded: true, median: 7.0, fills: 5, fill_units: 50.0, bids: Vec::new(), asks: vec![(35.0, 1000.0)], claimed: 0.0 });
    let bought = fixture.run(|engine| engine.buy_at("coal", 200.0));
    assert!((bought.price - 35.0).abs() < 1e-9 && bought.available, "{bought:?}");
    assert_eq!(bought.basis, crate::guide_model::PriceBasis::Book);
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let market = row(&rows, "smelt:steel_bar", Family::Market);
    let coal = market.flows.iter().find(|line| line.item == "coal").expect("coal line");
    assert!((coal.unit_price - 35.0).abs() < 1e-9, "the market row buys at the ask: {}", coal.unit_price);
    // Asks under the current price cost the current price.
    fixture.quotes.insert("coal", Quote { price: 7.0, traded: true, median: 7.0, fills: 5, fill_units: 50.0, bids: Vec::new(), asks: vec![(5.0, 1000.0)], claimed: 0.0 });
    let bought = fixture.run(|engine| engine.buy_at("coal", 200.0));
    assert!((bought.price - 7.0).abs() < 1e-9 && bought.basis == crate::guide_model::PriceBasis::Traded, "{bought:?}");
    // Past the asks' depth: the shop, else the current price (at least
    // the last ask walked), the availability a condition.
    fixture.quotes.insert("coal", Quote { price: 7.0, traded: true, median: 7.0, fills: 5, fill_units: 50.0, bids: Vec::new(), asks: vec![(35.0, 10.0)], claimed: 0.0 });
    let bought = fixture.run(|engine| engine.buy_at("coal", 200.0));
    assert!(!bought.available && bought.priced && (bought.price - 35.0).abs() < 1e-9, "{bought:?}");
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let market = row(&rows, "smelt:steel_bar", Family::Market);
    assert!(market.priced, "never blocked: {market:?}");
    let need = market.prereqs.market.iter().find(|need| need.item == "coal").expect("coal condition");
    assert_eq!((need.asks, need.recent, need.priced), (10.0, 50.0, true), "{need:?}");
    assert!(why_not(market, &level(30)).contains("coal"), "ten asks hold no lot");
    fixture.shop.insert("coal", 40.0);
    let bought = fixture.run(|engine| engine.buy_at("coal", 200.0));
    assert!((bought.price - (10.0 * 35.0 + 190.0 * 40.0) / 200.0).abs() < 1e-9 && bought.available, "{bought:?}");
    assert_eq!(bought.basis, crate::guide_model::PriceBasis::BookShop);
}

/// D15/Q7: an ask for one unit sells one unit. Past the asks' depth a
/// purchase goes to the F2P shop; with none (D22) the rest costs the
/// last ask walked of a never-traded object: the 100 % market row keeps
/// its profit, no bot without the coal can buy a lot (its condition).
#[test]
fn purchase_beyond_the_ask_depth_is_a_condition_not_a_block() {
    let mut fixture = Fixture::new();
    fixture.prices.remove("coal");
    fixture.books.insert("coal", (Vec::new(), vec![(10.0, 1.0)]));
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let market = row(&rows, "smelt:steel_bar", Family::Market);
    assert!(market.priced && market.profit_6000 != 0.0, "{market:?}");
    let coal = market.flows.iter().find(|line| line.item == "coal").expect("coal line");
    assert!((coal.unit_price - 10.0).abs() < 1e-9 && coal.basis == crate::guide_model::PriceBasis::BookNone, "{coal:?}");
    assert!(why_not(market, &level(30)).contains("coal"), "one ask holds no lot");
    let mut holder = level(30);
    holder.held = vec![("coal".to_string(), 19.0), ("iron_ore".to_string(), 10.0)];
    assert!(reachable(market, &holder), "a bot holding a lot of coal works it");
    // The chain mines its coal: within everyone's reach.
    assert!(reachable(row(&rows, "smelt:steel_bar", Family::Chain), &level(30)));
    fixture.shop.insert("coal", 12.0);
    let (rows, _) = fixture.run(|engine| money_guide(engine, &level(30)));
    let market = row(&rows, "smelt:steel_bar", Family::Market);
    assert!(reachable(market, &level(30)), "{market:?}");
    let coal = market.flows.iter().find(|line| line.item == "coal").expect("coal line");
    let units = -coal.per_6000;
    assert!((coal.unit_price - (10.0 + (units - 1.0) * 12.0) / units).abs() < 1e-9, "one at the ask, the rest at the shop: {}", coal.unit_price);
    assert_eq!(coal.basis, crate::guide_model::PriceBasis::BookShop);
}

/// Training: an input nothing makes at the level, nobody offers and no
/// shop sells leaves both supply modes unpriced (no effective rate,
/// ranked after the priced lines) with their conditions: no autonomous
/// source, nothing prices the logs (D22: listed, never blocked).
#[test]
fn training_input_nobody_makes_or_sells_is_unpriced_with_its_conditions() {
    let mut fixture = Fixture::new();
    fixture.prices.remove("logs");
    let filter = GuideFilter { skill: Some("firemaking".to_string()), level: Some(30), ..GuideFilter::default() };
    let (rows, _) = fixture.run(|engine| training_guide(engine, &filter));
    let fire: Vec<_> = rows.iter().filter(|row| row.method == "fire:logs").collect();
    assert_eq!(fire.len(), 2, "both modes listed: {fire:?}");
    assert!(fire.iter().all(|row| !row.priced && row.effective_xp_6000 == 0.0 && why_not(*row, &filter).contains("logs")), "{fire:?}");
    let first_unpriced = rows.iter().position(|row| !row.priced).expect("an unpriced row");
    assert!(rows[first_unpriced..].iter().all(|row| !row.priced), "unpriced rows rank last");
}

fn training_row<'a>(rows: &'a [crate::guide_model::TrainingRow], method: &str, supply: Supply) -> &'a crate::guide_model::TrainingRow {
    rows.iter().find(|row| row.method == method && row.supply == supply).unwrap_or_else(|| panic!("{method} {supply:?} row"))
}

/// D13 autonomous: the bot obtains its inputs itself. Uncut sapphires
/// only come as a mining by-product (one roll in 256, `mining.rs2`): the
/// autonomous line counts the ticks of mining every stone (D16 chain),
/// its effective rate collapses; the market line buys them.
#[test]
fn autonomous_rare_byproduct_input_collapses_the_effective_rate() {
    let mut fixture = Fixture::new();
    fixture.yields.insert("cut:sapphire", answer(&[("sapphire", "product", 5000.0), ("uncut_sapphire", "input", -5000.0)], &[("product", "sapphire", 5000.0)]));
    fixture.yields.insert("gather:mining/copper_ore", answer(&[("copper_ore", "product", 500.0), ("uncut_sapphire", "byproduct", 0.25)], &[("product", "copper_ore", 500.0)]));
    fixture.prices.extend([("sapphire", 300.0), ("uncut_sapphire", 25.0), ("copper_ore", 5.0)]);
    let filter = GuideFilter { skill: Some("crafting".to_string()), level: Some(30), ..GuideFilter::default() };
    let (rows, _) = fixture.run(|engine| training_guide(engine, &filter));
    let autonomous = training_row(&rows, "cut:sapphire", Supply::Autonomous);
    assert!(autonomous.priced && autonomous.prereqs.sources.is_empty(), "the mine is an autonomous source: {:?}", autonomous.prereqs.sources);
    let units = vec![("product".to_string(), "sapphire".to_string(), 5000.0)];
    let work = crate::unit_xp::derived_xp("cut:sapphire", "", &units, 30).into_iter().find(|(skill, _)| skill == "crafting").map(|(_, xp)| xp).expect("crafting XP");
    // 5000 stones a window of cutting, each 6000 / 0.25 ticks of mining.
    let expected = work * 6000.0 / (6000.0 + 5000.0 * 24000.0);
    assert!((autonomous.effective_xp_6000 - expected).abs() < 1e-6, "{} vs {expected}", autonomous.effective_xp_6000);
    assert!(autonomous.effective_xp_6000 < work / 1000.0, "collapsed: {}", autonomous.effective_xp_6000);
    assert!(!autonomous.flows.iter().any(|line| line.item == "uncut_sapphire" && line.per_6000 < 0.0), "nothing bought: {:?}", autonomous.flows);
    assert!(autonomous.other_xp_6000.iter().any(|(skill, xp)| skill == "mining" && *xp > 0.0), "the mining rides the line");
    let market = training_row(&rows, "cut:sapphire", Supply::Market);
    assert!(market.priced && market.effective_xp_6000 > autonomous.effective_xp_6000 * 100.0, "{market:?}");
}

/// D13 autonomous: an input no activity of the bot yields (logs without
/// a woodcutting yield), or whose only source needs a purchase (steel
/// bars without mined coal), is a source gap of the autonomous line
/// (D22: impossible today, listed), never a purchase in disguise; it
/// carries no effective rate. The market line still buys.
#[test]
fn autonomous_input_without_autonomous_source_is_a_source_gap() {
    let fixture = Fixture::new();
    let filter = GuideFilter { skill: Some("firemaking".to_string()), level: Some(30), ..GuideFilter::default() };
    let (rows, _) = fixture.run(|engine| training_guide(engine, &filter));
    let autonomous = training_row(&rows, "fire:logs", Supply::Autonomous);
    assert!(!autonomous.priced && why_not(autonomous, &filter).contains("logs"), "{autonomous:?}");
    assert!(crate::guide_feasible::conditions(&autonomous.prereqs).iter().any(|condition| condition.kind() == "source" && condition.impossible()));
    assert_eq!(autonomous.effective_xp_6000, 0.0, "an unpriced line has no rate");
    assert!(autonomous.ticks_to_band_end.is_none());
    let market = training_row(&rows, "fire:logs", Supply::Market);
    assert!(market.priced && reachable(market, &filter), "{market:?}");
    // Recursive: the dagger's bar is smelted, the bar's coal is not mined.
    let mut fixture = Fixture::new();
    fixture.yields.remove("gather:mining/coal");
    fixture.yields.insert("smith:steel_dagger", answer(&[("steel_dagger", "product", 1000.0), ("steel_bar", "input", -1000.0)], &[("product", "steel_dagger", 1000.0)]));
    fixture.prices.insert("steel_dagger", 80.0);
    let filter = GuideFilter { skill: Some("smithing".to_string()), level: Some(30), ..GuideFilter::default() };
    let (rows, _) = fixture.run(|engine| training_guide(engine, &filter));
    let dagger = training_row(&rows, "smith:steel_dagger", Supply::Autonomous);
    assert!(why_not(dagger, &filter).contains("steel_bar"), "{dagger:?}");
    let smelt = training_row(&rows, "smelt:steel_bar", Supply::Autonomous);
    assert!(why_not(smelt, &filter).contains("coal"), "{smelt:?}");
    // The money chain still buys what it does not farm (D16): coal.
    let price = crate::guide_chain::TimePrice::new(1200.0);
    let unit = fixture.run(|engine| crate::guide_chain::chain_unit(engine, "smelt:steel_bar", "", "steel_bar", 30, 0, &[], crate::guide_chain::Sourcing::Cheapest(price))).expect("chain");
    assert!(unit.gaps.is_empty(), "{:?}", unit.gaps);
    assert_eq!(unit.routes.iter().find(|route| route.item == "coal").map(|route| route.route), Some(crate::guide_model::Route::Buy));
}

/// Market mode: the inputs come from the asks for their depth, then an
/// F2P shop (the money guide's purchase rule); past both (D22) the line
/// keeps its rate at the last ask walked, its condition saying the asks
/// hold no lot.
#[test]
fn training_market_input_availability_is_a_condition() {
    let mut fixture = Fixture::new();
    fixture.prices.remove("logs");
    fixture.books.insert("logs", (Vec::new(), vec![(4.0, 10.0)]));
    let filter = GuideFilter { skill: Some("firemaking".to_string()), level: Some(30), ..GuideFilter::default() };
    let (rows, _) = fixture.run(|engine| training_guide(engine, &filter));
    let market = training_row(&rows, "fire:logs", Supply::Market);
    assert!(market.priced && market.effective_xp_6000 > 0.0, "{market:?}");
    assert!(why_not(market, &filter).contains("logs"), "ten asks hold no lot: {market:?}");
    fixture.shop.insert("logs", 5.0);
    let (rows, _) = fixture.run(|engine| training_guide(engine, &filter));
    let market = training_row(&rows, "fire:logs", Supply::Market);
    assert!(reachable(market, &filter) && market.effective_xp_6000 > 0.0, "{market:?}");
    let logs = market.flows.iter().find(|line| line.item == "logs").expect("logs line");
    assert_eq!(logs.basis, crate::guide_model::PriceBasis::BookShop);
}

#[path = "guide_d22_tests.rs"]
mod d22;

#[path = "guide_sourcing_tests.rs"]
mod sourcing;

#[path = "guide_volume_tests.rs"]
mod volume;

#[path = "guide_market_lot_tests.rs"]
mod market_lot;

#[path = "guide_magic_tests.rs"]
mod magic;
