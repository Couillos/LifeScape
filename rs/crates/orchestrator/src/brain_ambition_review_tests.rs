//! Decision D3 in the brain (user 2026-11-19, « pourquoi on a toujours
//! autant de cons qui coupent du yew ») : the one-time review of the
//! old-rule ambitions, the ladder end of a need-led bot, the goal of an
//! earlier ambition replaced (mila_wade). No market, no guide, no fleet
//! file in tests: the need is empty (uniform draw), the gaps none.

use lifescape_json::{object, Value};
use lifescape_planner::{
    needs_review, new_ambition, Ambition, Archetype, BotNow, GoalKind, HistoryEntry, Life, MilestoneStatus, Node, NodeKind, NodeStatus, Profile, Revision, Stamp, NEED_RULE,
    REVIEW_SPREAD_TICKS,
};

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
}

/// Live snapshot: woodcutting 99, fishing 50, everything else 30.
fn live() -> Value {
    let skill = |id: i64, level: i64| object(vec![("id", Value::Int(id)), ("level", Value::Int(level))]);
    let rows = (0..=20).map(|id| skill(id, 30)).filter(|row| ![8, 10].contains(&row.get("id").as_i64().unwrap_or(-1)));
    object(vec![("skills", Value::Arr(rows.chain([skill(8, 99), skill(10, 50)]).collect()))])
}

fn ambition(kind: &str, why: &str, secs: i64) -> Ambition {
    new_ambition(kind, why, &BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 }, 7, &[], Stamp { tick: 0, secs }).expect("an ambition")
}

fn life(account: &str, held: Ambition) -> Life {
    Life {
        schema: 1,
        account: account.to_string(),
        created_at: 0,
        seed: 11,
        profile: Profile::new(false, 0.5, 0.5, 11, Archetype::Producer),
        goal: GoalKind::MakeMoney,
        nodes: Vec::new(),
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: Some(held),
        goal_origin: None,
        memory: Default::default(),
    }
}

fn goal_entry(outcome: &str, at: i64) -> HistoryEntry {
    HistoryEntry { node: "goal".to_string(), started: at, ended: at, outcome: outcome.to_string() }
}

/// The woodcutter's ladder done (every rung materialized, every
/// milestone done), drawn long ago (past the change throttle).
fn achieved_woodcutter(account: &str) -> Life {
    let mut held = ambition("woodcutter", "random", now_secs() - 400_000);
    let ladder = lifescape_planner::ambition_catalog::spec("woodcutter").expect("woodcutter").ladder.len();
    held.rung = ladder;
    for stone in &mut held.milestones {
        stone.status = MilestoneStatus::Done;
    }
    assert!(lifescape_planner::achieved(&held), "precondition: ladder done");
    life(account, held)
}

/// Ladder end, need-led bot: redrawn by need (`achieved`), marked, never
/// the same ambition extended; the D3 exception (Zezima) keeps the
/// extension.
#[test]
fn a_need_led_ladder_end_redraws_and_an_exception_extends() {
    let mut jane = achieved_woodcutter("b0");
    super::before_draw(&mut jane, &live());
    let held = jane.ambition.as_ref().expect("an ambition");
    assert_eq!(held.revisions.last().map(|revision| revision.why.as_str()), Some("achieved"), "redrawn, not extended: {}", lifescape_planner::describe_ambition(held));
    assert_eq!(held.draw_rule, NEED_RULE, "drawn by need");
    assert_eq!(held.kind, "woodcutter", "no other bot holds woodcutting here: the sole holder keeps its group");
    let mut zezima = achieved_woodcutter("Zezima");
    super::before_draw(&mut zezima, &live());
    let kept = zezima.ambition.as_ref().expect("an ambition");
    assert!(kept.revisions.is_empty(), "the exception keeps its ambition");
    assert_eq!(kept.kind, "woodcutter");
    assert!(kept.milestones.iter().any(|stone| stone.origin == lifescape_planner::ambition::EXTEND_ORIGIN), "extended past the ladder");
}

/// One-time review: an unmarked (old-rule) ambition of a need-led bot is
/// redrawn by need once its slot is reached, marked, never again; the
/// exception is untouched. Its goal, linked to the old ambition, is then
/// replaced.
#[test]
fn the_review_redraws_old_rule_ambitions_once_and_spares_the_exceptions() {
    let drawn_long_ago = now_secs() - 400_000;
    let mut arthur = life("b0", ambition("woodcutter", "random", drawn_long_ago));
    arthur.goal = GoalKind::MaxSkills { skills: vec!["woodcutting".to_string()], level: 99 };
    let milestone = arthur.ambition.as_ref().and_then(|held| held.milestones.first()).map(|stone| stone.id.clone()).expect("a milestone");
    lifescape_planner::mark_drawn(&mut arthur, Some(&milestone));
    arthur.history.push(goal_entry("next:woodcutting 99", now_secs() - 5_000));
    let slot = lifescape_planner::review_slot_ticks("b0");
    if slot > 0 {
        assert!(!super::review_at(&mut arthur, &live(), slot - 1, now_secs() - 60), "not before its slot");
    }
    assert!(super::review_at(&mut arthur, &live(), REVIEW_SPREAD_TICKS, now_secs() - 60), "reviewed in the window");
    let held = arthur.ambition.clone().expect("an ambition");
    assert_eq!(held.draw_rule, NEED_RULE);
    assert_eq!(held.revisions.last().map(|revision| revision.why.as_str()), Some("review:need"));
    assert!(!super::review_at(&mut arthur, &live(), REVIEW_SPREAD_TICKS * 10, now_secs() - 60), "once only");
    assert_eq!(arthur.ambition.as_ref().map(|held| held.revisions.len()), Some(held.revisions.len()));
    // The goal the old woodcutter ambition drew is replaced.
    assert!(super::replace_stale_goal(&mut arthur, &live()), "goal of the old ambition replaced");
    assert!(arthur.history.iter().rev().find(|entry| entry.outcome.starts_with("next:")).is_some_and(|entry| entry.outcome != "next:woodcutting 99"));
    let mut zezima = life("Zezima", ambition("woodcutter", "random", drawn_long_ago));
    let before = zezima.ambition.clone();
    assert!(!super::review_at(&mut zezima, &live(), REVIEW_SPREAD_TICKS * 10, now_secs() - 60), "exception untouched");
    assert_eq!(zezima.ambition, before);
}

/// Review 2026-11-19: an operator ambition is never undone. The alcher
/// switch (`orchestrator ambition ryan_mason alcher`) applied at the start
/// of a pass (`brain_operator::apply_pending`) and the review in the same
/// pass leave it alcher; an ambition drawn after the daemon start (an
/// operator's, a fresh draw) is never reviewed, marked or not.
#[test]
fn an_operator_switch_survives_the_review_of_the_same_pass() {
    let started = now_secs() - 60;
    let mut ryan = life("b0", ambition("woodcutter", "random", now_secs() - 400_000));
    let switched = super::with_now(&live(), |now| lifescape_planner::become_alcher(&mut ryan, now, super::stamp_now()));
    assert!(switched.is_ok(), "{switched:?}");
    assert!(!super::review_at(&mut ryan, &live(), REVIEW_SPREAD_TICKS * 10, started), "the operator's alcher stays");
    assert_eq!(ryan.ambition.as_ref().map(|held| held.kind.as_str()), Some(lifescape_planner::ALCHER));
    // Even unmarked, an ambition changed after the start is not reviewed.
    let mut fresh = life("b0", ambition("woodcutter", "random", now_secs()));
    assert!(!super::review_at(&mut fresh, &live(), REVIEW_SPREAD_TICKS * 10, started), "changed after the daemon start");
    let mut old = life("b0", ambition("woodcutter", "random", now_secs() - 400_000));
    assert!(super::review_at(&mut old, &live(), REVIEW_SPREAD_TICKS * 10, started), "drawn before the start: reviewed");
}

/// mila_wade / nina_rhodes (live 2026-11-19): woodcutter -> fisher at
/// 18:24 (coverage repair), the goal « woodcutting 95 » drawn by the old
/// ambition ran on unlinked, yews cut 2.5 h later. It is replaced: its
/// nodes abandoned (the parked market buy kept), the next goal drawn from
/// the fisher's milestones. Deferred while a node runs; a bid goal runs
/// on.
#[test]
fn a_changed_ambition_replaces_the_goal_an_earlier_one_drew() {
    let now = now_secs();
    let mila = || {
        let mut fisher = ambition("fisher", "coverage:fishing", now - 400_000);
        fisher.revisions.push(Revision { tick: 0, at_secs: now - 9_000, from: "woodcutter".to_string(), to: "fisher".to_string(), why: "repair:coverage:fishing".to_string() });
        let mut mila = life("b0", fisher);
        mila.goal = GoalKind::MaxSkills { skills: vec!["woodcutting".to_string()], level: 95 };
        mila.history = vec![goal_entry("done:woodcutting", now - 70_000), goal_entry("next:woodcutting 95", now - 70_000)];
        mila.memory.floor = Some(697);
        let mut climb = Node::new("n697", NodeKind::ReachLevel { skill: "woodcutting".to_string(), level: 95 }, Vec::new(), 95);
        climb.status = NodeStatus::Ready;
        climb.method = Some("gather:woodcut/yew_logs".to_string());
        let mut parked = Node::new("n698", NodeKind::ObtainItems { item: "rune_axe".to_string(), count: 1 }, Vec::new(), 1);
        parked.status = NodeStatus::Ready;
        parked.method = Some("market:buy".to_string());
        mila.nodes = vec![climb, parked];
        mila.history.push(HistoryEntry { node: "n698".to_string(), started: now - 60, ended: now - 60, outcome: "awaiting_fill".to_string() });
        mila
    };
    let mut running = mila();
    running.nodes[0].status = NodeStatus::Active;
    running.active = Some("n697".to_string());
    assert!(!super::replace_stale_goal(&mut running, &live()), "deferred while a node runs");
    let mut bid = mila();
    bid.goal_origin = Some("bid:yew_logs".to_string());
    assert!(!super::replace_stale_goal(&mut bid, &live()), "a bid goal runs on");
    let mut replaced = mila();
    assert!(super::replace_stale_goal(&mut replaced, &live()), "the woodcutting goal is replaced");
    assert_ne!(replaced.goal, GoalKind::MaxSkills { skills: vec!["woodcutting".to_string()], level: 95 });
    let status = |id: &str| replaced.nodes.iter().find(|node| node.id == id).map(|node| node.status.clone());
    assert_eq!(status("n697"), Some(NodeStatus::Abandoned), "the yew cutting stops");
    assert_ne!(status("n698"), Some(NodeStatus::Abandoned), "the parked market buy is kept");
    let origin = replaced.goal_origin.clone().expect("linked to the fisher");
    assert!(replaced.ambition.as_ref().is_some_and(|held| held.milestones.iter().any(|stone| stone.id == origin)), "{origin}");
    assert!(!lifescape_planner::goal_from_earlier_ambition(&replaced), "nothing left to replace");
    assert!(!super::replace_stale_goal(&mut replaced, &live()), "once");
}

/// Offline check of the one-time review on a READ-ONLY COPY of the live
/// data (never `run/` itself): the need each kind carries now, and the
/// fleet's kinds before / after the review (same draw as the brain:
/// slots in order, saturation by the kinds the others hold, gaps first).
/// Ignored (builds a guide set on the copied store, tens of seconds):
/// `LIFESCAPE_REVIEW_LIVES=<copy>/lives LIFESCAPE_REVIEW_DB=<copy>/lifescape.db
/// LIFESCAPE_RATES_PATH=<copy>/rates-snapshot.json NODE_TICK_RATE=60
/// cargo test --release -p lifescape-orchestrator --lib live_fleet_review -- --ignored --nocapture`
#[test]
#[ignore]
fn live_fleet_review_distribution() {
    let (Ok(lives_dir), Ok(db)) = (std::env::var("LIFESCAPE_REVIEW_LIVES"), std::env::var("LIFESCAPE_REVIEW_DB")) else {
        println!("set LIFESCAPE_REVIEW_LIVES and LIFESCAPE_REVIEW_DB (copies, never run/)");
        return;
    };
    assert!(!lives_dir.contains("/LifeScape/run") && !db.contains("/LifeScape/run"), "copies only, never the live run/");
    let store = lifescape_store::Store::open(std::path::Path::new(&db)).expect("store copy");
    let orders = store.resting_orders().expect("resting orders");
    let now = crate::market_service::MarketService::now_clock();
    let name_of = |item_id: i64| lifescape_gamedata::obj_by_id(item_id.max(0) as usize).and_then(|obj| obj.name).map(str::to_string);
    let display_of = |item: &str| lifescape_gamedata::objs_by_name(item).into_iter().next().and_then(|obj| obj.display.or(obj.name)).unwrap_or(item).to_string();
    let reference = |item: &str| crate::rates_service::price_estimate(&store, &display_of(item)).0;
    let set = crate::guides_store::build_set(&store, &crate::rates_service::live_snapshot(), &crate::web_guides::StoreWaits::default());
    let all_profits = crate::brain_d17::kind_profits(&set);
    let supply = |item: &str| lifescape_planner::guide_engine::supply_of(set.quotes.get(item), &crate::web_guides::shop_window(item));
    for (kind, profit) in &all_profits {
        let best = set
            .money_catalog
            .rows
            .iter()
            .filter(|row| row.priced && row.profit_6000.is_finite())
            .filter(|row| (row.profit_6000 * lifescape_planner::guide_money::supplied_share(row, &supply) - profit).abs() < 1e-6)
            .find(|row| lifescape_planner::activity::band_skill(&row.method, &row.style).and_then(|skill| lifescape_planner::ambition_catalog::kind_for_skill(&skill)) == Some(kind.as_str()))
            .map(|row| format!("{} ({:?}, window {:.0})", row.method, row.family, row.profit_6000))
            .unwrap_or_default();
        println!("guide profit {kind:<12} {profit:>12.0} /6000 ticks  {best}");
    }
    if std::env::var("LIFESCAPE_REVIEW_ROWS").is_ok() {
        let mut top: Vec<&lifescape_planner::guide_model::MoneyRow> = set.money_catalog.rows.iter().filter(|row| row.priced && row.profit_6000.is_finite()).collect();
        top.sort_by(|left, right| right.profit_6000.total_cmp(&left.profit_6000));
        for row in top.iter().take(6) {
            println!("row {:?} {} profit {:.0} revenue {:.0} cost {:.0}", row.family, row.method, row.profit_6000, row.revenue_6000, row.cost_6000);
            for line in &row.flows {
                println!("    {} {} {:.1}/6000 @ {:.1} = {:.0} ({:?}, {:?})", line.item, line.cause, line.per_6000, line.unit_price, line.value_6000, line.outlet, line.basis);
            }
        }
    }
    let profits = all_profits.clone();
    let surplus = crate::market_demand::surplus_from(&orders, &name_of);
    let weigh = |items: &[lifescape_planner::DemandItem], profits: &[(String, f64)], surplus: &[(String, f64)]| lifescape_planner::ambition_catalog::kind_weights_with(items, profits, surplus);
    let demand_for = |account: Option<&str>| {
        let signals = crate::market_demand::signals_seen_from(orders.clone(), account, now, &name_of, &crate::brain_stuck::seconds_to_ticks);
        let items = crate::market_demand::demand_items_with(&signals, &reference);
        let weights = weigh(&items, &profits, &surplus);
        (items, weights)
    };
    let (fleet_items, fleet_weights) = demand_for(None);
    println!("open bids (non-maker) {} items, asks (non-maker) {} items, guide profits {} kinds", fleet_items.len(), surplus.len(), profits.len());
    let part = |rows: &[lifescape_planner::KindDemand], kind: &str| rows.iter().find(|row| row.kind == kind).map(|row| row.need).unwrap_or(0.0);
    let (offers, outlets) = (weigh(&fleet_items, &[], &[]), weigh(&[], &[], &surplus));
    let total_gain: f64 = profits.iter().map(|(_, profit)| profit.max(0.0)).sum();
    let gain_share = |kind: &str| if total_gain > 0.0 { profits.iter().filter(|(known, _)| known == kind).map(|(_, profit)| profit.max(0.0)).fold(0.0, f64::max) / total_gain } else { 0.0 };
    println!("need = (bids + outlet) x (1 + gain share)");
    println!("{:<12} {:>7} {:>7} {:>7} {:>7} {:>8}  driver / top surplus consumed (units, windows)", "kind", "need", "bids", "outlet", "gain", "windows");
    let mut rows = fleet_weights.clone();
    rows.sort_by(|left, right| right.need.total_cmp(&left.need));
    for row in &rows {
        let rates = lifescape_planner::ambition_catalog::consumption_of(&row.kind);
        let mut piles: Vec<(String, f64, f64)> = rates
            .iter()
            .filter_map(|(item, per_6000)| surplus.iter().find(|(known, _)| known == item).map(|(_, units)| (item.clone(), *units, units / per_6000)))
            .collect();
        piles.sort_by(|left, right| right.2.total_cmp(&left.2));
        let top: Vec<String> = piles.iter().take(3).map(|(item, units, windows)| format!("{item} {units:.0} u = {windows:.1} w")).collect();
        let windows = lifescape_planner::ambition_catalog::outlet_windows(&row.kind, &surplus);
        println!("{:<12} {:>7.3} {:>7.3} {:>7.3} {:>7.3} {:>8.1}  {} / {}", row.kind, row.need, part(&offers, &row.kind), part(&outlets, &row.kind), gain_share(&row.kind), windows, row.need_item.clone().unwrap_or_default(), top.join(", "));
    }
    let mut lives: Vec<Life> = std::fs::read_dir(&lives_dir)
        .expect("lives copy")
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        .filter_map(|entry| lifescape_planner::life_from_value(&lifescape_json::parse(&std::fs::read_to_string(entry.path()).ok()?).ok()?))
        .filter(|life| !crate::economy_guard::is_staff(&life.account) && !life.account.eq_ignore_ascii_case("bite"))
        .collect();
    lives.sort_by_key(|life| (lifescape_planner::review_slot_ticks(&life.account), life.account.clone()));
    let count = |lives: &[Life]| {
        let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for life in lives {
            *counts.entry(life.ambition.as_ref().map(|held| held.kind.clone()).unwrap_or_default()).or_default() += 1;
        }
        counts
    };
    let before = count(&lives);
    let mut moves = Vec::new();
    for index in 0..lives.len() {
        let levels = super::account_levels(&lives[index].account, None);
        let bot = super::fleet_bot(&lives[index], levels);
        let Some(held) = lives[index].ambition.clone() else { continue };
        let mut marked = held.clone();
        if lifescape_planner::adopt_need_mark(&mut marked) {
            moves.push(format!("{:<14} {:<12} marked need ({})", lives[index].account, held.kind, held.why));
            lives[index].ambition = Some(marked);
            continue;
        }
        if !needs_review(&held, &bot) {
            continue;
        }
        let others: Vec<String> = lives.iter().enumerate().filter(|(other, _)| *other != index).filter_map(|(_, life)| life.ambition.as_ref().map(|held| held.kind.clone())).collect();
        let refs_others: Vec<&str> = others.iter().map(String::as_str).collect();
        let gaps = lifescape_planner::redraw_gaps(&held.kind, &refs_others);
        let (items, weights) = demand_for(Some(&lives[index].account));
        let refs: Vec<&str> = others.iter().map(String::as_str).collect();
        let weights = lifescape_planner::saturate(&weights, &refs);
        let first = lifescape_planner::draw_kind(&bot, &gaps, &weights, &[]);
        let inputs = super::DrawInputs { items, weights, gaps };
        let level_of = |skill: &str| bot.level(skill);
        let now_bot = BotNow { level_of: &level_of, coins: 0, held: &|_| 0 };
        let Some((next, why)) = super::build(&lives[index].account, &bot, &now_bot, first, &inputs, &[], Stamp { tick: 0, secs: now }) else { continue };
        moves.push(format!("{:<14} {:<12} -> {:<12} ({why})", lives[index].account, held.kind, next.kind));
        lives[index].ambition = Some(next);
    }
    let after = count(&lives);
    println!("--- review of {} old-rule ambitions (slot order)", moves.len());
    for line in &moves {
        println!("{line}");
    }
    println!("--- kinds before -> after");
    let every: Vec<String> = lifescape_planner::ambition_catalog::specs().iter().map(|entry| entry.kind.to_string()).collect();
    let kinds: std::collections::BTreeSet<&String> = before.keys().chain(after.keys()).chain(every.iter()).collect();
    for kind in kinds {
        println!("{:<14} {:>3} -> {:>3}", kind, before.get(kind).copied().unwrap_or(0), after.get(kind).copied().unwrap_or(0));
    }
}
