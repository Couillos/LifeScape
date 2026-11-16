//! XP-bearing events that leave no object behind (plan rendements §7,
//! D19): spell casts, kills and damage per NPC type. Cumulative over the
//! session; the task report diffs a start copy against the end.
//!
//! - Casts: a spell sent (`*_t` ops, an `if_button`) is pending until the
//!   magic XP rises within `CAST_CONFIRM_TICKS`: only a cast the server
//!   paid counts (a splash pays its base XP; a refused cast pays none).
//!   Keyed by the spell button component; the report names the spell.
//! - Damage and kills: the NPC damage masks (0x1/0x10, `NpcInfoDamage`:
//!   damage, type, hp after the hit, max hp; Client-TS `Client.ts`
//!   HITMARK/HITMARK2) on the NPC the bot last targeted. The server sends
//!   no attacker: a hit another player lands on the same NPC counts too
//!   (bots avoid engaged NPCs, `engagement.rs`). A kill is the target's
//!   hp reaching 0, counted once per NPC slot.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

/// Ticks a sent spell waits for its magic XP.
pub const CAST_CONFIRM_TICKS: u64 = 12;
/// Ticks a dead NPC slot stays counted (its last hits arrive together).
const KILL_DEDUP_TICKS: u64 = 16;
/// Magic stat id (server order).
const MAGIC_STAT: u8 = 6;

/// One NPC damage mask as decoded from NPC_INFO.
#[derive(Debug, Clone, PartialEq)]
pub struct NpcHit {
    pub slot: u16,
    pub npc_type: u16,
    pub damage: u8,
    pub hit_type: u8,
    pub hp: u8,
    pub max_hp: u8,
    pub tick: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunEvents {
    /// Confirmed casts per spell button component.
    pub casts: BTreeMap<u16, u64>,
    /// Kills per NPC type.
    pub kills: BTreeMap<u16, u64>,
    /// Damage dealt per NPC type (hitpoints taken off the target).
    pub damage: BTreeMap<u16, u64>,
    pending_cast: Option<(u16, u64)>,
    target: Option<u16>,
    last_magic_xp: Option<i32>,
    dead_slots: Vec<(u16, u64)>,
}

impl RunEvents {
    /// A spell button was sent at `tick`.
    pub fn note_cast(&mut self, com: u16, tick: u64) {
        self.pending_cast = Some((com, tick));
    }

    /// An interface button was sent at `tick`: a teleport is a spell
    /// button too. It never displaces a spell already waiting for its XP
    /// (a dialogue click during a cast).
    pub fn note_button(&mut self, com: u16, tick: u64) {
        if self.pending_cast.is_none_or(|(_, sent)| tick > sent + CAST_CONFIRM_TICKS) {
            self.pending_cast = Some((com, tick));
        }
    }

    /// The NPC the bot targets now (attack, spell, any NPC op).
    pub fn note_target(&mut self, slot: u16) {
        self.target = Some(slot);
    }

    /// One observation: magic XP now and the NPC hits seen since the last.
    pub fn observe(&mut self, tick: u64, magic_xp: Option<i32>, hits: &[NpcHit]) {
        if let Some(xp) = magic_xp {
            let rose = self.last_magic_xp.is_some_and(|last| xp > last);
            self.last_magic_xp = Some(xp);
            if rose {
                if let Some((com, sent)) = self.pending_cast.take() {
                    if tick <= sent + CAST_CONFIRM_TICKS {
                        *self.casts.entry(com).or_insert(0) += 1;
                    }
                }
            }
        }
        if self.pending_cast.is_some_and(|(_, sent)| tick > sent + CAST_CONFIRM_TICKS) {
            self.pending_cast = None;
        }
        self.dead_slots.retain(|(_, at)| tick <= at + KILL_DEDUP_TICKS);
        for hit in hits {
            if Some(hit.slot) != self.target {
                continue;
            }
            if self.dead_slots.iter().any(|(slot, _)| *slot == hit.slot) {
                continue;
            }
            if hit.damage > 0 {
                *self.damage.entry(hit.npc_type).or_insert(0) += u64::from(hit.damage);
            }
            if hit.hp == 0 && hit.max_hp > 0 {
                *self.kills.entry(hit.npc_type).or_insert(0) += 1;
                self.dead_slots.push((hit.slot, hit.tick));
            }
        }
    }

    /// Counts since `start` (a copy taken at run start), per key.
    pub fn since(&self, start: &RunEvents) -> RunEvents {
        let diff = |now: &BTreeMap<u16, u64>, then: &BTreeMap<u16, u64>| -> BTreeMap<u16, u64> {
            now.iter()
                .filter_map(|(key, count)| {
                    let gained = count.saturating_sub(then.get(key).copied().unwrap_or(0));
                    (gained > 0).then_some((*key, gained))
                })
                .collect()
        };
        RunEvents {
            casts: diff(&self.casts, &start.casts),
            kills: diff(&self.kills, &start.kills),
            damage: diff(&self.damage, &start.damage),
            ..RunEvents::default()
        }
    }
}

/// Magic XP among (stat, xp) pairs.
pub fn magic_xp(stats: &[(u8, i32)]) -> Option<i32> {
    stats.iter().find(|(stat, _)| *stat == MAGIC_STAT).map(|(_, xp)| *xp)
}

#[cfg(test)]
mod tests {
    use super::{NpcHit, RunEvents};

    fn hit(slot: u16, damage: u8, hp: u8, tick: u64) -> NpcHit {
        NpcHit { slot, npc_type: 100, damage, hit_type: 1, hp, max_hp: 5, tick }
    }

    /// A cast counts only once the magic XP pays it, within the window.
    #[test]
    fn casts_count_on_paid_xp_only() {
        let mut events = RunEvents::default();
        events.observe(1, Some(100), &[]);
        events.note_cast(40, 2);
        events.observe(3, Some(100), &[]);
        events.observe(5, Some(155), &[]);
        assert_eq!(events.casts.get(&40), Some(&1));
        // Refused cast: no XP within the window, nothing counted.
        events.note_cast(40, 10);
        events.observe(30, Some(155), &[]);
        events.observe(31, Some(210), &[]);
        assert_eq!(events.casts.get(&40), Some(&1), "late XP never pays a stale cast");
    }

    /// Damage on the target adds up; its hp at 0 is one kill, once;
    /// hits on another NPC never count.
    #[test]
    fn target_hits_make_damage_and_one_kill() {
        let mut events = RunEvents::default();
        events.note_target(7);
        events.observe(1, None, &[hit(7, 2, 3, 1), hit(9, 4, 1, 1)]);
        events.observe(4, None, &[hit(7, 3, 0, 4), hit(7, 0, 0, 4)]);
        assert_eq!(events.damage.get(&100), Some(&5));
        assert_eq!(events.kills.get(&100), Some(&1));
        let start = RunEvents::default();
        assert_eq!(events.since(&start).kills.get(&100), Some(&1));
        assert!(events.since(&events.clone()).kills.is_empty());
    }
}
