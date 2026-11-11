//! Bot actions: clicks, items, dialogue, trade, shops and chat.
//!
//! Every wait is a pump loop to an observable state plus a tick bound:
//! never wall-clock sleep.

pub use super::exchange::ExchangeOutcome;

use crate::buffer::Writer;
use crate::client::Bot;
use crate::error::{Error, Result, TravelErrorKind};
use std::collections::HashMap;

use crate::words::to_base37;

fn op_name(kind: &str, op: u8) -> String {
    format!("{kind}{op}")
}

/// OPOBJT payload, split out so the field order locks by test (x, z,
/// obj, spell com, big-endian like every other op).
fn opobjt_bytes(x: i32, z: i32, obj: u16, spell_com: u16) -> Vec<u8> {
    let mut body = Writer::new();
    body.u16(x as u16);
    body.u16(z as u16);
    body.u16(obj);
    body.u16(spell_com);
    body.bytes()
}

/// OPHELDT payload (obj, pack slot, pack com, spell com).
fn opheldt_bytes(obj: u16, slot: u8, com: u16, spell_com: u16) -> Vec<u8> {
    let mut body = Writer::new();
    body.u16(obj);
    body.u16(u16::from(slot));
    body.u16(com);
    body.u16(spell_com);
    body.bytes()
}

/// OPLOCT payload (x, z, loc, spell com).
fn oploct_bytes(x: i32, z: i32, loc: u16, spell_com: u16) -> Vec<u8> {
    let mut body = Writer::new();
    body.u16(x as u16);
    body.u16(z as u16);
    body.u16(loc);
    body.u16(spell_com);
    body.bytes()
}

/// OPPLAYERT payload (slot, spell com), the `OpPlayerTDecoder` order.
fn opplayert_bytes(slot: u16, spell_com: u16) -> Vec<u8> {
    let mut body = Writer::new();
    body.u16(slot);
    body.u16(spell_com);
    body.bytes()
}

/// OPPLAYERU payload (slot, obj com, use slot, use com), the
/// `OpPlayerUDecoder` order (mirrors `op_npc_u`).
fn opplayeru_bytes(slot: u16, obj_com: u16, use_slot: u16, use_com: u16) -> Vec<u8> {
    let mut body = Writer::new();
    body.u16(slot);
    body.u16(obj_com);
    body.u16(use_slot);
    body.u16(use_com);
    body.bytes()
}


impl Bot {
    pub fn op_loc(&mut self, op: u8, x: i32, z: i32, loc_id: u16) -> Result<()> {
        self.act(format!("op_loc{op} {} id={loc_id} at ({x},{z})", (self.tables.loc_name)(loc_id).unwrap_or_default()));
        let mut body = Writer::new();
        body.u16(x as u16);
        body.u16(z as u16);
        body.u16(loc_id);
        self.send_named(&op_name("OPLOC", op), &body.bytes())
    }

    pub fn op_npc(&mut self, op: u8, slot: u16) -> Result<()> {
        let npc_label = self.world.npcs.get(&slot).map(|npc| (self.tables.npc_name)(npc.npc_type).unwrap_or_default()).unwrap_or_default();
        self.act(format!("op_npc{op} {npc_label} slot={slot}"));
        let mut body = Writer::new();
        body.u16(u16::from(slot));
        self.send_named(&op_name("OPNPC", op), &body.bytes())
    }

    pub fn op_npc_u(&mut self, slot: u16, obj_id: u16, use_slot: u16, com: u16) -> Result<()> {
        self.act(format!("op_npc_u slot={slot} obj={obj_id}"));
        let mut body = Writer::new();
        body.u16(u16::from(slot));
        body.u16(obj_id);
        body.u16(u16::from(use_slot));
        body.u16(com);
        self.send_named("OPNPCU", &body.bytes())
    }

    pub fn op_obj(&mut self, op: u8, x: i32, z: i32, obj_id: u16) -> Result<()> {
        self.act(format!("op_obj{op} {} id={obj_id} at ({x},{z})", (self.tables.obj_name)(obj_id).unwrap_or_default()));
        let mut body = Writer::new();
        body.u16(x as u16);
        body.u16(z as u16);
        body.u16(obj_id);
        self.send_named(&op_name("OPOBJ", op), &body.bytes())
    }

    pub fn op_loc_u(&mut self, x: i32, z: i32, loc_id: u16, obj_id: u16, slot: u16, com: u16) -> Result<()> {
        self.act(format!("op_loc_u {} id={loc_id} + {} at ({x},{z})", (self.tables.loc_name)(loc_id).unwrap_or_default(), (self.tables.obj_name)(obj_id).unwrap_or_default()));
        let mut body = Writer::new();
        body.u16(x as u16);
        body.u16(z as u16);
        body.u16(loc_id);
        body.u16(obj_id);
        body.u16(u16::from(slot));
        body.u16(com);
        self.send_named("OPLOCU", &body.bytes())
    }

    pub fn op_npc_t(&mut self, slot: u16, spell_com: u16) -> Result<()> {
        self.act(format!("op_npc_t slot={slot}"));
        let mut body = Writer::new();
        body.u16(u16::from(slot));
        body.u16(spell_com);
        self.send_named("OPNPCT", &body.bytes())
    }

    /// Cast a spellbook spell on a ground object (telegrab): x, z, obj,
    /// then the spell button com, exactly the `OpObjTDecoder` order.
    pub fn op_obj_t(&mut self, x: i32, z: i32, obj_id: u16, spell_com: u16) -> Result<()> {
        self.act(format!("op_obj_t {} id={obj_id} at ({x},{z})", (self.tables.obj_name)(obj_id).unwrap_or_default()));
        self.send_named("OPOBJT", &opobjt_bytes(x, z, obj_id, spell_com))
    }

    /// Cast a spellbook spell on a carried item (alchemy, superheat,
    /// enchant): obj, pack slot, pack com, then the spell button com,
    /// exactly the `OpHeldTDecoder` order.
    pub fn op_held_t(&mut self, obj: u16, slot: u8, com: u16, spell_com: u16) -> Result<()> {
        self.act(format!("op_held_t {} x1 slot={slot}", (self.tables.obj_name)(obj).unwrap_or_default()));
        self.send_named("OPHELDT", &opheldt_bytes(obj, slot, com, spell_com))
    }

    /// Cast a spellbook spell on a scenery object: x, z, loc, then the
    /// spell button com, exactly the `OpLocTDecoder` order.
    pub fn op_loc_t(&mut self, x: i32, z: i32, loc_id: u16, spell_com: u16) -> Result<()> {
        self.act(format!("op_loc_t {} id={loc_id} at ({x},{z})", (self.tables.loc_name)(loc_id).unwrap_or_default()));
        self.send_named("OPLOCT", &oploct_bytes(x, z, loc_id, spell_com))
    }

    pub fn op_player(&mut self, op: u8, slot: u16) -> Result<()> {
        self.act(format!("op_player{op} slot={slot}"));
        let mut body = Writer::new();
        body.u16(u16::from(slot));
        self.send_named(&op_name("OPPLAYER", op), &body.bytes())
    }

    /// Cast a spellbook spell on a player: slot, then the spell button
    /// com, exactly the client `OPPLAYERT` order (`Client.ts` TGT_PLAYER).
    pub fn op_player_t(&mut self, slot: u16, spell_com: u16) -> Result<()> {
        self.act(format!("op_player_t slot={slot}"));
        self.send_named("OPPLAYERT", &opplayert_bytes(slot, spell_com))
    }

    /// Use an item on a player: slot, obj com, use slot, use com,
    /// exactly the client `OPPLAYERU` order (mirrors `op_npc_u`).
    pub fn op_player_u(&mut self, slot: u16, obj_com: u16, use_slot: u16, use_com: u16) -> Result<()> {
        self.act(format!("op_player_u slot={slot} obj={obj_com}"));
        self.send_named("OPPLAYERU", &opplayeru_bytes(slot, obj_com, use_slot, use_com))
    }

    pub fn op_held(&mut self, op: u8, obj: u16, slot: u8, com: u16) -> Result<()> {
        self.act(format!("op_held{op} {} x1 slot={slot}", (self.tables.obj_name)(obj).unwrap_or_default()));
        let mut body = Writer::new();
        body.u16(obj);
        body.u16(u16::from(slot));
        body.u16(com);
        self.send_named(&op_name("OPHELD", op), &body.bytes())
    }

    pub fn op_held_u(&mut self, obj: u16, slot: u8, com: u16, use_obj: u16, use_slot: u8, use_com: u16) -> Result<()> {
        self.act(format!("op_held_u {} + {}", (self.tables.obj_name)(obj).unwrap_or_default(), (self.tables.obj_name)(use_obj).unwrap_or_default()));
        let mut body = Writer::new();
        body.u16(obj);
        body.u16(u16::from(slot));
        body.u16(com);
        body.u16(use_obj);
        body.u16(u16::from(use_slot));
        body.u16(use_com);
        self.send_named("OPHELDU", &body.bytes())
    }

    pub fn inv_button(&mut self, n: u8, obj: u16, slot: u8, com: u16) -> Result<()> {
        self.act(format!("inv_button{n} {} slot={slot} com={com}", (self.tables.obj_name)(obj).unwrap_or_default()));
        let mut body = Writer::new();
        body.u16(obj);
        body.u16(u16::from(slot));
        body.u16(com);
        self.send_named(&format!("INV_BUTTON{n}"), &body.bytes())
    }

    pub fn inv_button_d(&mut self, com: u16, slot: u8, target: u16, mode: u8) -> Result<()> {
        self.act(format!("inv_button_d com={com} slot={slot}"));
        let mut body = Writer::new();
        body.u16(com);
        body.u16(u16::from(slot));
        body.u16(target);
        body.u8(mode);
        self.send_named("INV_BUTTOND", &body.bytes())
    }

    pub fn if_button(&mut self, com: u16) -> Result<()> {
        self.act(format!("if_button com={com}"));
        let mut body = Writer::new();
        body.u16(com);
        self.send_named("IF_BUTTON", &body.bytes())
    }

    pub fn close_modal(&mut self) -> Result<()> {
        self.act("close_modal".to_string());
        self.send_named("CLOSE_MODAL", &[])
    }

    pub fn talk_to(&mut self, slot: u16) -> Result<()> {
        self.op_npc(1, slot)
    }

    /// Nearest visible NPC whose type name contains `name`.
    pub fn find_npc(&self, name: &str) -> Option<u16> {
        self.find_npcs(name).into_iter().next()
    }

    /// Every visible NPC whose type name contains `name`, nearest first.
    /// One spot can go stale (ghost slot, fished-out side); callers that
    /// need a catch rotate through these instead of hammering one.
    pub fn find_npcs(&self, name: &str) -> Vec<u16> {
        let wanted = name.to_lowercase();
        let mut found: Vec<(i32, u16)> = Vec::new();
        for (nid, npc) in &self.world.npcs {
            let label = (self.tables.npc_name)(npc.npc_type).unwrap_or_default();
            if !label.to_lowercase().contains(&wanted) {
                continue;
            }
            let dist = (npc.x - self.world.x).abs() + (npc.z - self.world.z).abs();
            found.push((dist, *nid));
        }
        found.sort();
        found.into_iter().map(|(_, nid)| nid).collect()
    }

    /// (slot, x, z) for every visible NPC whose type name is in `names`.
    pub fn npcs_named(&self, names: &[&str]) -> Vec<(u16, i32, i32)> {
        let wanted: Vec<String> = names.iter().map(|name| name.to_lowercase()).collect();
        let mut found = Vec::new();
        for (nid, npc) in &self.world.npcs {
            let label = (self.tables.npc_name)(npc.npc_type).unwrap_or_default().to_lowercase();
            if wanted.contains(&label) {
                found.push((*nid, npc.x, npc.z));
            }
        }
        found
    }

    // -- player-to-player trade window (com ids are rev-274 pack data) -----

    pub const TRADE_ACCEPT: u16 = 3420;
    pub const TRADE_CONFIRM_ACCEPT: u16 = 3546;
    pub const TRADE_STATUS: u16 = 3431;
    pub const TRADE_OFFER: u16 = 3415;
    pub const TRADE_PARTNER_OFFER: u16 = 3416;
    pub const TRADE_PACK: u16 = 3322;
    /// 'main', 'confirm' or None from the open window. Interface ids come
    /// from the component tables (the live revision moved the trade side
    /// panel); hardcoded ids are fallback only.
    pub fn trade_modal(&self) -> Option<&'static str> {
        let modal = self.world.modal.as_ref()?;
        let side = (self.tables.com_id)("tradeside").unwrap_or(3321);
        let confirm = (self.tables.com_id)("tradeconfirm").unwrap_or(3443);
        if modal.layer.as_str() == "main_side" && modal.interface_id == side {
            return Some("main");
        }
        if modal.layer.as_str() == "main" && modal.interface_id == confirm {
            return Some("confirm");
        }
        None
    }

    /// Server slot of a nearby player, by (case-insensitive) name.
    pub fn find_player(&self, name: &str) -> Option<u16> {
        let wanted = name.to_lowercase();
        let mut best: Option<(i32, u16)> = None;
        for (pid, player) in &self.world.players {
            let label = player.name.clone().unwrap_or_default();
            if !label.to_lowercase().contains(&wanted) {
                continue;
            }
            let dist = (player.x - self.world.x).abs() + (player.z - self.world.z).abs();
            if best.is_none_or(|(known, _)| dist < known) {
                best = Some((dist, *pid));
            }
        }
        best.map(|(_, pid)| pid)
    }

    /// Every tracked player: (slot, x, z, level, name or null). Diag for
    /// rendezvous misses: distinguishes an empty view from a nameless one.
    pub fn debug_players(&self) -> Vec<(u16, i32, i32, i32, Option<String>)> {
        let mut out: Vec<(u16, i32, i32, i32, Option<String>)> = self
            .world
            .players
            .iter()
            .map(|(pid, player)| (*pid, player.x, player.z, player.level, player.name.clone()))
            .collect();
        out.sort();
        out
    }

    /// Mutual trade request: knocks until the trademain window opens (the
    /// partner must knock back). One knock per quiet wait: rapid re-clicks
    /// cancel the pending engine interaction before it fires.
    pub fn trade_request(&mut self, name: &str, timeout: u64) -> Result<bool> {
        self.act(format!("trade_request {name}"));
        let target = self.world.ticks + timeout;
        while self.world.ticks < target {
            if self.trade_modal().is_some() {
                return Ok(true);
            }
            let Some(pid) = self.find_player(name) else {
                self.pump(5)?;
                continue;
            };
            let far = self.world.players.get(&pid).is_some_and(|player| {
                (player.x - self.world.x).abs() + (player.z - self.world.z).abs() > 1 || player.level != self.world.level
            });
            if far {
                let (px, pz) = self.world.players.get(&pid).map(|player| (player.x, player.z)).unwrap_or((self.world.x, self.world.z));
                let _ = self.approach(px, pz, self.world.level, 1, 750);
            }
            let _ = self.op_player(4, pid);
            for _ in 0..30 {
                self.pump(5)?;
                if self.trade_modal().is_some() {
                    return Ok(true);
                }
            }
        }
        Ok(self.trade_modal().is_some())
    }

    /// Offer `count` of an object from the backpack (inv_button op 1).
    /// A server "can't trade" refusal (untradeable, members in F2P) reads
    /// as Ok(false) instead of burning the timeout: the caller maps it to
    /// its typed refusal.
    pub fn trade_offer(&mut self, obj_id: u16, count: u32, timeout: u64) -> Result<bool> {
        self.act(format!("trade_offer {} x{count}", (self.tables.obj_name)(obj_id).unwrap_or_default()));
        let seen = self.world.messages.len();
        let target = self.world.ticks + timeout;
        while self.world.ticks < target {
            if self.world.modal.is_none() {
                return Ok(false);
            }
            let offered: u32 = self
                .world
                .inv
                .get(&Self::TRADE_OFFER)
                .map(|slots| slots.values().filter(|(held, _)| *held == obj_id).map(|(_, n)| *n).sum())
                .unwrap_or(0);
            if offered >= count {
                return Ok(true);
            }
            let (com, slots) = self.backpack();
            let Some(_com) = com else { return Ok(false) };
            let Some(slot) = slots.iter().find(|(_, (held, _))| *held == obj_id).map(|(slot, _)| *slot) else {
                return Ok(false);
            };
            // Offer in 10s, then 5s, then 1s (ops 3/2/1 on the trade
            // side): one coin per click never reaches a big count.
            let need = count - offered;
            let op = if need >= 10 { 3 } else if need >= 5 { 2 } else { 1 };
            self.inv_button(op, obj_id, slot, Self::TRADE_PACK)?;
            self.pump(5)?;
            if Self::trade_refused(&self.world.messages, seen) {
                return Ok(false);
            }
        }
        let offered: u32 = self
            .world
            .inv
            .get(&Self::TRADE_OFFER)
            .map(|slots| slots.values().filter(|(held, _)| *held == obj_id).map(|(_, n)| *n).sum())
            .unwrap_or(0);
        Ok(offered >= count)
    }

    /// {obj_id: count} offered by the partner, {} when unknown.
    pub fn trade_partner_offer(&self) -> HashMap<u16, u32> {
        let mut out = HashMap::new();
        if let Some(slots) = self.world.inv.get(&Self::TRADE_PARTNER_OFFER) {
            for (held, count) in slots.values() {
                { let total = out.entry(*held).or_insert(0u32); *total = total.saturating_add(*count); }
            }
        }
        out
    }

    pub fn trade_status_text(&self) -> String {
        self.world.texts.get(&Self::TRADE_STATUS).cloned().unwrap_or_default()
    }

    /// Click accept (stage 1 or confirm) until the window moves on.
    pub fn trade_accept(&mut self, timeout: u64) -> Result<bool> {
        self.act("trade_accept".to_string());
        let Some(before) = self.trade_modal() else { return Ok(false) };
        let target = self.world.ticks + timeout;
        while self.world.ticks < target {
            let now = self.trade_modal();
            if now != Some(before) {
                return Ok(true);
            }
            let com = if before == "main" { Self::TRADE_ACCEPT } else { Self::TRADE_CONFIRM_ACCEPT };
            self.if_button(com)?;
            self.pump(10)?;
        }
        let now = self.trade_modal();
        Ok(now.is_none() || now != Some(before))
    }

    pub fn trade_close(&mut self) -> Result<bool> {
        self.act("trade_close".to_string());
        if self.world.modal.is_some() {
            self.close_modal()?;
            self.pump(5)?;
        }
        Ok(self.world.modal.is_none())
    }


    /// Pick up ground items: walks to a neighbor tile, clicks, waits for
    /// the carried total to grow. Tick-bounded, no sleep.
    pub fn take(&mut self, obj_id: Option<u16>, name: Option<&str>, timeout: u64) -> Result<bool> {
        self.act(format!("take {:?}", name));
        let mut wanted = obj_id;
        if let Some(name) = name {
            let needle = name.to_lowercase();
            wanted = (self.tables.find_obj_by_name)(&needle).or(wanted);
        }
        let deadline = self.world.ticks + timeout;
        let mut items = HashMap::new();
        while self.world.ticks < deadline && items.is_empty() {
            self.pump(3)?;
            items = self.world.ground_items(self.world.x, self.world.z, self.world.level, 15, wanted);
        }
        if items.is_empty() {
            return Err(Error::Unreachable(format!(
                "no such item nearby at {:?} (ticks {})",
                self.position(),
                self.world.ticks
            )));
        }
        let ((tile, oid), _) =
            items.iter().min_by_key(|((tile, _), _)| (tile.0 - self.world.x).abs() + (tile.1 - self.world.z).abs()).unwrap();
        let (x, z, level) = *tile;
        let oid = *oid;
        let op = (self.tables.obj_take_op)(oid)
            .ok_or_else(|| Error::Unreachable(format!("obj {oid} has no Take op")))?;
        if (self.world.x - x).abs() + (self.world.z - z).abs() > 1 {
            let here = (self.world.x, self.world.z);
            let mut neighbors: Vec<(i32, i32)> =
                [(-1, -1), (-1, 0), (-1, 1), (0, -1), (0, 1), (1, -1), (1, 0), (1, 1)].iter().map(|(dx, dz)| (x + dx, z + dz)).collect();
            // Orthogonal stands answer, diagonal corners often refuse
            // (server NO_STRATEGY): try straight sides first.
            neighbors.sort_by_key(|tile| {
                let diagonal = (tile.0 - x != 0) && (tile.1 - z != 0);
                (diagonal, (tile.0 - here.0).abs() + (tile.1 - here.1).abs())
            });
            let mut stepped = false;
            for (nx, nz) in neighbors {
                let rest = deadline.saturating_sub(self.world.ticks).max(500);
                match self.walk_to(nx, nz, level, rest) {
                    Ok(()) => {
                        stepped = true;
                        break;
                    }
                    Err(err) if crate::error::is_interrupted(&err) || matches!(err, Error::Io(_) | Error::Login(_)) => return Err(err),
                    Err(_) => {}
                }
            }
            if !stepped {
                return Err(Error::Unreachable(format!("no reachable neighbor near {x},{z} from {:?}", self.position())));
            }
        }
        let had: u32 = self.inventory().values().sum();
        let mut seen = self.world.messages.len();
        // One re-stand: a diagonal corner refuses even when an orthogonal
        // side would answer (proven live: pickaxe 3081,3429 from 3082,3428
        // refused, from 3081,3428 taken). Bounded by the same deadline.
        let mut restood = false;
        loop {
            self.op_obj(op, x, z, oid)?;
            while self.world.ticks < deadline {
                self.pump(3)?;
                if self.inventory().values().sum::<u32>() > had {
                    return Ok(true);
                }
                // The engine answers an unpathable click with "I can't reach
                // that!" (Player.ts) and never picks up: fail typed instead
                // of burning the timeout (a ground item on a blocked tile,
                // e.g. under a table, refuses every sweep otherwise).
                if Self::bank_click_refused(&self.world.messages, seen) {
                    let diagonal = (self.world.x - x != 0) && (self.world.z - z != 0);
                    if !restood && diagonal {
                        restood = true;
                        let here = (self.world.x, self.world.z);
                        let mut sides: Vec<(i32, i32)> =
                            [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().map(|(dx, dz)| (x + dx, z + dz)).collect();
                        sides.sort_by_key(|tile| (tile.0 - here.0).abs() + (tile.1 - here.1).abs());
                        let mut moved = false;
                        for (nx, nz) in sides {
                            let rest = deadline.saturating_sub(self.world.ticks).max(500);
                            match self.walk_to(nx, nz, level, rest) {
                                Ok(()) => {
                                    moved = true;
                                    break;
                                }
                                Err(err) if crate::error::is_interrupted(&err) || matches!(err, Error::Io(_) | Error::Login(_)) => return Err(err),
                                Err(_) => {}
                            }
                        }
                        if moved {
                            seen = self.world.messages.len();
                            break;
                        }
                    }
                    return Err(Error::Unreachable(format!(
                        "obj {oid} at {x},{z} refused from {:?} (blocked tile?)",
                        self.position()
                    )));
                }
            }
            if self.world.ticks >= deadline {
                break;
            }
        }
        Ok(false)
    }

    pub fn inventory(&self) -> HashMap<u16, u32> {
        crate::snapshot::aggregate(&self.world.inv)
    }

    /// Deposit the backpack with the bank's "Deposit All" action (op 4 per
    /// slot, one burst, then one retry of leftovers). Tick-bounded.
    pub fn bank_deposit_all(&mut self, side_com: u16, keep_ids: &[u16], timeout: u64) -> Result<bool> {
        self.act(format!("bank_deposit_all keep={} timeout={timeout}", keep_ids.len()));
        for _ in 0..2 {
            let (_, slots) = self.backpack();
            let pending: Vec<(u8, u16)> =
                slots.iter().filter(|(_, (held, _))| !keep_ids.contains(held)).map(|(slot, (held, _))| (*slot, *held)).collect();
            if pending.is_empty() {
                return Ok(true);
            }
            for (slot, held) in pending {
                self.inv_button(4, held, slot, side_com)?;
            }
            let deadline = self.world.ticks + timeout;
            while self.world.ticks < deadline {
                self.pump(2)?;
                let (_, now) = self.backpack();
                if now.iter().all(|(_, (held, _))| keep_ids.contains(held)) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// (backpack com, slots) or (None, []) before the pack streams.
    /// Backpack com and slots: the canonical `inventory:inv` container the
    /// server streams, with a widest-container fallback for stale panels.
    pub fn backpack(&self) -> (Option<u16>, Vec<(u8, (u16, u32))>) {
        let canonical = (self.tables.com_id)("inventory:inv");
        let (com, slots) = crate::snapshot::backpack_view(&self.world.inv, &self.world.inv_cap, canonical);
        (com, slots.into_iter().map(|(slot, held, count)| (slot, (held, count))).collect())
    }

    pub fn drop(&mut self, obj_id: u16, timeout: u64) -> Result<bool> {
        self.act(format!("drop {} id={obj_id}", (self.tables.obj_name)(obj_id).unwrap_or_default()));
        let (com, slots) = self.backpack();
        let Some(com) = com else { return Err(Error::Unreachable("backpack not streamed".to_string())) };
        let Some(&(slot, _)) = slots.iter().find(|(_, (held, _))| *held == obj_id) else {
            return Err(Error::Unreachable(format!("no such item {obj_id} in inventory at {:?}", self.position())));
        };
        let op = (self.tables.obj_drop_op)(obj_id)
            .ok_or_else(|| Error::Unreachable(format!("obj {obj_id} has no Drop op")))?;
        let before: u32 = slots.iter().filter(|(_, (held, _))| *held == obj_id).map(|(_, (_, n))| *n).sum();
        self.op_held(op, obj_id, slot, com)?;
        let deadline = self.world.ticks + timeout;
        while self.world.ticks < deadline {
            self.pump(3)?;
            let (_, now) = self.backpack();
            let left: u32 = now.iter().filter(|(_, (held, _))| *held == obj_id).map(|(_, (_, n))| *n).sum();
            if left < before {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn chat_continue(&mut self) -> Result<()> {
        self.act("chat_continue".to_string());
        let mut body = Writer::new();
        body.u16(0);
        self.send_named("RESUME_PAUSEBUTTON", &body.bytes())
    }

    pub fn bank_items(&self) -> HashMap<u16, u32> {
        crate::snapshot::bank_items(&self.world.inv, (self.tables.com_id)("bank_main:inv"))
    }

    /// Consecutive fan refusals (walk or click) before the booth is
    /// declared unusable from here: two refusals still precede a success
    /// live (booth 3243 opened on its 3rd fan side after 2 refused clicks
    /// and a refused walk), the 3rd consecutive refusal never did — it is
    /// a counter or wall, not a busy booth. Measured 127-128 ticks per
    /// refused 1-tile walk (6 live samples), so 3 bounds the fan waste to
    /// ~400 ticks instead of ~1790.
    const BANK_FAN_MAX_REFUSALS: u32 = 3;

    /// One fan side step, in ticks. A fan tile is adjacent: successes land
    /// in 2-12 ticks live (28 samples, worst 28 after a stall burst), a
    /// refusal trips the 13-tick stall watch twice (send + the one replan)
    /// in the low 30s. 41 = worst success (28) + one stall window (13):
    /// a good side always fits, a dead side fails in ~41 instead of
    /// ~85-128 (the old approach ran an exact walk plus a 250-floored fan
    /// walk per tile). Same decision (try every side, abort at 3
    /// refusals), just faster.
    const BANK_FAN_STEP_TICKS: u64 = 41;

    pub fn open_bank(&mut self, x: i32, z: i32, loc_id: u16, timeout: u64) -> Result<bool> {
        self.act(format!("open_bank {} id={loc_id} at ({x},{z})", (self.tables.loc_name)(loc_id).unwrap_or_default()));
        self.approach(x, z, self.world.level, 1, 60)?;
        if self.click_bank(x, z, loc_id, timeout)? {
            return Ok(true);
        }
        // Booths sit behind counters: the nearest tile within 1 can face
        // a wall ("I can't reach that"). Walk around and click the booth
        // from neighboring sides, nearest first — but stop after consecutive
        // refusals: the booth is behind a counter here, try the next booth
        // instead of grinding all 8 sides (~1790 ticks measured live).
        let mut tiles: Vec<(i32, i32)> = (-1..=1)
            .flat_map(|dx| (-1..=1).map(move |dz| (x + dx, z + dz)))
            .filter(|tile| *tile != (x, z))
            .collect();
        tiles.sort_by_key(|tile| (tile.0 - self.world.x).abs() + (tile.1 - self.world.z).abs());
        let mut refusals = 0u32;
        for (tile_x, tile_z) in tiles {
            // One bounded walk, not an approach: the tile is adjacent, so
            // a 41-tick walk covers the worst live success (28) plus the
            // stall/replan settle of a refusal (low 30s). approach would
            // run an exact walk plus a 250-floored fan walk per tile and
            // burn ~85-128 ticks on each dead side.
            match self.walk_to(tile_x, tile_z, self.world.level, Self::BANK_FAN_STEP_TICKS) {
                Ok(()) => {}
                Err(err) if Self::bank_fan_counts_as_refusal(&err) => {
                    refusals += 1;
                    if Self::bank_fan_refused(refusals) {
                        return Ok(false);
                    }
                    continue;
                }
                Err(err) => return Err(err),
            }
            if self.click_bank(x, z, loc_id, timeout)? {
                return Ok(true);
            }
            refusals += 1;
            if Self::bank_fan_refused(refusals) {
                return Ok(false);
            }
        }
        Err(Error::Timeout(format!("bank did not open at {x},{z} from {:?} ({} ticks)", self.position(), timeout)))
    }

    /// True once refusals prove the booth unusable from here. Pure so the
    /// threshold is locked by test, not by live luck.
    fn bank_fan_refused(refusals: u32) -> bool {
        refusals >= Self::BANK_FAN_MAX_REFUSALS
    }

    /// True when a fan walk error proves the booth unusable from here:
    /// only Unreachable/Timeout are refusals (counter, wall). Transport
    /// and operator stops (Io/Login/Interrupted, anything else) propagate
    /// so the session reconnects instead of stranding on Ok(false). Pure
    /// so the classification is locked by test: the fan itself is
    /// unreachable offline (the click send fails first), but every error
    /// it can meet goes through here.
    fn bank_fan_counts_as_refusal(error: &Error) -> bool {
        match error {
            Error::Unreachable(_) | Error::Timeout(_) | Error::StepRefused(_) => true,
            // walk_to wraps its failures with a replay dossier: a refused
            // step (server shut), a stalled leg (no progress in 41 ticks)
            // and no route (planner says wall) all prove counter-or-wall.
            // Every other kind (door, ride, choice) and transport stops
            // propagate loud instead of stranding on Ok(false).
            Error::TravelFailure(failure) => matches!(
                failure.kind,
                TravelErrorKind::NoRoute | TravelErrorKind::StepRefused | TravelErrorKind::Timeout
            ),
            _ => false,
        }
    }

    /// Click the booth op and wait for the bank interface. Ok(false) when
    /// the engine ignored the click (wrong side of a counter); only send
    /// errors and missing tables propagate.
    fn click_bank(&mut self, x: i32, z: i32, loc_id: u16, timeout: u64) -> Result<bool> {
        let seen = self.world.messages.len();
        self.op_loc(2, x, z, loc_id)?;
        let com = (self.tables.com_id)("bank_main:inv")
            .ok_or_else(|| Error::State("bank_main:inv missing from component table".to_string()))?;
        let deadline = self.world.ticks + timeout;
        while self.world.ticks < deadline {
            self.pump(5)?;
            if self.world.inv.contains_key(&com) {
                return Ok(true);
            }
            // The engine answers a wrong-side click with "I can't reach
            // that!" (Player.ts) and never opens: stop burning the timeout.
            if Self::bank_click_refused(&self.world.messages, seen) {
                return Ok(false);
            }
        }
        Ok(false)
    }

    /// True when a fresh engine message refuses the booth click ("I can't
    /// reach that!"): the interface will never open, fail fast instead of
    /// waiting out the timeout.
    fn bank_click_refused(messages: &[String], seen: usize) -> bool {
        messages.get(seen..).unwrap_or(&[]).iter().any(|message| message.to_lowercase().contains("can't reach"))
    }

    /// Withdraw at least `count`, using the 10/5/1 buttons (all for coins).
    pub fn bank_withdraw(&mut self, obj_id: u16, count: u32, timeout: u64) -> Result<u32> {
        self.act(format!("bank_withdraw {} x{count}", (self.tables.obj_name)(obj_id).unwrap_or_default()));
        let main_com = (self.tables.com_id)("bank_main:inv")
            .ok_or_else(|| Error::State("bank_main:inv missing from component table".to_string()))?;
        let mut remaining = count;
        while remaining > 0 {
            let slot = self
                .world
                .inv
                .get(&main_com)
                .and_then(|slots| slots.iter().find(|(_, (held, _))| *held == obj_id).map(|(slot, _)| *slot));
            let Some(slot) = slot else { break };
            let (op, step) = if obj_id == 995 || remaining >= 1000 {
                (4, remaining)
            } else if remaining >= 10 {
                (3, 10)
            } else if remaining >= 5 {
                (2, 5)
            } else {
                (1, 1)
            };
            let before = self.inventory().get(&obj_id).copied().unwrap_or(0);
            self.inv_button(op, obj_id, slot, main_com)?;
            let deadline = self.world.ticks + timeout;
            let mut grew = false;
            while self.world.ticks < deadline {
                self.pump(2)?;
                if self.inventory().get(&obj_id).copied().unwrap_or(0) > before {
                    grew = true;
                    break;
                }
            }
            if !grew {
                return Err(Error::Timeout(format!("withdraw of {obj_id} timed out at {:?} ({} ticks)", self.position(), timeout)));
            }
            remaining = remaining.saturating_sub(step);
        }
        Ok(self.inventory().get(&obj_id).copied().unwrap_or(0))
    }

    pub fn cheat(&mut self, text: &str) -> Result<()> {
        self.act(format!("cheat {text}"));
        let mut body = Writer::new();
        let command = text.strip_prefix("::").unwrap_or(text);
        body.jstr(command, 10);
        self.send_named("CLIENT_CHEAT", &body.bytes())
    }

    pub fn chat_texts(&self) -> Vec<(u16, String)> {
        let mut texts: Vec<(u16, String)> = self.world.texts.iter().map(|(com, text)| (*com, text.clone())).collect();
        texts.sort();
        texts
    }

    /// (com, text) option rows under the latest "Select an Option" header.
    pub fn chat_options(&self) -> Vec<(u16, String)> {
        let chat_modal = self.world.modal.as_ref().filter(|modal| modal.layer == "chat").map(|modal| modal.interface_id);
        crate::snapshot::chat_options(&self.world.texts, chat_modal)
    }

    pub fn chat_choose(&mut self, text: &str) -> Result<u16> {
        self.act(format!("chat_choose {text}"));
        let needle = text.to_lowercase();
        let found = self.world.texts.iter().find(|(_, option)| option.to_lowercase().contains(&needle)).map(|(com, _)| *com);
        match found {
            Some(com) => {
                let mut body = Writer::new();
                body.u16(com);
                self.send_named("IF_BUTTON", &body.bytes())?;
                Ok(com)
            }
            None => Err(Error::Unreachable(format!("no dialog option matching {text:?} at {:?}", self.position()))),
        }
    }

    pub fn resume_pcount(&mut self, value: i32) -> Result<()> {
        let mut body = Writer::new();
        body.i32(value);
        self.send_named("RESUME_P_COUNTDIALOG", &body.bytes())
    }

    pub fn tut_clickside(&mut self, tab: u8) -> Result<()> {
        let mut body = Writer::new();
        body.u8(tab);
        self.send_named("TUT_CLICKSIDE", &body.bytes())
    }

    pub fn save_design(&mut self, gender: u8, kits: &[i8], colors: &[u8]) -> Result<()> {
        let mut body = Writer::new();
        body.u8(gender);
        for kit in kits {
            body.u8(if *kit < 0 { 255 } else { *kit as u8 });
        }
        for color in colors {
            body.u8(*color);
        }
        self.send_named("IDK_SAVEDESIGN", &body.bytes())
    }

    pub fn friend_add(&mut self, name: &str) -> Result<()> {
        let mut body = Writer::new();
        body.u64(to_base37(name));
        self.send_named("FRIENDLIST_ADD", &body.bytes())
    }

    pub fn friend_del(&mut self, name: &str) -> Result<()> {
        let mut body = Writer::new();
        body.u64(to_base37(name));
        self.send_named("FRIENDLIST_DEL", &body.bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Refusal is scoped to fresh messages: earlier traffic never fails
    /// a new click, unrelated chatter never matches.
    #[test]
    fn bank_click_refused_only_matches_fresh_refusal() {
        let messages = vec!["Welcome.".to_string(), "I can't reach that!".to_string()];
        assert!(!Bot::bank_click_refused(&messages, 2), "nothing fresh");
        assert!(Bot::bank_click_refused(&messages, 1), "fresh refusal fails fast");
        let other = vec!["A bank booth.".to_string()];
        assert!(!Bot::bank_click_refused(&other, 0), "unrelated text passes");
    }

    /// Two consecutive refusals still precede a success live (booth 3243
    /// opened on its 3rd fan side): only the 3rd consecutive refusal aborts.
    #[test]
    fn bank_fan_aborts_on_third_consecutive_refusal() {
        assert!(!Bot::bank_fan_refused(0), "no refusal keeps trying");
        assert!(!Bot::bank_fan_refused(1), "first refusal keeps trying");
        assert!(!Bot::bank_fan_refused(2), "second refusal keeps trying");
        assert!(Bot::bank_fan_refused(3), "third consecutive aborts");
        assert!(Bot::bank_fan_refused(8), "late refusal aborts");
    }

    /// A dead socket still propagates typed instead of degrading into
    /// Ok(false): adjacent to the booth the approach short-circuits, the
    /// click send fails loud, never masked as a refusal.
    #[test]
    fn open_bank_offline_propagates_send_error() {let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.level = 0;
        bot.world.x = 3091;
        bot.world.z = 3246;
        let err = bot.open_bank(3091, 3245, 2213, 50).expect_err("dead socket fails loud");
        assert!(format!("{err}").contains("not logged in"), "typed conn error, got: {err}");
    }

    /// Socket dead mid-fan degrades into Ok(false) under the old
    /// `.is_err()` count: 3 swallowed Io errors read as 3 refusals and
    /// `bank_trip` strands instead of reconnecting. Only true refusals
    /// count now, so transport/operator stops propagate. The fan loop
    /// itself is unreachable offline (the click send fails before it),
    /// so this locks the classification every fan error goes through.
    #[test]
    fn bank_fan_propagates_dead_socket_instead_of_refusing() {
        for err in [
            Error::Io("not logged in".to_string()),
            Error::Login("stale session".to_string()),
            Error::Interrupted("run stopped".to_string()),
            Error::State("bank_main:inv missing".to_string()),
            Error::Protocol("bad frame".to_string()),
        ] {
            assert!(!Bot::bank_fan_counts_as_refusal(&err), "propagates, never a refusal: {err}");
        }
    }

    /// 3 true refusals still abort the booth as before: Unreachable (wall,
    /// counter) and Timeout (stalled leg) are the only fan refusals.
    #[test]
    fn bank_fan_counts_only_true_refusals() {
        assert!(Bot::bank_fan_counts_as_refusal(&Error::Unreachable("counter".to_string())), "wall counts");
        assert!(Bot::bank_fan_counts_as_refusal(&Error::Timeout("stalled leg".to_string())), "stall counts");
        assert!(Bot::bank_fan_counts_as_refusal(&Error::StepRefused("shut".to_string())), "raw refused step counts");
        for kind in [TravelErrorKind::NoRoute, TravelErrorKind::StepRefused, TravelErrorKind::Timeout] {
            assert!(
                Bot::bank_fan_counts_as_refusal(&travel_failure(kind)),
                "walk_to dossier {:?} counts",
                kind
            );
        }
        for kind in
            [TravelErrorKind::DoorDidNotOpen, TravelErrorKind::RideDidNotMove, TravelErrorKind::Io, TravelErrorKind::Interrupted]
        {
            assert!(!Bot::bank_fan_counts_as_refusal(&travel_failure(kind)), "walk_to dossier {:?} propagates", kind);
        }
    }

    fn travel_failure(kind: TravelErrorKind) -> Error {
        Error::TravelFailure(crate::error::TravelError {
            kind,
            segment_index: None,
            position: (0, 0, 0),
            tick: 0,
            expected: String::new(),
            observed: String::new(),
            dossier: None,
        })
    }

    /// Spell-cast payloads lock the server decoder order (LostCity
    /// OpObjT/OpHeldT/OpLocTDecoder: g2 fields, big-endian): a swapped
    /// field ships a silent bad packet, so the bytes pin by hand.
    #[test]
    fn spell_cast_payloads_match_the_server_decoders() {
        // x=2930 (0x0B72), z=3515 (0x0DBB), obj=0x0010, spell=0x00A0.
        assert_eq!(super::opobjt_bytes(2930, 3515, 0x10, 0xA0), vec![0x0B, 0x72, 0x0D, 0xBB, 0x00, 0x10, 0x00, 0xA0]);
        // obj=0x0200, slot=3, com=0x0D12, spell=0x00A1.
        assert_eq!(super::opheldt_bytes(0x0200, 3, 0x0D12, 0xA1), vec![0x02, 0x00, 0x00, 0x03, 0x0D, 0x12, 0x00, 0xA1]);
        // x=100 (0x0064), z=200 (0x00C8), loc=0x0600, spell=0x00A2.
        assert_eq!(super::oploct_bytes(100, 200, 0x0600, 0xA2), vec![0x00, 0x64, 0x00, 0xC8, 0x06, 0x00, 0x00, 0xA2]);
    }

    /// Player spell/use payloads lock the server decoder order
    /// (`OpPlayerTDecoder`: slot, spell com; `OpPlayerUDecoder`: slot,
    /// obj com, use slot, use com — the same shape as `op_npc_t/u`).
    #[test]
    fn player_spell_and_use_payloads_match_the_server_decoders() {
        assert_eq!(super::opplayert_bytes(0x0007, 0x00A0), vec![0x00, 0x07, 0x00, 0xA0]);
        assert_eq!(super::opplayeru_bytes(0x0007, 0x0D12, 0x0003, 0x0A1B), vec![0x00, 0x07, 0x0D, 0x12, 0x00, 0x03, 0x0A, 0x1B]);
    }

    /// The wrappers resolve their op names (a bad name fails `Protocol`,
    /// never reaches the socket): dead socket reads `Login`, proving the
    /// send path is wired, framing included.
    #[test]
    fn player_spell_and_use_wrappers_reach_the_send_path() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        for outcome in [bot.op_player_t(0x0007, 0x00A0), bot.op_player_u(0x0007, 0x0D12, 0x0003, 0x0A1B)] {
            assert!(matches!(outcome, Err(Error::Login(_))), "dead socket fails at login, never at the op name: {outcome:?}");
        }
    }

    /// The wrappers resolve their op names (a bad name fails `Protocol`,
    /// never reaches the socket): dead socket reads `Login`, proving the
    /// send path is wired, framing included (8-byte payloads fit the
    /// table sizes or `frame_packet` would reject them live).
    #[test]
    fn spell_cast_wrappers_reach_the_send_path() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        for outcome in [
            bot.op_obj_t(2930, 3515, 0x10, 0xA0),
            bot.op_held_t(0x0200, 3, 0x0D12, 0xA1),
            bot.op_loc_t(100, 200, 0x0600, 0xA2),
        ] {
            assert!(
                matches!(outcome, Err(Error::Login(_))),
                "dead socket fails at login, never at the op name: {outcome:?}"
            );
        }
    }

}
