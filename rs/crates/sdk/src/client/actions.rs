//! Bot actions: clicks, items, dialogue, trade, shops and chat.
//!
//! Every wait is a pump loop to an observable state plus a tick bound:
//! never wall-clock sleep.

use crate::buffer::Writer;
use crate::client::Bot;
use crate::error::{Error, Result};
use std::collections::HashMap;

use crate::words::to_base37;

fn op_name(kind: &str, op: u8) -> String {
    format!("{kind}{op}")
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

    pub fn op_player(&mut self, op: u8, slot: u16) -> Result<()> {
        self.act(format!("op_player{op} slot={slot}"));
        let mut body = Writer::new();
        body.u16(u16::from(slot));
        self.send_named(&op_name("OPPLAYER", op), &body.bytes())
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
        let wanted = name.to_lowercase();
        let mut best: Option<(i32, u16)> = None;
        for (nid, npc) in &self.world.npcs {
            let label = (self.tables.npc_name)(npc.npc_type).unwrap_or_default();
            if !label.to_lowercase().contains(&wanted) {
                continue;
            }
            let dist = (npc.x - self.world.x).abs() + (npc.z - self.world.z).abs();
            if best.is_none_or(|(known, _)| dist < known) {
                best = Some((dist, *nid));
            }
        }
        best.map(|(_, nid)| nid)
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
    pub fn trade_offer(&mut self, obj_id: u16, count: u32, timeout: u64) -> Result<bool> {
        self.act(format!("trade_offer {} x{count}", (self.tables.obj_name)(obj_id).unwrap_or_default()));
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
            neighbors.sort_by_key(|tile| (tile.0 - here.0).abs() + (tile.1 - here.1).abs());
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
        self.op_obj(op, x, z, oid)?;
        while self.world.ticks < deadline {
            self.pump(3)?;
            if self.inventory().values().sum::<u32>() > had {
                return Ok(true);
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

    pub fn open_bank(&mut self, x: i32, z: i32, loc_id: u16, timeout: u64) -> Result<bool> {
        self.act(format!("open_bank {} id={loc_id} at ({x},{z})", (self.tables.loc_name)(loc_id).unwrap_or_default()));
        self.approach(x, z, self.world.level, 1, 60)?;
        if self.click_bank(x, z, loc_id, timeout)? {
            return Ok(true);
        }
        // Booths sit behind counters: the nearest tile within 1 can face
        // a wall ("I can't reach that"). Walk around and click the booth
        // from every neighboring side, nearest first.
        let mut tiles: Vec<(i32, i32)> = (-1..=1)
            .flat_map(|dx| (-1..=1).map(move |dz| (x + dx, z + dz)))
            .filter(|tile| *tile != (x, z))
            .collect();
        tiles.sort_by_key(|tile| (tile.0 - self.world.x).abs() + (tile.1 - self.world.z).abs());
        for (tile_x, tile_z) in tiles {
            if self.approach(tile_x, tile_z, self.world.level, 0, 150).is_err() {
                continue;
            }
            if self.click_bank(x, z, loc_id, timeout)? {
                return Ok(true);
            }
        }
        Err(Error::Timeout(format!("bank did not open at {x},{z} from {:?} ({} ticks)", self.position(), timeout)))
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
        crate::snapshot::chat_options(&self.world.texts)
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
}
