//! Item ledger (user 2026-11-15: "tracer d'où vient et où part chaque
//! item"). The coin ledger's rule for every object: each change of what
//! the pack carries is one flow, attributed to the action that caused it
//! (`Bot::coin_context` names it for a few ticks: shop, trade, fee, loot,
//! alch, dialogue, drop), else to the skill when XP moved in the same
//! window (made or consumed by work), else to a death; a change with the
//! bank open is a transfer, never a flow. What nothing explains stays
//! `unattributed`. The session report reads these flows instead of
//! guessing a provenance from the net holdings.

#![forbid(unsafe_code)]

use std::collections::HashMap;

/// Ticks an XP gain explains item changes after it (a smelt lands the
/// bar and the XP in the same tick or the next few).
pub const SKILL_TICKS: u64 = 8;

/// Kinds beyond the coin ledger's (`coin_ledger::kind`).
pub mod kind {
    /// Made or consumed by a skill action (XP in the same window).
    pub const SKILL: &str = "skill";
    /// Thrown to the ground by the bot (`Bot::drop`).
    pub const DROPPED: &str = "dropped";
    /// Lost to a death.
    pub const DEATH: &str = "death";
}

/// One attributed change of a carried object.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemFlow {
    pub obj: u16,
    pub kind: &'static str,
    pub delta: i64,
    pub tick: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ItemLedger {
    last_pack: Option<HashMap<u16, u32>>,
    last_xp: Option<i64>,
    xp_tick: Option<u64>,
    pub flows: Vec<ItemFlow>,
}

/// What one observation knows besides the pack.
pub struct Seen<'a> {
    pub pack: HashMap<u16, u32>,
    pub bank_open: bool,
    pub tick: u64,
    /// Total XP over every stat now.
    pub xp_total: i64,
    pub dead: bool,
    /// The action context the coin ledger holds (kind, until tick).
    pub context: Option<(&'a str, u64)>,
}

impl ItemLedger {
    /// One observation: records a flow per object whose count moved.
    pub fn observe(&mut self, seen: Seen<'_>) {
        if let Some(last) = self.last_xp {
            if seen.xp_total > last {
                self.xp_tick = Some(seen.tick);
            }
        }
        self.last_xp = Some(seen.xp_total);
        let Some(last) = self.last_pack.replace(seen.pack.clone()) else { return };
        if seen.bank_open {
            return;
        }
        let mut ids: Vec<u16> = last.keys().chain(seen.pack.keys()).copied().collect();
        ids.sort_unstable();
        ids.dedup();
        for obj in ids {
            let delta = i64::from(seen.pack.get(&obj).copied().unwrap_or(0)) - i64::from(last.get(&obj).copied().unwrap_or(0));
            if delta == 0 {
                continue;
            }
            let kind = self.kind_now(&seen);
            self.flows.push(ItemFlow { obj, kind, delta, tick: seen.tick });
        }
    }

    fn kind_now(&self, seen: &Seen<'_>) -> &'static str {
        if let Some((name, until)) = seen.context {
            if seen.tick <= until {
                return static_kind(name);
            }
        }
        if self.xp_tick.is_some_and(|at| seen.tick <= at + SKILL_TICKS) {
            return kind::SKILL;
        }
        if seen.dead {
            return kind::DEATH;
        }
        super::coin_ledger::kind::UNATTRIBUTED
    }

    /// Net change per (object, kind) over the flows from index `start` on.
    pub fn totals_since(&self, start: usize) -> Vec<(u16, &'static str, i64)> {
        let mut out: Vec<(u16, &'static str, i64)> = Vec::new();
        for flow in self.flows.iter().skip(start) {
            match out.iter_mut().find(|(obj, kind, _)| *obj == flow.obj && *kind == flow.kind) {
                Some(entry) => entry.2 += flow.delta,
                None => out.push((flow.obj, flow.kind, flow.delta)),
            }
        }
        out.retain(|(_, _, net)| *net != 0);
        out
    }
}

/// The ledger's own static name for a context kind.
fn static_kind(name: &str) -> &'static str {
    use super::coin_ledger::kind as coin;
    match name {
        coin::SHOP => coin::SHOP,
        coin::TRADE => coin::TRADE,
        coin::FEE => coin::FEE,
        coin::LOOT => coin::LOOT,
        coin::ALCH => coin::ALCH,
        coin::DIALOGUE => coin::DIALOGUE,
        kind::DROPPED => kind::DROPPED,
        _ => coin::UNATTRIBUTED,
    }
}

#[cfg(test)]
mod tests {
    use super::{kind, ItemLedger, Seen};
    use std::collections::HashMap;

    fn seen(pack: &[(u16, u32)], tick: u64, xp: i64, context: Option<(&'static str, u64)>, bank_open: bool) -> Seen<'static> {
        Seen { pack: pack.iter().copied().collect::<HashMap<_, _>>(), bank_open, tick, xp_total: xp, dead: false, context }
    }

    #[test]
    fn every_change_names_its_cause() {
        let mut ledger = ItemLedger::default();
        ledger.observe(seen(&[(440, 5)], 1, 100, None, false));
        // A smelt: ore out, bar in, XP up.
        ledger.observe(seen(&[(440, 4), (2349, 1)], 10, 106, None, false));
        // A shop buy: hammer in under the shop context.
        ledger.observe(seen(&[(440, 4), (2349, 1), (2347, 1)], 30, 106, Some(("shop", 40)), false));
        // A drop.
        ledger.observe(seen(&[(440, 4), (2347, 1)], 60, 106, Some((kind::DROPPED, 70)), false));
        // Banked: a transfer, no flow.
        ledger.observe(seen(&[], 90, 106, None, true));
        let totals = ledger.totals_since(0);
        assert!(totals.contains(&(440, kind::SKILL, -1)), "{totals:?}");
        assert!(totals.contains(&(2349, kind::SKILL, 1)));
        assert!(totals.contains(&(2347, "shop", 1)));
        assert!(totals.contains(&(2349, kind::DROPPED, -1)));
        assert_eq!(totals.len(), 4, "the bank transfer adds nothing: {totals:?}");
    }
}
