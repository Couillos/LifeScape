"""Live world state, parsed from server frames.

Tracks own position, nearby players and NPCs, ground objects and doors,
inventory, dialogs, chat and energy. Everything a script needs to decide.
"""

from .buffer import BitReader, Reader
from .protocol import ZONE_OPS
from .words import unpack

DIRS = [(-1, 1), (0, 1), (1, 1), (-1, 0), (1, 0), (-1, -1), (0, -1), (1, -1)]

CHARSET = "_" + "abcdefghijklmnopqrstuvwxyz" + "0123456789"


def to_base37(name):
    value = 0
    for char in name.lower()[:12]:
        value = value * 37 + CHARSET.index(char)
    return value


def base37_name(value):
    name = ""
    while value:
        value, digit = divmod(value, 37)
        name = CHARSET[digit] + name
    return name


def signed(bits, width):
    return bits - (1 << width) if bits >= (1 << (width - 1)) else bits


class ChatLine:
    def __init__(self, who, text, color=0, effect=0):
        self.who = who
        self.text = text
        self.color = color
        self.effect = effect

    def __repr__(self):
        return "%s: %s" % (self.who, self.text)


class World:
    def __init__(self):
        self.base_x = 0
        self.base_z = 0
        self.zone_x = 0
        self.zone_z = 0
        self.ticks = 0
        self.level = 0
        self.x = 0
        self.z = 0
        self.energy = 100
        self.players = {}
        self.player_order = []
        self.npcs = {}
        self.npc_order = []
        self.locs = {}
        self.static_locs = {}
        self.objs = {}
        self.static_objs = {}
        self.inv = {}
        self.inv_cap = {}
        self.varps = {}
        self.stats = {}
        self.texts = {}
        self.pending_texts = {}
        self.modal_models = {}
        self.chat_interfaces = []
        self.zone_trace = []
        self.modal = None
        self.hint = None
        self.side_flash = None
        self.count_dialog = False
        self.player_ops = {}
        self.messages = []
        self.chat = []
        self.private = []
        self.logged_out = False

    def ground_items(self, x, z, level, radius=15, obj_id=None):
        # Live server state shadows the pack template: a dynamic entry
        # (including removals) always wins over the static one.
        merged = dict(self.static_objs)
        merged.update(self.objs)
        found = {}
        for (key, oid), count in merged.items():
            if count is None:
                continue
            (ix, iz, ilvl) = key
            if ilvl != level or (obj_id is not None and oid != obj_id):
                continue
            if abs(ix - x) + abs(iz - z) <= radius:
                found[(key, oid)] = count
        return found
        self.level = 0
        self.x = 0
        self.z = 0
        self.zone_x = 0
        self.zone_z = 0
        self.energy = 100
        self.players = {}
        self.player_order = []
        self.npcs = {}
        self.npc_order = []
        self.locs = {}
        self.objs = {}
        self.inv = {}
        self.inv_cap = {}
        self.varps = {}
        self.stats = {}
        self.texts = {}
        self.pending_texts = {}
        self.modal_models = {}
        self.chat_interfaces = []
        self.zone_trace = []
        self.modal = None
        self.hint = None
        self.side_flash = None
        self.count_dialog = False
        self.player_ops = {}
        self.messages = []
        self.chat = []
        self.private = []
        self.logged_out = False
        self._masks = []

    @property
    def position(self):
        return (self.level, self.x, self.z)

    def feed(self, opcode, name, body):
        if name == "REBUILD_NORMAL":
            zone = Reader(body)
            self.zone_x = zone.u16()
            self.zone_z = zone.u16()
            self.base_x = (self.zone_x - 6) << 3
            self.base_z = (self.zone_z - 6) << 3
            self._trace_zone("rebuild", None)
        elif name == "PLAYER_INFO":
            self._player_info(body)
        elif name == "NPC_INFO":
            self._npc_info(body)
        elif name in ("UPDATE_ZONE_PARTIAL_FOLLOWS", "UPDATE_ZONE_FULL_FOLLOWS"):
            zone = Reader(body)
            self._set_zone(zone.u8(), zone.u8())
            self._trace_zone(name, None)
        elif name == "UPDATE_ZONE_PARTIAL_ENCLOSED":
            self._zone_enclosed(body)
            self._trace_zone(name, None)
        elif opcode in ZONE_OPS:
            self._trace_zone(name, body[:8])
            self._zone_message(opcode, Reader(body))
        elif name == "UPDATE_INV_FULL":
            self._inv_full(body)
        elif name == "UPDATE_INV_PARTIAL":
            self._inv_partial(body)
        elif name == "UPDATE_INV_STOP_TRANSMIT":
            self.inv.pop(Reader(body).u16(), None)
        elif name == "UPDATE_STAT":
            stat = Reader(body)
            index = stat.u8()
            exp = int.from_bytes(stat.raw(4), "big")
            level = stat.u8()
            self.stats[index] = (level, exp)
        elif name == "UPDATE_RUNENERGY":
            self.energy = Reader(body).u8()
        elif name == "MESSAGE_GAME":
            self.messages.append(Reader(body).jstr())
        elif name == "HINT_ARROW":
            self._hint_arrow(body)
        elif name == "TUT_FLASH":
            self.side_flash = Reader(body).u8()
        elif name == "MESSAGE_PRIVATE":
            message = Reader(body)
            sender = base37_name(int.from_bytes(message.raw(8), "big"))
            message.raw(4)
            message.u8()
            self.private.append(ChatLine(sender, unpack(message.raw(message.remaining()))))
        elif name == "IF_OPENMAIN":
            self._open_modal("main", body)
        elif name == "IF_OPENCHAT":
            self._open_modal("chat", body)
        elif name == "IF_OPENSIDE":
            self._open_modal("side", body)
        elif name == "IF_OPENMAIN_SIDE":
            self._open_modal("main_side", body)
        elif name == "IF_OPENOVERLAY":
            self._open_modal("overlay", body)
        elif name == "IF_CLOSE":
            self.modal = None
            self.texts = {}
            self.pending_texts = {}
            self.modal_models = {}
            self.chat_interfaces = []
        elif name == "IF_SETTEXT":
            text = Reader(body)
            com = text.u16()
            value = text.jstr()
            self.texts[com] = value
            self.pending_texts[com] = value
        elif name == "IF_SETOBJECT":
            obj = Reader(body)
            com = obj.u16()
            obj_id = obj.u16()
            obj.u16()  # scale
            self.modal_models[com] = obj_id
        elif name == "P_COUNTDIALOG":
            self.count_dialog = True
        elif name == "SET_PLAYER_OP":
            op = Reader(body)
            slot = op.u8()
            self.player_ops[slot] = (op.u8(), op.jstr())
        elif name == "VARP_SMALL":
            varp = Reader(body)
            index = varp.u16()
            self.varps[index] = varp.u8()
        elif name == "VARP_LARGE":
            varp = Reader(body)
            index = varp.u16()
            self.varps[index] = varp.i32()
        elif name == "LOGOUT":
            self.logged_out = True
        elif name == "CHAT_FILTER_SETTINGS":
            pass

    def _hint_arrow(self, body):
        arrow = Reader(body)
        kind = arrow.u8()
        if kind == 1:
            self.hint = ("npc", arrow.u16())
        elif 2 <= kind <= 6:
            self.hint = ("coord", arrow.u16(), arrow.u16(), arrow.u8())
        elif kind == 10:
            self.hint = ("player", arrow.u16())
        elif kind == 255:
            self.hint = None

    def loc_at(self, x, z, level=None):
        if level is None:
            level = self.level
        if (x, z, level) in self.locs:
            return self.locs[(x, z, level)]
        return self.static_locs.get((x, z, level))

    @property
    def busy(self):
        return self.modal is not None
        return self.modal is not None

    def _player_info(self, body):
        stream = BitReader(body)
        self.ticks += 1
        self._pending = []
        self._new_pids = []
        self._read_own(stream)
        known = stream.bits(8)
        known_order = list(self.player_order)
        for index in range(known):
            pid = known_order[index] if index < len(known_order) else None
            self._read_known_player(stream, pid)
        while stream.remaining_bits() > 10:
            pid = stream.bits(11)
            if pid == 2047:
                break
            dx = signed(stream.bits(5), 5)
            dz = signed(stream.bits(5), 5)
            jump = stream.bits(1)
            extended = stream.bits(1)
            self.players[pid] = {"x": self.x + dx, "z": self.z + dz, "level": self.level, "jump": jump, "name": None}
            if pid not in self.player_order:
                self.player_order.append(pid)
            if extended:
                self._new_pids.append(pid)
        self._read_masks(stream, body, player=True)

    def _read_own(self, stream):
        if stream.bits(1) == 0:
            return
        kind = stream.bits(2)
        if kind == 0:
            self._pending.append("own")
        elif kind == 1:
            dx, dz = DIRS[stream.bits(3)]
            self.x += dx
            self.z += dz
            if stream.bits(1) == 1:
                self._pending.append("own")
        elif kind == 2:
            for _ in range(2):
                dx, dz = DIRS[stream.bits(3)]
                self.x += dx
                self.z += dz
            if stream.bits(1) == 1:
                self._pending.append("own")
        else:
            level = stream.bits(2)
            x = self.base_x + stream.bits(7)
            z = self.base_z + stream.bits(7)
            stream.bits(1)
            if level != self.level or abs(x - self.x) > 15 or abs(z - self.z) > 15:
                self.players = {}
                self.player_order = []
                self.npcs = {}
                self.npc_order = []
            self.level = level
            self.x = x
            self.z = z
            if stream.bits(1) == 1:
                self._pending.append("own")

    def _read_known_player(self, stream, pid):
        if stream.bits(1) == 0:
            return
        kind = stream.bits(2)
        if kind == 3:
            if pid in self.players:
                del self.players[pid]
            if pid in self.player_order:
                self.player_order.remove(pid)
            return
        if kind == 0:
            if pid is not None:
                self._pending.append(pid)
            return
        if pid is not None and pid in self.players:
            if kind == 1:
                dx, dz = DIRS[stream.bits(3)]
                self.players[pid]["x"] += dx
                self.players[pid]["z"] += dz
            elif kind == 2:
                for _ in range(2):
                    dx, dz = DIRS[stream.bits(3)]
                    self.players[pid]["x"] += dx
                    self.players[pid]["z"] += dz
        else:
            if kind == 1:
                stream.bits(3)
            elif kind == 2:
                stream.bits(6)
        if stream.bits(1) == 1 and pid is not None:
            self._pending.append(pid)

    def _npc_info(self, body):
        stream = BitReader(body)
        self._npc_pending = []
        self._npc_new = []
        self._npc_index = 0
        known = stream.bits(8)
        for _ in range(known):
            self._read_known_npc(stream)
        while stream.remaining_bits() > 21:
            nid = stream.bits(14)
            if nid == 16383:
                break
            ntype = stream.bits(11)
            dx = signed(stream.bits(5), 5)
            dz = signed(stream.bits(5), 5)
            jump = stream.bits(1)
            extended = stream.bits(1)
            self.npcs[nid] = {"type": ntype, "x": self.x + dx, "z": self.z + dz, "level": self.level, "name": None}
            if nid not in self.npc_order:
                self.npc_order.append(nid)
            if extended:
                self._npc_new.append(nid)
        self._read_masks(stream, body, player=False)

    def _read_known_npc(self, stream):
        nid = self.npc_order[self._npc_index] if self._npc_index < len(self.npc_order) else None
        self._npc_index += 1
        if stream.bits(1) == 0:
            return
        kind = stream.bits(2)
        if kind == 3:
            if nid in self.npcs:
                del self.npcs[nid]
            if nid in self.npc_order:
                self.npc_order.remove(nid)
            return
        if kind == 0:
            if nid is not None:
                self._npc_pending.append(nid)
            return
        if nid is not None and nid in self.npcs:
            if kind == 1:
                dx, dz = DIRS[stream.bits(3)]
                self.npcs[nid]["x"] += dx
                self.npcs[nid]["z"] += dz
            elif kind == 2:
                for _ in range(2):
                    dx, dz = DIRS[stream.bits(3)]
                    self.npcs[nid]["x"] += dx
                    self.npcs[nid]["z"] += dz
        else:
            if kind == 1:
                stream.bits(3)
            elif kind == 2:
                stream.bits(6)
        if stream.bits(1) == 1 and nid is not None:
            self._npc_pending.append(nid)

    def _read_masks(self, stream, body, player):
        if stream.remaining_bits() <= 0:
            return
        pad = (-stream._bit) % 8
        stream.bits(pad)
        offset = stream._bit // 8
        if offset >= len(body):
            # No mask byte left: the padded move bits are the whole body.
            return
        data = Reader(body[offset:])
        if player:
            for pid in self._pending + self._new_pids:
                self._read_mask_set(data, pid, player=True)
        else:
            for nid in self._npc_pending + self._npc_new:
                self._read_mask_set(data, nid, player=False)

    def _read_mask_set(self, data, pid, player):
        header = data.u8()
        if player and header & 0x80:
            # Server writes ip2(masks | BIG): 16-bit little endian, so the
            # flag is on the low byte and the high bits come second.
            header = (header | (data.u8() << 8)) & ~0x80
        if player:
            order = [0x1, 0x2, 0x4, 0x8, 0x10, 0x20, 0x40, 0x100, 0x200, 0x400]
        else:
            order = [0x1, 0x2, 0x4, 0x8, 0x10, 0x20, 0x40, 0x80]
        for bit in order:
            if not header & bit:
                continue
            self._read_block(data, bit, pid, player)

    def _read_block(self, data, bit, pid, player):
        if player and bit == 0x1:
            self._read_appearance(data, pid)
        elif not player and bit == 0x20:
            self.npcs[pid]["type"] = data.u16()
        elif bit in (0x2,):
            data.raw(3)
        elif bit == 0x4:
            data.u16()
        elif bit == 0x8:
            self.chat.append(ChatLine(self._who(pid, player), data.jstr()))
        elif bit in (0x10, 0x400):
            data.raw(4)
        elif bit == 0x1 and not player:
            data.raw(4)
        elif bit == 0x20 and player:
            data.raw(4)
        elif bit == 0x40 and player:
            color = data.u8()
            effect = data.u8()
            data.u8()
            length = data.u8()
            self.chat.append(ChatLine(self._who(pid, player), unpack(data.raw(length)), color, effect))
        elif bit == 0x40 and not player:
            data.raw(6)
        elif bit == 0x100:
            data.raw(6)
        elif bit == 0x200:
            data.raw(9)
        elif bit == 0x80 and not player:
            data.raw(4)

    def _who(self, pid, player):
        if pid == "own" or not player:
            return "me"
        entry = self.players.get(pid)
        return (entry and entry.get("name")) or ("player%d" % pid)

    def _read_appearance(self, data, pid):
        start = data.pos
        length = data.u8()
        data.u8()
        data.u8()
        if data.peek_u8() == 0xFF:
            data.u16()
            data.u16()
        else:
            for _ in range(12):
                if data.peek_u8() == 0:
                    data.pos += 1
                else:
                    data.pos += 2
        data.raw(5)
        data.raw(14)
        name = base37_name(int.from_bytes(data.raw(8), "big"))
        data.u8()
        data.u16()
        # The block is length-prefixed: the length is authoritative, so a
        # misread field can never shift the following mask bytes.
        data.pos = start + 1 + length
        if pid != "own" and pid in self.players:
            self.players[pid]["name"] = name

    def _trace_zone(self, name, payload):
        self.zone_trace.append({
            "name": name,
            "zone": (self.zone_x, self.zone_z),
            "base": (getattr(self, "base_x", 0), getattr(self, "base_z", 0)),
            "payload": payload.hex() if payload is not None else None,
        })
        if len(self.zone_trace) > 100:
            del self.zone_trace[:-100]

    def _open_modal(self, kind, body):
        interface_id = Reader(body).u16()
        self.modal = (kind, interface_id)
        if kind == "chat":
            if interface_id in self.chat_interfaces:
                self.chat_interfaces.remove(interface_id)
            self.chat_interfaces.append(interface_id)
            self.chat_interfaces = self.chat_interfaces[-8:]
        # The server sets an interface's texts just before opening it. Keep
        # only that batch, so options from the previous dialogue disappear.
        # Models arrive after the open; start fresh the same way.
        self.texts = dict(self.pending_texts)
        self.modal_models = {}
        self.pending_texts = {}
        self.chat_interfaces = []

    def _set_zone(self, local_x, local_z):
        self.zone_x = (local_x + self.base_x) >> 3
        self.zone_z = (local_z + self.base_z) >> 3

    def _zone_enclosed(self, body):
        stream = Reader(body)
        self._set_zone(stream.u8(), stream.u8())
        while stream.remaining():
            self._zone_message(stream.u8(), stream)

    def _zone_message(self, prot, stream):
        coord = stream.u8()
        x = (self.zone_x << 3) + (coord >> 4)
        z = (self.zone_z << 3) + (coord & 0xF)
        key = (x, z, self.level)
        if prot == 81:
            obj = stream.u16()
            self.objs[(key, obj)] = stream.u16()
        elif prot == 52:
            self.objs[(key, stream.u16())] = None
        elif prot == 95:
            obj = stream.u16()
            stream.u16()
            self.objs[(key, obj)] = stream.u16()
        elif prot == 219:
            obj = stream.u16()
            self.objs[(key, obj)] = stream.u16()
            stream.u16()
        elif prot == 138:
            shape_angle = stream.u8()
            self.locs[key] = (stream.u16(), shape_angle >> 2, shape_angle & 0x3)
        elif prot == 173:
            stream.u8()
            self.locs[key] = None
        elif prot == 48:
            stream.u8()
            stream.u16()
        elif prot == 176:
            stream.raw(13)
        elif prot == 85:
            stream.raw(5)
        elif prot == 107:
            stream.raw(14)
        else:
            raise ValueError("unknown zone prot %d" % prot)

    def _inv_full(self, body):
        inv = Reader(body)
        com = inv.u16()
        slots = inv.u8()
        items = {}
        for slot in range(slots):
            obj = inv.u16()
            count = inv.u8()
            if count == 255:
                count = int.from_bytes(inv.raw(4), "big")
            if obj:
                items[slot] = (obj - 1, count)
        self.inv[com] = items
        self.inv_cap[com] = slots

    def _inv_partial(self, body):
        inv = Reader(body)
        com = inv.u16()
        items = dict(self.inv.get(com, {}))
        while inv.remaining():
            slot = inv.u8()
            obj = inv.u16()
            count = inv.u8()
            if count == 255:
                count = int.from_bytes(inv.raw(4), "big")
            if obj:
                items[slot] = (obj - 1, count)
            else:
                items.pop(slot, None)
        self.inv[com] = items
