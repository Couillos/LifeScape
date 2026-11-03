"""Lightweight web view over the orchestrator: live bots, state, spectate.

Stdlib only. Served by the daemon on LIFESCAPE_WEB_PORT (default 8765):

    /                    dashboard (accounts, intention, status)
    /api/status          orchestrator jobs
    /api/state/<account> merged state + bot snapshot (spectate payload)
    /api/logs/<account>?lines=60   log tail
    /ws                  everything the dashboard needs: the page never
                         polls HTTP, one socket pushes jobs, state and logs

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

MAP_CACHE = mapcache.MapCache()

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
        state = (dispatch({"cmd": "live", "account": account}) or {}).get("state") or {}
        _annotate_mapscene(state)
        position = state.get("position")
        if position:
            try:
                state["map"] = MAP_CACHE.grid(position[0], position[1], position[2])
            except Exception:
                state.pop("map", None)
        signature = _heavy_signature(state)
        if signature == previous_signature:
            # nothing moved: the browser keeps what it already has
            state.pop("locs", None)
            state.pop("map", None)
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


INDEX_HTML = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Spectator - LifeScape bots</title>
<style>
  :root { color-scheme: dark; }
  body { font-family: Arial, Helvetica, sans-serif; font-size: 13px; color: #eee; background: #000; margin: 0; padding: 12px; }
  a { color: #04A800; }
  h1 { font-size: 18px; margin: 0; }
  h2 { font-size: 13px; margin: 10px 0 4px; color: #ff0; font-weight: 700; }
  #bar { display: flex; gap: 12px; align-items: center; justify-content: space-between; flex-wrap: wrap; margin-bottom: 10px; }
  #bar .meta { color: #888; font-size: 12px; }
  button { font: inherit; color: #eee; background: #1b1b1b; border: 1px solid #444; border-radius: 3px; padding: 3px 9px; cursor: pointer; }
  button:hover { background: #262626; }
  #layout { display: flex; gap: 16px; flex-wrap: wrap; align-items: flex-start; }
  #list { min-width: 210px; max-width: 300px; flex: 1 1 210px; }
  body.list-hidden #list { display: none; }
  #list ul { list-style: none; padding: 0; margin: 8px 0; max-height: 70vh; overflow: auto; }
  #list li { margin: 3px 0; }
  #list li a { text-decoration: none; }
  #list li a.cur { font-weight: bold; color: #ff0; }
  .dot { width: 8px; height: 8px; border-radius: 50%; display: inline-block; margin-right: 6px; background: #555; }
  .running { background: #46c46f; } .reconnecting, .busy, .starting { background: #d9a441; }
  .failed, .disconnected { background: #e05a5a; } .finished, .stopped { background: #6b7280; }
  #detail { flex: 2 1 420px; min-width: 300px; }
  #title { color: #888; margin-bottom: 6px; }
  #zonename { color: #ff0; margin-left: 8px; }
  canvas { background: #111; border: 1px solid #444; image-rendering: pixelated; width: 100%; max-width: 560px; aspect-ratio: 1; display: block; }
  #side { flex: 1 1 280px; min-width: 260px; max-width: 420px; }
  .panel { background: #0a0a0a; border: 1px solid #333; padding: 6px; margin-top: 6px; max-height: 200px; overflow-y: auto; font-size: 12px; }
  .panel pre { white-space: pre-wrap; margin: 0; font-family: monospace; }
  .logpanel { background: none; border: 0; padding: 0; max-height: none; overflow: visible; }
  #log { max-height: 260px; overflow-y: auto; padding: 6px; border: 1px solid #333; background: #0a0a0a; }
  .muted { color: #888; }
  @media (max-width: 900px) {
    #layout { flex-direction: column; }
    #list, #detail, #side { max-width: none; width: 100%; flex: 1 1 auto; }
    #list ul { max-height: 30vh; }
  }
</style>
</head>
<body>
<div id="bar">
  <div style="display:flex;gap:12px;align-items:center">
    <button id="toggle" onclick="window.spectator.toggle()" aria-expanded="true" title="Show/hide accounts">☰ Accounts</button>
    <button id="rtbtn" title="Limit canvas redraws to once per second (default). Real-time redraws every frame.">Real-time: off</button>
    <h1>Spectator <span class="muted">— LifeScape bots</span></h1>
    <a href="/items" style="color:#04A800;text-decoration:none">Item holdings</a>
  </div>
  <span class="meta" id="stamp"></span>
</div>
<div id="layout">
  <div id="list">
    <div><span id="count">0</span> bot(s) supervised</div>
    <ul id="players"><li class="muted">loading…</li></ul>
  </div>
  <div id="detail">
    <div id="title" class="muted"><span id="titletext">Click a bot to follow it.</span><span id="zonename"></span></div>
    <canvas id="map" width="560" height="560"></canvas>
    <div class="panel"><pre id="infotext"></pre></div>
  </div>
  <div id="side">
    <h2>Dialogue / window</h2>
    <div class="panel"><pre id="dialog"></pre></div>
    <h2>Messages</h2>
    <div class="panel"><pre id="chat"></pre></div>
    <h2>Inventory</h2>
    <div class="panel"><pre id="inv"></pre></div>
    <h2>Skills</h2>
    <div class="panel"><pre id="skills"></pre></div>
    <h2>Scenery &amp; ground items</h2>
    <div class="panel"><pre id="scenery"></pre></div>
    <h2>Log</h2>
    <div class="panel logpanel"><pre id="log"></pre></div>
  </div>
</div>
<script src="/spectate_scenery.js"></script>
<script>
(() => {
// Everything stays inside this closure: a browser extension injected into
// the page shares the global scope, and generic globals (socket, select,
// connect...) collide with its bundle.
const SKILLS = ["attack","defence","strength","hitpoints","ranged","prayer","magic","cooking","woodcutting","fletching","fishing","firemaking","crafting","smithing","mining","herblore","agility","thieving","slayer","farming","runecraft"];

let selected = null;
let last = null;      // current snapshot
let prev = null;      // previous snapshot, for interpolation
let prevTime = 0;
let autoSelected = false;
let rt = false;
try { rt = localStorage.getItem("spectate-rt") === "1"; } catch (e) { rt = false; }
function paint() {
  // Canvas redraws are throttled to 1 Hz by default; real-time mode
  // redraws every animation frame. Data (panels, jobs) always stays live.
  if (rt) requestAnimationFrame(draw);
  else setTimeout(draw, 1000);
}
function syncRtBtn() {
  const btn = document.getElementById("rtbtn");
  if (btn) btn.textContent = rt ? "Real-time: on" : "Real-time: off";
}
const cv = document.getElementById('map');
const ctx = cv.getContext('2d');
const TILE = 16;      // px per tile on screen
const R = 16;         // drawn radius, in tiles
const BUF_TILE = 4;   // the client minimap buffer is 4 px per tile
const BUF_SIZE = (R * 2 + 1) * BUF_TILE;
const mapBuffer = document.createElement('canvas');
mapBuffer.width = mapBuffer.height = BUF_SIZE;
const bctx = mapBuffer.getContext('2d');
// ground and lines are separate layers: the ground is smoothed up to screen
// size (that is what makes tiles look soft) while the walls stay crisp
const groundBuffer = document.createElement('canvas');
groundBuffer.width = groundBuffer.height = BUF_SIZE;
const gctx = groundBuffer.getContext('2d');

const esc = (s) => String(s ?? "");
const statusClass = (s) => ["running","reconnecting","busy","starting","failed","disconnected","finished","stopped"].includes(s) ? s : "";

function toggleList(force) {
  const hidden = force !== undefined ? force : !document.body.classList.contains("list-hidden");
  document.body.classList.toggle("list-hidden", hidden);
  document.getElementById("toggle").setAttribute("aria-expanded", String(!hidden));
  localStorage.setItem("list-hidden", hidden ? "1" : "0");
}
if (localStorage.getItem("list-hidden") === "1") toggleList(true);

function renderList(jobs) {
  document.getElementById("count").textContent = jobs.length;
  const ul = document.getElementById("players");
  if (!jobs.length) { ul.innerHTML = '<li class="muted">Nobody supervised.</li>'; return jobs; }
  ul.innerHTML = jobs.map(job => {
    const [, x, z] = job.position || [0,0,0];
    const cur = job.account === selected ? "cur" : "";
    return `<li><a href="#" class="${cur}" onclick="window.spectator.select('${esc(job.account)}');return false">
      <span class="dot ${statusClass(job.status)}"></span><b>${esc(job.account_name || job.account)}</b> lvl ${job.combat ?? "-"} (${x},${z})${job.modal ? " [window]" : ""}
      <br><span class="muted">${esc(job.intention || job.status)} · ${esc(job.script)} · hp ${job.hitpoints ?? "-"}</span></a></li>`;
  }).join("");
  return jobs;
}

function setTitle(text, zone) {
  document.getElementById("titletext").textContent = text;
  document.getElementById("zonename").textContent = zone || "";
}

async function select(account) {
  selected = account;
  autoSelected = true;
  last = null; prev = null;
  setTitle("Following: " + account.replace(/_/g, " "), "");
  if (window.innerWidth <= 900) toggleList(true);
  follow(account);
}

function follow(account) {
  if (socket !== null && socket.readyState === WebSocket.OPEN) {
    socket.send(JSON.stringify({account: account || ""}));
  }
}

let socket = null;
let socketRetry = 500;

function setLink(text, good) {
  const stamp = document.getElementById("stamp");
  stamp.textContent = text;
  stamp.style.color = good ? "#46c46f" : "#d9a441";
}

function scheduleReconnect() {
  setLink("reconnecting…", false);
  setTimeout(connectSocket, socketRetry);
  socketRetry = Math.min(socketRetry * 2, 5000);
}

function connectSocket() {
  const scheme = location.protocol === "https:" ? "wss://" : "ws://";
  try {
    socket = new WebSocket(scheme + location.host + "/ws");
  } catch (e) {
    socket = null;
    scheduleReconnect();
    return;
  }
  socket.onopen = () => {
    socketRetry = 500;
    setLink("live " + new Date().toLocaleTimeString(), true);
    follow(selected);
  };
  socket.onmessage = (event) => {
    let data;
    try { data = JSON.parse(event.data); } catch (e) { return; }
    if (data.jobs) {
      renderList(data.jobs);
      if (!autoSelected && data.jobs.length) {
        select(data.jobs[0].account);
      }
    }
    if (data.logs) setLog(data.logs);
    if (data.state && data.state.account) {
      const incoming = data.state;
      if (Array.isArray(incoming.locs) && incoming.loc_fields) {
        // compact form: arrays + one id -> name table
        const fields = incoming.loc_fields;
        const names = incoming.loc_names || {};
        incoming.locs = incoming.locs.map(row => {
          const loc = {};
          fields.forEach((field, index) => { loc[field] = row[index]; });
          if (!loc.name) loc.name = names[loc.id] || "";
          return loc;
        });
      }
      const merged = incoming.partial && last ? Object.assign({}, last, incoming) : incoming;
      if (last !== merged) { prev = last; prevTime = performance.now(); }
      last = merged;
      setTitle("Following: " + (last.account_name || selected), last.zone);
      updatePanels();
    } else if (selected) {
      setTitle(selected + " (offline)", "");
    }
    setLink("live " + new Date().toLocaleTimeString(), true);
  };
  socket.onerror = () => { if (socket) socket.close(); };
  socket.onclose = () => { socket = null; scheduleReconnect(); };
}

function setLog(lines) {
  const pre = document.getElementById("log");
  // Follow new lines when the user is already at the bottom (or at the very
  // first render); otherwise keep the same distance from the bottom so new
  // lines never yank the view upwards.
  const before = pre.scrollHeight - pre.scrollTop - pre.clientHeight;
  const follow = before < 24;
  pre.textContent = lines.join("\\n") || "(no log)";
  if (follow) {
    pre.scrollTop = pre.scrollHeight;
  } else {
    pre.scrollTop = Math.max(0, pre.scrollHeight - pre.clientHeight - before);
  }
}

function interpPos() {
  if (!last) return null;
  const [, x, z] = last.position || [0,0,0];
  if (!prev) return {px: x, pz: z};
  const t = Math.min(1, (performance.now() - prevTime) / 250);
  const previous = prev.position || last.position || [0, x, z];
  const px = previous[1], pz = previous[2];
  return {px: px + (x - px) * t, pz: pz + (z - pz) * t};
}

function draw() {
  ctx.fillStyle = '#111';
  ctx.fillRect(0, 0, cv.width, cv.height);
  if (!last) {
    ctx.fillStyle = '#888';
    ctx.font = '14px monospace';
    ctx.fillText('Waiting for data…', 20, 280);
    paint();
    return;
  }
  const pos = interpPos();
  const cx = cv.width / 2, cy = cv.height / 2;
  ctx.imageSmoothingEnabled = false; // mapscene sprites are pixel art
  // Text is collected while shapes draw and painted in one pass at the end:
  // later tiles would otherwise cover earlier labels in dense areas.
  const labels = [];
  const dot = (x, z, color, label, big, sayText) => {
    const sx = cx + (x - pos.px) * TILE;
    const sy = cy - (z - pos.pz) * TILE;
    if (sx < -20 || sy < -20 || sx > cv.width + 20 || sy > cv.height + 20) return [sx, sy];
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.arc(sx, sy, big ? 5 : 3.5, 0, Math.PI * 2);
    ctx.fill();
    if (sayText) labels.push([sx + 7, sy - 9, '#fa5', '10px monospace', esc(sayText).slice(0, 40)]);
    if (label) labels.push([sx + 7, sy + 4, '#ccc', '10px monospace', label]);
    return [sx, sy];
  };
  const square = (x, z, color, label, big) => {
    const sx = cx + (x - pos.px) * TILE;
    const sy = cy - (z - pos.pz) * TILE;
    if (sx < -20 || sy < -20 || sx > cv.width + 20 || sy > cv.height + 20) return;
    ctx.fillStyle = color;
    const size = big ? 7 : 5;
    ctx.fillRect(sx - size / 2, sy - size / 2, size, size);
    if (label) labels.push([sx + 6, sy + 4, '#ccc', '10px monospace', label]);
  };

  const halfTile = TILE / 2;
  const map = last.map;
  // The client minimap is a 4 px per tile image. Build the same buffer and
  // scale it up, so ground, walls and sprites keep the client's exact
  // proportions instead of being redrawn at 16 px per tile.
  const originX = Math.round(pos.px) - R;
  const originZ = Math.round(pos.pz) - R;
  const northZ = originZ + R * 2;
  bctx.clearRect(0, 0, BUF_SIZE, BUF_SIZE);
  gctx.fillStyle = '#000000';
  gctx.fillRect(0, 0, BUF_SIZE, BUF_SIZE);
  if (map) {
    for (let row = 0; row < map.size; row++) {
      const tz = map.z + row;
      const by = (northZ - tz) * BUF_TILE;
      if (by < -BUF_TILE || by > BUF_SIZE) continue;
      for (let col = 0; col < map.size; col++) {
        const tx = map.x + col;
        const bx = (tx - originX) * BUF_TILE;
        if (bx < -BUF_TILE || bx > BUF_SIZE) continue;
        const cell = map.tiles[row][col];
        if (typeof cell === "number") {
          gctx.fillStyle = map.palette[cell] || '#000000';
          gctx.fillRect(bx, by, BUF_TILE, BUF_TILE);
          continue;
        }
        for (let index = 0; index < 16; index++) {
          gctx.fillStyle = map.palette[(cell.m >> index) & 1 ? cell.u : cell.o] || '#000000';
          gctx.fillRect(bx + (index & 3), by + (index >> 2), 1, 1);
        }
      }
    }
  }
  const scenery = window.SpectateScenery;
  if (scenery) {
    scenery.drawScenery(bctx, {cx: 0, cy: 0, tile: BUF_TILE, half: BUF_TILE / 2, originX: originX, northZ: northZ, px: pos.px, pz: pos.pz, w: BUF_SIZE, h: BUF_SIZE}, last.locs, last.objs, last.flora, labels, true);
  }
  const mapLeft = cx + (originX - pos.px) * TILE;
  const mapTop = cy - (northZ + 1 - pos.pz) * TILE;
  const mapSize = BUF_SIZE * (TILE / BUF_TILE);
  // nearest-neighbour, like the client's blit: smoothing upscales the ground
  // by ~4 px in every direction, which spills across the 1 px wall lines and
  // makes tiles look like they cross the walls
  ctx.imageSmoothingEnabled = false;
  ctx.drawImage(groundBuffer, mapLeft, mapTop, mapSize, mapSize);
  ctx.drawImage(mapBuffer, mapLeft, mapTop, mapSize, mapSize);
  const fx = pos.px - Math.floor(pos.px), fz = pos.pz - Math.floor(pos.pz);
  ctx.lineWidth = 1;
  if (window.showGrid) {
    // optional help overlay: the minimap itself has no tile grid
    for (let m = -R - 1; m <= R + 1; m++) {
      ctx.strokeStyle = 'rgba(0,0,0,0.18)';
      ctx.beginPath();
      ctx.moveTo(cx + (m + 0.5 - fx) * TILE, cy - (-R - 1.5 - fz) * TILE);
      ctx.lineTo(cx + (m + 0.5 - fx) * TILE, cy - (R + 1.5 - fz) * TILE);
      ctx.stroke();
      ctx.beginPath();
      ctx.moveTo(cx + (-R - 1.5 - fx) * TILE, cy - (m + 0.5 - fz) * TILE);
      ctx.lineTo(cx + (R + 1.5 - fx) * TILE, cy - (m + 0.5 - fz) * TILE);
      ctx.stroke();
    }
  }

  // pathfinding: the route the bot is following + its destination
  const route = last.route || [];
  if (route.length) {
    ctx.strokeStyle = '#3af';
    ctx.lineWidth = 1;
    ctx.beginPath();
    route.forEach(([route_x, route_z], index) => {
      const sx = cx + (route_x - pos.px) * TILE, sy = cy - (route_z - pos.pz) * TILE;
      if (index === 0) ctx.moveTo(sx, sy); else ctx.lineTo(sx, sy);
    });
    ctx.stroke();
    ctx.fillStyle = '#3af';
    for (const [route_x, route_z] of route) {
      const sx = cx + (route_x - pos.px) * TILE, sy = cy - (route_z - pos.pz) * TILE;
      ctx.fillRect(sx - 1.5, sy - 1.5, 3, 3);
    }
  }
  if (last.route_target) {
    const [target_x, target_z] = last.route_target;
    const sx = cx + (target_x - pos.px) * TILE, sy = cy - (target_z - pos.pz) * TILE;
    ctx.fillStyle = '#3af';
    ctx.beginPath();
    ctx.arc(sx, sy, 4, 0, Math.PI * 2);
    ctx.fill();
  }
  if (last.route_action) {
    const action = last.route_action;
    const ax = cx + (action.x - pos.px) * TILE, ay = cy - (action.z - pos.pz) * TILE;
    ctx.strokeStyle = '#2bff2b'; ctx.lineWidth = 3;
    ctx.beginPath();
    ctx.moveTo(ax - 7, ay - 7); ctx.lineTo(ax + 7, ay + 7);
    ctx.moveTo(ax + 7, ay - 7); ctx.lineTo(ax - 7, ay + 7);
    ctx.stroke();
    ctx.beginPath(); ctx.arc(ax, ay, 10, 0, Math.PI * 2); ctx.stroke();
    ctx.lineWidth = 1;
  }

  if (!window.SpectateScenery) {
    const halfTile = TILE / 2;
    for (const obj of last.objs || []) {
      square(obj.x, obj.z, '#fd3', esc(obj.name) || ('object #' + obj.id));
    }
    void halfTile;
  }
  const cross = (x, z) => {
    const sx = cx + (x - pos.px) * TILE, sy = cy - (z - pos.pz) * TILE;
    ctx.strokeStyle = '#0f0'; ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.moveTo(sx - 7, sy - 7); ctx.lineTo(sx + 7, sy + 7);
    ctx.moveTo(sx + 7, sy - 7); ctx.lineTo(sx - 7, sy + 7);
    ctx.stroke(); ctx.lineWidth = 1;
  };
  if (last.hint && last.hint[0] === 'coord') {
    cross(last.hint[1], last.hint[2]);
  } else if (last.hint && last.hint[0] === 'npc') {
    const hinted = (last.npcs || []).find(n => n.nid === last.hint[1]);
    if (hinted) cross(hinted.x, hinted.z);
  }
  for (const npc of last.npcs || []) dot(npc.x, npc.z, '#ff0', esc(npc.name) || ('npc #' + npc.nid));
  for (const player of last.players || []) dot(player.x, player.z, '#3af', player.name);
  const [sx, sy] = dot(pos.px, pos.pz, '#fff', (last.account_name || last.account || '?') + ' lvl ' + (last.combat ?? '?'), true);
  for (const [lx, ly, color, font, str] of labels) {
    if (lx < -100 || ly < -12 || lx > cv.width + 100 || ly > cv.height + 12) continue;
    ctx.font = font;
    ctx.fillStyle = color;
    ctx.fillText(str, lx, ly);
  }
  paint();
}

function updatePanels() {
  if (!last) return;
  const [level, x, z] = last.position || [0,0,0];
  const modal = last.modal_name || (last.modal ? (Array.isArray(last.modal) ? last.modal.join(' ') : JSON.stringify(last.modal)) : null);
  const hint = last.hint_name || (last.hint ? (Array.isArray(last.hint) ? last.hint.join(' ') : JSON.stringify(last.hint)) : null);
  const lines = [];
  lines.push(`${last.account_name || last.account} @ ${x},${z},${level} (combat ${last.combat ?? '?'}, energy ${last.energy ?? '?'}, hp ${last.hitpoints ?? '?'})`);
  lines.push(`intent: ${last.intention || '-'}`);
  lines.push(`status: ${last.status || '?'}`);
  lines.push(`script: ${last.script || '-'}`);
  lines.push(`ticks: ${last.ticks ?? '?'}`);
  lines.push(`pid: ${last.pid ?? last.session_pid ?? '?'}`);
  lines.push(`restarts: ${last.restarts ?? 0}`);
  if (last.last_error) lines.push(`last error: ${JSON.stringify(last.last_error)}`);
  lines.push(modal ? `window open: ${modal}` : 'no window open');
  if (hint) lines.push(`hint arrow: ${hint}`);
  if (last.flash !== null && last.flash !== undefined && last.flash !== -1) lines.push(`flashing tab: ${last.flash}`);
  if (last.last_event) lines.push(`last event: ${last.last_event.event} (resolved=${last.last_event.resolved})`);
  if (last.requirements) {
    const req = last.requirements;
    const reasons = (req.reasons || []).map(r => {
      if (r.type === 'item') return `${r.item} ${r.owned}/${r.required}`;
      if (r.type === 'skill') return `${r.skill} ${r.current}/${r.required}`;
      if (r.type === 'quest_points') return `quest points ${r.current}/${r.required}`;
      if (r.type === 'navigation') return r.detail || 'no route';
      return `quest ${r.name || r.quest}`;
    });
    lines.push(`quest ${req.quest}: ${req.met ? 'requirements met' : 'missing ' + reasons.join(', ')}`);
  }
  if (last.route && last.route.length) {
    const [target_x, target_z] = last.route_target || [0, 0];
    lines.push(`route: ${last.route.length} tiles to ${target_x},${target_z}`);
  }
  if (last.route_action) {
    const action = last.route_action;
    lines.push(`action: ${action.name || 'object #' + action.id} at ${action.x},${action.z}`);
  }
  document.getElementById('infotext').textContent = lines.join('\\n');

  document.getElementById('dialog').textContent = last.modal_name ? `window: ${last.modal_name}` : '(no window open)';
  document.getElementById('chat').textContent = (last.chat || []).join('\\n') || '(no messages)';

  const fmt = (items, empty) => (items && items.length)
    ? items.map(i => (i.count > 1 ? i.count + 'x ' : '') + (i.name || 'object')).join('\\n')
    : empty;
  document.getElementById('inv').textContent = 'Backpack:\\n' + fmt(last.inv, '(empty)') + '\\n\\nWorn:\\n' + fmt(last.worn, '(nothing)');
  document.getElementById('skills').textContent = (last.skills || []).map(s => `${SKILLS[s.id] || ('stat' + s.id)} ${s.level} (${s.xp}xp)`).join('\\n') || '(no skills)';

  const locCounts = {};
  let sceneryCount = 0;
  for (const loc of last.locs || []) {
    if (loc.cosmetic) continue; // decorative ground: drawn on the map, not listed
    sceneryCount += 1;
    const name = loc.name || ('scenery #' + loc.id);
    locCounts[name] = (locCounts[name] || 0) + 1;
  }
  const locLines = Object.entries(locCounts).map(([name, count]) => (count > 1 ? count + 'x ' : '') + name);
  const objLines = (last.objs || []).map(o => (o.name || ('object #' + o.id)) + ' @ ' + o.x + ',' + o.z);
  document.getElementById('scenery').textContent =
    `Scenery (${sceneryCount}):\\n` + (locLines.slice(0, 60).join('\\n') || '(none)') +
    `\\n\\nGround items (${(last.objs || []).length}):\\n` + (objLines.join('\\n') || '(none)');
}

connectSocket();
paint();
document.getElementById("rtbtn").onclick = () => {
  rt = !rt;
  try { localStorage.setItem("spectate-rt", rt ? "1" : "0"); } catch (e) { /* private mode */ }
  syncRtBtn();
};
syncRtBtn();
window.spectator = { select: select, toggle: toggleList };
})();
</script>
</body>
</html>
"""


ITEMS_HTML = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Item holdings - LifeScape</title>
<style>
  body { font-family: Arial, Helvetica, sans-serif; font-size: 13px; color: #eee; background: #000; margin: 0; padding: 12px; }
  a { color: #04A800; }
  h1 { font-size: 18px; margin: 0 0 4px; }
  h2 { font-size: 14px; margin: 12px 0 4px; color: #ff0; }
  #layout { display: flex; gap: 16px; flex-wrap: wrap; align-items: flex-start; }
  #list { min-width: 260px; max-width: 340px; flex: 1 1 260px; }
  #list ul { list-style: none; padding: 0; margin: 8px 0; max-height: 56vh; overflow-y: auto; }
  #list li { margin: 2px 0; cursor: pointer; }
  #list li.cur { font-weight: bold; color: #ff0; }
  #list li .qty { color: #888; }
  #search { width: 100%; box-sizing: border-box; padding: 4px 6px; background: #0a0a0a; color: #eee; border: 1px solid #444; }
  #pager { display: flex; gap: 8px; align-items: center; color: #888; margin-top: 4px; }
  #pager button { background: #1b1b1b; color: #eee; border: 1px solid #444; padding: 2px 8px; cursor: pointer; }
  #detail { flex: 2 1 520px; min-width: 340px; }
  canvas { background: #111; border: 1px solid #444; width: 100%; height: auto; display: block; }
  .muted { color: #888; }
</style>
</head>
<body>
<h1>Item holdings <span class="muted">(<a href="/">back to bots</a>)</span></h1>
<div id="layout">
  <div id="list">
    <h2>Items held</h2>
    <input id="search" list="item-names" placeholder="Search an item…" autocomplete="off" oninput="onSearch()">
    <datalist id="item-names"></datalist>
    <ul id="items"></ul>
    <div id="pager">
      <button onclick="page(-1)">‹</button>
      <span id="pageinfo"></span>
      <button onclick="page(1)">›</button>
    </div>
  </div>
  <div id="detail">
    <h2 id="title">Total items</h2>
    <canvas id="chart" width="720" height="340"></canvas>
  </div>
</div>
<script>
const PAGE_SIZE = 25;
let data = null;
let selected = null;
let pageIndex = 0;
let query = "";

async function load() {
  try {
    const response = await fetch('/api/items/history');
    const payload = await response.json();
    data = payload.data ?? payload;
  } catch (e) { return; }
  renderList();
  draw();
}

function filtered() {
  const needle = query.trim().toLowerCase();
  if (!needle) return data.items;
  return data.items.filter(item => item.name.toLowerCase().includes(needle));
}

function series() {
  if (!selected) return data.total;
  return data.items.find(item => item.id === selected)?.series ?? [];
}

function renderList() {
  const matches = filtered();
  const pages = Math.max(1, Math.ceil((matches.length + 1) / PAGE_SIZE));
  pageIndex = Math.min(Math.max(pageIndex, 0), pages - 1);
  const start = pageIndex * PAGE_SIZE;
  const slice = matches.slice(start, start + PAGE_SIZE);

  const rows = [];
  if (selected === null) rows.push(`<li class="cur" onclick="pick(null)">Total <span class="qty">(${data.liveTotal})</span></li>`);
  else rows.push(`<li onclick="pick(null)">Total <span class="qty">(${data.liveTotal})</span></li>`);
  for (const item of slice) {
    rows.push(`<li class="${selected === item.id ? 'cur' : ''}" data-id="${item.id}" onclick="pick(${item.id})">${item.name} <span class="qty">(${item.count})</span></li>`);
  }
  document.getElementById('items').innerHTML = rows.join('') || '<li class="muted">no match</li>';
  document.getElementById('pageinfo').textContent = `page ${pageIndex + 1}/${pages} · ${matches.length} item(s)`;
  document.getElementById('item-names').innerHTML = data.items.map(item => `<option value="${item.name}">`).join('');
}

function page(delta) {
  pageIndex += delta;
  renderList();
}

function onSearch() {
  query = document.getElementById('search').value;
  pageIndex = 0;
  renderList();
  const match = data.items.find(item => item.name.toLowerCase() === query.trim().toLowerCase());
  if (match) pick(match.id);
}

function pick(id) {
  selected = id;
  renderList();
  draw();
}

function draw() {
  const canvas = document.getElementById('chart');
  const ctx = canvas.getContext('2d');
  // Back the canvas at device resolution so the chart stays crisp.
  const ratio = window.devicePixelRatio || 1;
  const width = Math.max(320, canvas.clientWidth);
  const height = Math.round(width * 340 / 720);
  canvas.width = Math.round(width * ratio);
  canvas.height = Math.round(height * ratio);
  canvas.style.height = height + 'px';
  ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
  ctx.fillStyle = '#111';
  ctx.fillRect(0, 0, width, height);

  const values = series();
  document.getElementById('title').textContent = selected === null
    ? 'Total items'
    : (data.items.find(item => item.id === selected)?.name ?? ('obj ' + selected));
  const live = selected === null
    ? data.liveTotal
    : (data.items.find(item => item.id === selected)?.count ?? 0);

  if (!values.length) {
    ctx.fillStyle = '#888';
    ctx.font = '14px monospace';
    ctx.fillText('No point yet (one every ' + data.intervalMinutes + ' min).', 20, height / 2);
    return;
  }

  const points = values.map((value, index) => ({ value, time: data.samples[index] ?? null, live: false }));
  points.push({ value: live, time: Date.now(), live: true });

  let min = Infinity, max = -Infinity;
  for (const point of points) { min = Math.min(min, point.value); max = Math.max(max, point.value); }
  if (min === max) { min -= 1; max += 1; }
  const pad = 40;
  const chartW = width - pad * 2, chartH = height - pad * 2;
  const yOf = value => pad + chartH - ((value - min) / (max - min)) * chartH;
  const slot = points.length > 1 ? chartW / (points.length - 1) : 0;

  ctx.strokeStyle = '#333';
  ctx.lineWidth = 1;
  ctx.font = '10px monospace';
  for (let step = 0; step <= 4; step++) {
    const y = pad + (chartH * step) / 4;
    ctx.beginPath(); ctx.moveTo(pad, y); ctx.lineTo(width - pad, y); ctx.stroke();
    ctx.fillStyle = '#888';
    ctx.fillText(String(Math.round(max - ((max - min) * step) / 4)), 4, y + 3);
  }

  ctx.strokeStyle = '#4caf50';
  ctx.lineWidth = 2;
  ctx.beginPath();
  points.forEach((point, index) => {
    const x = pad + slot * index, y = yOf(point.value);
    if (index === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
  });
  ctx.stroke();
  ctx.lineWidth = 1;

  points.forEach((point, index) => {
    const x = pad + slot * index, y = yOf(point.value);
    ctx.fillStyle = point.live ? '#fff' : '#4caf50';
    ctx.beginPath();
    ctx.arc(x, y, point.live ? 4 : 2.5, 0, Math.PI * 2);
    ctx.fill();
    if (point.live) {
      ctx.fillStyle = '#888';
      ctx.font = '10px monospace';
      ctx.fillText(String(point.value), x + 6, y + 3);
    }
  });

  const step = Math.max(1, Math.ceil(points.length / 8));
  ctx.fillStyle = '#888';
  ctx.font = '10px monospace';
  for (let index = 0; index < points.length; index += step) {
    const x = pad + slot * index, point = points[index];
    let label = 'live';
    if (!point.live && point.time) {
      const date = new Date(point.time * 1000);
      label = String(date.getHours()).padStart(2, '0') + ':' + String(date.getMinutes()).padStart(2, '0');
    }
    ctx.fillText(label, x - 14, height - pad + 14);
  }
}

let resizeTimer = null;
window.addEventListener('resize', () => {
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(() => { if (data) draw(); }, 150);
});

load();
setInterval(load, 60000);
</script>
</body>
</html>
"""


_SCENERY_SCRIPT = None
_SCENERY_VERSION = None


def _scenery_script():
    global _SCENERY_SCRIPT
    if _SCENERY_SCRIPT is None:
        script_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "spectate_scenery.js")
        with open(script_path, "rb") as handle:
            _SCENERY_SCRIPT = handle.read()
    return _SCENERY_SCRIPT


def _scenery_version():
    global _SCENERY_VERSION
    if _SCENERY_VERSION is None:
        _SCENERY_VERSION = hashlib.md5(_scenery_script()).hexdigest()[:12]
    return _SCENERY_VERSION


# Cache-bust the module: the file changes with the code, and the dashboard
# must pick it up without waiting out a cache entry.
INDEX_HTML = INDEX_HTML.replace(
    'src="/spectate_scenery.js"', 'src="/spectate_scenery.js?v=%s"' % _scenery_version()
)


class WebHandler(BaseHTTPRequestHandler):
    dispatch = None

    def do_GET(self):
        parsed = urlparse(self.path)
        parts = [part for part in parsed.path.split("/") if part]
        try:
            if not parts:
                return self.reply(200, INDEX_HTML, "text/html; charset=utf-8")
            if parts[:2] == ["api", "status"]:
                return self.reply_json(self.dispatch({"cmd": "status"}))
            if parts[:2] == ["api", "state"]:
                account = parts[2] if len(parts) > 2 else ""
                reply = self.dispatch({"cmd": "show", "account": account})
                state = reply.get("state") if isinstance(reply, dict) else None
                position = state.get("position") if state else None
                if position:
                    try:
                        state["map"] = MAP_CACHE.grid(position[0], position[1], position[2])
                    except Exception:
                        state.pop("map", None)
                _annotate_mapscene(state)
                return self.reply_json(reply)
            if parts == ["items"]:
                return self.reply(200, ITEMS_HTML, "text/html; charset=utf-8")
            if parts == ["spectate_scenery.js"]:
                return self.reply(
                    200,
                    _scenery_script(),
                    "application/javascript; charset=utf-8",
                    cache_control="public, max-age=3600",
                )
            if len(parts) == 2 and parts[0] == "mapscene" and parts[1].endswith(".png"):
                sprite = MAP_CACHE.mapscene_png(int(parts[1][:-4])) if parts[1][:-4].isdigit() else None
                if sprite is None:
                    return self.reply_json({"ok": False, "error": "not found"}, status=404)
                return self.reply(200, sprite, "image/png", cache_control="public, max-age=86400")
            if parts == ["api", "items", "history"]:
                reply = self.dispatch({"cmd": "items"})
                return self.reply_json(reply.get("data", reply))
            if parts[:2] == ["api", "logs"]:
                account = parts[2] if len(parts) > 2 else ""
                lines = int(parse_qs(parsed.query).get("lines", ["60"])[0])
                return self.reply_json(self.dispatch({"cmd": "logs", "account": account, "lines": lines}))
            if parts == ["ws"]:
                return self.upgrade_websocket()
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
