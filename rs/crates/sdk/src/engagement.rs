//! Who an NPC is fighting, read from its update masks: FACE_ENTITY
//! (its target; Client-TS `faceEntity`, >= 32768 is a player slot +
//! 32768, set by the engine's `PathingEntity.setFaceEntity` from the
//! npc's interaction target) and the damage masks (any hit, misses too).
//! Mirrors the server refusal "Someone else is fighting that."
//! (player_combat.rs2 `player_in_combat_check`: the npc was hit in the
//! last 8 ticks by another player), so a hunt skips the foe before
//! walking to it instead of after the refused click.

#![forbid(unsafe_code)]

use crate::client::Bot;
use crate::world::{NpcInfo, World};

/// Server window of an npc's last combat (`%npc_lastcombat + 8`).
pub const NPC_COMBAT_WINDOW_TICKS: u64 = 8;

/// First FACE_ENTITY value naming a player (Client-TS: `faceEntity >= 32768`).
const PLAYER_FACE_BASE: u16 = 32768;

/// Player slot the npc targets, if it targets a player.
pub fn targeted_player(npc: &NpcInfo) -> Option<u16> {
    npc.face_entity.filter(|entity| *entity >= PLAYER_FACE_BASE).map(|entity| entity - PLAYER_FACE_BASE)
}

/// Pure verdict: the npc fights a player who is not us. It targets
/// another player slot, or it took a hit within the server window while
/// targeting no player (an attacker it does not retaliate against).
/// With our slot unknown (`UPDATE_PID` not seen) any player target
/// counts as another player. An npc targeting us is never "other".
pub fn engaged_by_other(npc: &NpcInfo, self_slot: Option<u16>, now: u64) -> bool {
    match targeted_player(npc) {
        Some(target) => self_slot != Some(target),
        None => npc.hit_tick.is_some_and(|tick| now.saturating_sub(tick) < NPC_COMBAT_WINDOW_TICKS),
    }
}

impl World {
    /// True when the npc in `slot` is fighting another player (see
    /// `engaged_by_other`); false for an unknown slot.
    pub fn npc_engaged_by_other(&self, slot: u16) -> bool {
        self.npcs.get(&slot).is_some_and(|npc| engaged_by_other(npc, self.self_slot, self.ticks))
    }
}

impl Bot {
    /// True when the npc in `slot` is fighting another player: a hunt
    /// skips it instead of walking up to a refused click.
    pub fn npc_engaged_by_other(&self, slot: u16) -> bool {
        self.world.npc_engaged_by_other(slot)
    }
}

#[cfg(test)]
mod tests {
    use super::{engaged_by_other, targeted_player, NPC_COMBAT_WINDOW_TICKS};
    use crate::world::{NpcInfo, World};

    fn chicken(face_entity: Option<u16>, hit_tick: Option<u64>) -> NpcInfo {
        NpcInfo { npc_type: 41, x: 3230, z: 3298, level: 0, name: None, anim: None, face_entity, hit_tick }
    }

    #[test]
    fn face_entity_decodes_players_only() {
        assert_eq!(targeted_player(&chicken(Some(32768 + 12), None)), Some(12));
        assert_eq!(targeted_player(&chicken(Some(40), None)), None, "npc slot, not a player");
        assert_eq!(targeted_player(&chicken(None, None)), None);
    }

    /// Another player's foe is skipped, ours and idle ones are not; a
    /// hit with no player target counts only inside the server window.
    #[test]
    fn engaged_by_other_reads_target_and_recent_hits() {
        let me = Some(7);
        assert!(engaged_by_other(&chicken(Some(32768 + 12), None), me, 100), "faces player 12");
        assert!(!engaged_by_other(&chicken(Some(32768 + 7), Some(99)), me, 100), "faces us: our fight");
        assert!(!engaged_by_other(&chicken(None, None), me, 100), "idle");
        assert!(engaged_by_other(&chicken(None, Some(95)), me, 100), "hit 5 ticks ago, no target");
        assert!(!engaged_by_other(&chicken(None, Some(100 - NPC_COMBAT_WINDOW_TICKS)), me, 100), "window over");
        assert!(!engaged_by_other(&chicken(Some(40), None), me, 100), "faces an npc, no hit");
        assert!(engaged_by_other(&chicken(Some(32768 + 7), None), None, 100), "self unknown: any player counts");
    }

    /// Wire path: UPDATE_PID sets our slot, an NPC_INFO face-entity mask
    /// on a fresh npc lands on it (bits as the engine writes them).
    #[test]
    fn masks_feed_the_verdict() {
        let mut world = World::new();
        world.feed("UPDATE_PID", &[0, 7, 0]).expect("pid");
        assert_eq!(world.self_slot, Some(7));
        world.x = 3230;
        world.z = 3298;
        // 0 known npcs; one fresh: nid 5 (14 bits), type 41 (11), dx 0
        // (5), dz 0 (5), jump 0, extended 1; end marker 16383; then the
        // mask block: 0x04 + face entity u16 (player 12).
        let mut bits: Vec<u8> = Vec::new();
        let mut push = |value: u32, width: u32| {
            for shift in (0..width).rev() {
                bits.push(((value >> shift) & 1) as u8);
            }
        };
        push(0, 8);
        push(5, 14);
        push(41, 11);
        push(0, 5);
        push(0, 5);
        push(0, 1);
        push(1, 1);
        push(16383, 14);
        while bits.len() % 8 != 0 {
            bits.push(0);
        }
        let mut body: Vec<u8> = bits.chunks(8).map(|byte| byte.iter().fold(0u8, |acc, bit| (acc << 1) | bit)).collect();
        let face = 32768u16 + 12;
        body.extend([0x04, (face >> 8) as u8, (face & 0xff) as u8]);
        world.feed("NPC_INFO", &body).expect("npc info");
        assert_eq!(world.npcs.get(&5).and_then(|npc| npc.face_entity), Some(face));
        assert!(world.npc_engaged_by_other(5), "fights player 12, not us (7)");
        assert!(!world.npc_engaged_by_other(6), "unknown slot");
    }
}
