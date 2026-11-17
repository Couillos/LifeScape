//! Guides API on an in-memory store and a fabricated snapshot (never
//! `run/`): the measured fight of a drop farm prices its whole loot, the
//! filters apply, the JSON carries the guide columns.

use super::*;
use crate::rates_service::Snapshot;
use crate::report_contract::ParsedContract;

fn fight(qty: i64) -> ParsedContract {
    ParsedContract {
        planner_method: "farm:feather".to_string(),
        method_source: "node".to_string(),
        site: "3235,3295,0".to_string(),
        target: "chickens".to_string(),
        tool: "bronze_sword".to_string(),
        combat_style: "melee:attack".to_string(),
        work_ticks: 1200,
        pause_ticks: 0,
        levels: vec![("attack".to_string(), 3)],
        flows: vec![("Feather".to_string(), "loot".to_string(), qty), ("Bones".to_string(), "loot".to_string(), 10), ("Coins".to_string(), "traded".to_string(), 900)],
        units: vec![("kill".to_string(), "chicken".to_string(), 10), ("damage".to_string(), "chicken".to_string(), 30)],
        alerts: Vec::new(),
    }
}

fn parsed() -> crate::rates_service::ParsedReport {
    crate::rates_service::ParsedReport {
        account: "t01".to_string(),
        node_id: "n1".to_string(),
        method: "farm:feather".to_string(),
        script: "melee".to_string(),
        wall_ticks: 1200,
        pause_ticks: 0,
        stop: "budget_exhausted".to_string(),
        level: 3,
        band: 1,
        items: Vec::new(),
        xp: Vec::new(),
    }
}

/// The money guide reads the fight the drop farms measured (its whole
/// loot priced, trades never), with its confidence; filters apply.
#[test]
fn money_guide_prices_the_measured_fight() -> Result<(), lifescape_store::Error> {
    let store = Store::open_in_memory()?;
    let mut snapshot = Snapshot::new();
    for qty in [50, 60, 70] {
        snapshot.record(&parsed(), Some(&fight(qty)), 9);
    }
    let filter = GuideFilter { skill: Some("attack".to_string()), level: Some(3), ..GuideFilter::default() };
    let guide = build_guide(&store, &snapshot, GuideKind::Money, &filter).map_err(lifescape_store::Error::Invalid)?;
    let rows = guide.get("rows").as_arr().cloned().unwrap_or_default();
    let measured = rows
        .iter()
        .find(|row| row.get("method").as_str() == Some("fight:melee/chickens") && row.get("site").as_str() == Some("3235,3295,0"))
        .expect("the measured site has its line");
    assert_eq!(measured.get("family").as_str(), Some("farm"));
    assert_eq!(measured.get("confidence").get("measured").as_bool(), Some(true));
    let flows = measured.get("flows").as_arr().cloned().unwrap_or_default();
    assert!(flows.iter().any(|line| line.get("item").as_str() == Some("feather")), "{flows:?}");
    assert!(flows.iter().all(|line| line.get("cause").as_str() != Some("traded")));
    assert!(rows.iter().all(|row| row.get("prereqs").get("levels").get("attack").as_i64().unwrap_or(0) <= 3 || !row.get("accessible").as_bool().unwrap_or(true)));
    for key in ["profit_6000", "xp_6000", "prereqs", "stages", "routes", "other_drops", "accessible", "band", "tool"] {
        assert!(!matches!(measured.get(key), Value::Null), "{key} column");
    }
    Ok(())
}

/// The query string sets the filter; empty values are no filter.
#[test]
fn query_sets_the_filter() {
    let params = crate::web::parse_query("skill=mining&level=41&family=&quests=doric,runemysteries");
    let filter = filter_of(&params);
    assert_eq!(filter.skill.as_deref(), Some("mining"));
    assert_eq!(filter.level, Some(41));
    assert_eq!(filter.family, None);
    assert_eq!(filter.quests_done, vec!["doric".to_string(), "runemysteries".to_string()]);
    assert!(filter.held.is_empty(), "the general guide holds nothing");
    let held = filter_of(&crate::web::parse_query("level=30&held=iron_ore:50,coal:500"));
    assert_eq!(held.held, vec![("iron_ore".to_string(), 50.0), ("coal".to_string(), 500.0)]);
}

/// D12: a never-traded object's quote carries no current price, only its
/// resting book (a lone bid for one unit is one unit deep); a traded one
/// carries its current price.
#[test]
fn never_traded_quote_carries_its_book_depth() -> Result<(), lifescape_store::Error> {
    let store = Store::open_in_memory()?;
    store.upsert_item(&lifescape_store::Item { item_id: 1095, name: "Leather chaps".to_string(), shop_price: 20, alch_value: 8 })?;
    store.place_order(&lifescape_store::NewOrder { account: "t01".to_string(), side: lifescape_store::OrderSide::Buy, item_id: 1095, quantity: 1, price: 15411, clock: 1, ..Default::default() })?;
    let quote = quote_of(&store, "leather_chaps");
    assert!(!quote.traded);
    assert_eq!(quote.bids, vec![(15411.0, 1.0)]);
    assert!(quote.asks.is_empty());
    // One fill at 400: traded on one fill (thin), the bid clamps the
    // price (D12) and stays in the quote for its one unit.
    store.record_execution(&lifescape_store::NewExecution { item_id: 1095, quantity: 1, price: 400, clock: 2, server_tick: 2, taker_side: Some(lifescape_store::OrderSide::Buy) })?;
    let quote = quote_of(&store, "leather_chaps");
    assert!(quote.traded);
    assert_eq!((quote.median, quote.fills, quote.fill_units), (400.0, 1, 1.0));
    assert_eq!(quote.price, 15411.0, "D12 clamp by the bid");
    assert_eq!(quote.bids, vec![(15411.0, 1.0)]);
    Ok(())
}

/// The shops' window the engine reads (§2): the base stock and the
/// restock of one window, cheapest first, never an unlimited shop.
#[test]
fn shop_window_is_finite_and_cheapest_first() {
    let steps = super::shop_window("feather");
    let units: f64 = steps.iter().map(|(_, units, _)| units).sum();
    assert!(units.is_finite() && units >= 7000.0, "{steps:?}");
    assert!(steps.windows(2).all(|pair| pair[0].0 <= pair[1].0));
    assert!(steps.iter().all(|(_, _, need)| need.is_open()), "feathers: no condition");
    assert!(super::shop_window("no_such_object").is_empty());
    // Q7: Oziach's rune platebody after Dragon Slayer (planner key), the
    // Champions' Guild's rune longsword behind 32 quest points.
    assert!(super::shop_window("rune_platebody").iter().all(|(_, _, need)| *need == ShopNeed { quest: Some("dragon"), quest_points: 0 }));
    assert!(!super::shop_window("rune_longsword").is_empty());
    assert!(super::shop_window("rune_longsword").iter().all(|(_, _, need)| *need == ShopNeed { quest: None, quest_points: 32 }));
    let (_, need) = super::npc_volume_value("rune_platebody").expect("a shop buys rune platebodies");
    assert!(need.quest == Some("dragon") || need.is_open(), "{need:?}");
}

/// Object id of a debug name in the game data.
fn obj_id(name: &str) -> i64 {
    lifescape_gamedata::objs_by_name(name).first().map(|obj| obj.obj_id as i64).unwrap_or_else(|| panic!("{name} in the dump"))
}

/// Live 2026-11-16 (money level 30: "enchant strung_sapphire_amulet
/// (market)" at 1.24 M/6000): strung and unstrung sapphire amulets share
/// the display "Sapphire amulet". The guide's quote reads the OBJECT
/// (its id in the store), never the first object of a display: the
/// unstrung amulets' 150 asks at 540 are no strung amulet.
#[test]
fn quote_reads_the_object_never_a_shared_display() -> Result<(), lifescape_store::Error> {
    let store = Store::open_in_memory()?;
    let (unstrung, strung) = (obj_id("unstrung_sapphire_amulet"), obj_id("strung_sapphire_amulet"));
    assert!(unstrung < strung, "the unstrung amulet comes first by id");
    for item_id in [unstrung, strung] {
        store.upsert_item(&lifescape_store::Item { item_id, name: "Sapphire amulet".to_string(), shop_price: 900, alch_value: 540 })?;
    }
    store.place_order(&lifescape_store::NewOrder { account: "t01".to_string(), side: lifescape_store::OrderSide::Sell, item_id: unstrung, quantity: 150, price: 540, clock: 1, ..Default::default() })?;
    store.place_order(&lifescape_store::NewOrder { account: "t02".to_string(), side: lifescape_store::OrderSide::Sell, item_id: strung, quantity: 1, price: 805, clock: 1, ..Default::default() })?;
    assert_eq!(quote_of(&store, "strung_sapphire_amulet").asks, vec![(805.0, 1.0)]);
    assert_eq!(quote_of(&store, "unstrung_sapphire_amulet").asks, vec![(540.0, 150.0)]);
    assert_eq!(quote_of(&store, "no_such_object"), Quote { price: 1.0, ..Quote::default() }, "an object the market does not list");
    Ok(())
}

/// The flows of an activity name ITS objects: "Sapphire amulet" in the
/// yield of `enchant:strung_sapphire_amulet` is the strung amulet the
/// spell takes (the first object of that display is the unstrung one).
#[test]
fn an_activity_flows_name_its_own_objects() {
    let ask = RateAsk { method: "enchant:strung_sapphire_amulet".to_string(), style: String::new(), site: None, tool: String::new(), level: 30 };
    let answer = answer_of(&Snapshot::new(), &ask, 9).expect("the game prior answers");
    let input = answer.flows.iter().find(|(_, cause, qty)| cause == "input" && *qty < 0.0).expect("an input flow");
    assert_eq!(input.0, "strung_sapphire_amulet", "{:?}", answer.flows);
    assert!(answer.flows.iter().any(|(item, cause, _)| item == "amulet_of_magic" && cause == "product"), "{:?}", answer.flows);
    assert!(answer.units.iter().all(|(_, subject, _)| subject != "unstrung_sapphire_amulet"), "{:?}", answer.units);
    assert_eq!(crate::rates_names::debug_of("Sapphire amulet"), "unstrung_sapphire_amulet", "outside an activity: the first object");
    // Stringing takes the unstrung amulet and makes the strung one: the
    // side of the flow tells them apart.
    let ask = RateAsk { method: "string:unstrung_sapphire_amulet".to_string(), ..ask };
    let answer = answer_of(&Snapshot::new(), &ask, 9).expect("the game prior answers");
    assert!(answer.flows.iter().any(|(item, cause, qty)| item == "unstrung_sapphire_amulet" && cause == "input" && *qty < 0.0), "{:?}", answer.flows);
    assert!(answer.flows.iter().any(|(item, cause, qty)| item == "strung_sapphire_amulet" && cause == "product" && *qty > 0.0), "{:?}", answer.flows);
}

/// End to end: the 100 % market enchant buys STRUNG amulets; one ask
/// and no F2P shop stock cannot feed a window, so the line is blocked
/// (never priced on the unstrung amulets' asks).
#[test]
fn market_enchant_buys_strung_amulets() -> Result<(), lifescape_store::Error> {
    let store = Store::open_in_memory()?;
    let (unstrung, strung, magic) = (obj_id("unstrung_sapphire_amulet"), obj_id("strung_sapphire_amulet"), obj_id("amulet_of_magic"));
    for (item_id, name) in [(unstrung, "Sapphire amulet"), (strung, "Sapphire amulet"), (magic, "Amulet of magic")] {
        store.upsert_item(&lifescape_store::Item { item_id, name: name.to_string(), shop_price: 900, alch_value: 540 })?;
    }
    store.place_order(&lifescape_store::NewOrder { account: "t01".to_string(), side: lifescape_store::OrderSide::Sell, item_id: unstrung, quantity: 5000, price: 540, clock: 1, ..Default::default() })?;
    store.place_order(&lifescape_store::NewOrder { account: "t02".to_string(), side: lifescape_store::OrderSide::Sell, item_id: strung, quantity: 1, price: 805, clock: 1, ..Default::default() })?;
    for clock in 0..5 {
        store.record_execution(&lifescape_store::NewExecution { item_id: magic, quantity: 1, price: 12134, clock: 2 + clock, server_tick: 2 + clock, taker_side: Some(lifescape_store::OrderSide::Sell) })?;
    }
    let filter = GuideFilter { level: Some(30), family: Some("market".to_string()), ..GuideFilter::default() };
    let guide = build_guide(&store, &Snapshot::new(), GuideKind::Money, &filter).map_err(lifescape_store::Error::Invalid)?;
    let rows = guide.get("rows").as_arr().cloned().unwrap_or_default();
    let enchant = rows.iter().find(|row| row.get("method").as_str() == Some("enchant:strung_sapphire_amulet")).expect("the market enchant line");
    let flows = enchant.get("flows").as_arr().cloned().unwrap_or_default();
    assert!(flows.iter().any(|line| line.get("item").as_str() == Some("strung_sapphire_amulet") && line.get("cause").as_str() == Some("input")), "{flows:?}");
    assert!(flows.iter().all(|line| line.get("item").as_str() != Some("unstrung_sapphire_amulet")), "{flows:?}");
    assert!(!enchant.get("blocked").as_arr().cloned().unwrap_or_default().is_empty(), "one strung amulet for sale: {enchant:?}");
    assert_eq!(enchant.get("accessible").as_bool(), Some(false));
    Ok(())
}
