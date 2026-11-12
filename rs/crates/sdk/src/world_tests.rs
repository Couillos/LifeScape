//! Unit tests for `world.rs` (split out to keep the file under 1000 lines).

use super::World;

/// MSB-first bit packer mirroring the server's `pBit` (test-only).
struct BitPacker {
    bytes: Vec<u8>,
    bit_len: usize,
}

impl BitPacker {
    fn new() -> BitPacker {
        BitPacker { bytes: Vec::new(), bit_len: 0 }
    }

    fn put(&mut self, width: u32, value: u32) {
        for shift in (0..width).rev() {
            if self.bit_len % 8 == 0 {
                self.bytes.push(0);
            }
            if (value >> shift) & 1 == 1 {
                let last = self.bytes.len() - 1;
                self.bytes[last] |= 0x80 >> (self.bit_len % 8);
            }
            self.bit_len += 1;
        }
    }
}

fn world_with_npcs(ids: &[u16]) -> World {
    let mut world = World::new();
    for (offset, nid) in ids.iter().enumerate() {
        world.npcs.insert(*nid, super::NpcInfo { npc_type: 1, x: 3200 + offset as i32, z: 3200, level: 0, name: None, anim: None });
        world.npc_order.push(*nid);
    }
    world
}

/// A removal in the middle of the known list must not shift the
/// updates of the following NPCs (server order = previous list order,
/// Client-TS `getNpcPosOldVis` reads `npcIds[i]` from the old list).
#[test]
fn npc_removal_mid_list_keeps_following_updates_aligned() {
    let mut world = world_with_npcs(&[10, 11, 12]);
    let mut packet = BitPacker::new();
    packet.put(8, 3);
    // npc 10: removed.
    packet.put(1, 1);
    packet.put(2, 3);
    // npc 11: walks east (DIRS[4] = (1, 0)), no mask.
    packet.put(1, 1);
    packet.put(2, 1);
    packet.put(3, 4);
    packet.put(1, 0);
    // npc 12: unchanged.
    packet.put(1, 0);
    packet.put(14, 16383);
    world.feed("NPC_INFO", &packet.bytes).unwrap();
    assert!(!world.npcs.contains_key(&10), "npc 10 removed");
    assert_eq!(world.npcs[&11].x, 3202, "npc 11 moved east by one");
    assert_eq!(world.npcs[&12].x, 3202, "npc 12 untouched");
    assert_eq!(world.npc_order, vec![11, 12]);
}

/// A shorter known count drops the tail of the list (out of view).
#[test]
fn npc_known_count_shorter_than_list_drops_tail() {
    let mut world = world_with_npcs(&[10, 11, 12]);
    let mut packet = BitPacker::new();
    packet.put(8, 1);
    packet.put(1, 0);
    packet.put(14, 16383);
    world.feed("NPC_INFO", &packet.bytes).unwrap();
    assert_eq!(world.npc_order, vec![10]);
    assert!(world.npcs.contains_key(&10));
    assert!(!world.npcs.contains_key(&11), "tail npc 11 dropped");
    assert!(!world.npcs.contains_key(&12), "tail npc 12 dropped");
}

/// Names survive a view flicker: removal stashes the display name,
/// re-entry without an appearance block restores it (bank trips must
/// not blind a rendezvous), and the next appearance still overwrites.
#[test]
fn player_names_survive_view_flicker() {
    let mut world = World::new();
    world.players.insert(2, super::PlayerInfo { x: 1, z: 1, level: 0, jump: false, name: Some("t02".to_string()) });
    // Removal (out of view) stashes the name.
    if let Some(gone) = world.players.remove(&2) {
        if let Some(name) = gone.name {
            world.player_names.insert(2, name);
        }
    }
    assert_eq!(world.player_names.get(&2).map(String::as_str), Some("t02"));
    // Re-entry restores it (mirrors the player_info insert path).
    let restored = world.player_names.get(&2).cloned();
    world.players.insert(2, super::PlayerInfo { x: 2, z: 2, level: 0, jump: false, name: restored });
    assert_eq!(world.players.get(&2).and_then(|entry| entry.name.clone()).as_deref(), Some("t02"));
}

#[test]
fn rebuild_sets_base() {
    let mut world = World::new();
    // zone (50, 50) -> base (44 << 3, 44 << 3).
    world.feed("REBUILD_NORMAL", &[0, 50, 0, 50]).unwrap();
    assert_eq!((world.base_x, world.base_z), (44 << 3, 44 << 3));
}

#[test]
fn ground_items_merge_live_static() {
    let mut world = World::new();
    world.static_objs.insert(((1, 2, 0), 5), 3);
    world.objs.insert(((1, 2, 0), 5), None);
    assert!(world.ground_items(1, 2, 0, 15, None).is_empty());
}

#[test]
fn overlay_minus_one_closes_modal() {
    let mut world = World::new();
    // Level-up splash opens...
    world.feed("IF_OPENOVERLAY", &[0, 193]).unwrap();
    assert!(world.modal.is_some(), "splash opens the modal");
    // ...then the server closes it with component -1, sent unsigned.
    world.feed("IF_OPENOVERLAY", &[0xFF, 0xFF]).unwrap();
    assert!(world.modal.is_none(), "65535 overlay must clear, not stick");
    // A real overlay still opens normally afterwards.
    world.feed("IF_OPENOVERLAY", &[0, 193]).unwrap();
    assert!(world.modal.is_some(), "real overlays still open");
}

#[test]
fn message_burst_stays_capped_and_paired() {
    use super::MESSAGE_CAP;
    let mut world = World::new();
    for index in 0..20_000u64 {
        world.ticks = index;
        world.push_message(format!("line {index}"));
    }
    let kept = world.messages.len();
    assert!(kept <= MESSAGE_CAP, "burst capped, got {kept}");
    assert!(kept > MESSAGE_CAP / 2, "drain keeps the newest half, got {kept}");
    assert_eq!(world.message_ticks.len(), kept, "ticks stay paired");
    for (offset, tick) in world.message_ticks.iter().enumerate() {
        let line_number = 20_000 - kept as u64 + offset as u64;
        assert_eq!(*tick, line_number, "tick paired at {offset}");
        assert_eq!(world.messages[offset], format!("line {line_number}"));
    }
}

#[test]
fn stale_cursor_reads_empty_after_drain() {
    let mut world = World::new();
    for index in 0..8_000u64 {
        world.ticks = index;
        world.push_message(format!("line {index}"));
    }
    let seen = world.messages.len();
    for index in 8_000..10_000u64 {
        world.ticks = index;
        world.push_message(format!("line {index}"));
    }
    assert!(world.messages.len() < seen, "drain shrank the log");
    assert!(world.messages[seen..].is_empty(), "direct index reads empty");
    assert!(world.message_ticks[seen..].is_empty(), "paired ticks read empty");
    assert!(world.messages.get(seen..).unwrap_or(&[]).is_empty());
    assert!(!world.messages.is_empty(), "newest lines survive");
}

/// A level change (stairs) removes NPCs through NPC_INFO op 3: the world
/// must still hold the old list when that packet arrives.
#[test]
fn npc_list_survives_own_level_change_until_npc_info() {
    let mut world = world_with_npcs(&[10, 11]);
    world.x = 3200;
    world.z = 3200;
    world.base_x = 3200 - 48;
    world.base_z = 3200 - 48;
    let mut players = BitPacker::new();
    players.put(1, 1);
    players.put(2, 3);
    players.put(2, 1);
    players.put(7, 48);
    players.put(7, 48);
    players.put(1, 0);
    players.put(1, 0);
    players.put(8, 0);
    players.put(11, 2047);
    world.feed("PLAYER_INFO", &players.bytes).unwrap();
    assert_eq!(world.level, 1, "climbed one level");
    let mut npcs = BitPacker::new();
    npcs.put(8, 2);
    for _ in 0..2 {
        npcs.put(1, 1);
        npcs.put(2, 3);
    }
    npcs.put(14, 16383);
    world.feed("NPC_INFO", &npcs.bytes).unwrap();
    assert!(world.npcs.is_empty() && world.npc_order.is_empty(), "both removed by op 3");
}

/// The server stops transmitting the bank at every close
/// (`bank.rs2: inv_stoptransmit(bank_main:inv)`): `inv` drops it (the
/// open-interface meaning holds) but the last contents stay known, so
/// banked coins remain visible to the brain and the market ledger.
#[test]
fn closed_bank_stays_known_until_retransmitted() {
    let mut world = World::new();
    let bank_com: u16 = 2006;
    // UPDATE_INV_FULL: com, slot count, then (obj + 1, count) per slot.
    let full = |coins: u8| vec![(bank_com >> 8) as u8, bank_com as u8, 2, 0x03, 0xE4, coins, 0x01, 0x3B, 7];
    world.feed("UPDATE_INV_FULL", &full(200)).expect("full");
    assert!(world.inv.contains_key(&bank_com), "open bank is live");
    world.feed("UPDATE_INV_STOP_TRANSMIT", &[(bank_com >> 8) as u8, bank_com as u8]).expect("stop");
    assert!(!world.inv.contains_key(&bank_com), "closed bank leaves the live inventories");
    let known = world.inv_known(bank_com).expect("last contents kept");
    assert!(known.values().any(|(obj, count)| *obj == 995 && *count == 200), "coins still known: {known:?}");
    // A fresh transmit replaces the closed copy.
    world.feed("UPDATE_INV_FULL", &full(50)).expect("full again");
    assert!(world.inv_closed.is_empty());
    assert!(world.inv_known(bank_com).expect("live").values().any(|(obj, count)| *obj == 995 && *count == 50));
}

/// A login seeds the bank from the save only when nothing is known yet.
#[test]
fn bank_seed_never_overrides_known_contents() {
    let mut world = World::new();
    let counts: std::collections::HashMap<u16, u32> = [(995u16, 427u32), (1739, 3), (314, 0)].into_iter().collect();
    world.seed_closed_inv(2006, &counts);
    let seeded = world.inv_known(2006).expect("seeded");
    assert_eq!(seeded.len(), 2, "empty stacks skipped: {seeded:?}");
    assert!(seeded.values().any(|(obj, count)| *obj == 995 && *count == 427));
    let other: std::collections::HashMap<u16, u32> = [(995u16, 1u32)].into_iter().collect();
    world.seed_closed_inv(2006, &other);
    assert!(world.inv_known(2006).expect("kept").values().any(|(obj, count)| *obj == 995 && *count == 427), "first truth kept");
}
