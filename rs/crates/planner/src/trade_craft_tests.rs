//! Tests of `trade_craft.rs` (plan rendements R5): mapping of the method,
//! lot bounded by capital, asks and the window, Ironman excluded, the
//! sequence buy -> craft -> close, the lot records.

use super::{eligible, input_count, is_trade, lot_finished, lot_size, method_of, note_made, offer_from_row, offers, open_inputs, open_lot, recipe_of, LotBound, TradeLot, LOTS_CAP};
use crate::expand::ReadyCtx;
use crate::guide_model::{Confidence, Family, FlowLine, MoneyRow, Outlet, PriceBasis, Prereqs, Quote};
use crate::model::{Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};

const BRONZE: &str = "smelt:bronze_bar";

fn line(item: &str, cause: &str, per_6000: f64, price: f64, outlet: Outlet) -> FlowLine {
    FlowLine { item: item.to_string(), cause: cause.to_string(), per_6000, unit_price: price, value_6000: per_6000 * price, outlet, traded: true, basis: PriceBasis::Traded }
}

/// The guide's market row of bronze bars: 100 bars per 6000 ticks sold
/// at 40, 100 copper at 10 and 100 tin at 12 bought.
fn bronze_row() -> MoneyRow {
    MoneyRow {
        family: Family::Market,
        method: BRONZE.to_string(),
        style: String::new(),
        site: None,
        tool: String::new(),
        band: 1,
        prereqs: Prereqs { levels: vec![("smithing".to_string(), 1)], capital: 616.0, ironman: false, ..Prereqs::default() },
        flows: vec![line("bronze_bar", "product", 100.0, 40.0, Outlet::Market), line("copper_ore", "input", -100.0, 10.0, Outlet::Market), line("tin_ore", "input", -100.0, 12.0, Outlet::Market)],
        other_drops: Default::default(),
        revenue_6000: 4000.0,
        cost_6000: 2200.0,
        profit_6000: 1800.0,
        xp_6000: Vec::new(),
        extra_ticks_per_6000: 0.0,
        stages: Vec::new(),
        routes: Vec::new(),
        confidence: Confidence::default(),
        price_confidence: "traded",
        blocked: Vec::new(),
        accessible: true,
    }
}

/// Copper: 30 asks at 10, 30 at 11 (60 units); tin: 200 at 12.
fn quote(item: &str) -> Option<Quote> {
    match item {
        "copper_ore" => Some(Quote { price: 10.0, traded: true, asks: vec![(11.0, 30.0), (10.0, 30.0)], ..Quote::default() }),
        "tin_ore" => Some(Quote { price: 12.0, traded: true, asks: vec![(12.0, 200.0)], ..Quote::default() }),
        _ => None,
    }
}

fn smith_ctx(held_copper: i64, held_tin: i64, wallet: i64) -> ReadyCtx {
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.levels = vec![("smithing".to_string(), 30)];
    ctx.combat = 40;
    let held = |item: &str| match item {
        "copper_ore" => held_copper,
        "tin_ore" => held_tin,
        _ => 0,
    };
    ctx.trades = vec![offer_from_row(&bronze_row(), &quote, &held).expect("bronze offer")];
    ctx.trade_wallet = wallet;
    ctx
}

fn money_life(node: Node, ironman: bool) -> Life {
    Life {
        schema: 1,
        account: "t00".to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(ironman, 0.5, 0.5, 7, Archetype::Producer),
        goal: GoalKind::MakeMoney,
        nodes: vec![node],
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
        memory: Default::default(),
    }
}

fn ready_money() -> Node {
    let mut node = Node::new("n1", NodeKind::MakeMoney, Vec::new(), 0);
    node.status = NodeStatus::Ready;
    node
}

#[test]
fn a_trade_method_works_its_recipe_and_maps_to_its_script_with_the_lot() {
    let method = method_of(BRONZE);
    assert_eq!(method, "trade:craft/smelt:bronze_bar");
    assert_eq!(recipe_of(&method), Some(BRONZE));
    assert!(is_trade(&method) && !is_trade(BRONZE) && recipe_of("trade:craft/").is_none());
    // The game data knows the work by its recipe: way, activity (the
    // craft's yield is measured under the recipe), quest, Iron guard.
    assert_eq!(crate::method_way::method_way(&method).map(|(product, _)| product).as_deref(), Some("bronze_bar"));
    assert_eq!(crate::activity::activity_of_method(&method), crate::activity::activity_of_method(BRONZE));
    assert!(!crate::space::iron_may_use(&method), "an Ironman never trades");
    assert!(crate::space::iron_may_use(BRONZE), "the recipe itself stays open to him");
    let mut node = Node::new("n4", NodeKind::MakeMoney, Vec::new(), 0);
    node.method = Some(method.clone());
    node.batch = 14;
    let (script, args) = crate::mapping::map_method(&method, &node, Some((6000, 9000))).expect("mapped");
    let (plain_script, plain_args) = crate::mapping::map_method(BRONZE, &node, Some((6000, 9000))).expect("recipe mapped");
    assert_eq!(script, plain_script, "the recipe's own script");
    let want = args.iter().position(|arg| arg == "--want").expect("lot rides as --want");
    assert_eq!(&args[want..want + 3], &["--want".to_string(), "bronze_bar".to_string(), "14".to_string()]);
    let node_flag = args.iter().position(|arg| arg == "--node").expect("node flag");
    assert!(want < node_flag, "before the brain flags: {args:?}");
    let without: Vec<String> = args.iter().enumerate().filter(|(index, _)| *index < want || *index >= want + 3).map(|(_, arg)| arg.clone()).collect();
    assert_eq!(without, plain_args, "otherwise the recipe's arguments");
    node.batch = 0;
    let (_, unbounded) = crate::mapping::map_method(&method, &node, None).expect("mapped");
    assert!(!unbounded.iter().any(|arg| arg == "--want"), "no lot, no want");
}

#[test]
fn the_lot_is_bounded_by_the_window_the_asks_and_the_wallet() {
    let held = |_: &str| 0;
    let offer = offer_from_row(&bronze_row(), &quote, &held).expect("offer");
    assert_eq!(offer.product, "bronze_bar");
    assert!((offer.unit_profit() - 18.0).abs() < 1e-9, "guide profit per bar");
    assert_eq!(offer.inputs.len(), 2);
    assert!(offer.inputs.iter().all(|input| (input.per_unit - 1.0).abs() < 1e-9));
    assert_eq!(offer.inputs[0].asks, vec![(10.0, 30.0), (11.0, 30.0)], "asks cheapest first");
    // Rich: the 60 copper asks bound the lot (the window would take 100).
    let deep = lot_size(&offer, 1_000_000);
    assert_eq!((deep.lot, deep.bound), (60, LotBound::Depth));
    assert_eq!(deep.cost, 30 * 10 + 30 * 11 + 60 * 12);
    // 500 coins: 22 bars (220 copper + 264 tin = 484), 23 would cost 506.
    let poor = lot_size(&offer, 500);
    assert_eq!((poor.lot, poor.bound, poor.cost), (22, LotBound::Capital, 484));
    assert_eq!(poor.buys, vec![("copper_ore".to_string(), 22), ("tin_ore".to_string(), 22)]);
    assert_eq!(lot_size(&offer, 0).lot, 0, "no wallet, no lot");
    // Held ore is crafted, never bought again; deep asks leave the window.
    let held = |item: &str| if item == "copper_ore" { 70 } else { 0 };
    let stocked = offer_from_row(&bronze_row(), &quote, &held).expect("offer");
    let window = lot_size(&stocked, 1_000_000);
    assert_eq!((window.lot, window.bound), (100, LotBound::Window), "one window's volume at most");
    assert_eq!(window.buys, vec![("copper_ore".to_string(), 30), ("tin_ore".to_string(), 100)]);
    // A traded input costs at least its current price (D15).
    let dear = |item: &str| quote(item).map(|known| if item == "tin_ore" { Quote { price: 15.0, ..known } } else { known });
    let floored = offer_from_row(&bronze_row(), &dear, &|_| 0).expect("offer");
    assert_eq!(lot_size(&floored, 1_000_000).cost, 30 * 10 + 30 * 11 + 60 * 15);
}

#[test]
fn an_ironman_never_trades() {
    let row = bronze_row();
    let level = |_: &str| 30u8;
    assert!(eligible(&row, &level, &[], false));
    assert!(!eligible(&row, &level, &[], true), "Ironman excluded");
    assert!(offers(&[&row], &quote, &|_| 0, &level, &[], true).is_empty());
    assert!(!eligible(&row, &|_| 0u8, &[], false), "the level the row needs");
    let mut ctx = smith_ctx(0, 0, 10_000);
    assert!(crate::resolve_obtain::money_methods(&ctx).contains(&method_of(BRONZE)));
    ctx.ironman = true;
    assert!(!crate::resolve_obtain::money_methods(&ctx).iter().any(|method| is_trade(method)), "no trade method for an Ironman");
    assert!(!super::workable(&method_of(BRONZE), &ctx));
}

#[test]
fn only_unblocked_earning_market_rows_with_market_purchases_trade() {
    let held = |_: &str| 0;
    let mut shop = bronze_row();
    shop.flows[2].outlet = Outlet::Shop;
    assert!(offer_from_row(&shop, &quote, &held).is_err(), "a shop purchase has no market leg");
    let mut blocked = bronze_row();
    blocked.blocked.push("tin_ore: asks hold 0".to_string());
    assert!(offer_from_row(&blocked, &quote, &held).is_err());
    let mut recipe = bronze_row();
    recipe.family = Family::Recipe;
    assert!(offer_from_row(&recipe, &quote, &held).is_err());
    let mut loss = bronze_row();
    loss.profit_6000 = -5.0;
    assert!(offer_from_row(&loss, &quote, &held).is_err());
    let mut alch = bronze_row();
    alch.method = "alch:bronze_bar".to_string();
    assert!(offer_from_row(&alch, &quote, &held).is_err(), "alchemy makes coins: nothing to craft nor sell");
}

/// The whole lot: resolve sizes it and blocks on the ore, the block buys
/// it on the market only, the bought ore is crafted first, the run that
/// ends on its ore spent closes the lot.
#[test]
fn a_lot_buys_its_inputs_then_crafts_them() {
    let method = method_of(BRONZE);
    let ctx = smith_ctx(0, 0, 500);
    let mut node = ready_money();
    crate::expand::resolve_ready(&mut node, &ctx);
    assert_eq!(node.method.as_deref(), Some(method.as_str()), "the best guide profit wins: {:?}", node.alternatives);
    assert_eq!(node.batch, 22, "lot bounded by the wallet");
    assert_eq!(node.status, NodeStatus::Blocked { reason: "missing:copper_ore,tin_ore".to_string() });
    let mut life = money_life(node, false);
    let mut lots: Vec<TradeLot> = Vec::new();
    let opened = open_lot(&mut lots, &life.nodes[0], &ctx, 1_000).expect("lot recorded");
    assert_eq!((opened.lot, opened.cost, opened.predicted, opened.bound.as_str()), (22, 484, 396, "capital"));
    assert!(open_lot(&mut lots, &life.nodes[0], &ctx, 2_000).is_none(), "one record per lot");
    let fold = crate::lifecycle::record_block(&mut life, 0, &["copper_ore".to_string(), "tin_ore".to_string()]);
    assert_eq!(fold, crate::lifecycle::FailureFold::Retry);
    assert_eq!(life.nodes[0].blocked_by.as_deref(), Some(method.as_str()));
    let buys: Vec<&Node> = life.nodes.iter().filter(|node| matches!(node.kind, NodeKind::ObtainItems { .. })).collect();
    assert_eq!(buys.len(), 2);
    for buy in &buys {
        assert!(matches!(&buy.kind, NodeKind::ObtainItems { count: 22, .. }), "sized on the lot: {:?}", buy.kind);
        let legs: Vec<&str> = buy.alternatives.iter().map(|alt| alt.method.as_str()).collect();
        assert_eq!(legs, vec!["market:buy"], "100 % market: no production leg");
    }
    assert!(open_inputs(&life).contains(&"copper_ore".to_string()), "bought ore is kept from the sale");
    // Purchases landed: the bought ore is crafted, whatever ranks first.
    for node in life.nodes.iter_mut().filter(|node| matches!(node.kind, NodeKind::ObtainItems { .. })) {
        node.status = NodeStatus::Done;
    }
    crate::lifecycle::normalize(&mut life);
    assert_eq!(life.nodes[0].status, NodeStatus::Ready);
    let mut stocked = smith_ctx(22, 22, 0);
    stocked.method_profit.push(("fight:melee/cows".to_string(), 1_000_000.0));
    crate::expand::resolve_ready(&mut life.nodes[0], &stocked);
    assert_eq!(life.nodes[0].method.as_deref(), Some(method.as_str()), "bought stock never strands");
    assert_eq!(life.nodes[0].batch, 22);
    assert_eq!(life.nodes[0].status, NodeStatus::Ready);
    let (_, args) = crate::mapping::map_method(&method, &life.nodes[0], Some((6000, 6000))).expect("mapped");
    assert!(args.windows(3).any(|window| window == ["--want", "bronze_bar", "22"]), "{args:?}");
    // The run ends on its ore spent with bars made: the lot is over.
    assert!(lot_finished(&method, &["copper_ore".to_string()], 21));
    assert!(!lot_finished(&method, &["copper_ore".to_string()], 0), "nothing made: a real block");
    assert!(!lot_finished(&method, &["hammer".to_string()], 21), "a tool is a real block");
    assert!(!lot_finished(BRONZE, &["copper_ore".to_string()], 21), "only a trade lot");
    assert!(note_made(&mut lots, "n1", 21, 1_260));
    assert_eq!(lots[0].made, Some(21));
    assert!((lots[0].craft_yield().unwrap_or(0.0) - 100.0).abs() < 1e-9, "21 bars in 1260 ticks = 100 / 6000");
}

#[test]
fn a_lot_already_held_crafts_without_buying() {
    let method = method_of(BRONZE);
    let ctx = smith_ctx(100, 100, 0);
    let mut node = ready_money();
    crate::expand::resolve_ready(&mut node, &ctx);
    assert_eq!(node.method.as_deref(), Some(method.as_str()));
    assert_eq!(node.status, NodeStatus::Ready, "nothing to buy");
    assert_eq!(node.batch, 100);
    let mut lots = Vec::new();
    let lot = open_lot(&mut lots, &node, &ctx, 0).expect("recorded");
    assert_eq!((lot.buys.len(), lot.cost, lot.bound.as_str()), (0, 0, "held"), "nothing bought");
}

#[test]
fn no_wallet_no_trade() {
    let ctx = smith_ctx(0, 0, 0);
    let mut node = ready_money();
    crate::expand::resolve_ready(&mut node, &ctx);
    assert_ne!(node.method.as_deref(), Some(method_of(BRONZE).as_str()), "no lot fits: another money method");
}

#[test]
fn trade_inputs_size_on_the_recipe_and_its_failures() {
    assert_eq!(input_count(&method_of(BRONZE), "copper_ore", 22), Some(22));
    assert_eq!(input_count(&method_of("smelt:iron_bar"), "iron_ore", 10), Some(20), "an iron smelt fails half the time");
    assert_eq!(input_count(&method_of(BRONZE), "copper_ore", 0), None);
    assert_eq!(input_count(BRONZE, "copper_ore", 22), None, "only a trade lot");
    assert!(super::buys_input(&method_of(BRONZE), "tin_ore"));
    assert!(!super::buys_input(&method_of("smith:bronze_dagger"), "hammer"), "a tool comes by the usual legs");
    assert!(!super::buys_input(BRONZE, "tin_ore"));
}

#[test]
fn lots_round_trip_in_the_memory_and_stay_capped() {
    let mut memory = crate::life_memory::LifeMemory::default();
    let ctx = smith_ctx(0, 0, 500);
    for number in 0..(LOTS_CAP + 3) {
        let mut node = Node::new(&format!("n{number}"), NodeKind::MakeMoney, Vec::new(), 0);
        node.method = Some(method_of(BRONZE));
        node.batch = 22;
        open_lot(&mut memory.trades, &node, &ctx, number as i64).expect("recorded");
    }
    assert_eq!(memory.trades.len(), LOTS_CAP);
    assert_eq!(memory.trades[0].node, "n3", "oldest first, the first three dropped");
    note_made(&mut memory.trades, "n5", 20, 1_200);
    let value = crate::life_memory::memory_to_value(&memory);
    let back = crate::life_memory::memory_from_value(&lifescape_json::parse(&lifescape_json::stringify(&value)).expect("json"));
    assert_eq!(back, memory, "exact round trip");
    assert_eq!(super::lot_products(&back.trades), vec!["bronze_bar".to_string()]);
}

/// A lot whose inputs were bought is crafted even when its row left the
/// guide (the bare offer of its recipe), and that offer never ranks as a
/// money method (no guide profit).
#[test]
fn a_bought_lot_whose_row_left_the_guide_is_still_crafted() {
    let method = method_of(BRONZE);
    let mut node = ready_money();
    node.blocked_by = Some(method.clone());
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.trades = vec![super::recipe_offer(&method, &|_| 22).expect("bare offer")];
    assert!(!crate::resolve_obtain::money_methods(&ctx).contains(&method), "no profit: never ranked");
    crate::expand::resolve_ready(&mut node, &ctx);
    assert_eq!(node.method.as_deref(), Some(method.as_str()));
    assert_eq!(node.batch, 22);
    assert!(super::recipe_offer(BRONZE, &|_| 22).is_none(), "only a trade method");
}
