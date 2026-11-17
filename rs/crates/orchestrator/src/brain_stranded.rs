//! Stranded bots (plan tour 2, P2-1a, N1).
//!
//! Live 2026-11-17 11:10: bellarhodes, desynced on the Karamja boat,
//! reappeared on the Port Sarim ship deck (3032,3214,1), a static
//! component without exit. Nothing stopped it: 2 231 NoRoute dossiers,
//! 1 052 dead deals and 298 failed runs in one hour, every one starting
//! from the same tile.
//!
//! Evidence comes from the nav dossiers: a `Disconnected` NoRoute whose
//! search never got farther than `STRAND_RADIUS` from its origin while
//! the destination lies beyond it says the ORIGIN is walled in (a sealed
//! destination leaves its closest tile near the destination instead).
//! Two such dossiers from the same origin towards two different
//! destinations make the bot `Stranded` there (one NoRoute alone may be
//! a transient). The brain then holds every launch while the bot stands
//! on that tile, logs `stranded at` once (error kind `stranded`, a
//! question for the operator, rule 7), and the market retires its
//! orders. Leaving the tile (operator, map fix and restart) clears it.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Farthest the search may get from a walled-in origin (Chebyshev).
pub const STRAND_RADIUS: i64 = 16;
/// Distinct destinations refused from one origin before the verdict.
pub const STRAND_EVIDENCE: usize = 2;
/// Log marker of the verdict (`errors_model` kind `stranded`).
pub const STRANDED_MARKER: &str = "stranded at ";
/// Cancel reason of the orders a stranded bot loses.
pub const STRANDED_REASON: &str = "stranded";

pub type Tile3 = (i64, i64, i64);

/// The tile a `closest=(x,z,level)` clause names.
fn closest_of(message: &str) -> Option<Tile3> {
    let tail = message.split("Disconnected closest=(").nth(1)?;
    let inner = tail.split(')').next()?;
    let parts: Vec<i64> = inner.split(',').filter_map(|part| part.trim().parse::<i64>().ok()).collect();
    match parts.as_slice() {
        [x, z, level] => Some((*x, *z, *level)),
        _ => None,
    }
}

fn chebyshev(left: Tile3, right: Tile3) -> i64 {
    if left.2 != right.2 {
        return i64::MAX;
    }
    (left.0 - right.0).abs().max((left.1 - right.1).abs())
}

/// True when a NoRoute `message` from `origin` to `dest` proves the
/// origin walled in: `Disconnected`, closest tile within the radius of
/// the origin, destination beyond it (or on another level).
pub fn origin_walled_in(origin: Tile3, dest: Tile3, message: &str) -> bool {
    let Some(closest) = closest_of(message) else {
        return false;
    };
    chebyshev(origin, closest) <= STRAND_RADIUS && chebyshev(origin, dest) > STRAND_RADIUS
}

/// Per-account evidence: origin and the distinct destinations refused.
#[derive(Default)]
pub struct StrandBook {
    evidence: HashMap<String, (Tile3, Vec<Tile3>)>,
    stranded: HashMap<String, Tile3>,
    announced: HashMap<String, Tile3>,
}

impl StrandBook {
    /// Folds one NoRoute dossier. Returns the stranded tile when this
    /// dossier completes the evidence.
    pub fn observe(&mut self, account: &str, origin: Tile3, dest: Tile3, message: &str) -> Option<Tile3> {
        if !origin_walled_in(origin, dest, message) {
            return None;
        }
        let entry = self.evidence.entry(account.to_string()).or_insert((origin, Vec::new()));
        if entry.0 != origin {
            *entry = (origin, Vec::new());
        }
        if !entry.1.contains(&dest) {
            entry.1.push(dest);
        }
        if entry.1.len() >= STRAND_EVIDENCE && self.stranded.get(account) != Some(&origin) {
            self.stranded.insert(account.to_string(), origin);
            return Some(origin);
        }
        None
    }

    /// The tile the bot is stranded on while it still stands there;
    /// a bot seen elsewhere is cleared (evidence included).
    pub fn holds(&mut self, account: &str, position: Option<Tile3>) -> Option<Tile3> {
        let tile = *self.stranded.get(account)?;
        if position.is_some_and(|here| here != tile) {
            self.stranded.remove(account);
            self.evidence.remove(account);
            self.announced.remove(account);
            return None;
        }
        Some(tile)
    }

    /// True once per stranded tile: the verdict is logged (and the
    /// orders retired) a single time.
    pub fn first_announce(&mut self, account: &str, tile: Tile3) -> bool {
        self.announced.insert(account.to_string(), tile) != Some(tile)
    }
}

fn book() -> &'static Mutex<StrandBook> {
    static BOOK: OnceLock<Mutex<StrandBook>> = OnceLock::new();
    BOOK.get_or_init(|| Mutex::new(StrandBook::default()))
}

/// Dossier hook (`errors_ingest::scan_dossiers`): a NoRoute row.
pub(crate) fn observe_no_route(account: &str, origin: Option<Tile3>, dest: Option<Tile3>, message: &str) {
    let (Some(origin), Some(dest)) = (origin, dest) else { return };
    if account.is_empty() {
        return;
    }
    book().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).observe(account, origin, dest, message);
}

/// Launch gate: `Some(tile)` while the bot stands stranded there. The
/// first time, logs the verdict and returns `(tile, true)`.
pub(crate) fn gate(account: &str, position: Option<Tile3>) -> Option<(Tile3, bool)> {
    let mut guard = book().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let tile = guard.holds(account, position)?;
    let first = guard.first_announce(account, tile);
    Some((tile, first))
}

/// The verdict line (one per stranded tile).
pub fn verdict_line(account: &str, tile: Tile3) -> String {
    format!(
        "brain {account}: {STRANDED_MARKER}({},{},{}): no route leaves this tile (NoRoute Disconnected to {STRAND_EVIDENCE} destinations), launches held, orders retired; operator decision needed",
        tile.0, tile.1, tile.2
    )
}

#[cfg(test)]
mod tests {
    use super::{origin_walled_in, StrandBook};

    const DECK: (i64, i64, i64) = (3032, 3214, 1);

    fn refused(dest: (i64, i64, i64)) -> String {
        format!("travel: no route (3032,3214,1) -> ({},{},{}): Disconnected closest=(3032,3216,1) blocked=[]", dest.0, dest.1, dest.2)
    }

    /// Live N1: the deck dossiers prove the origin walled in; a sealed
    /// destination (closest near the destination) does not.
    #[test]
    fn a_search_stuck_near_its_origin_walls_the_origin() {
        assert!(origin_walled_in(DECK, (3017, 3351, 0), &refused((3017, 3351, 0))));
        let sealed_tree = "travel: no route (3200,3200,0) -> (3168,3350,0): Disconnected closest=(3163,3355,0) blocked=[]";
        assert!(!origin_walled_in((3200, 3200, 0), (3168, 3350, 0), sealed_tree), "sealed destination");
        assert!(!origin_walled_in(DECK, (3020, 3220, 1), &refused((3020, 3220, 1))), "destination within the radius");
        assert!(!origin_walled_in(DECK, (3017, 3351, 0), "travel: no route: BudgetExhausted closest=(3032,3216,1)"));
    }

    /// Red test P2-1a: two refusals from the same tile towards two
    /// destinations strand the bot; launches stay held while it stands
    /// there, the verdict is announced once, and leaving the tile clears.
    #[test]
    fn two_refusals_from_one_tile_strand_the_bot() {
        let mut book = StrandBook::default();
        assert_eq!(book.observe("bot", DECK, (3017, 3351, 0), &refused((3017, 3351, 0))), None, "one refusal is no verdict");
        assert_eq!(book.observe("bot", DECK, (3017, 3351, 0), &refused((3017, 3351, 0))), None, "same destination twice");
        assert_eq!(book.holds("bot", Some(DECK)), None);
        assert_eq!(book.observe("bot", DECK, (3092, 3243, 0), &refused((3092, 3243, 0))), Some(DECK));
        assert_eq!(book.holds("bot", Some(DECK)), Some(DECK), "held on the deck");
        assert!(book.first_announce("bot", DECK));
        assert!(!book.first_announce("bot", DECK), "announced once");
        assert_eq!(book.holds("bot", None), Some(DECK), "no position yet: still held");
        assert_eq!(book.holds("bot", Some((3029, 3217, 0))), None, "off the deck: cleared");
        assert_eq!(book.holds("bot", Some(DECK)), None, "fresh evidence needed again");
    }

    /// The verdict lands in error_events once, as kind `stranded`.
    #[test]
    fn the_verdict_is_an_error_row() {
        let row = crate::errors_model::from_brain_line(&super::verdict_line("bot", DECK), 0).expect("brain row");
        assert_eq!((row.kind.as_str(), row.account.as_str()), ("stranded", "bot"));
    }
}
