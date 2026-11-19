//! Decision D3 in the life files (user 2026-11-19, « pourquoi on a
//! toujours autant de cons qui coupent du yew ») : the old-rule ambitions
//! are reviewed by need, a need-led ladder end redraws, a changed
//! ambition replaces the goal an earlier one drew (mila_wade, nina_rhodes:
//! woodcutter -> fisher at 18:24, still cutting yews 2.5 h later).

use super::{
    adopt_need_mark, redraw_gaps, OPERATOR_RULE, draw_rule_for, goal_from_earlier_ambition, is_milestone_origin, keeps_ladder_end, mark_rule, needs_review, review_slot_ticks, LEVEL_RULE, NEED_RULE,
    REVIEW_SPREAD_TICKS, STALE_GOAL_CAUSE,
};
use crate::ambition::{ambition_from_value, ambition_to_value, new_ambition, pinned_ambition, revise, Ambition, BotNow, Revision, Stamp};
use crate::ambition_fleet::FleetBot;
use crate::market_maker::MARKET_MAKER_KIND;
use crate::model::{Archetype, GoalKind, HistoryEntry, Life, Node, NodeKind, NodeStatus, Profile};

const ZERO: Stamp = Stamp { tick: 0, secs: 0 };

fn bot(account: &str, kind: &str) -> FleetBot {
    FleetBot {
        account: account.to_string(),
        levels: vec![("woodcutting".to_string(), 95)],
        ironman: false,
        market_affinity: 0.5,
        seed: 7,
        kind: Some(kind.to_string()),
        market_maker: false,
    }
}

fn ambition(kind: &str, why: &str) -> Ambition {
    new_ambition(kind, why, &BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 }, 7, &[], ZERO).expect("an ambition")
}

fn life(account: &str, held: Ambition) -> Life {
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
        ambition: Some(held),
        goal_origin: None,
        memory: Default::default(),
    }
}

fn goal_entry(outcome: &str, at: i64) -> HistoryEntry {
    HistoryEntry { node: "goal".to_string(), started: at, ended: at, outcome: outcome.to_string() }
}

/// An ambition drawn under the old rule (no mark) of a need-led bot is
/// reviewed; once marked by need, never again.
#[test]
fn an_unmarked_ambition_of_a_need_led_bot_is_reviewed() {
    let woodcutter = bot("arthur_75", "woodcutter");
    let mut held = ambition("woodcutter", "random");
    assert_eq!(held.draw_rule, "", "a fresh build carries no mark until the draw stamps it");
    assert!(needs_review(&held, &woodcutter), "drawn by the old rule: reviewed");
    assert_eq!(draw_rule_for(&woodcutter), NEED_RULE);
    mark_rule(&mut held, &woodcutter);
    assert_eq!(held.draw_rule, NEED_RULE);
    assert!(!needs_review(&held, &woodcutter), "drawn by need: kept");
    // A level-marked ambition on a need-led bot is no need draw either.
    held.draw_rule = LEVEL_RULE.to_string();
    assert!(needs_review(&held, &woodcutter));
}

/// Zezima, N0valyfe, Yogosun (any case) and the market makers keep the
/// level-led draw: never reviewed. An operator ambition is never undone:
/// `pinned()` (skill pin, overall, alchemist, market maker) and every
/// ambition an operator constructor marked `operator` (ryan_mason, alcher
/// by the user 2026-11-18). An unmarked generalist is reviewed.
#[test]
fn exceptions_and_operator_ambitions_are_never_reviewed() {
    for account in ["Zezima", "n0valyfe", "YOGOSUN"] {
        let exception = bot(account, "woodcutter");
        assert!(!needs_review(&ambition("woodcutter", "random"), &exception), "{account}");
        assert_eq!(draw_rule_for(&exception), LEVEL_RULE, "{account}");
    }
    let mut maker = bot("arthur_hart", "woodcutter");
    maker.market_maker = true;
    assert!(!needs_review(&ambition("woodcutter", "random"), &maker), "a market maker bot");
    assert!(!needs_review(&ambition(MARKET_MAKER_KIND, "operator:market_maker"), &bot("arthur_hart", MARKET_MAKER_KIND)), "a market_maker ambition");
    let now = BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 };
    let pin = pinned_ambition("ranged", 99, &now, 7, &[], ZERO).expect("a pin");
    assert!(!needs_review(&pin, &bot("frank_shaw", "ranger")), "operator pin");
    assert_eq!(pin.draw_rule, OPERATOR_RULE, "the pin constructor marks it");
    for kind in [crate::ambition::OVERALL, crate::ambition::ALCHER] {
        assert!(!needs_review(&ambition(kind, "operator"), &bot("ryan_mason", kind)), "{kind}: operator-only, never undone");
    }
    let mut ryan = life("ryan_mason", ambition("woodcutter", "random"));
    crate::alcher::become_alcher(&mut ryan, &BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 }, ZERO).expect("alcher");
    let alcher = ryan.ambition.clone().expect("alcher");
    assert_eq!(alcher.draw_rule, OPERATOR_RULE, "become_alcher marks it");
    assert!(!needs_review(&alcher, &bot("ryan_mason", crate::ambition::ALCHER)));
    let mut generalist = ambition(crate::ambition::GENERALIST, "random");
    assert!(needs_review(&generalist, &bot("b0", crate::ambition::GENERALIST)), "an unmarked generalist is reviewed");
    generalist.draw_rule = OPERATOR_RULE.to_string();
    assert!(!needs_review(&generalist, &bot("b0", crate::ambition::GENERALIST)), "an operator generalist is kept");
}

/// The ~5 ambitions drawn by need since 03a27a2 (`why` `need` or
/// `need:<item>`, before the mark existed) are marked, never redrawn.
#[test]
fn need_draws_before_the_mark_are_marked_not_redrawn() {
    let woodcutter = bot("tom_king", "crafter");
    for why in ["need", "need:diamond"] {
        let mut held = ambition("crafter", why);
        assert!(!needs_review(&held, &woodcutter), "{why}: already a need draw");
        assert!(adopt_need_mark(&mut held), "{why}: marked");
        assert_eq!(held.draw_rule, NEED_RULE);
        assert!(!adopt_need_mark(&mut held), "once");
    }
    let mut old = ambition("crafter", "demand:amulet_of_power");
    assert!(!adopt_need_mark(&mut old) && old.draw_rule.is_empty(), "the old rule's `demand:` is no need draw");
    assert!(needs_review(&old, &woodcutter));
}

/// A redraw never takes a bot off a group it holds alone (sole holder,
/// `ambition_fleet` rule): its gaps are its own group only; a redundant
/// bot sees the fleet's open gaps (computed with itself).
#[test]
fn a_sole_holder_keeps_its_group_through_a_redraw() {
    assert_eq!(redraw_gaps("fisher", &["woodcutter", "smith"]), vec!["fishing".to_string()], "sole fisher keeps fishing");
    let redundant = redraw_gaps("fisher", &["fisher", "smith"]);
    assert!(!redundant.contains(&"fishing".to_string()) && redundant.contains(&"firemaking".to_string()), "{redundant:?}");
    assert!(redraw_gaps(crate::ambition::GENERALIST, &["fisher"]).contains(&"smithing".to_string()), "no group of its own: the fleet's gaps");
}

/// The mark is a persistent field of the life; files written before it
/// read unmarked (reviewed once).
#[test]
fn the_draw_rule_round_trips_and_old_lives_read_unmarked() {
    let mut held = ambition("fisher", "need:raw_lobster");
    held.draw_rule = NEED_RULE.to_string();
    let value = ambition_to_value(&held);
    assert_eq!(value.get("draw_rule").as_str(), Some(NEED_RULE));
    assert_eq!(ambition_from_value(&value).expect("read back").draw_rule, NEED_RULE);
    let old = lifescape_json::parse(r#"{"kind":"woodcutter","why":"random","milestones":[],"rung":3}"#).expect("json");
    assert_eq!(ambition_from_value(&old).expect("old life").draw_rule, "", "no key: drawn before the mark");
}

/// The review spreads over `REVIEW_SPREAD_TICKS` (never every bot in the
/// same tick), every slot inside the window.
#[test]
fn review_slots_spread_inside_the_window() {
    let slots: Vec<u64> = (0..64).map(|index| review_slot_ticks(&format!("bot_{index}"))).collect();
    assert!(slots.iter().all(|slot| *slot < REVIEW_SPREAD_TICKS), "{slots:?}");
    let mut distinct = slots.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert!(distinct.len() >= 48, "spread, not one burst: {distinct:?}");
    assert_eq!(review_slot_ticks("arthur_75"), review_slot_ticks("arthur_75"), "stable per account");
}

/// Ladder end: a need-led bot redraws by need (no automatic extension of
/// the same ambition); the exceptions and the operator pins extend.
#[test]
fn a_ladder_end_redraws_unless_an_exception() {
    let woodcutter = ambition("woodcutter", "random");
    assert!(!keeps_ladder_end(&woodcutter, &bot("jane_95", "woodcutter")), "need-led: redraw");
    assert!(keeps_ladder_end(&woodcutter, &bot("Zezima", "woodcutter")), "exception: extension");
    let mut maker = bot("hana_91", "woodcutter");
    maker.market_maker = true;
    assert!(keeps_ladder_end(&woodcutter, &maker), "market maker: extension");
    let now = BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 };
    let pin = pinned_ambition("smithing", 99, &now, 7, &[], ZERO).expect("a pin");
    assert!(keeps_ladder_end(&pin, &bot("owen_wood", "smith")), "operator pin: extension toward its level");
}

/// `revise` drops a milestone link (the old ambition's), never a side,
/// bid or alchemy link (those goals run on through the change).
#[test]
fn revise_keeps_side_links_and_drops_milestone_links() {
    assert!(is_milestone_origin("m2") && is_milestone_origin("m41"));
    assert!(!is_milestone_origin("bid:gold_ore") && !is_milestone_origin("side:money") && !is_milestone_origin("alch:x") && !is_milestone_origin("m"));
    for (origin, kept) in [("m2", None), ("bid:gold_ore", Some("bid:gold_ore")), ("side:demand:clay", Some("side:demand:clay")), ("alch:gold_bar", Some("alch:gold_bar"))] {
        let mut bot_life = life("b0", ambition("woodcutter", "random"));
        bot_life.goal_origin = Some(origin.to_string());
        revise(&mut bot_life, ambition("fisher", "need"), "review:need", Stamp { tick: 10, secs: 1000 });
        assert_eq!(bot_life.goal_origin.as_deref(), kept, "{origin}");
    }
}

/// The fisher whose ambition changed at 1 000 (mila_wade) still runs the
/// « woodcutting 95 » its woodcutter ambition drew at 500: replaced. A goal
/// drawn after the change, a closed one, a bid, side or alchemy goal, a
/// goal linked to the current ambition and an operator kind are kept.
#[test]
fn a_goal_drawn_by_an_earlier_ambition_is_detected() {
    let changed = |outcome: &str, drawn_at: i64| {
        let mut fisher = ambition("fisher", "coverage:fishing");
        fisher.revisions.push(Revision { tick: 0, at_secs: 1000, from: "woodcutter".to_string(), to: "fisher".to_string(), why: "repair:coverage:fishing".to_string() });
        let mut mila = life("mila_wade", fisher);
        mila.goal = GoalKind::MaxSkills { skills: vec!["woodcutting".to_string()], level: 95 };
        mila.history = vec![goal_entry("done:woodcutting", drawn_at), goal_entry(outcome, drawn_at)];
        mila
    };
    assert!(goal_from_earlier_ambition(&changed("next:woodcutting 95", 500)), "mila_wade: replaced");
    assert!(!goal_from_earlier_ambition(&changed("next:fishing 60", 1500)), "drawn after the change");
    assert!(!goal_from_earlier_ambition(&changed("next:produce gold_ore 74 (bid:gold_ore)", 500)), "a bid goal runs on");
    assert!(!goal_from_earlier_ambition(&changed("next:produce clay 41 (side:demand:clay)", 500)), "a side goal runs on");
    let mut closed = changed("next:woodcutting 95", 500);
    closed.history.push(goal_entry("done:woodcutting", 600));
    assert!(!goal_from_earlier_ambition(&closed), "already closed: the next draw follows the new ambition");
    let mut bid = changed("next:woodcutting 95", 500);
    bid.goal_origin = Some("bid:yew_logs".to_string());
    assert!(!goal_from_earlier_ambition(&bid), "linked to a bid");
    let mut linked = changed("next:woodcutting 95", 500);
    linked.goal_origin = Some("m1".to_string());
    assert!(!goal_from_earlier_ambition(&linked), "linked to the current ambition");
    let mut generalist = changed("next:woodcutting 95", 500);
    let mut held = ambition(crate::ambition::GENERALIST, "random");
    held.drawn_at = 1000;
    generalist.ambition = Some(held.clone());
    assert!(goal_from_earlier_ambition(&generalist), "redrawn to generalist: the woodcutter's goal is replaced too");
    generalist.history.push(goal_entry("next:fishing 20", 1200));
    assert!(!goal_from_earlier_ambition(&generalist), "its own unlinked draw after the change runs on");
    let mut quoting = changed("next:market maker", 500);
    quoting.ambition = Some(ambition(MARKET_MAKER_KIND, "operator:market_maker"));
    assert!(!goal_from_earlier_ambition(&quoting), "a market maker");
}

/// The goal's work leaves (abandoned with the cause), the running node,
/// a market order, a gear step, the janitor and an older goal's nodes
/// stay.
#[test]
fn abandon_goal_work_spares_market_running_and_side_nodes() {
    let node = |id: &str, kind: NodeKind, deps: &[&str], status: NodeStatus, method: Option<&str>| {
        let mut made = Node::new(id, kind, deps.iter().map(|dep| dep.to_string()).collect(), 1);
        made.status = status;
        made.method = method.map(str::to_string);
        made
    };
    let climb = |level: u8| NodeKind::ReachLevel { skill: "woodcutting".to_string(), level };
    let mut mila = life("mila_wade", ambition("fisher", "need"));
    mila.memory.floor = Some(10);
    mila.nodes = vec![
        node("n5", climb(60), &[], NodeStatus::Ready, None),
        node("n10", climb(95), &[], NodeStatus::Ready, Some("gather:woodcut/yew_logs")),
        node("n11", climb(96), &["n10"], NodeStatus::Pending, None),
        node("n12", NodeKind::ObtainItems { item: "rune_axe".to_string(), count: 1 }, &[], NodeStatus::Ready, Some("market:buy")),
        node("n13", NodeKind::SpaceCleanup, &[], NodeStatus::Ready, None),
        node("n14", NodeKind::ObtainItems { item: "yew_logs".to_string(), count: 5 }, &[], NodeStatus::Active, Some("gather:woodcut/yew_logs")),
        node("n15", climb(94), &[], NodeStatus::Done, None),
        node("n16", NodeKind::ObtainItems { item: "yew_logs".to_string(), count: 50 }, &[], NodeStatus::Ready, Some("market:buy")),
    ];
    mila.active = Some("n14".to_string());
    // n16's buy rests on the book (parked); n12's was never posted.
    mila.history.push(HistoryEntry { node: "n16".to_string(), started: 1, ended: 1, outcome: "awaiting_fill".to_string() });
    let abandoned = crate::lifecycle::abandon_goal_work(&mut mila, STALE_GOAL_CAUSE);
    assert_eq!(abandoned, vec!["n10".to_string(), "n11".to_string(), "n12".to_string()], "an unposted market buy leaves with the goal");
    let status = |id: &str| mila.nodes.iter().find(|known| known.id == id).map(|known| known.status.clone()).expect("node");
    for kept in ["n5", "n13", "n16"] {
        assert_ne!(status(kept), NodeStatus::Abandoned, "{kept}");
    }
    assert_eq!(status("n14"), NodeStatus::Active, "the running node: its report decides");
    assert_eq!(status("n15"), NodeStatus::Done);
    assert_eq!(mila.nodes.iter().find(|known| known.id == "n10").map(|known| known.last_error.clone()), Some(STALE_GOAL_CAUSE.to_string()));
}
