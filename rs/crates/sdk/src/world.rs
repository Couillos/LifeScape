//! Live world state, parsed from server frames.
//!
//! Tracks own position, nearby players and NPCs, ground objects and doors,
//! inventory, dialogs, chat and energy. Everything a script needs to decide.

use std::collections::HashMap;

use crate::buffer::{BitReader, Reader};
use crate::error::{Error, Result};
use crate::words::{base37_name, unpack};

const DIRS: [(i32, i32); 8] = [(-1, 1), (0, 1), (1, 1), (-1, 0), (1, 0), (-1, -1), (0, -1), (1, -1)];

fn signed(bits: u32, width: u32) -> i32 {
    let value = bits as i32;
    if value >= 1 << (width - 1) { value - (1 << width) } else { value }
}

/// One line of chat (public say or private message).
#[derive(Debug, Clone)]
pub struct ChatLine {
    pub who: String,
    pub text: String,
    pub color: u8,
    pub effect: u8,
}

/// A nearby player: position plus appearance name when streamed.
#[derive(Debug, Clone)]
pub struct PlayerInfo {
    pub x: i32,
    pub z: i32,
    pub level: i32,
    pub jump: bool,
    pub name: Option<String>,
}

/// A nearby NPC: type plus position (anim when the emote mask arrives).
#[derive(Debug, Clone)]
pub struct NpcInfo {
    pub npc_type: u16,
    pub x: i32,
    pub z: i32,
    pub level: i32,
    pub name: Option<String>,
    pub anim: Option<u16>,
    /// FACE_ENTITY mask (0x4): the npc's target, Client-TS encoding
    /// (< 32768 npc slot, >= 32768 player slot + 32768); None = no target.
    pub face_entity: Option<u16>,
    /// Tick of the last damage mask (0x1/0x10, any hitter, misses too).
    pub hit_tick: Option<u64>,
}

/// Hint arrow target: npc slot, coord, player slot, or cleared.
#[derive(Debug, Clone)]
pub enum Hint {
    Npc(u16),
    Coord(u16, u16, u8),
    Player(u16),
}

/// Open interface: (layer, interface id).
#[derive(Debug, Clone)]
pub struct Modal {
    pub layer: String,
    pub interface_id: u16,
}

pub type Tile = (i32, i32, i32);
pub type LocValue = (u16, u8, u8);

/// Cap on retained `MESSAGE_GAME` lines (and paired ticks): past it the oldest
/// half of BOTH logs is dropped, like `trail` (cap 4000, drain 2000). Sized
/// from t01: 67 517 msg-lines at ~27 chars avg (~1,8 Mo text, ~5 Mo RAM with
/// `String`+tick overhead), woodcut flow ~1 msg every few seconds: 8192 is
/// hours of history (<1 Mo) against cursor windows of 20 (dossier) to 25
/// (heartbeat), script loops re-reading every few pumps.
pub const MESSAGE_CAP: usize = 8192;

/// Game-message (or tick) history with saturating cursors: after a drain, a
/// `seen` cursor taken before it reads empty (as when nothing arrived), never panics.
#[derive(Debug, Clone)]
pub struct MessageHistory<LoggedEntry> {
    entries: Vec<LoggedEntry>,
}

impl<LoggedEntry> Default for MessageHistory<LoggedEntry> {
    fn default() -> MessageHistory<LoggedEntry> {
        MessageHistory { entries: Vec::new() }
    }
}

impl<LoggedEntry> MessageHistory<LoggedEntry> {
    pub fn push(&mut self, entry: LoggedEntry) {
        self.entries.push(entry);
    }

    pub(crate) fn drop_oldest(&mut self, drop_count: usize) {
        let bounded = drop_count.min(self.entries.len());
        self.entries.drain(..bounded);
    }
}

impl<LoggedEntry> std::ops::Deref for MessageHistory<LoggedEntry> {
    type Target = [LoggedEntry];

    fn deref(&self) -> &[LoggedEntry] {
        &self.entries
    }
}

impl<LoggedEntry> std::ops::Index<usize> for MessageHistory<LoggedEntry> {
    type Output = LoggedEntry;

    fn index(&self, position: usize) -> &LoggedEntry {
        &self.entries[position]
    }
}

impl<LoggedEntry> std::ops::Index<std::ops::RangeFrom<usize>> for MessageHistory<LoggedEntry> {
    type Output = [LoggedEntry];

    fn index(&self, range: std::ops::RangeFrom<usize>) -> &[LoggedEntry] {
        let start = range.start.min(self.entries.len());
        &self.entries[start..]
    }
}

impl<LoggedEntry> std::ops::Index<std::ops::Range<usize>> for MessageHistory<LoggedEntry> {
    type Output = [LoggedEntry];

    fn index(&self, range: std::ops::Range<usize>) -> &[LoggedEntry] {
        let kept = self.entries.len();
        let start = range.start.min(kept);
        let end = range.end.clamp(start, kept);
        &self.entries[start..end]
    }
}

impl<LoggedEntry> std::ops::Index<std::ops::RangeFull> for MessageHistory<LoggedEntry> {
    type Output = [LoggedEntry];

    fn index(&self, _whole: std::ops::RangeFull) -> &[LoggedEntry] {
        &self.entries
    }
}

impl<LoggedEntry> std::ops::Index<std::ops::RangeTo<usize>> for MessageHistory<LoggedEntry> {
    type Output = [LoggedEntry];

    fn index(&self, range: std::ops::RangeTo<usize>) -> &[LoggedEntry] {
        let end = range.end.min(self.entries.len());
        &self.entries[..end]
    }
}

/// Live world state fed by server frames.
#[derive(Debug, Clone, Default)]
pub struct World {
    pub base_x: i32,
    pub base_z: i32,
    pub zone_x: i32,
    pub zone_z: i32,
    pub ticks: u64,
    pub level: i32,
    pub x: i32,
    pub z: i32,
    pub energy: u8,
    pub players: HashMap<u16, PlayerInfo>,
    pub player_order: Vec<u16>,
    /// Last known display name per server slot. The server stops sending
    /// appearance blocks for players flickering in and out of view (bank
    /// trips); without this their re-added entry stays nameless and
    /// name-based lookups (trade rendezvous) fail. Appearance masks still
    /// overwrite on arrival.
    pub player_names: HashMap<u16, String>,
    pub npcs: HashMap<u16, NpcInfo>,
    pub npc_order: Vec<u16>,
    pub locs: HashMap<Tile, Option<LocValue>>,
    pub static_locs: HashMap<Tile, LocValue>,
    pub objs: HashMap<(Tile, u16), Option<u32>>,
    pub static_objs: HashMap<(Tile, u16), u32>,
    pub inv: HashMap<u16, HashMap<u8, (u16, u32)>>,
    /// Last contents of an inventory the server stopped transmitting
    /// (`inv_stoptransmit`: the bank at every close). `inv` keeps its
    /// "interface open" meaning; this keeps the last truth readable, so
    /// banked coins and stock stay visible between bank visits (only a
    /// bank visit can change them). A fresh `UPDATE_INV_FULL` replaces it.
    pub inv_closed: HashMap<u16, HashMap<u8, (u16, u32)>>,
    pub inv_cap: HashMap<u16, u8>,
    pub varps: HashMap<u16, i32>,
    pub stats: HashMap<u8, (u8, i32)>,
    pub texts: HashMap<u16, String>,
    pub pending_texts: HashMap<u16, String>,
    pub modal_models: HashMap<u16, u16>,
    pub pending_models: HashMap<u16, u16>,
    pub chat_interfaces: Vec<u16>,
    pub modal: Option<Modal>,
    pub hint: Option<Hint>,
    pub side_flash: Option<u8>,
    pub count_dialog: bool,
    pub player_ops: HashMap<u8, (u8, String)>,
    pub messages: MessageHistory<String>,
    pub message_ticks: MessageHistory<u64>,
    pub chat: Vec<ChatLine>,
    pub private: Vec<ChatLine>,
    pub logged_out: bool,
    /// Our own player slot (`UPDATE_PID`), the pid other entities face.
    pub self_slot: Option<u16>,
}

impl World {
    pub fn new() -> World {
        World { energy: 100, ..World::default() }
    }

    pub fn position(&self) -> Tile {
        (self.level, self.x, self.z)
    }

    /// Dynamic loc at a tile, else the static one (None = removed).
    pub fn loc_at(&self, x: i32, z: i32, level: i32) -> Option<LocValue> {
        let key = (x, z, level);
        if let Some(slot) = self.locs.get(&key) {
            return *slot;
        }
        self.static_locs.get(&key).copied()
    }

    /// The server explicitly cleared this tile (loc removed), as opposed to
    /// a tile that simply has not streamed yet. Scripts must not treat a
    /// cleared tile as "still standing": that is how a chopped tree keeps
    /// being clicked.
    pub fn loc_removed(&self, x: i32, z: i32, level: i32) -> bool {
        matches!(self.locs.get(&(x, z, level)), Some(None))
    }

    pub fn busy(&self) -> bool {
        self.modal.is_some()
    }

    /// Record one `MESSAGE_GAME` line with its tick, dropping the oldest
    /// half of BOTH logs past `MESSAGE_CAP` so they stay paired by index.
    fn push_message(&mut self, text: String) {
        self.messages.push(text);
        self.message_ticks.push(self.ticks);
        if self.messages.len() > MESSAGE_CAP {
            let drop_count = self.messages.len() / 2;
            self.messages.drop_oldest(drop_count);
            self.message_ticks.drop_oldest(drop_count);
        }
    }

    /// Ground items near a tile: live removals shadow the static template.
    pub fn ground_items(&self, x: i32, z: i32, level: i32, radius: i32, obj_id: Option<u16>) -> HashMap<(Tile, u16), u32> {
        let mut merged: HashMap<(Tile, u16), Option<u32>> = HashMap::new();
        for (key, count) in &self.static_objs {
            merged.insert(*key, Some(*count));
        }
        for (key, count) in &self.objs {
            merged.insert(*key, *count);
        }
        let mut found = HashMap::new();
        for ((tile, oid), count) in merged {
            let Some(count) = count else { continue };
            if tile.2 != level || obj_id.is_some_and(|wanted| wanted != oid) {
                continue;
            }
            if (tile.0 - x).abs() + (tile.1 - z).abs() <= radius {
                found.insert((tile, oid), count);
            }
        }
        found
    }

    /// Feed one decoded server frame into the live state.
    pub fn feed(&mut self, name: &str, body: &[u8]) -> Result<()> {
        let mut zone = Reader::new(body);
        match name {
            "REBUILD_NORMAL" => {
                self.zone_x = i32::from(zone.u16()?);
                self.zone_z = i32::from(zone.u16()?);
                self.base_x = (self.zone_x - 6) << 3;
                self.base_z = (self.zone_z - 6) << 3;
            }
            "PLAYER_INFO" => self.player_info(body)?,
            "UPDATE_PID" => self.self_slot = Some(zone.u16()?),
            "NPC_INFO" => self.npc_info(body)?,
            "UPDATE_ZONE_PARTIAL_FOLLOWS" => {
                self.set_zone(i32::from(zone.u8()?), i32::from(zone.u8()?));
            }
            "UPDATE_ZONE_FULL_FOLLOWS" => {
                self.set_zone(i32::from(zone.u8()?), i32::from(zone.u8()?));
                self.reset_zone();
            }
            "UPDATE_ZONE_PARTIAL_ENCLOSED" => self.zone_enclosed(body)?,
            "UPDATE_INV_FULL" => self.inv_full(body)?,
            "UPDATE_INV_PARTIAL" => self.inv_partial(body)?,
            "UPDATE_INV_STOP_TRANSMIT" => {
                let com = zone.u16()?;
                if let Some(items) = self.inv.remove(&com) {
                    self.inv_closed.insert(com, items);
                }
            }
            "UPDATE_STAT" => {
                let index = zone.u8()?;
                let exp = i32::from_be_bytes(zone.raw(4)?[..4].try_into().map_err(|_| crate::buffer::ReadError("stat exp".to_string()))?);
                let level = zone.u8()?;
                self.stats.insert(index, (level, exp));
            }
            "UPDATE_RUNENERGY" => self.energy = zone.u8()?,
            "MESSAGE_GAME" => self.push_message(zone.jstr(10)?),
            "HINT_ARROW" => self.hint_arrow(body)?,
            "TUT_FLASH" => self.side_flash = Some(zone.u8()?),
            "MESSAGE_PRIVATE" => {
                let sender = base37_name(u64::from_be_bytes(zone.raw(8)?[..8].try_into().map_err(|_| crate::buffer::ReadError("pm sender".to_string()))?));
                zone.raw(4)?;
                zone.u8()?;
                let rest = zone.raw(zone.remaining())?;
                self.private.push(ChatLine { who: sender, text: unpack(&rest), color: 0, effect: 0 });
            }
            "IF_OPENMAIN" => self.open_modal("main", body)?,
            "IF_OPENCHAT" => self.open_modal("chat", body)?,
            "IF_OPENSIDE" => self.open_modal("side", body)?,
            "IF_OPENMAIN_SIDE" => self.open_modal("main_side", body)?,
            "IF_OPENOVERLAY" => self.open_modal("overlay", body)?,
            "IF_CLOSE" => {
                self.modal = None;
                self.texts.clear();
                self.pending_texts.clear();
                self.modal_models.clear();
                self.pending_models.clear();
                self.chat_interfaces.clear();
            }
            "IF_SETTEXT" => {
                let com = zone.u16()?;
                let value = zone.jstr(10)?;
                self.texts.insert(com, value.clone());
                self.pending_texts.insert(com, value);
            }
            "IF_SETOBJECT" => {
                let com = zone.u16()?;
                let obj_id = zone.u16()?;
                zone.u16()?;
                self.modal_models.insert(com, obj_id);
                self.pending_models.insert(com, obj_id);
            }
            "P_COUNTDIALOG" => self.count_dialog = true,
            "SET_PLAYER_OP" => {
                let slot = zone.u8()?;
                let op = zone.u8()?;
                let text = zone.jstr(10)?;
                self.player_ops.insert(slot, (op, text));
            }
            "VARP_SMALL" => {
                let index = zone.u16()?;
                self.varps.insert(index, i32::from(zone.u8()?));
            }
            "VARP_LARGE" => {
                let index = zone.u16()?;
                self.varps.insert(index, zone.i32()?);
            }
            "LOGOUT" => self.logged_out = true,
            "OBJ_ADD" | "OBJ_DEL" | "OBJ_COUNT" | "OBJ_REVEAL" | "LOC_ADD_CHANGE" | "LOC_DEL" | "LOC_ANIM" | "LOC_MERGE"
            | "MAP_ANIM" | "MAP_PROJANIM" => {
                let mut stream = Reader::new(body);
                let opcode = match name {
                    "OBJ_ADD" => 81,
                    "OBJ_DEL" => 52,
                    "OBJ_COUNT" => 95,
                    "OBJ_REVEAL" => 219,
                    "LOC_ADD_CHANGE" => 138,
                    "LOC_DEL" => 173,
                    "LOC_ANIM" => 48,
                    "LOC_MERGE" => 176,
                    "MAP_ANIM" => 85,
                    _ => 107,
                };
                self.zone_message(opcode, &mut stream)?;
            }
            "CHAT_FILTER_SETTINGS" | "RESET_CLIENT_VARCACHE" | "RESET_ANIMS" | "CAM_LOOKAT" | "CAM_SHAKE" | "CAM_MOVETO"
            | "CAM_RESET" | "LAST_LOGIN_INFO" | "FRIENDLIST_LOADED" | "UPDATE_FRIENDLIST"
            | "UPDATE_IGNORELIST" | "UNSET_MAP_FLAG" | "UPDATE_RUNWEIGHT" | "UPDATE_REBOOT_TIMER" | "SET_MULTIWAY"
            | "IF_SETTAB" | "IF_SETTAB_ACTIVE" | "IF_SETCOLOUR" | "IF_SETHIDE" | "IF_SETMODEL" | "IF_SETANIM"
            | "IF_SETPLAYERHEAD" | "IF_SETNPCHEAD" | "IF_SETPOSITION" | "IF_SETSCROLLPOS" | "SYNTH_SOUND" | "MIDI_SONG"
            | "MIDI_JINGLE" | "TUT_OPEN" | "MINIMAP_TOGGLE" => {}
            _ => {}
        }
        Ok(())
    }

    fn hint_arrow(&mut self, body: &[u8]) -> Result<()> {
        let mut arrow = Reader::new(body);
        match arrow.u8()? {
            1 => self.hint = Some(Hint::Npc(arrow.u16()?)),
            kind @ 2..=6 => self.hint = Some(Hint::Coord(arrow.u16()?, arrow.u16()?, kind)),
            10 => self.hint = Some(Hint::Player(arrow.u16()?)),
            255 => self.hint = None,
            _ => {}
        }
        Ok(())
    }

    fn open_modal(&mut self, kind: &str, body: &[u8]) -> Result<()> {
        let mut reader = Reader::new(body);
        let interface_id = if kind == "main_side" {
            reader.u16()?;
            reader.u16()?
        } else {
            reader.u16()?
        };
        // The server closes the overlay with component -1, sent unsigned
        // (65535): `openMainOverlay(-1)` → `IfOpenOverlay(p2)`. That is a
        // close, not a dialogue: without this the bot believes a modal is
        // open forever (e.g. after accepting a level-up splash) and every
        // chop aborts on a phantom dialog.
        if kind == "overlay" && interface_id == 65535 {
            self.modal = None;
            return Ok(());
        }
        self.modal = Some(Modal { layer: kind.to_string(), interface_id });
        if kind == "chat" {
            if !self.chat_interfaces.contains(&interface_id) {
                self.chat_interfaces.push(interface_id);
            }
            let skip = self.chat_interfaces.len().saturating_sub(8);
            self.chat_interfaces.drain(..skip);
        }
        // Only the batch staged just before the open survives: option texts
        // from the previous dialogue disappear with it.
        self.texts = self.pending_texts.clone();
        if kind != "chat" {
            self.modal_models = self.pending_models.clone();
        }
        self.pending_texts.clear();
        self.chat_interfaces.clear();
        Ok(())
    }

    fn set_zone(&mut self, local_x: i32, local_z: i32) {
        self.zone_x = (local_x + self.base_x) >> 3;
        self.zone_z = (local_z + self.base_z) >> 3;
    }

    /// Full follows: the server "completely resets the client zone to
    /// default — no objs and the original locs" (Zone.ts writeFullFollows,
    /// Client.ts clears groundObj and reverts locChanges), then re-sends
    /// every obj and changed loc still there. A zone re-enters view this
    /// way, so anything taken or despawned while it was out of view would
    /// otherwise stay a ghost forever. Static spawns are shadowed until
    /// their OBJ_ADD comes back.
    fn reset_zone(&mut self) {
        let (min_x, min_z, level) = (self.zone_x << 3, self.zone_z << 3, self.level);
        let inside = |tile: &Tile| tile.2 == level && (min_x..min_x + 8).contains(&tile.0) && (min_z..min_z + 8).contains(&tile.1);
        self.objs.retain(|(tile, _), _| !inside(tile));
        let shadowed: Vec<(Tile, u16)> = self.static_objs.keys().filter(|(tile, _)| inside(tile)).copied().collect();
        for key in shadowed {
            self.objs.insert(key, None);
        }
        self.locs.retain(|tile, _| !inside(tile));
    }

    fn zone_enclosed(&mut self, body: &[u8]) -> Result<()> {
        let mut stream = Reader::new(body);
        let local_x = i32::from(stream.u8()?);
        let local_z = i32::from(stream.u8()?);
        self.set_zone(local_x, local_z);
        while stream.remaining() > 0 {
            let prot = stream.u8()?;
            self.zone_message(prot, &mut stream)?;
        }
        Ok(())
    }

    fn zone_message(&mut self, prot: u8, stream: &mut Reader) -> Result<()> {
        let coord = stream.u8()?;
        let x = (self.zone_x << 3) + i32::from(coord >> 4);
        let z = (self.zone_z << 3) + i32::from(coord & 0xF);
        let key = (x, z, self.level);
        match prot {
            81 => {
                let obj = stream.u16()?;
                self.objs.insert((key, obj), Some(u32::from(stream.u16()?)));
            }
            52 => {
                self.objs.insert((key, stream.u16()?), None);
            }
            95 => {
                let obj = stream.u16()?;
                stream.u16()?;
                self.objs.insert((key, obj), Some(u32::from(stream.u16()?)));
            }
            219 => {
                let obj = stream.u16()?;
                self.objs.insert((key, obj), Some(u32::from(stream.u16()?)));
                stream.u16()?;
            }
            138 => {
                let shape_angle = stream.u8()?;
                self.locs.insert(key, Some((stream.u16()?, shape_angle >> 2, shape_angle & 0x3)));
            }
            173 => {
                stream.u8()?;
                self.locs.insert(key, None);
            }
            48 => {
                stream.u8()?;
                stream.u16()?;
            }
            176 => {
                stream.raw(13)?;
            }
            85 => {
                stream.raw(5)?;
            }
            107 => {
                stream.raw(14)?;
            }
            _ => return Err(crate::buffer::ReadError(format!("unknown zone prot {prot}")).into()),
        }
        Ok(())
    }

    fn inv_full(&mut self, body: &[u8]) -> Result<()> {
        let mut inv = Reader::new(body);
        let com = inv.u16()?;
        let slots = inv.u8()?;
        let mut items = HashMap::new();
        for slot in 0..slots {
            let obj = inv.u16()?;
            let mut count = u32::from(inv.u8()?);
            if count == 255 {
                count = u32::from_be_bytes(inv.raw(4)?[..4].try_into().map_err(|_| crate::buffer::ReadError("inv count".to_string()))?);
            }
            if obj != 0 {
                items.insert(slot, (obj - 1, count));
            }
        }
        self.inv_cap.insert(com, slots);
        self.inv_closed.remove(&com);
        self.inv.insert(com, items);
        Ok(())
    }

    /// Contents of `com`: live while transmitted, else the last known
    /// contents (bank closed), else `None` (never seen this session).
    pub fn inv_known(&self, com: u16) -> Option<&HashMap<u8, (u16, u32)>> {
        self.inv.get(&com).or_else(|| self.inv_closed.get(&com))
    }

    /// Seeds the last known contents of `com` from per-object counts (the
    /// account save read at login: the server loaded it, so it is the
    /// truth until the first live transmit). Never overrides a live or
    /// already known inventory.
    pub fn seed_closed_inv(&mut self, com: u16, counts: &HashMap<u16, u32>) {
        if self.inv.contains_key(&com) || self.inv_closed.contains_key(&com) {
            return;
        }
        let mut objs: Vec<(&u16, &u32)> = counts.iter().filter(|(_, count)| **count > 0).collect();
        objs.sort();
        let items: HashMap<u8, (u16, u32)> =
            objs.into_iter().enumerate().filter_map(|(slot, (obj, count))| u8::try_from(slot).ok().map(|slot| (slot, (*obj, *count)))).collect();
        self.inv_closed.insert(com, items);
    }

    fn inv_partial(&mut self, body: &[u8]) -> Result<()> {
        let mut inv = Reader::new(body);
        let com = inv.u16()?;
        let mut items = self.inv.get(&com).cloned().unwrap_or_default();
        while inv.remaining() > 0 {
            let slot = inv.u8()?;
            let obj = inv.u16()?;
            let mut count = u32::from(inv.u8()?);
            if count == 255 {
                count = u32::from_be_bytes(inv.raw(4)?[..4].try_into().map_err(|_| crate::buffer::ReadError("inv count".to_string()))?);
            }
            if obj != 0 {
                items.insert(slot, (obj - 1, count));
            } else {
                items.remove(&slot);
            }
        }
        self.inv.insert(com, items);
        Ok(())
    }

    // -- player info -------------------------------------------------------

    fn player_info(&mut self, body: &[u8]) -> Result<()> {
        let mut stream = BitReader::new(body);
        self.ticks += 1;
        let mut pending: Vec<Option<u16>> = Vec::new();
        let mut fresh: Vec<u16> = Vec::new();
        self.read_own(&mut stream, &mut pending)?;
        let known = stream.bits(8)? as usize;
        // Rebuild the list from the previous one, like Client-TS
        // `getPlayerPosOldVis` (and `npc_info` here): entry `index` of the
        // packet updates entry `index` of the old list, survivors are
        // re-appended in order and the tail past `known` left the view.
        // Live 2026-11-13: the server empties its list on a build-area
        // rebuild and sends a shorter count; keeping the tail shifted every
        // later update onto the wrong player, and two bots 12 tiles apart
        // at the Varrock square never saw each other (deals 807–810).
        let previous = std::mem::take(&mut self.player_order);
        if known > previous.len() {
            return Err(Error::Protocol(format!("player_info: {known} known players but only {} tracked", previous.len())));
        }
        for gone in &previous[known..] {
            if let Some(left) = self.players.remove(gone) {
                if let Some(name) = left.name {
                    self.player_names.insert(*gone, name);
                }
            }
        }
        for pid in &previous[..known] {
            if self.read_known_player(&mut stream, Some(*pid), &mut pending)? {
                self.player_order.push(*pid);
            }
        }
        while stream.remaining_bits() > 10 {
            let pid = stream.bits(11)? as u16;
            if pid == 2047 {
                break;
            }
            let dx = signed(stream.bits(5)?, 5);
            let dz = signed(stream.bits(5)?, 5);
            let jump = stream.bits(1)? == 1;
            let extended = stream.bits(1)? == 1;
            let name = self.player_names.get(&pid).cloned();
            self.players.insert(pid, PlayerInfo { x: self.x + dx, z: self.z + dz, level: self.level, jump, name });
            if !self.player_order.contains(&pid) {
                self.player_order.push(pid);
            }
            if extended {
                fresh.push(pid);
            }
        }
        self.read_masks(&mut stream, body, true, &pending, &fresh)?;
        Ok(())
    }

    fn read_own(&mut self, stream: &mut BitReader, pending: &mut Vec<Option<u16>>) -> Result<()> {
        if stream.bits(1)? == 0 {
            return Ok(());
        }
        match stream.bits(2)? {
            0 => pending.push(None),
            1 => {
                let (dx, dz) = DIRS[stream.bits(3)? as usize];
                self.x += dx;
                self.z += dz;
                if stream.bits(1)? == 1 {
                    pending.push(None);
                }
            }
            2 => {
                for _ in 0..2 {
                    let (dx, dz) = DIRS[stream.bits(3)? as usize];
                    self.x += dx;
                    self.z += dz;
                }
                if stream.bits(1)? == 1 {
                    pending.push(None);
                }
            }
            _ => {
                let level = stream.bits(2)? as i32;
                let x = self.base_x + stream.bits(7)? as i32;
                let z = self.base_z + stream.bits(7)? as i32;
                stream.bits(1)?;
                // Neither players nor NPCs are cleared here: the server
                // keeps its own lists and drops them in the same packet
                // (a shorter known count right below), as Client-TS
                // expects. Clearing locally desynced the known order.
                self.level = level;
                self.x = x;
                self.z = z;
                if stream.bits(1)? == 1 {
                    pending.push(None);
                }
            }
        }
        Ok(())
    }

    /// One known player's update; false when it left the view (op 3).
    fn read_known_player(&mut self, stream: &mut BitReader, pid: Option<u16>, pending: &mut Vec<Option<u16>>) -> Result<bool> {
        if stream.bits(1)? == 0 {
            return Ok(true);
        }
        match stream.bits(2)? {
            3 => {
                if let Some(pid) = pid {
                    if let Some(gone) = self.players.remove(&pid) {
                        if let Some(name) = gone.name {
                            self.player_names.insert(pid, name);
                        }
                    }
                }
                return Ok(false);
            }
            0 => {
                if pid.is_some() {
                    pending.push(pid);
                }
            }
            kind => {
                if let Some(pid) = pid {
                    if let Some(player) = self.players.get_mut(&pid) {
                        match kind {
                            1 => {
                                let (dx, dz) = DIRS[stream.bits(3)? as usize];
                                player.x += dx;
                                player.z += dz;
                            }
                            _ => {
                                for _ in 0..2 {
                                    let (dx, dz) = DIRS[stream.bits(3)? as usize];
                                    player.x += dx;
                                    player.z += dz;
                                }
                            }
                        }
                    } else if kind == 1 {
                        stream.bits(3)?;
                    } else {
                        stream.bits(6)?;
                    }
                } else if kind == 1 {
                    stream.bits(3)?;
                } else {
                    stream.bits(6)?;
                }
                if stream.bits(1)? == 1 && pid.is_some() {
                    pending.push(pid);
                }
            }
        }
        Ok(true)
    }

    // -- npc info ----------------------------------------------------------

    fn npc_info(&mut self, body: &[u8]) -> Result<()> {
        let mut stream = BitReader::new(body);
        let mut pending: Vec<u16> = Vec::new();
        let mut fresh: Vec<u16> = Vec::new();
        let known = stream.bits(8)? as usize;
        // Rebuild the list from the previous one, like Client-TS
        // `getNpcPosOldVis`: entry `index` of the packet updates entry
        // `index` of the old list, survivors are re-appended in order and
        // the tail past `known` left the view.
        let previous = std::mem::take(&mut self.npc_order);
        if known > previous.len() {
            return Err(Error::Protocol(format!("npc_info: {known} known npcs but only {} tracked", previous.len())));
        }
        for gone in &previous[known..] {
            self.npcs.remove(gone);
        }
        for nid in &previous[..known] {
            if self.read_known_npc(&mut stream, *nid, &mut pending)? {
                self.npc_order.push(*nid);
            }
        }
        while stream.remaining_bits() > 21 {
            let nid = stream.bits(14)? as u16;
            if nid == 16383 {
                break;
            }
            let ntype = stream.bits(11)? as u16;
            let dx = signed(stream.bits(5)?, 5);
            let dz = signed(stream.bits(5)?, 5);
            let jump = stream.bits(1)? == 1;
            let extended = stream.bits(1)? == 1;
            let _ = jump;
            self.npcs.insert(
                nid,
                NpcInfo { npc_type: ntype, x: self.x + dx, z: self.z + dz, level: self.level, name: None, anim: None, face_entity: None, hit_tick: None },
            );
            if !self.npc_order.contains(&nid) {
                self.npc_order.push(nid);
            }
            if extended {
                fresh.push(nid);
            }
        }
        self.read_masks(&mut stream, body, false, &pending.iter().map(|nid| Some(*nid)).collect::<Vec<_>>(), &fresh)?;
        Ok(())
    }

    /// Reads one entry of the known list; `Ok(false)` when the server
    /// removed it (the caller drops it from the rebuilt order).
    /// Reads one entry of the known list; `Ok(false)` when the server
    /// removed it (the caller drops it from the rebuilt order).
    fn read_known_npc(&mut self, stream: &mut BitReader, nid: u16, pending: &mut Vec<u16>) -> Result<bool> {
        if stream.bits(1)? == 0 {
            return Ok(true);
        }
        let kind = stream.bits(2)?;
        if kind == 3 {
            self.npcs.remove(&nid);
            return Ok(false);
        }
        if kind != 0 {
            let steps = if kind == 1 { 1 } else { 2 };
            for _ in 0..steps {
                let (dx, dz) = DIRS[stream.bits(3)? as usize];
                if let Some(npc) = self.npcs.get_mut(&nid) {
                    npc.x += dx;
                    npc.z += dz;
                }
            }
        }
        if kind == 0 || stream.bits(1)? == 1 {
            pending.push(nid);
        }
        Ok(true)
    }

    // -- masks -------------------------------------------------------------

    fn read_masks(
        &mut self,
        stream: &mut BitReader,
        body: &[u8],
        player: bool,
        pending: &[Option<u16>],
        fresh: &[u16],
    ) -> Result<()> {
        if stream.remaining_bits() == 0 {
            return Ok(());
        }
        stream.align_byte()?;
        let offset = stream.bit_pos() / 8;
        if offset >= body.len() {
            return Ok(());
        }
        let mut data = Reader::new(&body[offset..]);
        let mut order: Vec<Option<u16>> = pending.to_vec();
        order.extend(fresh.iter().map(|id| Some(*id)));
        for pid in order {
            self.read_mask_set(&mut data, pid, player)?;
        }
        Ok(())
    }

    fn read_mask_set(&mut self, data: &mut Reader, pid: Option<u16>, player: bool) -> Result<()> {
        let mut header = u32::from(data.u8()?);
        if player && header & 0x80 != 0 {
            // Server writes ip2(masks | BIG): 16-bit little endian, so the
            // flag is on the low byte and the high bits come second.
            header = (header | (u32::from(data.u8()?) << 8)) & !0x80;
        }
        let order: &[u32] = if player {
            &[0x1, 0x2, 0x4, 0x8, 0x10, 0x20, 0x40, 0x100, 0x200, 0x400]
        } else {
            &[0x1, 0x2, 0x4, 0x8, 0x10, 0x20, 0x40, 0x80]
        };
        for bit in order {
            if header & bit != 0 {
                self.read_block(data, *bit, pid, player)?;
            }
        }
        Ok(())
    }

    fn read_block(&mut self, data: &mut Reader, bit: u32, pid: Option<u16>, player: bool) -> Result<()> {
        if player && bit == 0x1 {
            self.read_appearance(data, pid)?;
        } else if !player && bit == 0x20 {
            if let Some(nid) = pid {
                if let Some(npc) = self.npcs.get_mut(&nid) {
                    npc.npc_type = data.u16()?;
                } else {
                    data.u16()?;
                }
            } else {
                data.u16()?;
            }
        } else if !player && bit == 0x2 {
            // NpcInfoAnim: u16 seq + u8 delay (the mime rescue reads it).
            let anim = data.u16()?;
            data.u8()?;
            if let Some(nid) = pid {
                if let Some(npc) = self.npcs.get_mut(&nid) {
                    npc.anim = Some(anim);
                }
            }
        } else if !player && bit == 0x4 {
            let entity = data.u16()?;
            if let Some(npc) = pid.and_then(|nid| self.npcs.get_mut(&nid)) {
                npc.face_entity = (entity != 65535).then_some(entity);
            }
        } else if !player && (bit == 0x1 || bit == 0x10) {
            // NpcInfoDamage(2): damage, type, hp, max hp (4 bytes).
            data.raw(4)?;
            let now = self.ticks;
            if let Some(npc) = pid.and_then(|nid| self.npcs.get_mut(&nid)) {
                npc.hit_tick = Some(now);
            }
        } else if player && bit == 0x2 {
            data.raw(3)?;
        } else if bit == 0x4 {
            data.u16()?;
        } else if bit == 0x8 {
            let who = self.mask_who(pid, player);
            let text = data.jstr(10)?;
            self.chat.push(ChatLine { who, text, color: 0, effect: 0 });
        } else if bit == 0x10 || bit == 0x400 {
            data.raw(4)?;
        } else if bit == 0x1 {
            data.raw(4)?;
        } else if player && bit == 0x20 {
            data.raw(4)?;
        } else if player && bit == 0x40 {
            let color = data.u8()?;
            let effect = data.u8()?;
            data.u8()?;
            let length = data.u8()? as usize;
            let who = self.mask_who(pid, player);
            let text = unpack(&data.raw(length)?);
            self.chat.push(ChatLine { who, text, color, effect });
        } else if bit == 0x40 {
            data.raw(6)?;
        } else if bit == 0x100 {
            data.raw(6)?;
        } else if bit == 0x200 {
            data.raw(9)?;
        } else if bit == 0x80 {
            data.raw(4)?;
        }
        Ok(())
    }

    fn mask_who(&self, pid: Option<u16>, player: bool) -> String {
        if pid.is_none() || !player {
            return "me".to_string();
        }
        let pid = pid.unwrap_or(0);
        self.players.get(&pid).and_then(|entry| entry.name.clone()).unwrap_or_else(|| format!("player{pid}"))
    }

    fn read_appearance(&mut self, data: &mut Reader, pid: Option<u16>) -> Result<()> {
        let start = data.pos;
        let length = usize::from(data.u8()?);
        data.u8()?;
        data.u8()?;
        if data.peek_u8()? == 0xFF {
            data.u16()?;
            data.u16()?;
        } else {
            for _ in 0..12 {
                if data.peek_u8()? == 0 {
                    data.pos += 1;
                } else {
                    data.pos += 2;
                }
            }
        }
        data.raw(5)?;
        data.raw(14)?;
        let name = base37_name(data.u64()?);
        data.u8()?;
        data.u16()?;
        // The block is length-prefixed: the length is authoritative, so a
        // misread field can never shift the following mask bytes.
        data.pos = start + 1 + length;
        if let Some(pid) = pid {
            if let Some(player) = self.players.get_mut(&pid) {
                player.name = Some(name.clone());
                self.player_names.insert(pid, name);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "world_tests.rs"]
mod tests;
