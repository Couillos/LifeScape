//! lily_north, live 2026-11-20 22:22 -> 23:22 (`bid:mithril_ore`, n1401
//! waiting on `full:farm:mithril_ore` the whole hour).

use super::{blocked_bid, BlockedBid, BID_WAIT_RELEASE_TICKS};
use crate::model::{Archetype, GoalKind, HistoryEntry, Life, Node, NodeKind, NodeStatus, Profile};

/// History stamps are plain ticks here (`ticks_since` = now - stamp).
const NOW: i64 = 100_000;

fn entry(node: &str, outcome: &str, at: i64) -> HistoryEntry {
    HistoryEntry { node: node.to_string(), started: at, ended: at, outcome: outcome.to_string() }
}

fn lily(origin: &str, history: Vec<HistoryEntry>) -> Life {
    let mut node = Node::new("n1401", NodeKind::ObtainItems { item: "mithril_ore".to_string(), count: 93 }, Vec::new(), 93);
    node.status = NodeStatus::Ready;
    node.method = Some("farm:mithril_ore".to_string());
    let mut life = Life {
        schema: 1,
        account: "lily_north".to_string(),
        created_at: 0,
        seed: 1,
        profile: Profile::new(false, 0.5, 0.5, 1, Archetype::Producer),
        goal: GoalKind::Produce { item: "mithril_ore".to_string(), count: 93 },
        nodes: vec![node],
        engagement: None,
        active: None,
        history,
        ambition: None,
        goal_origin: Some(origin.to_string()),
        memory: Default::default(),
    };
    life.memory.floor = Some(1401);
    life
}

/// The bid goal drawn at `drawn`, its node waiting since `first_wait`.
fn waiting_since(drawn: i64, first_wait: i64) -> Vec<HistoryEntry> {
    vec![
        entry("goal", "next:produce mithril_ore 93 (bid:mithril_ore)", drawn),
        entry("cool", "cool:farm:mithril_ore|6000|full:farm:mithril_ore", first_wait),
        entry("n1401", "cool_wait:farm:mithril_ore", first_wait),
        // The cooldown lapsed, the launch met other miners, the node
        // waits again on the method cooled anew.
        entry("n1401", "launched", first_wait + 6_100),
        entry("n1401", "contended", first_wait + 6_150),
        entry("cool", "cool:farm:mithril_ore|6000|full:farm:mithril_ore", first_wait + 6_150),
        entry("n1401", "cool_wait:farm:mithril_ore", first_wait + 6_150),
    ]
}

fn ticks_since(stamp: i64) -> u64 {
    (NOW - stamp).max(0) as u64
}

fn cooled() -> Vec<String> {
    vec!["farm:mithril_ore".to_string()]
}

/// Past one cooldown of waiting (the re-read after the lapse found the
/// seats still full): the answer is given back.
#[test]
fn a_bid_answer_waiting_past_one_cooldown_is_given_back() {
    let first_wait = NOW - BID_WAIT_RELEASE_TICKS as i64 - 200;
    let life = lily("bid:mithril_ore", waiting_since(first_wait - 1, first_wait));
    assert_eq!(
        blocked_bid(&life, &cooled(), &ticks_since),
        Some(BlockedBid { origin: "bid:mithril_ore".to_string(), node: "n1401".to_string(), waited_ticks: BID_WAIT_RELEASE_TICKS + 200 })
    );
}

/// Under the delay, a lapsed cooldown (the resolve has not re-read the
/// seats yet), work in between, or a milestone goal: no give-back.
#[test]
fn a_short_wait_a_lapsed_cooldown_work_or_a_milestone_keep_the_goal() {
    let recent = NOW - 300;
    let life = lily("bid:mithril_ore", vec![entry("goal", "next:produce mithril_ore 93 (bid:mithril_ore)", recent - 1), entry("n1401", "cool_wait:farm:mithril_ore", recent)]);
    assert_eq!(blocked_bid(&life, &cooled(), &ticks_since), None, "under one cooldown");
    let old = NOW - BID_WAIT_RELEASE_TICKS as i64 - 200;
    let life = lily("bid:mithril_ore", waiting_since(old - 1, old));
    assert_eq!(blocked_bid(&life, &[], &ticks_since), None, "the cooldown lapsed: the resolve reads the seats first");
    let mut worked = waiting_since(old - 1, old);
    worked.insert(3, entry("n1401", "goal_reached", old + 3_000));
    worked.insert(3, entry("n1401", "launched", old + 2_000));
    let life = lily("bid:mithril_ore", worked);
    let found = blocked_bid(&life, &cooled(), &ticks_since);
    assert!(found.as_ref().is_none_or(|blocked| blocked.waited_ticks < BID_WAIT_RELEASE_TICKS), "it mined in between: the wait counts from its last work: {found:?}");
    let life = lily("m2", waiting_since(old - 1, old));
    assert_eq!(blocked_bid(&life, &cooled(), &ticks_since), None, "a milestone goal waits for its ambition");
    let mut closed = waiting_since(old - 1, old);
    closed.push(entry("goal", "done:mining", NOW - 10));
    assert_eq!(blocked_bid(&lily("bid:mithril_ore", closed), &cooled(), &ticks_since), None, "a closed goal is the draw's business");
}

/// A wait recorded before the current goal was drawn (an earlier goal's
/// node id) never counts for this answer.
#[test]
fn a_wait_before_the_draw_never_counts() {
    let old = NOW - BID_WAIT_RELEASE_TICKS as i64 - 200;
    let history = vec![entry("n1401", "cool_wait:farm:mithril_ore", old), entry("goal", "next:produce mithril_ore 93 (bid:mithril_ore)", NOW - 100)];
    assert_eq!(blocked_bid(&lily("bid:mithril_ore", history), &cooled(), &ticks_since), None);
}

/// After a give-back the same object is left alone for `BID_SHUN_TICKS`
/// (blocked by players or its own cooled sites, nothing the seats show).
#[test]
fn a_given_back_object_is_shunned_for_three_waits() {
    use super::{shunned_items, BID_SHUN_TICKS};
    let history = vec![
        entry("goal", "bid_released:bid:mithril_ore|n1401|6200", NOW - 100),
        entry("goal", "bid_released:bid:gold_ore|n900|6100", NOW - BID_SHUN_TICKS as i64 - 1),
        entry("n1401", "bid_released:bid:coal|n1|1", NOW - 10),
    ];
    assert_eq!(shunned_items(&history, &ticks_since), vec!["mithril_ore".to_string()], "gold ore's shun is over; only goal entries count");
}
