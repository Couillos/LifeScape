use super::{build_report, report_value, snapshot_holdings, task_start_for, Holdings, ReportCtx, TaskReport};
use crate::report_meta::RunMeta;
use lifescape_sdk::client::coin_ledger::CoinFlow;
use lifescape_sdk::client::item_ledger::ItemFlow;
use lifescape_sdk::client::run_events::RunEvents;
use lifescape_sdk::Bot;
use std::collections::HashMap;

fn live_bot() -> Bot {
    let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
    lifescape_scripts::attach_live_tables(&mut bot);
    bot
}

fn id(bot: &Bot, name: &str) -> u16 {
    (bot.tables.find_obj_by_name)(name).unwrap_or_else(|| panic!("{name} in dumps"))
}

fn empty_bank() -> HashMap<u16, u32> {
    HashMap::new()
}

fn holdings(items: &[(u16, u32)], stats: &[(u8, (u8, i32))], tick: u64) -> Holdings {
    Holdings {
        items: items.iter().map(|(id, count)| (*id, *count)).collect(),
        stats: stats.iter().map(|(stat, value)| (*stat, *value)).collect(),
        tick,
        pause_ticks: 0,
        seen_messages: 0,
        bank_opens: 0,
        coin_flows: 0,
        item_flows: 0,
        events: RunEvents::default(),
    }
}

fn ctx(activity: &str, stop: &str) -> ReportCtx {
    ReportCtx { activity: activity.to_string(), node_id: "n9".to_string(), stop: stop.to_string(), missing: Vec::new(), error: None, meta: RunMeta::default() }
}

fn item(bot: &mut Bot, obj: u16, kind: &'static str, delta: i64, skills: &[u8]) {
    let skills = skills.iter().fold(0u32, |mask, stat| mask | (1 << stat));
    bot.item_ledger.flows.push(ItemFlow { obj, kind, delta, tick: 1, skills });
}

fn coin(bot: &mut Bot, kind: &'static str, delta: i64) {
    bot.coin_ledger.flows.push(CoinFlow { kind, delta, tick: 1 });
}

fn lines(report: &TaskReport) -> Vec<(String, &'static str, i64)> {
    report.items.iter().map(|flow| (flow.name.clone(), flow.provenance, flow.qty)).collect()
}

fn line(name: &str, label: &'static str, qty: i64) -> (String, &'static str, i64) {
    (name.to_string(), label, qty)
}

/// Woodcut: the ledger's skill flow is the product; levels ride along.
#[test]
fn logs_with_xp_are_produced() {
    let mut bot = live_bot();
    let logs = id(&bot, "logs");
    item(&mut bot, logs, "skill", 25, &[8]);
    let before = holdings(&[], &[(8, (2, 100))], 1000);
    let after = holdings(&[(logs, 25)], &[(8, (2, 350))], 1600);
    let report = build_report(&bot, &before, &empty_bank(), &after, &empty_bank(), &ctx("woodcut", "goal_reached"));
    assert_eq!(report.ticks, 600);
    assert_eq!(report.xp, vec![(8, "woodcutting".to_string(), 250)]);
    assert_eq!(lines(&report), vec![line("Logs", "produced", 25)]);
    assert_eq!(report.flows, vec![line("Logs", "product", 25)]);
    assert!(report.alerts.is_empty(), "{:?}", report.alerts);
    assert_eq!(report.units.products.get("Logs"), Some(&25));
    assert_eq!(report.levels_before, vec![(8, 2)]);
}

/// Melee: bones picked then buried (the net diff saw nothing), food
/// eaten; the legacy lines keep the aggregation's provenances.
#[test]
fn melee_bones_and_food_both_ways() {
    let mut bot = live_bot();
    let bones = id(&bot, "bones");
    let shrimp = id(&bot, "shrimp");
    item(&mut bot, bones, "pickup", 2, &[]);
    item(&mut bot, bones, "bury", -2, &[5]);
    item(&mut bot, shrimp, "eat", -1, &[0]);
    let before = holdings(&[(shrimp, 3)], &[(0, (23, 6900)), (5, (3, 100))], 1000);
    let after = holdings(&[(shrimp, 2)], &[(0, (23, 7100)), (5, (3, 109))], 2000);
    let report = build_report(&bot, &before, &empty_bank(), &after, &empty_bank(), &ctx("melee", "budget_exhausted"));
    assert_eq!(lines(&report), vec![line("Bones", "consumed", -2), line("Bones", "loot", 2), line("Shrimps", "consumed", -1)]);
    assert_eq!(report.flows, vec![line("Bones", "input", -2), line("Bones", "loot", 2), line("Shrimps", "food", -1)]);
    assert_eq!(report.units.inputs.get("Bones"), Some(&2), "the prayer unit is visible");
    assert!(report.alerts.is_empty(), "{:?}", report.alerts);
}

/// Shop flows come from the shop context, never from a coins coupling.
#[test]
fn shop_context_marks_shop_flows() {
    let mut bot = live_bot();
    let logs = id(&bot, "logs");
    let axe = id(&bot, "bronze_axe");
    let coins = id(&bot, "coins");
    item(&mut bot, logs, "shop", -20, &[]);
    item(&mut bot, axe, "shop", 1, &[]);
    coin(&mut bot, "shop", 18);
    coin(&mut bot, "shop", -20);
    let before = holdings(&[(logs, 20), (coins, 5)], &[], 100);
    let after = holdings(&[(coins, 3), (axe, 1)], &[], 200);
    let report = build_report(&bot, &before, &empty_bank(), &after, &empty_bank(), &ctx("woodcut", "goal_reached"));
    assert_eq!(lines(&report), vec![line("Bronze axe", "shop_bought", 1), line("Coins", "shop_bought", -2), line("Logs", "shop_sold", -20)]);
    assert!(report.alerts.is_empty(), "{:?}", report.alerts);
}

/// A loss the ledger never saw is no flow: a `residual` alert (the
/// coins "lost" of the 2026-11-16 audit), never a yield or a cost.
#[test]
fn unseen_losses_are_residual_alerts() {
    let bot = live_bot();
    let coins = id(&bot, "coins");
    let before = holdings(&[(coins, 30)], &[], 100);
    let after = holdings(&[(coins, 20)], &[], 200);
    let report = build_report(&bot, &before, &empty_bank(), &after, &empty_bank(), &ctx("woodcut", "failed"));
    assert!(report.items.is_empty());
    assert_eq!(report.alerts.len(), 1);
    assert_eq!((report.alerts[0].kind, report.alerts[0].qty), ("residual", -10));
}

/// Lone coins no action explains are `unattributed` (D45): an alert,
/// never a yield.
#[test]
fn lone_coin_gains_are_unattributed() {
    let mut bot = live_bot();
    let coins = id(&bot, "coins");
    coin(&mut bot, "unattributed", 50);
    let before = holdings(&[(coins, 200)], &[], 100);
    let after = holdings(&[(coins, 250)], &[], 200);
    let report = build_report(&bot, &before, &empty_bank(), &after, &empty_bank(), &ctx("woodcut", "goal_reached"));
    assert_eq!(lines(&report), vec![line("Coins", "unattributed", 50)]);
    assert_eq!(report.alerts.iter().map(|alert| alert.kind).collect::<Vec<_>>(), vec!["unattributed"]);
}

/// Market runs label every leg `traded` (D45); lunch stays consumed.
#[test]
fn market_runs_label_traded() {
    let mut bot = live_bot();
    let feather = id(&bot, "feather");
    let coins = id(&bot, "coins");
    let shrimp = id(&bot, "shrimp");
    item(&mut bot, feather, "trade", 100, &[]);
    item(&mut bot, shrimp, "eat", -1, &[]);
    coin(&mut bot, "trade", -200);
    let before = holdings(&[(coins, 200), (shrimp, 3)], &[], 100);
    let after = holdings(&[(feather, 100), (shrimp, 2)], &[], 200);
    let report = build_report(&bot, &before, &empty_bank(), &after, &empty_bank(), &ctx("market", "budget_exhausted"));
    assert_eq!(lines(&report), vec![line("Coins", "traded", -200), line("Feather", "traded", 100), line("Shrimps", "consumed", -1)]);
}

/// Pre-stock cancels in the coherence check: 100 logs banked at the
/// first open plus 25 cut and deposited balance the ledger's 25.
#[test]
fn prestock_cancels_against_the_baseline() {
    let mut bot = live_bot();
    let logs = id(&bot, "logs");
    item(&mut bot, logs, "skill", 25, &[8]);
    let before = holdings(&[(logs, 3)], &[(8, (2, 100))], 100);
    let after = holdings(&[(logs, 3)], &[(8, (2, 350))], 200);
    let baseline: HashMap<u16, u32> = [(logs, 100)].into_iter().collect();
    let end: HashMap<u16, u32> = [(logs, 125)].into_iter().collect();
    let report = build_report(&bot, &before, &baseline, &after, &end, &ctx("woodcut", "goal_reached"));
    assert_eq!(lines(&report), vec![line("Logs", "produced", 25)]);
    assert!(report.alerts.is_empty(), "{:?}", report.alerts);
}

/// Casts and kills of the run become units by spell and NPC name.
#[test]
fn casts_and_kills_are_units() {
    let bot = live_bot();
    let strike = lifescape_gamedata::com_id("magic:wind_strike").expect("wind strike button");
    let before = holdings(&[], &[(6, (10, 1000))], 100);
    let mut after = holdings(&[], &[(6, (10, 1165))], 400);
    after.events.casts.insert(strike, 3);
    after.events.casts.insert(1, 1);
    let chicken = lifescape_gamedata::npcs_by_name("chicken").into_iter().next().expect("chicken in dumps").npc_id as u16;
    after.events.kills.insert(chicken, 2);
    after.events.damage.insert(chicken, 6);
    let report = build_report(&bot, &before, &empty_bank(), &after, &empty_bank(), &ctx("magic", "budget_exhausted"));
    assert_eq!(report.units.casts.get("wind_strike"), Some(&3));
    assert_eq!(report.units.casts.len(), 1, "a non-spell button is no cast");
    assert_eq!(report.units.kills.values().sum::<i64>(), 2);
    assert_eq!(report.units.damage.values().sum::<i64>(), 6);
}

/// Report JSON carries the legacy fields and the contract-2 ones.
#[test]
fn report_json_carries_the_contract() {
    let mut bot = live_bot();
    let logs = id(&bot, "logs");
    item(&mut bot, logs, "skill", 25, &[8]);
    let before = holdings(&[], &[(8, (2, 100))], 100);
    let after = holdings(&[(logs, 25)], &[(8, (2, 350))], 700);
    let mut context = ctx("woodcut", "budget_exhausted");
    context.meta = RunMeta {
        planner_method: "gather:woodcut/logs".to_string(),
        method_source: "node",
        site: Some((3190, 3240, 0)),
        target: "logs".to_string(),
        tool: "bronze_axe".to_string(),
        combat_style: String::new(),
        levels_start: vec![(8, 2), (0, 1)],
    };
    let report = build_report(&bot, &before, &empty_bank(), &after, &empty_bank(), &context);
    let value = report_value("t01", &report);
    assert_eq!(value.get("contract").as_i64(), Some(2));
    assert_eq!(value.get("activity").as_str(), Some("woodcut"));
    assert_eq!(value.get("planner_method").as_str(), Some("gather:woodcut/logs"));
    assert_eq!(value.get("method_source").as_str(), Some("node"));
    assert_eq!(value.get("site").as_arr().map(|parts| parts.len()), Some(3));
    assert_eq!(value.get("levels_start").get("woodcutting").as_i64(), Some(2));
    assert_eq!((value.get("ticks").as_i64(), value.get("work_ticks").as_i64(), value.get("pause_ticks").as_i64()), (Some(600), Some(600), Some(0)));
    assert_eq!(value.get("flows").as_arr().map(|rows| rows.len()), Some(1));
    assert_eq!(value.get("units").get("product").get("Logs").as_i64(), Some(25));
    assert_eq!(value.get("items").as_arr().map(|rows| rows.len()), Some(1));
    let parsed = crate::report_contract::parse_contract(&value).expect("parses").expect("contract 2");
    assert_eq!((parsed.planner_method.as_str(), parsed.site.as_str(), parsed.work_ticks), ("gather:woodcut/logs", "3190,3240,0", 600));
    // A run with no carried method says so: nothing invented.
    let bare = report_value("t01", &build_report(&bot, &before, &empty_bank(), &after, &empty_bank(), &ctx("woodcut", "goal_reached")));
    assert_eq!((bare.get("method_source").as_str(), bare.get("planner_method").as_str()), (Some("none"), Some("")));
    // The legacy rates parser reads the contract-2 report unchanged.
    let legacy = crate::rates_service::parse_report(&value).expect("legacy parse");
    assert_eq!((legacy.method.as_str(), legacy.items.len()), ("woodcut", 1));
}

/// Start carrier: the contract scripts parse their CLI into a budget and
/// a node; the carried method lands in the run context.
#[test]
fn task_start_parses_budget_node_and_method() {
    let bot = live_bot();
    let args = |list: &[&str]| list.iter().map(|arg| arg.to_string()).collect::<Vec<String>>();
    let start = task_start_for(&bot, "woodcut", &args(&["logs", "--budget-max", "6000", "--node", "n9", "--site", "3190,3240,0"]), Some("gather:woodcut/logs"))
        .expect("contract script carries a start");
    assert_eq!((start.budget_max, start.node_id.as_str()), (6000, "n9"));
    assert_eq!((start.meta.method_source, start.meta.planner_method.as_str(), start.meta.site, start.meta.target.as_str()), ("node", "gather:woodcut/logs", Some((3190, 3240, 0)), "logs"));
    assert!(task_start_for(&bot, "quest_cook", &[], None).is_none(), "quests stay on the minimal path");
    assert!(task_start_for(&bot, "woodcut", &args(&["--frobnicate"]), None).is_none(), "junk CLI carries nothing");
    let node = |script: &str, list: &[&str]| task_start_for(&bot, script, &args(list), None).map(|start| (start.script, start.budget_max, start.node_id, start.meta.method_source));
    assert_eq!(node("tutorial", &["--node", "n2"]), Some(("tutorial".to_string(), 0, "n2".to_string(), "none")));
    assert_eq!(node("fishing", &["--level", "fishing", "--node", "n1"]), Some(("fishing".to_string(), 0, "n1".to_string(), "none")));
    assert_eq!(node("tutorial", &["--node"]), None, "a bare flag names no node");
    let post = task_start_for(&bot, "market", &args(&["make", "--post", "3211,3427,0", "--budget-max", "9000", "--node", "n3"]), None).expect("a maker's post carries a start");
    assert_eq!((post.budget_max, post.node_id.as_str()), (9000, "n3"));
    assert!(task_start_for(&bot, "market", &args(&["deliver", "1", "2", "3=1,1"]), None).is_none(), "deliver legs stay minimal");
}

/// The bank's side panel mirrors the pack: never counted twice (live
/// admin 2026-11-16: carried 803408 -> 401704 coins, the half read lost).
#[test]
fn pack_mirrors_are_not_holdings() {
    let mut bot = live_bot();
    let coins = id(&bot, "coins");
    let pack_com = (bot.tables.com_id)("inventory:inv").expect("inventory:inv in dumps");
    let side_com = (bot.tables.com_id)("bank_side:inv").expect("bank_side:inv in dumps");
    let worn_com = (bot.tables.com_id)("wornitems:wear").expect("wornitems:wear in dumps");
    let arrows = id(&bot, "bronze_arrow");
    bot.world.inv.insert(pack_com, [(0u8, (coins, 401_704u32))].into_iter().collect());
    bot.world.inv.insert(side_com, [(0u8, (coins, 401_704u32))].into_iter().collect());
    bot.world.inv.insert(worn_com, [(13u8, (arrows, 50u32))].into_iter().collect());
    let held = snapshot_holdings(&bot);
    assert_eq!(held.items.get(&coins), Some(&401_704));
    assert_eq!(held.items.get(&arrows), Some(&50), "worn ammo is carried");
}

/// Closed bank: the cache stands in while open; the snapshot holds the
/// visible pack/worn only.
#[test]
fn closed_bank_reads_from_cache() {
    use super::{refresh_bank_cache, split_bank};
    let mut bot = live_bot();
    let logs = id(&bot, "logs");
    let bank_com = (bot.tables.com_id)("bank_main:inv").expect("bank_main in dumps");
    let pack_com = (bot.tables.com_id)("inventory:inv").expect("inventory:inv in dumps");
    bot.world.inv.insert(bank_com, [(0u8, (logs, 100u32))].into_iter().collect());
    bot.world.inv.insert(pack_com, [(0u8, (logs, 3u32))].into_iter().collect());
    let mut cache = HashMap::new();
    assert!(refresh_bank_cache(&bot, &mut cache).is_some(), "open bank warms the cache");
    assert_eq!(cache.get(&logs), Some(&100));
    let (visible, live) = split_bank(&bot);
    assert_eq!((visible.get(&logs), live.get(&logs)), (Some(&3), Some(&100)));
    bot.world.inv.remove(&bank_com);
    let mut cache_closed = cache.clone();
    assert!(refresh_bank_cache(&bot, &mut cache_closed).is_none(), "closed bank refreshes nothing");
    assert_eq!(cache_closed.get(&logs), Some(&100), "closed bank keeps the cache");
    assert_eq!(snapshot_holdings(&bot).items.get(&logs), Some(&3), "snapshot holds visible only");
    bot.world.inv.insert(bank_com, HashMap::new());
    assert!(refresh_bank_cache(&bot, &mut cache_closed).is_some(), "emptied bank still streams");
    assert!(cache_closed.is_empty(), "emptied bank clears the cache");
}

/// Live 2026-11-16 (amber_moore fight:melee/men): a market pause bought
/// 281 noted iron ore, the run banked it (the bank holds the base
/// object): residual +281 (base, bank 1856->2137, flows 0) and -281
/// (note, flows 281) in one report. Notes fold into their base object;
/// the reverse leg too (alice_knight jewelry: 26 ore withdrawn noted then
/// sold, residual -26 and +26), and noted stock carried at the start
/// then banked (ida_reed: emerald amulet +2 and -2).
#[test]
fn noted_lots_fold_into_their_base_object() {
    let mut bot = live_bot();
    let ore = id(&bot, "iron_ore");
    let noted = id(&bot, "cert_iron_ore");
    item(&mut bot, noted, "trade", 281, &[]);
    item(&mut bot, noted, "trade", -26, &[]);
    let bank_start: HashMap<u16, u32> = [(ore, 1856)].into_iter().collect();
    let bank_end: HashMap<u16, u32> = [(ore, 2111)].into_iter().collect();
    let report = build_report(&bot, &holdings(&[], &[], 100), &bank_start, &holdings(&[], &[], 200), &bank_end, &ctx("melee", "goal_reached"));
    assert!(report.alerts.is_empty(), "{:?}", report.alerts);
    assert_eq!(report.flows, vec![line("Iron ore", "traded", -26), line("Iron ore", "traded", 281)]);
    let amulet = id(&bot, "strung_emerald_amulet");
    let noted_amulet = id(&bot, "cert_strung_emerald_amulet");
    let banked: HashMap<u16, u32> = [(amulet, 2)].into_iter().collect();
    let report = build_report(&live_bot(), &holdings(&[(noted_amulet, 2)], &[], 100), &empty_bank(), &holdings(&[], &[], 200), &banked, &ctx("mining", "failed"));
    assert!(report.alerts.is_empty(), "{:?}", report.alerts);
}

/// Live 2026-11-16 (daisy_hunt fight:melee/men t=2096-2119): deposit 10
/// salmon, withdraw 10, close. One pump drained the withdrawal's bank
/// update with the stop-transmit, so a cache refreshed only while the
/// bank streamed kept 694 instead of 684: residual +10 salmon (same for
/// coins +1/+10, wooden shield -1, jug of water -1). Closed, the cache
/// takes the bank as it stood at its stop-transmit.
#[test]
fn closed_bank_cache_takes_the_stop_transmit_state() {
    use super::refresh_bank_cache;
    let mut bot = live_bot();
    let salmon = id(&bot, "salmon");
    let bank_com = (bot.tables.com_id)("bank_main:inv").expect("bank_main in dumps");
    let pack_com = (bot.tables.com_id)("inventory:inv").expect("inventory:inv in dumps");
    let salmon_slot = |count: u32| {
        let mut body = bank_com.to_be_bytes().to_vec();
        body.push(3);
        body.extend((salmon + 1).to_be_bytes());
        body.push(255);
        body.extend(count.to_be_bytes());
        body
    };
    bot.world.inv.insert(bank_com, [(3u8, (salmon, 684u32))].into_iter().collect());
    bot.world.inv.insert(pack_com, HashMap::new());
    let mut cache = HashMap::new();
    let baseline = refresh_bank_cache(&bot, &mut cache).expect("open bank streams");
    // The deposit, seen while the bank streams.
    bot.world.feed("UPDATE_INV_PARTIAL", &salmon_slot(694)).expect("deposit");
    assert!(refresh_bank_cache(&bot, &mut cache).is_some());
    // One drain, no refresh between: the withdrawal's bank update, then
    // the stop-transmit.
    bot.world.feed("UPDATE_INV_PARTIAL", &salmon_slot(684)).expect("withdrawal");
    bot.world.feed("UPDATE_INV_STOP_TRANSMIT", &bank_com.to_be_bytes()).expect("stop transmit");
    assert!(refresh_bank_cache(&bot, &mut cache).is_none(), "closed bank streams nothing");
    assert_eq!(cache.get(&salmon), Some(&684), "the bank at its stop-transmit");
    // 10 salmon carried at start and end, deposited then withdrawn
    // (transfers, no flow): no residual.
    let before = holdings(&[(salmon, 10)], &[], 2035);
    let after = holdings(&[(salmon, 10)], &[], 3535);
    let report = build_report(&bot, &before, &baseline, &after, &cache, &ctx("melee", "budget_exhausted"));
    assert!(report.alerts.is_empty(), "{:?}", report.alerts);
}
