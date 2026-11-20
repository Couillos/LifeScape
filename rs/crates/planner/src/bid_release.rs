//! A bid answer whose production every site refuses is given back
//! (2026-11-20). Live 22:20-23:20 UTC: lily_north answered a 93 mithril
//! ore lot (`bid:mithril_ore`), its only leg `farm:mithril_ore` met
//! `full:` at once (one seat per mithril site, every one held), the node
//! waited open (`cool_wait:`, tour 3 point 1) and the bot filled the whole
//! hour (fill-n1401) while its claim kept the lot from every other
//! producer; 56 bots were « awaiting its fill », 41 % of the fleet's time
//! went to fillers (26 % before).
//!
//! Rule: a node of the current `bid:` goal that has waited out every
//! method (`cooldown::waiting_nodes`: its latest entry `cool_wait:`, the
//! method still cooled) since `BID_WAIT_RELEASE_TICKS` or more, with no
//! work in between (only waits, launches refused as `contended`), makes
//! the answer given back: the brain abandons the goal's work, closes the
//! goal without any ban, frees its claims and seats, and draws again (the
//! ambition, untouched, comes back). A milestone goal keeps waiting (its
//! ambition wants it; the filler works meanwhile).
//!
//! Delay: one cooldown (`cooldown::COOL_TICKS`, 6000 ticks, 1 h in prod).
//! The block cools the method for exactly that long; the resolve after
//! the lapse reads the seats again, and only when it finds them still
//! full (the method cooled again, the node back to waiting) is the answer
//! given back. Shorter would give back an answer before the one re-read of
//! the sites; longer leaves the bot in fillers and the lot's claim
//! blocking the other producers for a second hour with nothing new to
//! read. Pure: elapsed ticks come from the caller (`ticks_since`).

use crate::model::Life;

/// Ticks a bid answer's node may wait out its refused methods before the
/// answer is given back (module doc).
pub const BID_WAIT_RELEASE_TICKS: u64 = crate::cooldown::COOL_TICKS;

/// History outcome (node `goal`) of a given-back answer:
/// `bid_released:<origin>|<node>|<ticks waited>`.
pub const RELEASED_TAG: &str = "bid_released:";

/// Ticks a bot leaves an object alone after giving back its answer
/// (`shunned_items`): three waits. A bot that answers, waits out
/// `BID_WAIT_RELEASE_TICKS`, gives back and answers again at once would
/// lose half its time to the same blocked object (contended by players,
/// its own cooled sites: nothing the fleet's seats show). Shunned for
/// 3 × 6000 ticks, a bot blocked again every time waits at most
/// 6000 / (6000 + 18 000) = 25 % of its time on that object, under the
/// fleet's reference filler share (26 %, before the 41 % measured live).
pub const BID_SHUN_TICKS: u64 = 3 * BID_WAIT_RELEASE_TICKS;

/// Objects whose bid answer this bot gave back less than
/// `BID_SHUN_TICKS` ago: no answer is drawn for them.
pub fn shunned_items(history: &[crate::model::HistoryEntry], ticks_since: &dyn Fn(i64) -> u64) -> Vec<String> {
    let mut out: Vec<String> = history
        .iter()
        .filter(|entry| entry.node == "goal" && ticks_since(entry.ended) < BID_SHUN_TICKS)
        .filter_map(|entry| entry.outcome.strip_prefix(RELEASED_TAG)?.split('|').next()?.strip_prefix(crate::bid_response::BID_PREFIX).map(str::to_string))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Node history outcomes that are no work: the node waited
/// (`cool_wait:`), was launched and refused on a contended site.
const IDLE_OUTCOMES: [&str; 2] = ["launched", "contended"];

/// A bid answer to give back: its origin, the node that waits, and since
/// how many ticks it waits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedBid {
    pub origin: String,
    pub node: String,
    pub waited_ticks: u64,
}

/// The current `bid:` goal's node that has waited at least
/// `BID_WAIT_RELEASE_TICKS` (module doc), `None` otherwise.
pub fn blocked_bid(life: &Life, cooled: &[String], ticks_since: &dyn Fn(i64) -> u64) -> Option<BlockedBid> {
    let origin = life.goal_origin.as_deref().filter(|origin| origin.starts_with(crate::bid_response::BID_PREFIX))?;
    if crate::goals::goal_closed(&life.history) {
        return None;
    }
    // Only what happened since the goal was drawn counts.
    let drawn = life.history.iter().rposition(|entry| entry.node == "goal" && entry.outcome.starts_with(crate::ambition_review::NEXT_TAG))?;
    let since = &life.history[drawn + 1..];
    let floor = life.memory.floor.unwrap_or(0);
    let mut found: Option<BlockedBid> = None;
    for node in crate::cooldown::waiting_nodes(since, cooled) {
        if crate::lifecycle_retire::node_number(&node).is_none_or(|number| number < floor) {
            continue;
        }
        // The node's trailing run of idle entries; its first wait is when
        // the block began.
        let mut first_wait: Option<i64> = None;
        for entry in since.iter().rev().filter(|entry| entry.node == node) {
            if entry.outcome.starts_with(crate::cooldown::WAIT_TAG) {
                first_wait = Some(entry.ended);
            } else if !IDLE_OUTCOMES.contains(&entry.outcome.as_str()) {
                break;
            }
        }
        let Some(stamp) = first_wait else { continue };
        let waited_ticks = ticks_since(stamp);
        if waited_ticks >= BID_WAIT_RELEASE_TICKS && found.as_ref().is_none_or(|known| known.waited_ticks < waited_ticks) {
            found = Some(BlockedBid { origin: origin.to_string(), node, waited_ticks });
        }
    }
    found
}

#[cfg(test)]
#[path = "bid_release_tests.rs"]
mod tests;
