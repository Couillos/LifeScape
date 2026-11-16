//! Item ledger (user 2026-11-15: "tracer d'où vient et où part chaque
//! item"; plan rendements §8, D19: every input AND output of a run is a
//! flow with a cause, the net diff is only a coherence check). Each
//! change of what the bot CARRIES (pack plus worn: wielding moves nothing
//! out, a fired arrow leaves the quiver) is one flow, attributed in this
//! order:
//! 1. bank open: a transfer (pack <-> bank), never a flow; bank closed
//!    less than `BANK_GRACE_TICKS` ago: still a transfer (the deposit's
//!    inventory update can land just after the client closed the bank
//!    locally), unless a scoped action sent since the bank was last seen
//!    open names it (live 2026-11-16, cleanup: withdraw, close, drop two
//!    ticks later: the drops read as transfers, residual -2 raw shrimps,
//!    -163 body runes);
//! 2. a loss while dead or within `DEATH_TICKS` of a death: `death`;
//! 3. the newest scoped action that matches the object and the sign
//!    (`Bot::item_action`: eat, bury, drop, pickup, cast);
//! 4. a shop, player trade or dialogue sale in force (`coin_ledger`
//!    context): those modals move items both ways;
//! 5. skill XP moved within `SKILL_TICKS`: `skill` (made or eaten by work);
//! 6. nothing: `unattributed`, kept visible (the report raises an alert).
//!
//! Every flow also carries the bitmask of the stats whose XP rose within
//! the window, so the report can tell a product from a by-product (a gem
//! while mining) with the game data, which the SDK never reads.

#![forbid(unsafe_code)]

use std::collections::HashMap;

/// Ticks an XP gain explains item changes after it (a smelt lands the
/// bar and the XP in the same tick or the next few).
pub const SKILL_TICKS: u64 = 8;
/// Ticks after the bank was last seen open during which carried changes
/// are still transfers.
pub const BANK_GRACE_TICKS: u64 = 2;
/// Ticks after a death during which losses are the death's (the server
/// clears the pack at respawn, once the hitpoints came back).
pub const DEATH_TICKS: u64 = 30;
/// Default lifetime of a scoped action (eat, bury, drop: the server
/// answers within a couple of ticks).
pub const ACTION_TICKS: u64 = 10;

/// Kinds beyond the coin ledger's (`coin_ledger::kind`).
pub mod kind {
    /// Made or consumed by a skill action (XP in the same window).
    pub const SKILL: &str = "skill";
    /// Thrown to the ground by the bot (`Bot::drop`).
    pub const DROPPED: &str = "dropped";
    /// Lost to a death.
    pub const DEATH: &str = "death";
    /// Eaten or drunk (`op_held` on an Eat/Drink option).
    pub const EAT: &str = "eat";
    /// Buried (`op_held` on a Bury option).
    pub const BURY: &str = "bury";
    /// Taken off the ground (`op_obj`).
    pub const PICKUP: &str = "pickup";
    /// Moved by a spell the bot cast (runes out, product or target).
    pub const CAST: &str = "cast";
}

/// One attributed change of a carried object.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemFlow {
    pub obj: u16,
    pub kind: &'static str,
    pub delta: i64,
    pub tick: u64,
    /// Bit `stat` set: that stat's XP rose within `SKILL_TICKS`.
    pub skills: u32,
}

/// One action the bot just sent that explains the next changes: the
/// object it touches (`None`: any) and the sign it moves (0: both),
/// from the tick it was sent until `until`.
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub kind: &'static str,
    pub obj: Option<u16>,
    pub sign: i8,
    pub from: u64,
    pub until: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ItemLedger {
    last_pack: Option<HashMap<u16, u32>>,
    last_xp: HashMap<u8, i32>,
    xp_ticks: HashMap<u8, u64>,
    actions: Vec<Action>,
    bank_seen: Option<u64>,
    death_seen: Option<u64>,
    pub flows: Vec<ItemFlow>,
}

/// What one observation knows besides the carried objects.
pub struct Seen<'a> {
    /// Pack plus worn, summed per object.
    pub pack: HashMap<u16, u32>,
    pub bank_open: bool,
    pub tick: u64,
    /// XP per stat now.
    pub xp: Vec<(u8, i32)>,
    pub dead: bool,
    /// The coin ledger's context in force (kind, until tick).
    pub context: Option<(&'a str, u64)>,
}

/// Actions kept at most (oldest dropped): a handful live at once.
const ACTIONS_CAP: usize = 16;

impl ItemLedger {
    /// Names what the next change of `obj` (any object: `None`) with the
    /// sign `sign` (0: both) comes from, sent at tick `from`, until tick
    /// `until`.
    pub fn note_action(&mut self, kind: &'static str, obj: Option<u16>, sign: i8, from: u64, until: u64) {
        self.actions.push(Action { kind, obj, sign, from, until });
        if self.actions.len() > ACTIONS_CAP {
            let drop = self.actions.len() - ACTIONS_CAP;
            self.actions.drain(..drop);
        }
    }

    /// Stats whose XP rose within `SKILL_TICKS` of `tick` (bitmask).
    pub fn skills_at(&self, tick: u64) -> u32 {
        self.xp_ticks
            .iter()
            .filter(|(_, at)| tick <= **at + SKILL_TICKS)
            .fold(0u32, |mask, (stat, _)| if *stat < 32 { mask | (1 << stat) } else { mask })
    }

    /// One observation: records a flow per object whose count moved.
    pub fn observe(&mut self, seen: Seen<'_>) {
        for (stat, xp) in &seen.xp {
            if self.last_xp.get(stat).is_some_and(|last| xp > last) {
                self.xp_ticks.insert(*stat, seen.tick);
            }
            self.last_xp.insert(*stat, *xp);
        }
        if seen.bank_open {
            self.bank_seen = Some(seen.tick);
        }
        if seen.dead {
            self.death_seen = Some(seen.tick);
        }
        self.actions.retain(|action| seen.tick <= action.until);
        let Some(last) = self.last_pack.replace(seen.pack.clone()) else { return };
        if seen.bank_open {
            return;
        }
        let grace = self.bank_seen.filter(|at| seen.tick <= at + BANK_GRACE_TICKS);
        let skills = self.skills_at(seen.tick);
        let mut ids: Vec<u16> = last.keys().chain(seen.pack.keys()).copied().collect();
        ids.sort_unstable();
        ids.dedup();
        for obj in ids {
            let delta = i64::from(seen.pack.get(&obj).copied().unwrap_or(0)) - i64::from(last.get(&obj).copied().unwrap_or(0));
            if delta == 0 {
                continue;
            }
            let kind = match grace {
                // Just after the bank closed: a transfer, unless an action
                // sent since the bank was last seen open names the change.
                Some(open_at) => match self.scoped(obj, delta).filter(|action| action.from >= open_at) {
                    Some(action) => action.kind,
                    None => continue,
                },
                None => self.kind_now(&seen, obj, delta, skills),
            };
            self.flows.push(ItemFlow { obj, kind, delta, tick: seen.tick, skills });
        }
    }

    /// The newest scoped action in force matching `obj` and the sign of
    /// `delta`.
    fn scoped(&self, obj: u16, delta: i64) -> Option<&Action> {
        let sign: i8 = if delta > 0 { 1 } else { -1 };
        self.actions.iter().rev().find(|action| action.obj.is_none_or(|wanted| wanted == obj) && (action.sign == 0 || action.sign == sign))
    }

    fn kind_now(&self, seen: &Seen<'_>, obj: u16, delta: i64, skills: u32) -> &'static str {
        use super::coin_ledger::kind as coin;
        if delta < 0 && self.death_seen.is_some_and(|at| seen.tick <= at + DEATH_TICKS) {
            return kind::DEATH;
        }
        if let Some(action) = self.scoped(obj, delta) {
            return action.kind;
        }
        if let Some((name, until)) = seen.context {
            if seen.tick <= until && matches!(name, coin::SHOP | coin::TRADE | coin::DIALOGUE) {
                return static_kind(name);
            }
        }
        if skills != 0 {
            return kind::SKILL;
        }
        coin::UNATTRIBUTED
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
        coin::DIALOGUE => coin::DIALOGUE,
        _ => coin::UNATTRIBUTED,
    }
}

#[cfg(test)]
#[path = "item_ledger_tests.rs"]
mod tests;
