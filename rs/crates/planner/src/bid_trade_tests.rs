//! Live 2026-11-20 00:15 -> 01:15 UTC: 60 of the 63 bots outside the
//! market makers held a `bid:` goal; nina_87 (runecrafter) made mithril
//! bars, lily_north (priest) mithril battleaxes, james_mason (cook)
//! water runes; cooking, prayer and firemaking 0 % of the fleet's time.

use super::{answers_trade, off_trade_answer};
use crate::ambition::Ambition;
use crate::model::{Archetype, GoalKind, HistoryEntry, Life, Profile};

fn ambition(kind: &str) -> Ambition {
    Ambition {
        kind: kind.to_string(),
        label: kind.to_string(),
        groups: Vec::new(),
        why: "random".to_string(),
        milestones: Vec::new(),
        drawn_at_tick: 0,
        drawn_at: 0,
        rung: 0,
        revisions: Vec::new(),
        pin: None,
        draw_rule: "need".to_string(),
    }
}

fn life(account: &str, kind: Option<&str>, origin: Option<&str>) -> Life {
    Life {
        schema: 1,
        account: account.to_string(),
        created_at: 0,
        seed: 1,
        profile: Profile::new(false, 0.5, 0.5, 1, Archetype::Producer),
        goal: GoalKind::Produce { item: "mithril_bar".to_string(), count: 53 },
        nodes: Vec::new(),
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: kind.map(ambition),
        goal_origin: origin.map(str::to_string),
        memory: Default::default(),
    }
}

#[test]
fn a_kind_answers_what_its_primary_skill_makes() {
    assert!(answers_trade(&life("eva_dale", Some("smith"), None), "mithril_platebody"));
    assert!(answers_trade(&life("eva_dale", Some("smith"), None), "mithril_bar"));
    assert!(answers_trade(&life("nina_87", Some("runecrafter"), None), "waterrune"));
    assert!(answers_trade(&life("james_mason", Some("cook"), None), "lobster"));
    assert!(answers_trade(&life("paul_rhodes", Some("miner"), None), "coal"));
    assert!(answers_trade(&life("tom_king", Some("crafter"), None), "sapphire_necklace"));
}

#[test]
fn off_its_trade_a_need_kind_answers_nothing() {
    // The secondary group (runecrafter: mining for essence) is no trade
    // to sell: mithril bars are a smith's.
    assert!(!answers_trade(&life("nina_87", Some("runecrafter"), None), "mithril_bar"));
    assert!(!answers_trade(&life("lily_north", Some("priest"), None), "mithril_battleaxe"));
    assert!(!answers_trade(&life("james_mason", Some("cook"), None), "waterrune"));
    // A miner sells ore, not the bars made of it.
    assert!(!answers_trade(&life("paul_rhodes", Some("miner"), None), "mithril_bar"));
    // The consumers make nothing: they train and buy what they burn.
    for kind in ["priest", "firemaker", "ranger", "warrior"] {
        assert!(!answers_trade(&life("someone", Some(kind), None), "steel_platebody"), "{kind}");
    }
}

#[test]
fn the_exceptions_answer_the_whole_market() {
    // D3: Zezima, N0valyfe, Yogosun keep the level-led draw and the
    // whole market; kinds without a trade (market maker, overall,
    // generalist, alchemist) and lives without ambition too.
    assert!(answers_trade(&life("Zezima", Some("runecrafter"), None), "mithril_bar"));
    assert!(answers_trade(&life("n0valyfe", Some("priest"), None), "mithril_bar"));
    for kind in ["market_maker", "overall", "generalist", "alcher"] {
        assert!(answers_trade(&life("someone", Some(kind), None), "mithril_bar"), "{kind}");
    }
    assert!(answers_trade(&life("someone", None, None), "mithril_bar"));
}

#[test]
fn a_held_answer_off_the_trade_is_named() {
    let nina = life("nina_87", Some("runecrafter"), Some("bid:mithril_bar"));
    assert_eq!(off_trade_answer(&nina), Some("bid:mithril_bar".to_string()));
    let smith = life("eva_dale", Some("smith"), Some("bid:mithril_bar"));
    assert_eq!(off_trade_answer(&smith), None);
    let milestone = life("nina_87", Some("runecrafter"), Some("m1"));
    assert_eq!(off_trade_answer(&milestone), None);
    // A goal already closed is the draw's to replace.
    let mut closed = life("nina_87", Some("runecrafter"), Some("bid:mithril_bar"));
    closed.history.push(HistoryEntry { node: "goal".to_string(), started: 1, ended: 1, outcome: format!("{}produce mithril_bar 53", crate::goals::DONE_TAG) });
    assert!(crate::goals::goal_closed(&closed.history));
    assert_eq!(off_trade_answer(&closed), None);
}
