"""Bot actions: clicks, items, dialogue, trade, shops and chat.

The mixin assumes the core session (connection, world, tick pump) and the
movement mixin (walk_to/approach) are present on the same Bot instance.
"""

import time

from .. import objs as objs_mod
from ..buffer import Writer
from ..connection import LoginError
from ..protocol import client_op_id
from ..world import to_base37
from .core import StateError, Unreachable


class ActionsMixin:

    def op_loc(self, op, x, z, loc_id):
        body = Writer()
        body.u16(x)
        body.u16(z)
        body.u16(loc_id)
        self.connection.send(client_op_id("OPLOC%d" % op), body.bytes())

    def op_npc(self, op, slot):
        body = Writer()
        body.u16(slot)
        self.connection.send(client_op_id("OPNPC%d" % op), body.bytes())

    def op_npc_u(self, slot, obj_id, use_slot, com):
        body = Writer()
        body.u16(slot)
        body.u16(obj_id)
        body.u16(use_slot)
        body.u16(com)
        self.connection.send(client_op_id("OPNPCU"), body.bytes())

    def op_obj(self, op, x, z, obj_id):
        body = Writer()
        body.u16(x)
        body.u16(z)
        body.u16(obj_id)
        self.connection.send(client_op_id("OPOBJ%d" % op), body.bytes())

    def op_loc_u(self, x, z, loc_id, obj_id, slot, com):
        body = Writer()
        body.u16(x)
        body.u16(z)
        body.u16(loc_id)
        body.u16(obj_id)
        body.u16(slot)
        body.u16(com)
        self.connection.send(client_op_id("OPLOCU"), body.bytes())

    def op_npc_t(self, slot, spell_com):
        body = Writer()
        body.u16(slot)
        body.u16(spell_com)
        self.connection.send(client_op_id("OPNPCT"), body.bytes())

    def op_player(self, op, slot):
        body = Writer()
        body.u16(slot)
        self.connection.send(client_op_id("OPPLAYER%d" % op), body.bytes())

    def op_held(self, op, obj, slot, com):
        body = Writer()
        body.u16(obj)
        body.u16(slot)
        body.u16(com)
        self.connection.send(client_op_id("OPHELD%d" % op), body.bytes())

    def op_held_u(self, obj, slot, com, use_obj, use_slot, use_com):
        body = Writer()
        body.u16(obj)
        body.u16(slot)
        body.u16(com)
        body.u16(use_obj)
        body.u16(use_slot)
        body.u16(use_com)
        self.connection.send(client_op_id("OPHELDU"), body.bytes())

    def inv_button(self, n, obj, slot, com):
        body = Writer()
        body.u16(obj)
        body.u16(slot)
        body.u16(com)
        self.connection.send(client_op_id("INV_BUTTON%d" % n), body.bytes())

    def inv_button_d(self, com, slot, target, mode=0):
        body = Writer()
        body.u16(com)
        body.u16(slot)
        body.u16(target)
        body.u8(mode)
        self.connection.send(client_op_id("INV_BUTTOND"), body.bytes())

    def if_button(self, com):
        body = Writer()
        body.u16(com)
        self.connection.send(client_op_id("IF_BUTTON"), body.bytes())

    def close_modal(self):
        self.connection.send(client_op_id("CLOSE_MODAL"))

    def talk_to(self, slot):
        self.op_npc(1, slot)

    def find_npc(self, name):
        self._ensure_nav()
        best = None
        for nid, npc in self.world.npcs.items():
            entry = self._loctypes["npc"][npc["type"]] if npc["type"] < len(self._loctypes["npc"]) else None
            label = entry["name"] if entry else ""
            if name.lower() not in label.lower():
                continue
            dist = abs(npc["x"] - self.world.x) + abs(npc["z"] - self.world.z)
            if best is None or dist < best[0]:
                best = (dist, nid)
        return best[1] if best else None

    def npcs_named(self, names):
        """(nid, npc) for every visible NPC whose type name is in names."""
        self._ensure_nav()
        wanted = {name.lower() for name in names}
        found = []
        for nid, npc in self.world.npcs.items():
            entry = self._loctypes["npc"][npc["type"]] if npc["type"] < len(self._loctypes["npc"]) else None
            if entry and (entry["name"] or "").lower() in wanted:
                found.append((nid, npc))
        return found

    def trade_to(self, name, near=None):
        if self._scan_and_trade(name, near):
            return True
        try:
            self.relogin()
            self.close_modal()
            self.pump(5)
        except (LoginError, ConnectionError, OSError, TimeoutError, StateError) as error:
            print("relogin before trade to %s failed: %s" % (name, error), flush=True)
            return False
        if self._scan_and_trade(name, near):
            return True
        base = (self.world.x, self.world.z)
        for dx, dz in ((20, 0), (0, 0), (-20, 0)):
            if dx or dz:
                try:
                    self.walk_to(base[0] + dx, base[1] + dz, self.world.level, open_doors=True, timeout=25)
                except (Unreachable, TimeoutError) as error:
                    print("trade approach step failed at %s: %s" % (self.world.position, error), flush=True)
            if self._scan_and_trade(name, None):
                return True
        return False

    def _scan_and_trade(self, name, near):
        if near is not None and abs(self.world.x - near[0]) + abs(self.world.z - near[1]) > 4:
            try:
                self.walk_to(near[0], near[1], self.world.level, timeout=25, open_doors=True)
            except (Unreachable, TimeoutError) as error:
                print("trade walk to %s failed: %s" % (near, error), flush=True)
        deadline = time.time() + 10
        found = None
        while time.time() < deadline and found is None:
            self.pump(5)
            found = self.find_npc(name)
        if found is None:
            return False
        return self._trade_from_best_tile(found)

    def _trade_from_best_tile(self, slot, rounds=2):
        for _round in range(rounds):
            npc = self.world.npcs.get(slot)
            if npc is None:
                self.pump(5)
                continue
            tiles = sorted(
                [(npc["x"] + dx, npc["z"] + dz) for dx in (-1, 0, 1) for dz in (-1, 0, 1) if dx or dz],
                key=lambda tile: abs(tile[0] - self.world.x) + abs(tile[1] - self.world.z),
            )
            for tile_x, tile_z in tiles:
                try:
                    self.walk_to(tile_x, tile_z, self.world.level, timeout=15, open_doors=True)
                except (Unreachable, TimeoutError) as error:
                    print("trade tile %d,%d failed: %s" % (tile_x, tile_z, error), flush=True)
                    continue
                self.op_npc(3, slot)
                end = time.time() + 8
                while not self.world.modal and time.time() < end:
                    self.pump(5)
                if self.world.modal is not None:
                    return True
        return False

    def take(self, obj_id=None, name=None, timeout=30):

        self._ensure_nav()
        if name is not None:
            for index, entry in enumerate(self._loctypes["obj"]):
                if entry and entry["name"] and name.lower() in entry["name"].lower():
                    obj_id = index
                    break
        deadline = time.time() + timeout
        items = {}
        while time.time() < deadline and not items:
            self.pump(3)
            items = self.world.ground_items(self.world.x, self.world.z, self.world.level, obj_id=obj_id)
        if not items:
            raise Unreachable("no such item nearby")
        (key, oid), _count = min(items.items(), key=lambda item: abs(item[0][0][0] - self.world.x) + abs(item[0][0][1] - self.world.z))
        (x, z, level) = key
        op = objs_mod.take_op(self._loctypes["obj"], oid)
        before = dict(self.inventory())
        if abs(self.world.x - x) + abs(self.world.z - z) > 1:
            neighbors = sorted(
                [(x + dx, z + dz) for dx in (-1, 0, 1) for dz in (-1, 0, 1) if dx or dz],
                key=lambda tile: abs(tile[0] - self.world.x) + abs(tile[1] - self.world.z),
            )
            stepped = False
            for nx, nz in neighbors:
                try:
                    self.walk_to(nx, nz, level, timeout=max(10, deadline - time.time()))
                    stepped = True
                    break
                except (Unreachable, TimeoutError) as error:
                    print("take approach to %d,%d failed: %s" % (nx, nz, error), flush=True)
                    continue
            if not stepped:
                raise Unreachable("no reachable neighbor near %d,%d" % (x, z))
        had = sum(self.inventory().values())
        self.op_obj(op, x, z, oid)
        deadline = time.time() + timeout
        while time.time() < deadline:
            self.pump(3)
            if sum(self.inventory().values()) > had:
                return True
        return False

    def inventory(self):
        items = {}
        for com, slots in self.world.inv.items():
            for slot, (obj_id, count) in slots.items():
                items[obj_id] = items.get(obj_id, 0) + count
        return items

    def bank_deposit_all(self, side_com, keep_ids=(), timeout=80):
        """Deposit the backpack with the bank's "Deposit All" action.

        This revision has no single "deposit inventory" button: the bulk
        action is right-click Deposit All on each slot. Send every click as
        one burst (what a human spam-clicking does), then wait until the pack
        holds only the kept items, and retry the leftovers once.
        """
        keep = set(keep_ids)
        for _round in range(2):
            _com, slots = self.backpack()
            pending = [(slot, held) for slot, (held, _count) in sorted(slots.items()) if held not in keep]
            if not pending:
                return True
            for slot, held in pending:
                self.inv_button(4, held, slot, side_com)
            deadline = self.world.ticks + timeout
            while self.world.ticks < deadline:
                self.pump(2)
                _com, slots = self.backpack()
                if not [held for held, _count in slots.values() if held not in keep]:
                    return True
        return False

    def backpack(self):
        for com, slots in self.world.inv.items():
            if self.world.inv_cap.get(com) == 28:
                return com, slots
        for com, slots in self.world.inv.items():
            if len(slots) <= 28:
                return com, slots
        return None, {}

    def drop(self, obj_id, timeout=15):

        com, slots = self.backpack()
        for slot, (held, _count) in slots.items():
            if held != obj_id:
                continue
            op = objs_mod.drop_op(self._loctypes["obj"], held)
            before = sum(count for (carried, count) in slots.values() if carried == obj_id)
            body = Writer()
            body.u16(held)
            body.u16(slot)
            body.u16(com)
            self.connection.send(client_op_id("OPHELD%d" % op), body.bytes())
            deadline = time.time() + timeout
            while time.time() < deadline:
                self.pump(3)
                _com, now = self.backpack()
                if sum(count for (carried, count) in now.values() if carried == obj_id) < before:
                    return True
            return False
        raise Unreachable("no such item in inventory")

    def chat_continue(self):
        body = Writer()
        body.u16(0)
        self.connection.send(client_op_id("RESUME_PAUSEBUTTON"), body.bytes())

    def bank_items(self):
        com = self._com_id("bank_main:inv")
        items = {}
        for held, count in self.world.inv.get(com, {}).values():
            items[held] = items.get(held, 0) + count
        return items

    def open_bank(self, x, z, loc_id, timeout=100):
        self.approach(x, z, radius=1, timeout=60, open_doors=True)
        self.op_loc(2, x, z, loc_id)
        com = self._com_id("bank_main:inv")
        deadline = self.world.ticks + timeout
        while self.world.ticks < deadline:
            self.pump(5)
            if com in self.world.inv:
                return True
        raise TimeoutError("bank did not open at %d,%d" % (x, z))

    def bank_withdraw(self, obj_id, count, timeout=100):
        """Withdraw at least count, using the 10/5/1 buttons; all for coins."""
        main_com = self._com_id("bank_main:inv")
        remaining = count
        while remaining > 0:
            slots = self.world.inv.get(main_com, {})
            slot = next((index for index, (held, _count) in slots.items() if held == obj_id), None)
            if slot is None:
                break
            if obj_id == 995 or remaining >= 1000:
                op, step = 4, remaining
            elif remaining >= 10:
                op, step = 3, 10
            elif remaining >= 5:
                op, step = 2, 5
            else:
                op, step = 1, 1
            before = self.inventory().get(obj_id, 0)
            self.inv_button(op, obj_id, slot, main_com)
            deadline = self.world.ticks + timeout
            while self.world.ticks < deadline:
                self.pump(2)
                if self.inventory().get(obj_id, 0) > before:
                    break
            else:
                raise TimeoutError("withdraw of %d timed out" % obj_id)
            remaining -= step
        return self.inventory().get(obj_id, 0)

    def cheat(self, text):
        body = Writer()
        body.jstr(text[2:] if text.startswith("::") else text)
        self.connection.send(client_op_id("CLIENT_CHEAT"), body.bytes())

    def chat_texts(self):
        return sorted(self.world.texts.items())

    def chat_options(self):
        """(com, text) for the current chat interface's option components.

        The option interface is separate from the chatbox interface (both
        are opened with IF_OPENCHAT), so keep every chat interface id and
        pick the most recent one that carries a "Select an Option" header.
        """
        items = list(self.world.texts.items())
        headers = [
            index for index, (_com, text) in enumerate(items)
            if "select an option" in text.lower()
        ]
        if not headers:
            return []
        header = headers[-1]
        base = items[header][0]
        return [
            (com, text) for com, text in items[header + 1:header + 6]
            if base < com <= base + 5
        ]

    def chat_choose(self, text):
        for com, option in self.world.texts.items():
            if text.lower() in option.lower():
                body = Writer()
                body.u16(com)
                self.connection.send(client_op_id("IF_BUTTON"), body.bytes())
                return com
        raise KeyError("no dialog option matching %r" % text)

    def resume_pcount(self, value):
        body = Writer()
        body.i32(value)
        self.connection.send(client_op_id("RESUME_P_COUNTDIALOG"), body.bytes())

    def tut_clickside(self, tab):
        body = Writer()
        body.u8(tab)
        self.connection.send(client_op_id("TUT_CLICKSIDE"), body.bytes())

    def save_design(self, gender, kits, colors):
        body = Writer()
        body.u8(gender)
        for kit in kits:
            body.u8(255 if kit < 0 else kit)
        for color in colors:
            body.u8(color)
        self.connection.send(client_op_id("IDK_SAVEDESIGN"), body.bytes())

    def friend_add(self, name):

        body = Writer()
        body.u64(to_base37(name))
        self.connection.send(client_op_id("FRIENDLIST_ADD"), body.bytes())

    def friend_del(self, name):

        body = Writer()
        body.u64(to_base37(name))
        self.connection.send(client_op_id("FRIENDLIST_DEL"), body.bytes())
