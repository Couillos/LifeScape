//! The Lives page view of the progression (A1 to A4): how the ambition was
//! given, the revisions, the real milestone states, the goal origin, the
//! value of time. Fixtures from the live files of 2026-11-21.

use super::*;
use lifescape_planner::{new_ambition, pinned_ambition, Archetype, BotNow, GoalKind, Profile, Revision, Stamp};

const ZERO: Stamp = Stamp { tick: 0, secs: 0 };

fn held(kind: &str, why: &str) -> Ambition {
    new_ambition(kind, why, &BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 }, 7, &[], ZERO).expect("ambition")
}

fn life_of(account: &str, ambition: Ambition) -> Life {
    Life {
        schema: 1,
        account: account.to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer),
        goal: GoalKind::MakeMoney,
        nodes: Vec::new(),
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: Some(ambition),
        goal_origin: None,
        memory: Default::default(),
    }
}

fn revision(why: &str) -> Revision {
    Revision { tick: 1, at_secs: 5, from: "a".to_string(), to: "b".to_string(), why: why.to_string() }
}

fn cool_entry(key: &str, ticks: u64, cause: &str, ended: i64) -> HistoryEntry {
    HistoryEntry { node: "cool".to_string(), started: ended, ended, outcome: format!("cool:{key}|{ticks}|{cause}") }
}

fn source_of(view: &Value) -> String {
    view.get("source").as_str().unwrap_or("").to_string()
}

#[test]
fn the_figures_of_a_value_pick_are_read_back() {
    let rebalance = parse_numbers("value 9136 per 6000 ticks (kept: no change beats it by 25 %), 7 bots already on miner, driven by amulet_of_power (climb 0 ticks)");
    assert_eq!(rebalance.value_6000, Some(9136));
    assert!(rebalance.kept);
    assert_eq!(rebalance.holders, Some(7));
    assert_eq!(rebalance.driver.as_deref(), Some("amulet_of_power"));
    assert_eq!(rebalance.climb_ticks, Some(0));
    let plain = parse_numbers("value 224 per 6000 ticks, 7 bots already on ranger");
    assert_eq!((plain.value_6000, plain.holders, plain.driver.clone(), plain.kept), (Some(224), Some(7), None, false));
    let alone = parse_numbers("value 36118 per 6000 ticks, 1 bot already on mage, driven by amulet_of_power (climb 311431 ticks)");
    assert_eq!((alone.holders, alone.climb_ticks), (Some(1), Some(311_431)));
    assert_eq!(parse_numbers("nothing"), ValueNumbers::default());
}

#[test]
fn the_last_logged_value_line_of_the_account_and_kind_is_found() {
    let log = "[15:11:24] brain tim_wolfe: ambition firemaker given by value (value 1495 per 6000 ticks, 5 bots already on firemaker)\n\
               [15:21:15] brain mila_wade: ambition mage given by value (value 36118 per 6000 ticks, 9 bots already on mage, driven by amulet_of_power (climb 311431 ticks))\n\
               [15:30:00] brain mila_wade: ambition cook given by value (value 10 per 6000 ticks, 2 bots already on cook)\n";
    let found = logged_value_in(log, "mila_wade", "mage").expect("mage line");
    assert_eq!(found.at, "15:21:15");
    assert!(found.numbers.starts_with("value 36118 per 6000 ticks") && found.numbers.ends_with("(climb 311431 ticks)"), "{}", found.numbers);
    assert!(logged_value_in(log, "mila_wade", "smith").is_none());
    assert!(logged_value_in(log, "other", "mage").is_none());
}

/// The page never rereads the whole log: a backward block scan stops at
/// the first block holding the line (found across a block edge too), and
/// the lookup is reused.
#[test]
fn the_log_is_scanned_backwards_by_blocks_and_reused() {
    let dir = std::env::temp_dir().join(format!("lifescape-brain-log-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let mut text = String::new();
    text.push_str("[10:00:00] brain old_one: ambition mage given by value (value 5 per 6000 ticks)\n");
    for index in 0..400 {
        text.push_str(&format!("[10:01:{:02}] brain filler{index}: something happened here and there on the world\n", index % 60));
    }
    text.push_str("[11:00:00] brain amber_moore: ambition smith given by value (value 9576 per 6000 ticks, 11 bots already on smith)\n");
    for index in 0..40 {
        text.push_str(&format!("[11:01:{:02}] brain tail{index}: more lines after the one we look for, to cross blocks\n", index % 60));
    }
    std::fs::write(dir.join("orchestrator.log"), &text).expect("log");
    let path = dir.join("orchestrator.log");
    let found = scan_log_backwards(&path, "amber_moore", "smith", 512).expect("found across small blocks");
    assert_eq!((found.at.as_str(), found.numbers.starts_with("value 9576")), ("11:00:00", true));
    let old = scan_log_backwards(&path, "old_one", "mage", 512).expect("an older line is still within the tail");
    assert_eq!(old.at, "10:00:00");
    assert!(scan_log_backwards(&path, "nobody", "mage", 512).is_none());
    // Reused: the file may vanish, the answer stays for the next polls.
    assert!(logged_value(&dir, "amber_moore", "smith").is_some());
    std::fs::remove_file(&path).expect("remove");
    assert!(logged_value(&dir, "amber_moore", "smith").is_some(), "served from the reuse");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Only a value draw looks in the log.
#[test]
fn only_a_value_draw_reads_the_log() {
    let mut drawn = held("mage", "value:amulet_of_power");
    drawn.revisions.push(revision("achieved"));
    assert!(is_value_draw("mila_wade", &drawn, None));
    let mut rebalanced = held("miner", "value:iron_ore");
    rebalanced.revisions.push(revision("rebalance"));
    let plan = PlanInfo { from: "priest".to_string(), kind: "miner".to_string(), numbers: "value 1 per 6000 ticks".to_string(), applied: true, computed_secs: 100 };
    assert!(!is_value_draw("felix_rook", &rebalanced, Some(&plan)), "the marker has the figures");
    assert!(!is_value_draw("amber_moore", &held("smith", "need:rune_axe"), None), "old draw");
    assert!(!is_value_draw("Zezima", &held(lifescape_planner::OVERALL, "protected:overall"), None));
}

#[test]
fn revisions_name_the_retired_rules() {
    for why in ["blocked:m15x3", "change:need:rune_axe", "repair:coverage:prayer", "review:need"] {
        let (text, retired) = revision_text(why);
        assert!(retired && !text.is_empty(), "{why}: {text}");
    }
    for why in ["rebalance", "achieved", "operator:natural", "protected:overall", "operator:alcher", "market maker retired"] {
        assert!(!revision_text(why).1, "{why}");
    }
    assert!(revision_text("rebalance").0.contains("21/11"));
}

#[test]
fn milestone_states_are_the_real_ones() {
    let mut ambition = held("woodcutter", "random");
    let kind = ambition.kind.clone();
    let ids: Vec<String> = ambition.milestones.iter().map(|stone| stone.id.clone()).take(4).collect();
    assert!(ids.len() >= 4, "{ids:?}");
    ambition.milestones[0].status = MilestoneStatus::Done;
    ambition.milestones[1].status = MilestoneStatus::Blocked;
    ambition.milestones[1].blocked_count = 3;
    ambition.milestones[2].status = MilestoneStatus::Blocked;
    ambition.milestones[2].blocked_count = 1;
    let cools = vec![
        Cool { key: milestone_cool_key(&kind, &ids[1]), ticks_left: 4_000, cause: "set aside".to_string() },
        Cool { key: milestone_cool_key(&kind, &ids[3]), ticks_left: 900, cause: "gear nobody makes".to_string() },
    ];
    let state = |index: usize| milestone_state(&kind, &ambition.milestones[index], &cools);
    assert_eq!(state(0).0, "done");
    let aside = state(1);
    assert_eq!((aside.0, aside.2), ("set_aside", Some(4_000)));
    assert!(aside.1.contains("4000 ticks"), "{}", aside.1);
    assert_eq!(state(2).0, "open");
    assert!(state(2).1.contains("1/3"), "{}", state(2).1);
    let waiting = state(3);
    assert_eq!((waiting.0, waiting.2), ("waiting", Some(900)));
    assert!(waiting.1.contains("personne ne fabrique") && waiting.1.contains("900"), "{}", waiting.1);
    // A set-aside milestone whose cooldown is over: resumable, no countdown.
    let lapsed = milestone_state(&kind, &ambition.milestones[1], &[]);
    assert_eq!((lapsed.0, lapsed.2), ("set_aside", None));
    assert!(lapsed.1.contains("aucun autre jalon"), "{}", lapsed.1);
}

#[test]
fn cooldowns_are_the_active_ones_with_the_ticks_left() {
    let history = vec![
        cool_entry("gather:mining/iron_ore@3295,3310,0", 6_000, "contended", 100),
        cool_entry("gather:mining/iron_ore@3295,3310,0", 6_000, "contended", 300),
        cool_entry("gather:woodcut/oak", 50, "failed", 100),
        cool_entry("milestone:miner/m3", 10_000, "set aside", 100),
    ];
    let since = |stamp: i64| (1_000 - stamp) as u64 / 2;
    let cools = cooling(&history, &since);
    assert_eq!(cools.len(), 2, "the expired one is gone: {cools:?}");
    let site = cools.iter().find(|cool| cool.key.starts_with("gather:mining")).expect("site");
    assert_eq!(site.ticks_left, 6_000 - 350, "the most recent entry wins (longest left)");
    let view = cooling_view(&cools);
    let rows = view.as_arr().cloned().unwrap_or_default();
    assert_eq!(rows.len(), 1, "a milestone's cooldown is not listed with the sites");
    assert_eq!(rows[0].get("site").as_str(), Some("3295,3310,0"));
    assert_eq!(rows[0].get("method").as_str(), Some("gather:mining/iron_ore"));
}

#[test]
fn the_ambition_is_shown_with_the_way_it_was_given() {
    // Protected: overall for good.
    let overall = life_of("Zezima", held(lifescape_planner::OVERALL, "protected:overall"));
    let view = ambition_view("Zezima", &overall, None, None, &[]);
    assert_eq!(source_of(view.get("given")), "protected");
    assert_eq!(view.get("protected").as_bool(), Some(true));
    // Operator pin.
    let pin = pinned_ambition("smithing", 99, &BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 }, 7, &[], ZERO).expect("pin");
    let pinned = life_of("owen_wood", pin);
    let view = ambition_view("owen_wood", &pinned, None, None, &[]);
    assert_eq!(source_of(view.get("given")), "pin");
    assert!(view.get("given").get("text").as_str().unwrap_or("").contains("smithing niveau 99"));
    // A4 rebalance: the row of the marker gives the figures.
    let mut miner = held("miner", "value:iron_ore");
    miner.revisions.push(revision("rebalance"));
    let plan = PlanInfo { from: "priest".to_string(), kind: "miner".to_string(), numbers: "value 909 per 6000 ticks, 14 bots already on miner, driven by iron_ore (climb 0 ticks)".to_string(), applied: true, computed_secs: 100 };
    let view = ambition_view("felix_rook", &life_of("felix_rook", miner), Some(&plan), None, &[]);
    let given = view.get("given");
    assert_eq!(source_of(given), "a4_rebalance");
    assert_eq!(given.get("numbers").get("value_6000").as_i64(), Some(909));
    assert_eq!(given.get("numbers").get("holders").as_i64(), Some(14));
    assert_eq!(given.get("numbers").get("driver").as_str(), Some("iron_ore"));
    // Kept by the rebalance (thorn_vera: an old need draw no row beat).
    let mage = held("mage", "need:amulet_of_power");
    let kept = PlanInfo { from: "mage".to_string(), kind: "mage".to_string(), numbers: "value 53598 per 6000 ticks (kept: no change beats it by 25 %), 9 bots already on mage".to_string(), applied: true, computed_secs: 100 };
    // A revision made after the plan is not the plan's: not « kept ».
    let mut later = held("mage", "need:amulet_of_power");
    later.revisions.push(Revision { at_secs: 500, ..revision("blocked:m1x3") });
    assert_eq!(source_of(ambition_view("thorn_vera", &life_of("thorn_vera", later), Some(&kept), None, &[]).get("given")), "legacy");
    let view = ambition_view("thorn_vera", &life_of("thorn_vera", mage), Some(&kept), None, &[]);
    assert_eq!(source_of(view.get("given")), "a4_kept");
    // Value draw at achievement: the figures come from the daemon log.
    let mut next = held("mage", "value:amulet_of_power");
    next.revisions.push(revision("achieved"));
    let line = LoggedValue { kind: "mage".to_string(), numbers: "value 36118 per 6000 ticks, 9 bots already on mage".to_string(), at: "15:21:15".to_string() };
    let view = ambition_view("mila_wade", &life_of("mila_wade", next.clone()), None, Some(&line), &[]);
    assert_eq!(source_of(view.get("given")), "a4_draw");
    assert_eq!(view.get("given").get("numbers").get("value_6000").as_i64(), Some(36_118));
    let view = ambition_view("mila_wade", &life_of("mila_wade", next), None, None, &[]);
    assert!(view.get("given").get("numbers").is_null(), "no figure invented when the log has none");
    // Before A4: a need draw nothing replaced.
    let old = held("smith", "need:rune_axe");
    assert_eq!(source_of(ambition_view("amber_moore", &life_of("amber_moore", old), None, None, &[]).get("given")), "legacy");
    // Revisions explained, retired rules flagged, newest first.
    let mut history = held("mage", "need:amulet_of_power");
    history.revisions = vec![revision("blocked:m15x3"), revision("rebalance")];
    let view = ambition_view("x", &life_of("x", history), None, None, &[]);
    let rows = view.get("revision_rows").as_arr().cloned().unwrap_or_default();
    assert_eq!((rows[0].get("why").as_str(), rows[0].get("retired_rule").as_bool()), (Some("rebalance"), Some(false)));
    assert_eq!((rows[1].get("why").as_str(), rows[1].get("retired_rule").as_bool()), (Some("blocked:m15x3"), Some(true)));
    assert_eq!(view.get("revision_total").as_i64(), Some(2));
}

#[test]
fn the_goal_origin_is_named() {
    let mut ambition = held("miner", "random");
    let first = ambition.milestones[0].id.clone();
    ambition.milestones[0].origin = "demand:iron_bar".to_string();
    let mut life = life_of("t", ambition);
    let origin = |life: &Life, filler: Option<&(String, String)>| goal_origin_view(life, filler).get("kind").as_str().unwrap_or("").to_string();
    life.goal_origin = Some(first);
    assert_eq!(origin(&life, None), "demand");
    life.goal_origin = Some(life.ambition.as_ref().map(|held| held.milestones[1].id.clone()).unwrap_or_default());
    assert_eq!(origin(&life, None), "milestone");
    life.goal_origin = Some("bid:gold_ore".to_string());
    assert_eq!(origin(&life, None), "bid");
    assert!(goal_origin_view(&life, None).get("text").as_str().unwrap_or("").contains("offre d'achat"));
    life.goal_origin = Some("side:money".to_string());
    assert_eq!(origin(&life, None), "side");
    life.goal_origin = None;
    assert_eq!(origin(&life, None), "none");
    let filler = ("n1315".to_string(), "gather:woodcut/yew_logs".to_string());
    assert_eq!(origin(&life, Some(&filler)), "filler");
    assert_eq!(filler_view(Some(&filler)).get("method").as_str(), Some("gather:woodcut/yew_logs"));
    assert!(filler_view(None).is_null());
}

#[test]
fn the_value_of_time_lifts_over_the_measured_rate_when_a_climb_pays() {
    assert!(training_view(None).is_null());
    let lucrative = Investment {
        item: "amulet_of_power".to_string(),
        method: "craft".to_string(),
        gates: vec![lifescape_planner::investment::Gate { skill: "crafting".to_string(), level: 70, climb_ticks: 120_000.0 }],
        climb_ticks: 120_000.0,
        gain_per_tick: 3.0,
        margin: 100.0,
        capacity: 50.0,
        units_per_tick: 0.01,
        ladder: vec![(2600.0, 10.0)],
        cost_per_unit: 500.0,
        value_6000: 60_000.0,
    };
    let view = training_view(Some(&TrainingInput { skill: "crafting".to_string(), own_rate: 1.5, pending: vec![lucrative.clone()] }));
    assert_eq!(view.get("skill").as_str(), Some("crafting"));
    assert_eq!(view.get("lifted").as_bool(), Some(true));
    assert!((view.get("time_value").as_f64().unwrap_or(0.0) - 10.0).abs() < 1e-9, "{view:?}");
    assert_eq!(view.get("pending").as_arr().map(Vec::len), Some(1));
    assert_eq!(view.get("pending").as_arr().and_then(|rows| rows[0].get("gates").as_arr().map(Vec::len)), Some(1));
    let flat = training_view(Some(&TrainingInput { skill: "crafting".to_string(), own_rate: 30.0, pending: vec![lucrative] }));
    assert_eq!(flat.get("lifted").as_bool(), Some(false), "the measured rate is larger");
    let empty = training_view(Some(&TrainingInput { skill: "mining".to_string(), own_rate: 2.0, pending: Vec::new() }));
    assert_eq!((empty.get("lifted").as_bool(), empty.get("time_value").as_f64()), (Some(false), Some(2.0)));
}
