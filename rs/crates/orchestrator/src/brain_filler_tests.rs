//! Market waits never idle the worker (user rule 2026-11-13).

use super::parked_waits;
use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::guide_feasible::BotState;
use lifescape_planner::guide_model::{Confidence, Family, FlowLine, MoneyRow, Outlet, PriceBasis, Prereqs, Quote};
use lifescape_planner::guide_pick::GuideLine;
use lifescape_planner::{Archetype, GoalKind, HistoryEntry, Life, Node, NodeKind, NodeStatus, Profile};

fn entry(node: &str, outcome: &str, at: i64) -> HistoryEntry {
    HistoryEntry { node: node.to_string(), started: at, ended: at, outcome: outcome.to_string() }
}

fn obtain(id: &str, method: Option<&str>, status: NodeStatus) -> Node {
    let mut node = Node::new(id, NodeKind::ObtainItems { item: "amulet_of_power".to_string(), count: 1 }, Vec::new(), 1);
    node.method = method.map(str::to_string);
    node.status = status;
    node
}

fn life_of(nodes: Vec<Node>, history: Vec<HistoryEntry>) -> Life {
    Life {
        schema: 1,
        account: "t00".to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer),
        goal: GoalKind::MakeMoney,
        nodes,
        engagement: None,
        active: None,
        history,
        ambition: None,
        goal_origin: None,
        memory: Default::default(),
    }
}

/// A parked wait is a ready `market:buy` obtain whose own last history
/// entry is `awaiting_fill`; a later entry (filled, failed, partial) or
/// another method ends it.
#[test]
fn parked_waits_read_the_node_history() {
    let life = life_of(
        vec![
            obtain("n1", Some("market:buy"), NodeStatus::Ready),
            obtain("n2", Some("market:buy"), NodeStatus::Ready),
            obtain("n3", Some("smith:bronze_bar"), NodeStatus::Ready),
            obtain("n4", Some("market:buy"), NodeStatus::Done),
        ],
        vec![
            entry("n1", "awaiting_fill", 100),
            entry("n2", "awaiting_fill", 100),
            entry("n2", "partial_fill", 150),
            entry("n3", "awaiting_fill", 100),
            entry("n4", "awaiting_fill", 100),
            entry("n9", "launched", 180),
        ],
    );
    assert_eq!(parked_waits(&life), vec![(0, "n1".to_string(), 100)]);
}

fn report(stop: &str, ticks: i64, error: Option<&str>) -> lifescape_json::Value {
    let error_value = match error {
        Some(message) => lifescape_json::object(vec![("type", lifescape_json::str_value("Failed")), ("message", lifescape_json::str_value(message))]),
        None => lifescape_json::Value::Null,
    };
    lifescape_json::object(vec![
        ("node_id", lifescape_json::str_value("fill-n238")),
        ("stop", lifescape_json::str_value(stop)),
        ("ticks", lifescape_json::Value::Int(ticks)),
        ("error", error_value),
    ])
}

/// Live 2026-11-14 (george_97): `melee men --node fill-n238` ended
/// goal_reached after 0 ticks ("no spawn for men" on the mill top), the
/// fold logged "report for unknown node fill-n238, ignored" and the same
/// filler relaunched every 10 s for an hour. A filler report is read:
/// a 0-tick or failed run keeps its skill out of the fillers for a while,
/// a real window does not.
#[test]
fn failed_filler_sits_out_its_skill() {
    use super::{failed_filler_skills, filler_worked, fold_filler_report, FILLER_RETRY_TICKS};
    assert_eq!(filler_worked(&report("goal_reached", 0, None)), Some(false), "0 ticks is no work");
    assert_eq!(filler_worked(&report("failed", 120, Some("no spawn"))), Some(false));
    assert_eq!(filler_worked(&report("budget_exhausted", 1500, None)), Some(true));
    assert_eq!(filler_worked(&report("stopped", 10, None)), None, "operator stop is neutral");
    let mut life = life_of(Vec::new(), vec![entry("fill-n238", "filler:fight:melee/men", 100)]);
    let line = fold_filler_report(&mut life, "fill-n238", &report("goal_reached", 0, None));
    assert!(line.contains("fight:melee/men") && line.contains("failed"), "{line}");
    let fresh = |_: i64| 10u64;
    assert_eq!(failed_filler_skills(&life, &fresh), vec!["attack".to_string()], "melee sits out");
    let stale = |_: i64| FILLER_RETRY_TICKS;
    assert!(failed_filler_skills(&life, &stale).is_empty(), "the cooldown ends");
    assert!(crate::brain_cool::active(&life).contains(&"fight:melee/men".to_string()), "the failed filler is cooled like a node");
    let mut working = life_of(Vec::new(), vec![entry("fill-n238", "filler:fight:melee/men", 100)]);
    fold_filler_report(&mut working, "fill-n238", &report("budget_exhausted", 1500, None));
    assert!(failed_filler_skills(&working, &fresh).is_empty(), "a real window never sits out");
}

/// Tour 2 P2-6 red test: a filler blocked on a missing tool cools its
/// method at its site (the launch tag carries it), so the next filler
/// and the next node resolve never pick it hot again.
#[test]
fn a_failed_filler_cools_its_method_at_its_site() {
    use super::fold_filler_report;
    let mut life = life_of(Vec::new(), vec![entry("fill-n297", "filler:gather:woodcut/yew_logs@3087,3481,0", 100)]);
    let blocked = lifescape_json::object(vec![
        ("node_id", lifescape_json::str_value("fill-n297")),
        ("stop", lifescape_json::str_value("blocked")),
        ("ticks", lifescape_json::Value::Int(3)),
        (
            "error",
            lifescape_json::object(vec![("type", lifescape_json::str_value("Blocked")), ("message", lifescape_json::str_value("blocked, missing: bronze_axe"))]),
        ),
    ]);
    let line = fold_filler_report(&mut life, "fill-n297", &blocked);
    assert!(line.contains("gather:woodcut/yew_logs") && line.contains("cooled"), "{line}");
    let cooled = crate::brain_cool::active(&life);
    assert!(cooled.contains(&"gather:woodcut/yew_logs@3087,3481,0".to_string()), "cooled at its site: {cooled:?}");
}

/// Plan équilibre step 2: a filler's claim on the order board ends with
/// its report, whatever the window made.
#[test]
fn a_filler_report_releases_its_claim() {
    let mut life = life_of(Vec::new(), vec![entry("fill-n9", "filler:gather:mining/coal", 100)]);
    life.account = "filler_claims_probe".to_string();
    crate::market_claims::claim_run("filler_claims_probe", "fill-n9", "filler_claims_coal", 25, 600);
    assert_eq!(crate::market_claims::claimed_total().get("filler_claims_coal"), Some(&25));
    super::fold_filler_report(&mut life, "fill-n9", &report("budget_exhausted", 1500, None));
    assert_eq!(crate::market_claims::claimed_total().get("filler_claims_coal"), None, "released at its report");
}

fn guide_row(family: Family, method: &str, profit: f64) -> MoneyRow {
    MoneyRow {
        family,
        method: method.to_string(),
        style: String::new(),
        site: None,
        tool: String::new(),
        band: 60,
        prereqs: Prereqs::default(),
        flows: Vec::new(),
        other_drops: Default::default(),
        revenue_6000: profit,
        cost_6000: 0.0,
        profit_6000: profit,
        xp_6000: Vec::new(),
        extra_ticks_per_6000: 0.0,
        stages: Vec::new(),
        routes: Vec::new(),
        confidence: Confidence { runs: 4, ..Confidence::default() },
        price_confidence: "traded",
        priced: true,
    }
}

/// Yew logs cut 250 a window, sold at 20 on the market: the guide's
/// best row (5000 per 6000 ticks).
fn yew_row() -> MoneyRow {
    let flow = FlowLine { item: "yew_logs".to_string(), cause: "product".to_string(), per_6000: 250.0, unit_price: 20.0, value_6000: 5000.0, outlet: Outlet::Market, traded: true, basis: PriceBasis::Traded };
    MoneyRow { flows: vec![flow], ..guide_row(Family::Farm, "gather:woodcut/yew_logs", 5000.0) }
}

/// Cooking its own raw trout (3000 per 6000 ticks).
fn trout_row() -> MoneyRow {
    let mut trout = guide_row(Family::Recipe, "cook:trout", 3000.0);
    trout.prereqs.stock = vec![("raw_trout".to_string(), 28.0)];
    trout
}

/// A woodcutter-cook (woodcutting 60, cooking 40) with a rune axe.
fn cook_ctx() -> ReadyCtx {
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.levels = vec![("woodcutting".to_string(), 60), ("cooking".to_string(), 40)];
    ctx.tools = vec!["rune_axe".to_string()];
    ctx
}

/// The money lines of `rows` for that bot holding 200 raw trout, the
/// other producers having claimed `claimed` yew logs on the order board
/// (the brain's `brain_money::publish`, shared demand of step 2).
fn cook_lines(rows: &[MoneyRow], claimed: f64) -> Vec<GuideLine> {
    let rows: Vec<&MoneyRow> = rows.iter().collect();
    let ctx = cook_ctx();
    let level_of = |skill: &str| if skill == "woodcutting" { 60 } else { 40 };
    let none = |_: &str| 0.0;
    let asks = |_: &str| None;
    let bot = BotState { level_of: &level_of, combat: 40, quests_done: &[], held: &none, coins: Some(500.0), ironman: false, asks: &asks };
    let stock = vec![("raw_trout".to_string(), 200.0)];
    let quote = |item: &str| (item == "yew_logs").then(|| Quote { price: 20.0, traded: true, median: 20.0, fills: 5, fill_units: 250.0, claimed, ..Default::default() });
    let no_shop = |_: &str| None;
    let demand = lifescape_planner::guide_share::SharedDemand::new(&quote, &no_shop);
    let shared = |row: &MoneyRow| demand.profit(row);
    crate::brain_money::lines_for(&rows, &ctx, &bot, &stock, 0.5, &|_| None, &shared)
}

/// The coin node of a filler window (`launch_filler`).
fn filler_node() -> Node {
    Node::new("fill-n7", NodeKind::HaveCoins { amount: 1 }, Vec::new(), 0)
}

/// Plan équilibre step 3 (live 2026-11-19: fillers were 20 % of the
/// fleet's time, 52 % of it yew logs resting at 1 coin): a filler picks
/// like a money node, from the money guide on the shared demand. A bot
/// whose best line left is cooking its own raw trout cooks them; its
/// yew logs, whose demand the other producers' claims already took, are
/// no line, never the filler.
#[test]
fn a_filler_draws_the_money_guide_on_the_shared_demand() {
    let rows = [yew_row(), trout_row()];
    let open = cook_lines(&rows, 0.0);
    assert_eq!(open.first().map(|line| line.method.as_str()), Some("gather:woodcut/yew_logs"), "open demand: the yews lead");
    let taken = cook_lines(&rows, 250.0);
    assert_eq!(taken.iter().map(|line| line.method.as_str()).collect::<Vec<_>>(), vec!["cook:trout"], "the yews' demand is claimed");
    let mut node = filler_node();
    let mut ctx = cook_ctx();
    let mut publish = |ctx: &mut ReadyCtx, _: &Node| ctx.money_guide = taken.clone();
    let pick = super::pick_window(&mut node, &mut ctx, &mut publish, &[], &[]).expect("a filler works");
    assert_eq!(pick.method, "cook:trout", "{pick:?}");
    assert!(pick.from_guide, "drawn from the guide: {pick:?}");
    assert_eq!(node.batch, 200, "its lot: the trout it holds");
}

fn line(method: &str, family: &str, profit: f64) -> GuideLine {
    GuideLine { method: method.to_string(), family: family.to_string(), profit_6000: profit, weight: profit, runs: 4, feasible: true, lot: 0 }
}

/// A filler window obtains nothing: a line whose tool is still to buy
/// (the net of the shrimps: class 2) or a 100 % market lot (its
/// purchases are graph nodes) is never its draw. With nothing else in
/// the guide the static money methods rank, as for every money node
/// without a drawable line: never a window blocked on the net for the
/// whole wait.
#[test]
fn a_filler_never_draws_a_line_it_must_equip_or_buy_for() {
    let shrimps = line("gather:fishing/raw_shrimp", "farm", 5000.0);
    let bars = line("trade:craft/smelt:bronze_bar", "market", 4000.0);
    let ctx = cook_ctx();
    assert!(!super::window_works(&shrimps, &ctx, &[]), "the net is still to buy");
    assert!(!super::window_works(&bars, &ctx, &[]), "a market lot buys its inputs");
    assert!(super::window_works(&line("gather:woodcut/yew_logs", "farm", 100.0), &ctx, &[]), "the axe is held");
    let mut node = filler_node();
    let mut ctx = cook_ctx();
    let mut publish = |ctx: &mut ReadyCtx, _: &Node| ctx.money_guide = vec![shrimps.clone(), bars.clone()];
    let pick = super::pick_window(&mut node, &mut ctx, &mut publish, &[], &[]).expect("a static window works");
    assert!(!pick.from_guide && ctx.money_guide.is_empty(), "{pick:?}");
    assert!(pick.method != "gather:fishing/raw_shrimp" && !pick.method.starts_with("trade:"), "{pick:?}");
    assert!(!matches!(node.status, NodeStatus::Blocked { .. }), "never blocked on the net: {:?}", node.status);
}

/// Review lot 3, point 1: a woodcutter whose guide lines are all logs
/// and whose yew filler just failed (route error) never idles on "no
/// filler: every workable money method trains a banned skill" for 500
/// ticks: the lines of a skill that just failed are no window's draw,
/// the static money methods rank (R7) and the window works off it.
#[test]
fn a_filler_skips_the_guide_lines_of_a_skill_that_just_failed() {
    let failed = vec!["woodcutting".to_string()];
    let yews = line("gather:woodcut/yew_logs", "farm", 5000.0);
    let willows = line("gather:woodcut/willow_logs", "farm", 2000.0);
    let mut node = filler_node();
    let mut ctx = cook_ctx();
    let mut publish = |ctx: &mut ReadyCtx, _: &Node| ctx.money_guide = vec![yews.clone(), willows.clone()];
    let pick = super::pick_window(&mut node, &mut ctx, &mut publish, &[], &failed).expect("a window works off woodcutting");
    assert!(!pick.from_guide && !pick.method.starts_with("gather:woodcut/"), "{pick:?}");
}

/// Review lot 3, point 2: a 10-hitpoint fighter without food never
/// fills on a spot that bites it (class 1 of `money_pick`: feasible,
/// food to obtain first; the hunt would stop `food_blocked` after the
/// bank and cool its skill): only what a window works at once (class 0).
#[test]
fn a_foodless_fighter_never_fills_on_a_biting_spot() {
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.levels = vec![("hitpoints".to_string(), 10), ("attack".to_string(), 10), ("strength".to_string(), 10), ("defence".to_string(), 10)];
    ctx.combat = 13;
    let men = line("fight:melee/men", "farm", 900.0);
    assert_eq!(lifescape_planner::money_pick::class_of(&men, &ctx), Some(1), "feasible, food first");
    assert!(!super::window_works(&men, &ctx, &[]), "no food in hand");
    let mut node = filler_node();
    let mut publish = |ctx: &mut ReadyCtx, _: &Node| ctx.money_guide = vec![men.clone()];
    let pick = super::pick_window(&mut node, &mut ctx, &mut publish, &[], &[]).expect("a window works");
    assert!(!pick.from_guide && pick.method != "fight:melee/men", "{pick:?}");
}

/// Lot 3, live 2026-11-19 (lara_lane, owen_wood, ...): coal drawn by
/// every filler, the second window sent to the four Barbarian Village
/// coal rocks a first one already worked, "contended" 38 times in two
/// hours. A filler reads the fleet's sites like a money node: rocks the
/// crowd already empties (100-tick respawn: one miner) are never drawn
/// by a second filler; it works the coal elsewhere.
#[test]
fn a_crowded_site_is_never_drawn_by_a_second_filler() {
    let barbarian = (3089, 3434, 0);
    let miner_ctx = |crowd: Vec<((i64, i64, i64), u32)>| {
        let mut ctx = ReadyCtx::blind(0.5);
        ctx.levels = vec![("mining".to_string(), 60)];
        ctx.tools = vec!["rune_pickaxe".to_string()];
        ctx.combat = 40;
        ctx.position = Some((3185, 3436, 0));
        ctx.seats = crowd.iter().map(|(site, count)| (*site, "gather:mining/coal".to_string(), *count)).collect();
        ctx.crowd = crowd;
        ctx
    };
    let coal = line("gather:mining/coal", "farm", 20000.0);
    let pick = |crowd: Vec<((i64, i64, i64), u32)>| {
        let mut node = filler_node();
        let mut ctx = miner_ctx(crowd);
        let mut publish = |ctx: &mut ReadyCtx, _: &Node| ctx.money_guide = vec![coal.clone()];
        let pick = super::pick_window(&mut node, &mut ctx, &mut publish, &[], &[]).expect("coal is workable");
        (pick, node.site)
    };
    // The fastest coal (bank loads weighed, `combat::bank_weight`),
    // whichever field it is; filled, the next filler goes elsewhere.
    let _ = barbarian;
    let (first, first_site) = pick(Vec::new());
    let field = first_site.expect("a coal site");
    assert_eq!(first.method.as_str(), "gather:mining/coal");
    let seats = lifescape_planner::combat::site_capacity("gather:mining/coal", field).expect("a rock field") as u32;
    let (second, second_site) = pick(vec![(field, seats)]);
    let site = second_site.expect("a site");
    assert!(!lifescape_planner::combat::site_full("gather:mining/coal", site, &[(field, seats)]), "second filler on the crowded rocks: {second:?} at {site:?}");
}

/// Lot 3 review, CPU (filler.launch 27 → 212 ms CPU a call): the money
/// guide publication read the bot's holdings ~270 times, each a copy of
/// the live snapshot and the locks' store queries. The cached read
/// (`brain_plan::Holdings`, once per publication) counts exactly what
/// `holdings_count` counts, every object, asked once or twice; and a
/// window reads what the bot keeps once, the alchemy's `kept` (its node
/// is never in the graph).
#[test]
fn a_filler_pick_reads_the_same_holdings_from_the_cache() {
    let stack = |name: &str, count: i64| {
        let obj = lifescape_gamedata::objs_by_name(name).into_iter().next().expect("in the dump");
        lifescape_json::object(vec![("id", lifescape_json::Value::Int(obj.obj_id as i64)), ("name", lifescape_json::str_value(obj.display.unwrap_or(name))), ("count", lifescape_json::Value::Int(count))])
    };
    let live = lifescape_json::object(vec![
        ("inv", lifescape_json::Value::Arr(vec![stack("coins", 1234), stack("iron_ore", 5), stack("rune_pickaxe", 1)])),
        ("bank", lifescape_json::Value::Arr(vec![stack("iron_ore", 20), stack("cert_coal", 300), stack("raw_trout", 28)])),
        ("worn", lifescape_json::Value::Arr(vec![stack("bronze_platebody", 1)])),
    ]);
    let job = crate::jobs::Job::new("filler_holdings_probe");
    job.inner.lock().expect("job").live = Some(live.clone());
    let cached = crate::brain_plan::Holdings::of(&job);
    for item in ["coins", "iron_ore", "coal", "raw_trout", "rune_pickaxe", "bronze_platebody", "yew_logs", "iron_ore", "coal"] {
        assert_eq!(cached.count(item), crate::brain_plan::holdings_count(&job, item), "{item}");
    }
    assert_eq!(cached.count("iron_ore"), 25);
    assert_eq!(cached.count("coal"), 300, "the banknotes count as the object");
    let life = life_of(vec![obtain("n1", Some("market:buy"), NodeStatus::Ready)], Vec::new());
    assert_eq!(crate::brain_alch::kept_except(&life, &live, Some("fill-n1")), crate::brain_alch::kept(&life, &live), "the window's keep is the alchemy's");
}

/// Review lot 3, point 6: a refused filler attempt (two guide reads)
/// is not tried again every pass: the account waits
/// `FILLER_REFUSAL_TICKS`, a launch ends the wait.
#[test]
fn a_refused_filler_waits_before_reading_the_guide_again() {
    let account = "filler_refusal_probe";
    assert!(!super::refused_recently(account, 1000));
    super::note_refusal(account, 1000);
    assert!(super::refused_recently(account, 1000), "right after its refusal");
    assert!(!super::refused_recently(account, 1000 + 3600), "an hour later");
    super::note_refusal(account, 2000);
    super::clear_refusal(account);
    assert!(!super::refused_recently(account, 2000), "a launch ends the wait");
}
