"""Lightweight web view over the orchestrator: live bots, state, spectate.

Stdlib only. Served by the daemon on LIFESCAPE_WEB_PORT (default 8765):

    /                    dashboard (accounts, intention, status)
    /spectator.js        dashboard script (map, panels, session controls)
    /items               holdings page
    /items.js            holdings chart script
    /api/status          orchestrator jobs
    /api/scripts         launchable script names
    /api/accounts        known accounts (open sessions + state on disk)
    /api/open|run|unload|pause|resume|close (POST)
                         session control: create, launch, stop, freeze,
                         unfreeze, cut the connection
    /api/state/<account> merged state + bot snapshot (spectate payload)
    /api/logs/<account>?lines=60   log tail
    /ws                  everything the dashboard needs: the page never
                         polls HTTP, one socket pushes jobs, state and logs

The HTML/JS lives in orchestration/static/ and is re-read from disk on
every request, so front edits show up without restarting the daemon.
web.py itself stays logic-only (no embedded pages).

The right side mirrors the game server's /spectate: a canvas map with
the same tile grid, dots for players/npcs, squares for scenery and
ground items, a hint cross, and the same info/chat/inventory/skills
panels, in the server's black/yellow style. The account list collapses,
the layout is responsive, and the log follows new lines only while the
user is already at the bottom.
"""

import base64
import hashlib
import json
import os
import select
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

from . import mapcache
from .minimap import Minimap

MAP_CACHE = mapcache.MapCache()
MAP_MINIMAP = Minimap(MAP_CACHE)

WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
WS_INTERVAL = 0.2


def _ws_frame(payload, opcode=0x1):
    header = bytearray([0x80 | opcode])
    length = len(payload)
    if length < 126:
        header.append(length)
    elif length < 65536:
        header.append(126)
        header += length.to_bytes(2, "big")
    else:
        header.append(127)
        header += length.to_bytes(8, "big")
    return bytes(header) + payload


def _ws_read(sock, length):
    data = bytearray()
    while len(data) < length:
        chunk = sock.recv(length - len(data))
        if not chunk:
            raise ConnectionError("websocket closed")
        data += chunk
    return bytes(data)


def _ws_recv(sock):
    head = _ws_read(sock, 2)
    opcode = head[0] & 0x0F
    masked = head[1] & 0x80
    length = head[1] & 0x7F
    if length == 126:
        length = int.from_bytes(_ws_read(sock, 2), "big")
    elif length == 127:
        length = int.from_bytes(_ws_read(sock, 8), "big")
    mask = _ws_read(sock, 4) if masked else b"\x00\x00\x00\x00"
    data = _ws_read(sock, length) if length else b""
    if masked:
        data = bytes(value ^ mask[index % 4] for index, value in enumerate(data))
    return opcode, data


def _annotate_mapscene(state):
    """loc id -> mapscene sprite and its own origin, so the browser can draw it."""
    for loc in state.get("locs") or []:
        sprite = MAP_CACHE.mapscene_for(loc.get("id", -1))
        loc["mapscene"] = sprite
        if sprite >= 0:
            meta = MAP_CACHE.mapscene_size(sprite) or (0, 0)
            offset = MAP_CACHE.mapscene_origin(sprite)
            loc["mapsceneX"], loc["mapsceneY"] = offset
        else:
            loc["mapsceneX"], loc["mapsceneY"] = 0, 0
    return state


_LOC_FIELDS = ("x", "z", "id", "shape", "angle", "act", "active", "block", "width", "length", "mapscene", "mapsceneX", "mapsceneY", "cosmetic")


def _heavy_signature(state):
    """Cheap signature of the fields that only change when the bot moves.

    ``locs`` is by far the biggest part of the payload (hundreds of entries),
    so it is only resent when this signature changes instead of five times a
    second.
    """
    locs = state.get("locs") or []
    signature = [(loc.get("x"), loc.get("z"), loc.get("id"), loc.get("shape"), loc.get("angle")) for loc in locs]
    return (state.get("position"), state.get("zone"), len(locs), hash(tuple(signature)))


def _compact_locs(state):
    """Encode locs as arrays plus one id -> name table.

    The dict form repeats the key names of ~14 fields for hundreds of locs,
    which dominated the websocket traffic.
    """
    locs = state.get("locs")
    if not locs:
        return
    names = {}
    table = []
    for loc in locs:
        loc_id = loc.get("id")
        if loc_id not in names:
            names[loc_id] = loc.get("name") or ""
        row = []
        for field in _LOC_FIELDS:
            value = loc.get(field)
            row.append(int(value) if isinstance(value, bool) else value)
        table.append(row)
    state["locs"] = table
    state["loc_names"] = names
    state["loc_fields"] = list(_LOC_FIELDS)


_JOB_LIST_FIELDS = (
    "account",
    "account_name",
    "combat",
    "position",
    "modal",
    "intention",
    "status",
    "paused",
    "phase",
    "script",
    "hitpoints",
    "zone",
    "ticks",
    "last_error",
    "last_event",
    "updated",
)


def _state_payload(dispatch, account, previous_signature=None):
    jobs = dispatch({"cmd": "status"}).get("jobs") or []
    # the job list only needs the few fields the sidebar draws: the rest
    # (locs, inv, npcs...) duplicates the followed account's state and used
    # to make every frame ~100 KB
    payload = {"jobs": [{key: job.get(key) for key in _JOB_LIST_FIELDS} for job in jobs]}
    if account:
        state = dict((dispatch({"cmd": "live", "account": account}) or {}).get("state") or {})
        _annotate_mapscene(state)
        signature = _heavy_signature(state)
        if signature == previous_signature:
            # nothing moved: the browser keeps what it already has
            state.pop("locs", None)
            state["partial"] = True
        else:
            _compact_locs(state)
        payload["state"] = state
        payload["signature"] = signature
    return payload


def ws_session(sock, dispatch):
    """Push jobs + the followed account's state until the client leaves.

    The log tail rides along when the file grows, so the page never polls
    HTTP: one socket carries jobs, state and logs.
    """
    from . import paths as _paths
    account = ""
    last_send = 0.0
    log_mark = None
    signature = None
    while True:
        now = time.time()
        if now - last_send >= WS_INTERVAL:
            try:
                payload = _state_payload(dispatch, account, signature)
                signature = payload.pop("signature", None)
                if account:
                    try:
                        _merged, _partial, log_path = _paths.user_paths(account)
                        stat = os.stat(log_path)
                        mark = (stat.st_mtime_ns, stat.st_size)
                    except OSError:
                        mark = None
                    if mark is not None and mark != log_mark:
                        try:
                            reply = dispatch({"cmd": "logs", "account": account, "lines": 120})
                            lines = reply.get("lines", []) if isinstance(reply, dict) else []
                        except Exception:
                            lines = None
                        if lines is not None:
                            payload["logs"] = lines
                            log_mark = mark
                sock.sendall(_ws_frame(json.dumps(payload).encode()))
            except OSError:
                return
            last_send = now
        readable, _writable, _error = select.select([sock], [], [], WS_INTERVAL)
        if not readable:
            continue
        try:
            opcode, data = _ws_recv(sock)
        except (OSError, ConnectionError):
            return
        if opcode == 0x8:
            return
        if opcode == 0x9:
            try:
                sock.sendall(_ws_frame(data, 0xA))
            except OSError:
                return
        elif opcode == 0x1:
            try:
                request = json.loads(data.decode() or "{}")
            except ValueError:
                continue
            account = request.get("account") or ""


STATIC_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "static")


def _asset(name):
    """Read a front file from disk on every request.

    The dashboard HTML/JS lives in orchestration/static/ so front edits
    show up without restarting the daemon: web.py itself is imported once
    at daemon start, but these files are re-read per request.
    """
    path = os.path.join(STATIC_DIR, name)
    with open(path, "rb") as handle:
        return handle.read()


class WebHandler(BaseHTTPRequestHandler):
    dispatch = None

    def do_GET(self):
        parsed = urlparse(self.path)
        parts = [part for part in parsed.path.split("/") if part]
        try:
            if not parts:
                return self.reply(200, _asset("index.html"), "text/html; charset=utf-8")
            if parts == ["spectator.js"]:
                return self.reply(200, _asset("spectator.js"), "application/javascript")
            if parts[:2] == ["api", "status"]:
                return self.reply_json(self.dispatch({"cmd": "status"}))
            if parts[:2] == ["api", "state"]:
                account = parts[2] if len(parts) > 2 else ""
                reply = self.dispatch({"cmd": "show", "account": account})
                state = reply.get("state") if isinstance(reply, dict) else None
                _annotate_mapscene(state)
                return self.reply_json(reply)
            if parts == ["items"]:
                return self.reply(200, _asset("items.html"), "text/html; charset=utf-8")
            if parts == ["items.js"]:
                return self.reply(200, _asset("items.js"), "application/javascript")
            if parts == ["minimap.png"]:
                query = parse_qs(parsed.query)
                level = int((query.get("level") or ["0"])[0])
                x = int((query.get("x") or ["0"])[0])
                z = int((query.get("z") or ["0"])[0])
                size = int((query.get("size") or ["33"])[0])
                return self.reply(
                    200,
                    MAP_MINIMAP.png(level, x, z, size),
                    "image/png",
                    cache_control="no-store",
                )
            if len(parts) == 2 and parts[0] == "mapscene" and parts[1].endswith(".png"):
                sprite = MAP_CACHE.mapscene_png(int(parts[1][:-4])) if parts[1][:-4].isdigit() else None
                if sprite is None:
                    return self.reply_json({"ok": False, "error": "not found"}, status=404)
                return self.reply(200, sprite, "image/png", cache_control="public, max-age=86400")
            if parts == ["api", "items", "history"]:
                reply = self.dispatch({"cmd": "items"})
                return self.reply_json(reply.get("data", reply))
            if parts == ["api", "scripts"]:
                return self.reply_json(self.dispatch({"cmd": "scripts"}))
            if parts == ["api", "accounts"]:
                return self.reply_json(self.dispatch({"cmd": "accounts"}))
            if parts[:2] == ["api", "logs"]:
                account = parts[2] if len(parts) > 2 else ""
                lines = int(parse_qs(parsed.query).get("lines", ["60"])[0])
                return self.reply_json(self.dispatch({"cmd": "logs", "account": account, "lines": lines}))
            if parts == ["ws"]:
                return self.upgrade_websocket()
            return self.reply_json({"ok": False, "error": "not found"}, status=404)
        except Exception as error:
            return self.reply_json({"ok": False, "error": repr(error)}, status=500)

    def do_POST(self):
        parsed = urlparse(self.path)
        parts = [part for part in parsed.path.split("/") if part]
        try:
            length = int(self.headers.get("Content-Length") or 0)
        except ValueError:
            length = 0
        try:
            body = self.rfile.read(length) if length else b"{}"
            payload = json.loads(body.decode() or "{}") if body else {}
        except ValueError:
            return self.reply_json({"ok": False, "error": "invalid JSON"}, status=400)
        if not isinstance(payload, dict):
            return self.reply_json({"ok": False, "error": "invalid JSON"}, status=400)
        try:
            if parts[:2] == ["api", "open"]:
                return self.reply_json(self.dispatch({"cmd": "open", "account": payload.get("account") or ""}))
            if parts[:2] == ["api", "run"]:
                return self.reply_json(self.dispatch({
                    "cmd": "run", "account": payload.get("account") or "",
                    "script": payload.get("script") or "",
                    "args": payload.get("args") or [],
                }))
            if parts[:2] == ["api", "unload"]:
                return self.reply_json(self.dispatch({"cmd": "unload", "account": payload.get("account") or ""}))
            if parts[:2] == ["api", "pause"]:
                return self.reply_json(self.dispatch({"cmd": "pause", "account": payload.get("account") or ""}))
            if parts[:2] == ["api", "resume"]:
                return self.reply_json(self.dispatch({"cmd": "resume", "account": payload.get("account") or ""}))
            if parts[:2] == ["api", "close"]:
                return self.reply_json(self.dispatch({"cmd": "close", "account": payload.get("account") or ""}))
            return self.reply_json({"ok": False, "error": "not found"}, status=404)
        except Exception as error:
            return self.reply_json({"ok": False, "error": repr(error)}, status=500)

    def upgrade_websocket(self):
        key = self.headers.get("Sec-WebSocket-Key") or ""
        if not key:
            return self.reply_json({"ok": False, "error": "websocket key required"}, status=400)
        accept = base64.b64encode(hashlib.sha1((key + WS_GUID).encode()).digest()).decode()
        self.send_response(101)
        self.send_header("Upgrade", "websocket")
        self.send_header("Connection", "Upgrade")
        self.send_header("Sec-WebSocket-Accept", accept)
        self.end_headers()
        self.wfile.flush()
        self.close_connection = True
        try:
            ws_session(self.connection, self.dispatch)
        except (OSError, ConnectionError):
            pass

    def reply(self, status, body, content_type, cache_control="no-store"):
        payload = body.encode() if isinstance(body, str) else body
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Cache-Control", cache_control)
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def reply_json(self, data, status=200):
        self.reply(status, json.dumps(data), "application/json")

    def log_message(self, _format, *_args):
        pass


def start_web(dispatch, port, host="0.0.0.0"):
    handler = type("BoundWebHandler", (WebHandler,), {"dispatch": staticmethod(dispatch)})
    server = ThreadingHTTPServer((host, port), handler)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    print("webapp listening on http://0.0.0.0:%d/" % port, flush=True)
    return server


if __name__ == "__main__":
    # The daemon hosts this server; run `orchestrator.py daemon` instead.
    print("the webapp is served by orchestration.daemon")
