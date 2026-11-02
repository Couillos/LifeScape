"""Bot core: connection lifecycle, tick pump and live state snapshots.

Movement, actions and state are split into mixins (see the package
__init__); this module owns the session and the orchestrator heartbeat.
"""

import json
import os
import signal
import threading
import time

from .. import collision as collision_mod
from .. import route as route_mod
from .. import spots as spots_mod
from ..buffer import Writer
from ..connection import Connection, LoginError, default_paths
from ..protocol import client_op_id
from ..words import pack
from ..world import World

SAFE_POINT_TICKS = 500  # a queued SDK call waits at most this long for a pump
UNLOAD_TICKS = 250  # a script thread gets this long to notice the stop flag


class Unreachable(Exception):
    """No reachable tile, door, target or portal for the requested action."""


class StateError(RuntimeError):
    """The SDK cannot read or expose player state (bad component table, write)."""


class StopRequested(BaseException):
    """The session asked this script to stop (unload/close).

    BaseException so a script's ``except Exception`` cannot swallow it:
    only the session wrapper handles it.
    """


class StaleConnection(ConnectionError):
    """A send from a script thread that outlived its unload (orphan)."""


_active_bot = None


def _exit_on_terminate(_signum, _frame):
    # SIGTERM default kills the process outright, skipping every finally and
    # leaving the session to the server's idle timeout. PyPy ignores an
    # exception raised by a handler defined outside __main__, so close here
    # and exit hard instead of relying on the unwinding path.
    bot = _active_bot
    if bot is not None:
        try:
            bot.close()
        except (OSError, ConnectionError) as error:
            print("close during SIGTERM failed: %s" % (error,), flush=True)
    os._exit(143)


class BotCore:

    def __init__(self, host="127.0.0.1", port=43594):
        host = os.environ.get("LIFESCAPE_SERVER_HOST", host)
        port = int(os.environ.get("LIFESCAPE_SERVER_PORT", port))
        self.connection = Connection(host, port)
        self.world = World()
        self.zone = (0, 0)
        self.username = None
        self.password = None
        self.paths = None
        self._intent = ""
        self._state_path = None
        self._state_tick = 0
        self._state_written = 0.0
        self._state_error = None
        self._last_send_tick = 0
        self._send_lock = threading.Lock()
        self._keepalive_stop = threading.Event()
        self._keepalive_thread = None
        self._connection_epoch = 0
        self._commands = []
        self._commands_lock = threading.Lock()
        self._command_seq = 0
        self._stop_event = threading.Event()
        self._generation = 0
        self._muted_idents = set()
        self._snapshot = {}
        self._snapshot_lock = threading.Lock()
        self._com_ids = None
        self._static_loc_index = None
        self._static_obj_index = None
        self._route = []
        self._route_target = None
        self._action_route = []
        self._route_action = None
        self.collision = None
        self.loctypes = None
        self.wall_doors = None
        self.door_footprints = None
        self.stair_tiles = None
        self._blocked_doors = set()
        self._blocked_doors_exp = {}
        self._bad_transitions = {}
        self.unsealed_doors = set()
        self.recent_doors = {}
        self._opening = set()
        self._door_failures = {}
        self._blocked_tiles = {}
        self.opened_map = None
        self.planning_map = None
        self._planner = None
        self.router = route_mod.Router()
        self._portals = []
        self._in_portal_route = False
        # Verbose trace for pathfinding work: every move, click and ride.
        # On with LIFESCAPE_DEBUG=1 in the environment, or by creating the
        # run/debug file (no daemon restart needed: touch run/debug).
        run_dir = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "run"))
        self._debug = os.environ.get("LIFESCAPE_DEBUG") == "1" or os.path.exists(os.path.join(run_dir, "debug"))

    def dbg(self, message):
        if getattr(self, "_debug", False):
            print("DBG %s %s" % (self.world.position, message), flush=True)

    def login(self, username, password=None, **paths):
        global _active_bot
        try:
            signal.signal(signal.SIGTERM, _exit_on_terminate)
        except ValueError:
            pass  # not the main thread: no signal handler to install
        defaults = default_paths()
        defaults.update(paths)
        self.paths = defaults
        _active_bot = self
        self._state_path = os.environ.get("LIFESCAPE_STATE") or None
        self.username = username
        self.password = password or username
        self.connection.connect()
        self._track_connection()
        info = self._login_with_retry(defaults)
        self._drain_startup()
        self.intend("connected")
        self._ensure_keepalive()
        return info

    def _login_with_retry(self, defaults, attempts=15, spacing=2):
        """A crashed client can still hold its session on the server for a
        few seconds: wait it out instead of crashing the job. Attempts and
        spacing are a process guard in seconds: ticks are frozen outside a
        connection, so no tick bound can apply here."""
        last = None
        for _attempt in range(attempts):
            try:
                return self.connection.login(self.username, self.password, defaults["pack_dir"], defaults["server_key"])
            except LoginError as error:
                last = error
                if "already logged in" not in str(error):
                    raise
                print("session still busy, retrying login", flush=True)
                host, port = self.connection.host, self.connection.port
                try:
                    self.connection.close()
                except (OSError, ConnectionError) as close_error:
                    print("closing busy session failed: %s" % (close_error,), flush=True)
                time.sleep(spacing)
                self._new_connection(host, port)
        raise last

    def relogin(self, attempts=9, spacing=10):
        """Reconnect after a dropped connection, bounded. Attempts and
        spacing are a process guard in seconds: world.ticks is frozen with
        no connection, so no tick bound can apply here."""
        last = None
        for _attempt in range(attempts):
            self._reset_session()
            try:
                return self.login(self.username, self.password)
            except (LoginError, ConnectionError, OSError) as error:
                last = error
                print("relogin attempt failed: %s" % (error,), flush=True)
                try:
                    self.connection.close()
                except (OSError, ConnectionError) as close_error:
                    print("closing failed session failed: %s" % (close_error,), flush=True)
                time.sleep(spacing)
        raise last

    def _reset_session(self):
        try:
            self.connection.close()
        except (OSError, ConnectionError) as error:
            print("closing session before relogin failed: %s" % (error,), flush=True)
        host, port = self.connection.host, self.connection.port
        self._new_connection(host, port)
        self.world = World()
        self._last_send_tick = 0  # fresh ticks: never mute keepalives after a relogin
        self.zone = (0, 0)
        self.collision = None
        self.loctypes = None
        self._static_loc_index = None
        self._static_obj_index = None
        self.wall_doors = None
        self.door_footprints = None
        self._blocked_doors = set()
        self._blocked_doors_exp = {}
        self._bad_transitions = {}
        self.unsealed_doors = set()
        self.recent_doors = {}
        self._opening = set()
        self._door_failures = {}
        self._blocked_tiles = {}
        self.opened_map = None
        self.planning_map = None
        self._planner = None
        self._route = []
        self._route_target = None
        self._action_route = []
        self._route_action = None

    def _new_connection(self, host, port):
        """Swap the socket and guard sends against the previous epoch.

        A script thread that survives its unload (CPU loop without pump)
        keeps a reference to the old wrapper: its sends raise instead of
        reaching the server on the recycled session.
        """
        self.connection = Connection(host, port)
        self.connection.connect()
        self._track_connection()

    def _track_connection(self):
        """Guard sends against the previous epoch. (see _new_connection)."""
        self._connection_epoch += 1
        epoch = self._connection_epoch
        raw_send = Connection.send.__get__(self.connection)

        def send(op_id, payload=b""):
            with self._send_lock:
                if epoch != self._connection_epoch:
                    raise StaleConnection("stale send from an unloaded script thread")
                if threading.get_ident() in self._muted_idents:
                    raise StaleConnection("stale send from an unloaded script thread")
                raw_send(op_id, payload)
                self._last_send_tick = self.world.ticks

        self.connection.send = send

    def note_run_thread(self):
        """A fresh script thread owns its ident again (idents get recycled)."""
        self._muted_idents.discard(threading.get_ident())

    def _ensure_keepalive(self):
        """Background NO_TIMEOUT sender for planning stalls.

        The server drops a session after 100 ticks without any response
        (2 s at the 20 ms tick rate): a long planning phase with no pump
        would look like a dead client. This thread answers every 0.5 s of
        wall clock while nothing else was sent in the last 40 ticks. It is
        connection maintenance, not game pacing: all sends stay serialized
        through the send lock so the cipher stream never interleaves.
        """
        if self._keepalive_thread is not None and self._keepalive_thread.is_alive():
            return
        self._keepalive_stop.clear()
        self._keepalive_thread = threading.Thread(target=self._keepalive_loop, daemon=True)
        self._keepalive_thread.start()

    def _keepalive_due(self):
        """True when no send happened for 40 ticks or 1 s of wall clock.

        Ticks freeze with no pump (long planning), so the tick gate alone
        would never fire mid-stall: the wall backstop covers it.
        """
        connection = self.connection
        if connection is None or getattr(connection, "sock", None) is None:
            return False
        if connection.outgoing is None:
            return False
        return self.world.ticks - self._last_send_tick >= 40 or time.time() - connection.last_send >= 1.0

    def _keepalive_loop(self):
        while not self._keepalive_stop.wait(0.5):
            try:
                if self._keepalive_due():
                    self.idle()
            except (OSError, ConnectionError):
                pass

    def combat_level(self):
        from . import snapshot as snapshot_mod

        return snapshot_mod.combat_level(self.world.stats)

    def want_run(self):
        return self.world.energy >= 100

    def say(self, text, color=0, effect=0):
        body = Writer()
        body.u8(color)
        body.u8(effect)
        body.raw(pack(text))
        self.connection.send(client_op_id("MESSAGE_PUBLIC"), body.bytes())

    def idle(self):
        # NO_TIMEOUT, never IDLE_TIMER: the server treats IDLE_TIMER as an
        # idle-logout request and drops us, while NO_TIMEOUT has no handler
        # and simply counts as a response, keeping long computations alive.
        self.connection.send(client_op_id("NO_TIMEOUT"))

    def pump(self, count=1):
        self._check_stop()
        self._drain_command()
        frames = []
        for _ in range(count):
            # Keepalive in ticks: the server counts timeouts in ticks, so a
            # long planning phase without sends must still answer regularly.
            if self.connection.outgoing is not None and self.world.ticks - self._last_send_tick >= 50:
                self.idle()
        for _ in range(count):
            opcode, name, body = self.connection.recv()
            self.world.feed(opcode, name, body)
            if name == "REBUILD_NORMAL":
                self.zone = (self.world.zone_x, self.world.zone_z)
            frames.append((opcode, name, body))
            # Keep up with a scene rebuild: read everything already waiting,
            # without waiting for the next tick (a slow client gets dropped).
            for _pending in range(256):
                if not self.connection.available():
                    break
                if self.connection.outgoing is not None and self.world.ticks - self._last_send_tick >= 50:
                    self.idle()
                opcode, name, body = self.connection.recv()
                self.world.feed(opcode, name, body)
                if name == "REBUILD_NORMAL":
                    self.zone = (self.world.zone_x, self.world.zone_z)
                frames.append((opcode, name, body))
        self._check_stop()
        self._drain_command()
        # The dashboard follows this file: write often enough to look live,
        # paced in ticks so a slow tick rate never makes it stutter.
        if self._state_path is not None and self.world.ticks - self._state_tick >= 10:
            self._write_state()
        return frames

    def request_stop(self):
        """Ask the script thread to stop at its next pump (unload/close)."""
        self._stop_event.set()

    def stop_requested(self):
        return self._stop_event.is_set()

    def clear_stop(self):
        self._stop_event.clear()

    def invalidate_sends(self, thread=None):
        """Mute an unloaded thread's future sends without touching the socket.

        Last resort when the script thread survives its tick bound (CPU loop
        with no pump): rewrapping alone cannot mute it, because it looks up
        the shared wrapper on every send. The muted ident belongs to the
        orphan thread itself, so the next script (new ident) sends normally.
        """
        if thread is not None:
            ident = thread.ident if hasattr(thread, "ident") else thread
            if ident is not None:
                self._muted_idents.add(ident)
        self._track_connection()

    def _check_stop(self):
        if self._stop_event.is_set():
            raise StopRequested("session asked the script to stop")

    def queue_command(self, method, args):
        """Enqueue one SDK call for the next pump safe-point. Returns the
        command record; the pumper fills result/error and sets done."""
        with self._commands_lock:
            self._command_seq += 1
            command = {
                "id": self._command_seq,
                "method": method,
                "args": list(args),
                "start_tick": self.world.ticks,
                "cancelled": False,
                "started": threading.Event(),
                "done": threading.Event(),
                "result": None,
                "error": None,
            }
            self._commands.append(command)
        return command

    def drop_command(self, command):
        """Cancel a queued call that waited past its tick bound."""
        with self._commands_lock:
            command["cancelled"] = True
            if command in self._commands:
                self._commands.remove(command)
        command["done"].set()

    def _drain_command(self):
        """Run one queued SDK call on this (pumping) thread: the safe-point."""
        if getattr(self, "_draining", False):
            return
        with self._commands_lock:
            command = next((item for item in self._commands if not item["cancelled"]), None)
            if command is not None:
                self._commands.remove(command)
        if command is None:
            return
        command["started"].set()
        self._draining = True
        try:
            try:
                target = getattr(self, command["method"])
            except AttributeError:
                command["error"] = "unknown method %r" % (command["method"],)
            else:
                if not callable(target) or command["method"].startswith("_"):
                    command["error"] = "unknown method %r" % (command["method"],)
                else:
                    try:
                        command["result"] = target(*command["args"])
                    except Exception as error:
                        command["error"] = "%s: %s" % (type(error).__name__, error)
                        import traceback
                        traceback.print_exc()
        finally:
            self._draining = False
            command["done"].set()

    def snapshot(self):
        """Last heartbeat's raw state, for serving reads off the pump thread."""
        import copy

        with self._snapshot_lock:
            return copy.deepcopy(self._snapshot)

    def intend(self, intention):
        """Record what the bot is doing right now (shown by the orchestrator)."""
        self._intent = intention
        self._write_state()

    def _write_state(self):
        if self._state_path is None:
            return
        self._ensure_names()
        self._state_tick = self.world.ticks
        self._state_written = time.time()
        state = {
            "account": self.username,
            "script": os.environ.get("LIFESCAPE_SCRIPT", ""),
            "pid": os.getpid(),
            "intention": self._intent,
            "position": self.world.position,
            "hitpoints": self.world.stats.get(3, (1, 0))[0],
            "ticks": self.world.ticks,
            "updated": time.time(),
            "modal": self.world.modal,
            "combat": spots_mod.combat_level({index: level for index, (level, _xp) in self.world.stats.items()}),
            "energy": self.world.energy,
            "hint": self.world.hint,
            "hint_type": self._hint_type(),
            "flash": self.world.side_flash,
            "route": self._action_route or self._route,
            "route_target": self._route_target,
            "route_action": self._route_action,
            "skills": [
                {"id": index, "level": level, "xp": xp}
                for index, (level, xp) in sorted(self.world.stats.items())
            ],
            "npcs": self._state_npcs(),
            "locs": self._state_locs(),
            "objs": self._state_objs(),
            "players": [
                {"name": player.get("name") or "?", "x": player["x"], "z": player["z"]}
                for player in self.world.players.values()
                if player.get("name")
            ][:20],
            "inv": self._state_items("inventory:inv"),
            "worn": self._state_items("wornitems:wear"),
            "models": dict(self.world.modal_models),
            "bank": self._state_items("bank_main:inv"),
            "chat": [line.text for line in self.world.chat[-6:]],
        }
        try:
            temporary = self._state_path + ".tmp"
            with open(temporary, "w") as handle:
                json.dump(state, handle, sort_keys=True)
            os.replace(temporary, self._state_path)
            self._state_error = None
        except OSError as error:
            if self._state_error != str(error):
                self._state_error = str(error)
                print("state write failed (%s): %s" % (self._state_path, error), flush=True)
        import copy

        with self._snapshot_lock:
            self._snapshot = {
                "inv": copy.deepcopy(self.world.inv),
                "stats": dict(self.world.stats),
                "texts": dict(self.world.texts),
                "position": tuple(self.world.position),
                "modal": self.world.modal,
                "com_ids": dict(self._com_ids or {}),
                "inv_cap": dict(self.world.inv_cap),
            }

    def _hint_type(self):
        hint = self.world.hint
        if hint and hint[0] == "npc":
            npc = self.world.npcs.get(hint[1])
            return npc["type"] if npc else None
        return None

    def _state_npcs(self):
        found = []
        for nid, npc in self.world.npcs.items():
            if abs(npc["x"] - self.world.x) + abs(npc["z"] - self.world.z) > 12:
                continue
            entry = None
            if self.loctypes is not None and npc["type"] < len(self.loctypes["npc"]):
                entry = self.loctypes["npc"][npc["type"]]
            found.append({"nid": nid, "type": npc["type"], "name": (entry["name"] if entry else "") or "", "x": npc["x"], "z": npc["z"]})
        found.sort(key=lambda item: abs(item["x"] - self.world.x) + abs(item["z"] - self.world.z))
        return found[:25]

    def _static_near(self, source, cache_name, key_value):
        """(tile, value) for the static map entries of the 3x3 zones around
        the bot. Built once: the cache holds ~750k locs, the canvas only
        needs the ones the bot can see."""
        index = getattr(self, cache_name)
        if index is None:
            if not source:
                return []  # map not loaded yet; do not cache an empty index
            index = {}
            for key, value in source.items():
                tile, tile_value = key_value(key, value)
                x, z, level = tile
                index.setdefault((level, x >> 3, z >> 3), []).append(((x, z), tile_value))
            setattr(self, cache_name, index)
        zone_x, zone_z = self.world.x >> 3, self.world.z >> 3
        found = []
        for dz in (-1, 0, 1):
            for dx in (-1, 0, 1):
                found.extend(index.get((self.world.level, zone_x + dx, zone_z + dz), ()))
        return found

    def _static_locs_near(self):
        return self._static_near(self.world.static_locs, "_static_loc_index", lambda key, value: (key, value[0]))

    def _static_objs_near(self):
        return self._static_near(self.world.static_objs, "_static_obj_index", lambda key, value: (key[0], (key[1], value)))

    def _state_locs(self):
        level = self.world.level
        tiles = {}
        for (loc_x, loc_z), loc_id in self._static_locs_near():
            tiles[(loc_x, loc_z)] = loc_id
        for (loc_x, loc_z, loc_level), value in self.world.locs.items():
            if loc_level != level:
                continue
            if abs(loc_x - self.world.x) + abs(loc_z - self.world.z) > 8:
                continue
            if value is None:
                tiles.pop((loc_x, loc_z), None)
            else:
                tiles[(loc_x, loc_z)] = value[0]
        found = []
        for (loc_x, loc_z), loc_id in tiles.items():
            if abs(loc_x - self.world.x) + abs(loc_z - self.world.z) > 8:
                continue
            entry = None
            if self.loctypes is not None and loc_id < len(self.loctypes["loc"]):
                entry = self.loctypes["loc"][loc_id]
            name = (entry["name"] if entry else "") or ""
            if not name or name.startswith("null"):
                continue
            ops = entry.get("ops") if entry else None
            found.append({
                "name": name,
                "x": loc_x,
                "z": loc_z,
                "id": loc_id,
                "act": bool(ops and any(ops)),
                "block": bool(entry.get("blockwalk")) if entry else False,
            })
        found.sort(key=lambda loc: abs(loc["x"] - self.world.x) + abs(loc["z"] - self.world.z))
        return found[:400]

    def _state_objs(self):
        level = self.world.level
        tiles = {}
        for (obj_x, obj_z), (obj_id, count) in self._static_objs_near():
            tiles[(obj_x, obj_z, obj_id)] = count
        for (coord, obj_id), count in self.world.objs.items():
            obj_x, obj_z, obj_level = coord
            if obj_level != level:
                continue
            if abs(obj_x - self.world.x) + abs(obj_z - self.world.z) > 8:
                continue
            if not count:
                tiles.pop((obj_x, obj_z, obj_id), None)
            else:
                tiles[(obj_x, obj_z, obj_id)] = count
        found = []
        for (obj_x, obj_z, obj_id), count in tiles.items():
            if abs(obj_x - self.world.x) + abs(obj_z - self.world.z) > 8:
                continue
            entry = None
            if self.loctypes is not None and obj_id < len(self.loctypes["obj"]):
                entry = self.loctypes["obj"][obj_id]
            found.append({"name": (entry["name"] if entry else "") or "", "x": obj_x, "z": obj_z, "id": obj_id, "count": count})
        found.sort(key=lambda obj: abs(obj["x"] - self.world.x) + abs(obj["z"] - self.world.z))
        return found[:120]

    def _ensure_names(self):
        """Load the type table for snapshot names without building nav maps.

        Snapshots are written from login on, long before the first walk;
        jobs that never move (give/probe) would otherwise publish nameless
        items, NPCs and scenery to the dashboard.
        """
        if self.loctypes is not None:
            return
        if not self.paths or not self.paths.get("types"):
            return
        try:
            self.loctypes = collision_mod.load_types(self.paths["types"])
        except (OSError, ValueError):
            pass

    def _state_items(self, com_name):
        com = self._com_id(com_name)
        slots = self.world.inv.get(com, {})
        if not slots and com_name == "inventory:inv":
            _com, slots = self.backpack()
        found = []
        for _slot, (held, count) in sorted(slots.items()):
            entry = None
            if self.loctypes is not None and held < len(self.loctypes["obj"]):
                entry = self.loctypes["obj"][held]
            found.append({"id": held, "name": (entry["name"] if entry else "") or "", "count": count})
        return found

    def close(self):
        global _active_bot
        self._keepalive_stop.set()
        thread, self._keepalive_thread = self._keepalive_thread, None
        if thread is not None and thread is not threading.current_thread():
            thread.join(2)
        self._intent = "closed"
        try:
            self._write_state()
        except StateError as error:
            print("state write before close failed: %s" % (error,), flush=True)
        try:
            self._request_logout()
        finally:
            try:
                self.connection.close()
            except (OSError, ConnectionError) as error:
                print("closing connection failed: %s" % (error,), flush=True)
            if _active_bot is self:
                _active_bot = None

    def _com_id(self, name):
        if self._com_ids is None:
            if not self.paths or not self.paths.get("comps"):
                raise StateError("component table path is not configured")
            path = self.paths["comps"]
            try:
                with open(path) as handle:
                    rows = json.load(handle)
            except OSError as error:
                raise StateError("cannot read component table %r: %s" % (path, error)) from error
            except ValueError as error:
                raise StateError("invalid component table %r: %s" % (path, error)) from error
            self._com_ids = {}
            for row in rows:
                if row and row.get("name"):
                    self._com_ids[row["name"]] = row["id"]
        return self._com_ids.get(name)

    def _logout_com_id(self):
        return self._com_id("logout:try_logout")

    def _request_logout(self, timeout=8):
        """Run the server's logout trigger, so the session is released at once
        instead of lingering until the server's idle/no-response timeout.
        Both timeouts below are socket/teardown guards (the connection is
        already closing, not game pacing), so they stay wall-clock."""
        if self.paths is None or self.world.ticks <= 0 or self.connection.sock is None:
            return
        try:
            com = self._logout_com_id()
            if com is None:
                print("logout trigger missing from the component table", flush=True)
                return
            self.if_button(com)
            self.connection.sock.settimeout(1)
            deadline = time.time() + timeout
            while time.time() < deadline:
                try:
                    self.connection.recv()
                except (ConnectionError, OSError):
                    return
        except StateError as error:
            print("logout trigger unavailable: %s" % (error,), flush=True)
        except (ConnectionError, OSError) as error:
            print("logout request failed: %s" % (error,), flush=True)

    @property
    def position(self):
        return self.world.position

    def _drain_startup(self):
        for _ in range(500):
            for _opcode, name, _body in self.pump(1):
                if name == "REBUILD_NORMAL":
                    self.zone = (self.world.zone_x, self.world.zone_z)
                if self.world.ticks > 0:
                    return
