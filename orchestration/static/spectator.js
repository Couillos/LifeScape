(() => {
// Everything stays inside this closure: a browser extension injected into
// the page shares the global scope, and generic globals (socket, select,
// connect...) collide with its bundle.
const SKILLS = ["attack","defence","strength","hitpoints","ranged","prayer","magic","cooking","woodcutting","fletching","fishing","firemaking","crafting","smithing","mining","herblore","agility","thieving","slayer","farming","runecraft"];

let selected = null;
let last = null;      // current snapshot (map + overlays always draw from this one object)
let autoSelected = false;
let rt = false;
try { rt = localStorage.getItem("spectate-rt") === "1"; } catch (e) { rt = false; }
let frameTimer = null;
let framePending = false;
function paint() {
  // Both modes paint atomic frames only (see draw): real-time redraws
  // every animation frame, throttled mode redraws at most once a second
  // (default, button title). Panels stay live in both modes: they update
  // on every WS message, only the canvas (and its map image fetch) is
  // throttled here.
  // One scheduled redraw at a time: a map image landing mid-second must
  // redraw now, not queue another second on top of the pending one.
  if (rt) {
    if (framePending) return;
    framePending = true;
    requestAnimationFrame(() => { framePending = false; draw(); });
  } else {
    if (frameTimer !== null) return;
    frameTimer = setTimeout(() => { frameTimer = null; draw(); }, 1000);
  }
}
function syncRtBtn() {
  const btn = document.getElementById("rtbtn");
  if (btn) btn.textContent = rt ? "Real-time: on" : "Real-time: off";
}
const cv = document.getElementById('map');
const ctx = cv.getContext('2d');
const TILE = 16;      // px per tile on screen
const R = 16;         // drawn radius, in tiles
// The map is precomputed server-side (one PNG per painted cluster per
// level, 4 px per tile, same passes as the client minimap) and served
// immutable. The client only crops the 33-tile window around the player
// (position already arrives over the WS at 1 Hz): zero image requests.
// Frames are atomic: draw() only paints when the crop is fully covered,
// otherwise it keeps the previous complete frame.
let mapState = 'loading'; // loading | ready | missing
let mapDir = null;
let mapImages = null; // [{file, level, x0, z0, w, h}] in tiles, from the manifest
const mapImgs = {}; // file -> Image (one per level file, loaded once)
async function loadStaticMap() {
  try {
    const info = await (await fetch('/api/map')).json();
    if (!info.ok || !info.dir) throw new Error('no static map');
    mapDir = '/' + info.dir + '/';
    const manifest = await (await fetch(mapDir + 'manifest.json')).json();
    mapImages = manifest.images || [];
    mapState = 'ready';
    for (const entry of mapImages) {
      const image = new Image();
      image.onload = () => paint();
      image.onerror = () => { image.broken = true; paint(); };
      image.src = mapDir + entry.file;
      mapImgs[entry.file] = image;
    }
    paint();
  } catch (e) {
    mapState = 'missing';
    paint();
  }
}
// dots, labels and the route are drawn over the map image; turn this off to
// compare the map pixels alone against the client minimap
window.showOverlay = true;

const esc = (s) => String(s ?? "");
const statusClass = (s) => ["running","reconnecting","busy","starting","failed","disconnected","finished","stopped"].includes(s) ? s : "";
// Script pill: what the session's script is doing (running, crashed,
// paused, stopped, empty). last_error clears on every fresh run, so a
// crash never sticks once a new script starts.
function scriptState(job) {
  if (!job) return "empty";
  if (job.paused) return "paused";
  if (job.phase === "running") return "running";
  if (job.last_error) return "crashed";
  if (job.status === "finished" || job.status === "failed" || job.status === "stopped") return "stopped";
  if (job.last_code !== null && job.last_code !== undefined) return "stopped";
  return "empty";
}
function renderPill() {
  const pill = document.getElementById("scriptpill");
  const name = document.getElementById("scriptname");
  if (!pill) return;
  if (!last) {
    pill.style.display = "none";
    if (name) name.style.display = "none";
    syncPauseBtn();
    return;
  }
  pill.style.display = "";
  setText(pill, scriptState(last));
  pill.className = "pill " + scriptState(last);
  // Script name pill next to the status pill.
  if (name) {
    if (last.script) {
      name.style.display = "";
      setText(name, last.script);
    } else {
      name.style.display = "none";
    }
  }
  syncPauseBtn();
}

// Single pause/resume toggle: label follows the followed session state.
function syncPauseBtn() {
  const btn = document.getElementById("btn-pause-toggle");
  if (!btn) return;
  const paused = !!(last && last.paused);
  btn.textContent = paused ? "resume" : "pause";
  btn.title = paused ? "Unfreeze the script" : "Freeze the script, keep the connection";
}

function toggleList(force) {
  const hidden = force !== undefined ? force : !document.body.classList.contains("list-hidden");
  document.body.classList.toggle("list-hidden", hidden);
  document.getElementById("toggle").setAttribute("aria-expanded", String(!hidden));
  localStorage.setItem("list-hidden", hidden ? "1" : "0");
}
if (localStorage.getItem("list-hidden") === "0") toggleList(false);

// HTML-escaped text for the bot table (esc above only stringifies: it
// also feeds canvas labels, which must stay raw).
const escHtml = (s) => String(s ?? "").replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
const dash = (value) => (value === null || value === undefined || value === "") ? "-" : value;
// Bot table columns (shared TableSort, table.js): raw values sort, fmt
// only changes the display. Goal/task come from the life file (backend
// web_job_lives.rs); manual bots have none.
const BOT_COLUMNS = [
  { key: "state", label: "●", kind: "str",
    fmt: (value, row) => `<span class="dot ${statusClass(row.status)}" title="${escHtml(row.status)}"></span>${row.paused ? "⏸" : ""}` },
  { key: "name", label: "Bot", kind: "str" },
  { key: "combat", label: "Cb", kind: "num" },
  { key: "hitpoints", label: "PV", kind: "num" },
  { key: "script", label: "Script", kind: "str" },
  { key: "zone", label: "Zone", kind: "str" },
  { key: "position", label: "Position", kind: "str" },
  { key: "goal", label: "But", kind: "str" },
  { key: "task", label: "Tâche", kind: "str",
    fmt: (value, row) => `<span class="clip" title="${escHtml(row.task_title || dash(value))}">${escHtml(dash(value))}</span>` },
];
function botRow(job) {
  const position = Array.isArray(job.position) && job.position.length >= 3 ? `${job.position[1]},${job.position[2]}` : null;
  return {
    account: job.account,
    status: job.status,
    paused: !!job.paused,
    state: (job.status || "") + (job.paused ? " paused" : ""),
    name: job.account_name || job.account,
    combat: job.combat ?? null,
    hitpoints: job.hitpoints ?? null,
    script: job.script || null,
    zone: typeof job.zone === "string" ? job.zone : null,
    position: position,
    goal: job.goal || null,
    task: job.task || null,
    task_title: job.task_title || "",
  };
}
// The rows re-render 1x/s: while a mouse button is held inside the table
// the render waits for the release, so a header click (press + release
// on the same <th>) is never split across two rebuilt nodes. A press
// older than 2 s (release lost outside the page) stops holding it.
let listPressed = false;
let listPressedAt = 0;
let pendingJobs = null;
function renderList(jobs) {
  document.getElementById("count").textContent = jobs.length;
  if (listPressed && Date.now() - listPressedAt < 2000) { pendingJobs = jobs; return jobs; }
  pendingJobs = null;
  const table = document.getElementById("players");
  if (!jobs.length) { table.innerHTML = '<tr><td class="left muted">Nobody supervised.</td></tr>'; return jobs; }
  TableSort.render("players", BOT_COLUMNS, jobs.map(botRow), {
    rowAttrs: (row) => ({ "data-account": row.account, class: row.account === selected ? "cur" : "" }),
  });
  return jobs;
}

function setTitle(text, zone) {
  setText(document.getElementById("titletext"), text);
  setText(document.getElementById("zonename"), zone || "");
}

async function select(account) {
  selected = account;
  autoSelected = true;
  last = null;
  logLines = [];
  logShown = 0;
  // New bot, new base: drop the previous bot's line nodes and caches so
  // the next frames rebuild from scratch (same as a full server resend).
  for (const id of ["infotext", "log", "dialog", "chat", "inv", "skills", "scenery"]) {
    const pre = document.getElementById(id);
    if (pre) { pre.textContent = ""; pre._lines = undefined; }
  }
  paint(); // drop the previous bot at once; the next state paints the new one
  setTitle("Following: " + account.replace(/_/g, " "), "");
  const picker = document.getElementById("account");
  if (picker && !picker.value && account !== "admin") {
    if (![...picker.options].some(o => o.value === account)) {
      const opt = document.createElement("option");
      opt.value = account; opt.textContent = account;
      picker.appendChild(opt);
    }
    picker.value = account;
  }
  follow(account);
}

function follow(account) {
  if (socket !== null && socket.readyState === WebSocket.OPEN) {
    socket.send(JSON.stringify({account: account || ""}));
  }
}

let socket = null;
let socketRetry = 500;

function scheduleReconnect() {
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
    follow(selected);
  };
  socket.onmessage = (event) => {
    let data;
    try { data = JSON.parse(event.data); } catch (e) { return; }
    if (data.jobs) {
      renderList(data.jobs);
      if (!autoSelected && data.jobs.length) {
        // never auto-follow the staff account: it is only followed on click
        const first = data.jobs.find(j => j.account !== "admin") || data.jobs[0];
        select(first.account);
      }
    }
    if (data.logs) { logLines = data.logs; renderLogFull(); }
    if (data.log_append && data.log_append.length) {
      logLines = logLines.concat(data.log_append).slice(-500);
      appendLog(data.log_append);
    }
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
      const merged = last ? Object.assign({}, last, incoming) : incoming;
      // Light frames (1 Hz) carry compact entity positions only
      // (`[nid,x,z]`, `[name,x,z]`); names ride the rare full frames.
      // Rebuild the drawn arrays from the fresh positions, keeping the
      // last known name per nid: without this the followed bot moved
      // every second while the scene around it froze until the next full
      // frame (100 ticks, i.e. 30 s in prod).
      if (Array.isArray(incoming.npcs_at)) {
        const named = new Map(((last && last.npcs) || []).map(npc => [npc.nid, npc.name]));
        merged.npcs = incoming.npcs_at.map(([nid, x, z]) => ({nid, x, z, name: named.get(nid) || ""}));
      }
      if (Array.isArray(incoming.players_at)) {
        merged.players = incoming.players_at.map(([name, x, z]) => ({name, x, z}));
      }
      last = merged;
      // Session gap, not a dead pipe: the socket stays open while the
      // worker relogs. Say so with the stale age instead of showing a
      // frozen frame as if it were live.
      const gap = last.status === "reconnecting" || last.status === "disconnected";
      const stale = last.updated ? Math.max(0, Date.now() / 1000 - last.updated) : 0;
      setTitle(
        "Following: " + (last.account_name || selected) +
          (gap ? " — " + last.status + "… (data " + Math.round(stale) + "s old)" : ""),
        last.zone,
      );
      renderPill();
      updatePanels();
      paint();
    } else if (selected) {
      setTitle(selected + " (offline)", "");
      last = null;
      renderPill();
    }
  };
  socket.onerror = () => { if (socket) socket.close(); };
  socket.onclose = () => { socket = null; scheduleReconnect(); };
}

let logLines = [];
let logShown = 0; // line nodes currently in #log
// Scroll + selection discipline for the blocks around the canvas. The
// server pushes a state diff at 1 Hz and log tails as they grow;
// rewriting textContent on every frame would yank scrolled-up readers
// and destroy any text selection. So:
// - stickyWrite: only auto-scroll a block that was already at the
//   bottom (< 24 px); a block the user scrolled up is never touched.
// - syncStatePre: one text node per line, mutated in place; unchanged
//   lines keep their DOM nodes, so a selection inside them survives.
// - setText: assign textContent only when the string actually changed.
function scrollerFor(pre) {
  return pre.id === "log" ? pre : pre.parentElement;
}
function stickyWrite(pre, write) {
  const box = scrollerFor(pre);
  const follow = box.scrollHeight - box.scrollTop - box.clientHeight < 24;
  write();
  if (follow) box.scrollTop = box.scrollHeight;
}
function setText(node, text) {
  if (node && node.textContent !== text) node.textContent = text;
}
function syncStatePre(pre, lines, emptyText) {
  const wanted = lines.length ? lines : [emptyText];
  let nodes = pre._lines;
  if (!nodes) { pre.textContent = ""; nodes = pre._lines = []; }
  stickyWrite(pre, () => {
    while (nodes.length < wanted.length) {
      if (nodes.length > 0) pre.appendChild(document.createTextNode("\n"));
      nodes.push(pre.appendChild(document.createTextNode("")));
    }
    while (nodes.length > wanted.length) {
      pre.removeChild(nodes.pop());
      if (pre.lastChild) pre.removeChild(pre.lastChild);
    }
    for (let line = 0; line < wanted.length; line++) {
      if (nodes[line].nodeValue !== wanted[line]) nodes[line].nodeValue = wanted[line];
    }
  });
}

function renderLogFull() {
  const pre = document.getElementById("log");
  stickyWrite(pre, () => {
    pre.textContent = logLines.join("\n") || "(no log)";
    logShown = logLines.length;
  });
}
// Append-only: the server already sends just the tail (log_append,
// web.rs log_delta), so new line nodes go after the old ones without
// touching them — a selection above survives, and the 500-line cap
// drops whole head lines.
function appendLog(lines) {
  if (!lines.length) return;
  const pre = document.getElementById("log");
  stickyWrite(pre, () => {
    if (logShown === 0) pre.textContent = "";
    for (const line of lines) {
      if (logShown > 0) pre.appendChild(document.createTextNode("\n"));
      pre.appendChild(document.createTextNode(line));
      logShown++;
    }
    let drop = logShown - 500;
    while (drop-- > 0) {
      if (pre.firstChild) pre.removeChild(pre.firstChild);
      if (pre.firstChild) pre.removeChild(pre.firstChild);
      logShown--;
    }
  });
}

function draw() {
  if (!last) {
    ctx.fillStyle = '#111';
    ctx.fillRect(0, 0, cv.width, cv.height);
    ctx.fillStyle = '#888';
    ctx.font = '14px monospace';
    ctx.fillText('Waiting for data…', 20, 280);
    return;
  }
  // Integer tile position, no glide: the dot, the route and the map window
  // all anchor on the same tile, so the player is always on its route.
  // (The old lerp drew the dot between tiles while the route stayed on the
  // new tile, which is why the dot left the trace for ~250 ms a step.)
  const [level, px, pz] = last.position || [0, 0, 0];
  const pos = {px, pz};
  const cx = cv.width / 2, cy = cv.height / 2;
  ctx.imageSmoothingEnabled = false; // mapscene sprites are pixel art
  // Text is collected while shapes draw and painted in one pass at the end:
  // later tiles would otherwise cover earlier labels in dense areas.
  const labels = [];
  const DOT_RADIUS = 5;     // every marker is the size of the player dot
  const LABEL_FONT = 'bold 12px monospace';
  const dot = (x, z, color, label, big, sayText) => {
    const sx = cx + (x - pos.px) * TILE;
    const sy = cy - (z - pos.pz) * TILE;
    if (sx < -20 || sy < -20 || sx > cv.width + 20 || sy > cv.height + 20) return [sx, sy];
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.arc(sx, sy, DOT_RADIUS, 0, Math.PI * 2);
    ctx.fill();
    if (sayText) labels.push([sx + 8, sy - 10, '#fa5', LABEL_FONT, esc(sayText).slice(0, 40)]);
    if (label) labels.push([sx + 8, sy + 4, '#ccc', LABEL_FONT, label]);
    return [sx, sy];
  };
  // Ground, walls and mapscene sprites come from the precomputed static
  // image of the current level: crop the 33-tile window around the player
  // and upscale 4x (imageSmoothingEnabled stays off, pixel art is kept).
  const originX = pos.px - R;
  const originZ = pos.pz - R;
  // Tile-edge alignment: the file covers whole tiles, so its pixels sit
  // half a tile outside the outer row centers. Without this the dots
  // and route sat 8 px southwest of the map (a player drawn on a wall it
  // walks beside).
  const mapLeft = cx - R * TILE - TILE / 2;
  const mapTop = cy - (R + 1) * TILE + TILE / 2;
  const mapSize = (R * 2 + 1) * TILE;
  const full = mapImages
    ? mapImages.find((entry) => entry.level === level
      && originX >= entry.x0 && originZ >= entry.z0
      && originX + R * 2 + 1 <= entry.x0 + entry.w
      && originZ + R * 2 + 1 <= entry.z0 + entry.h)
    : null;
  const fullImg = full ? mapImgs[full.file] : null;
  if (fullImg && !fullImg.broken && fullImg.complete && fullImg.naturalWidth > 0) {
    ctx.fillStyle = '#111';
    ctx.fillRect(0, 0, cv.width, cv.height);
    ctx.imageSmoothingEnabled = false;
    const sx = (originX - full.x0) * 4, sy = (full.z0 + full.h - 1 - (originZ + R * 2)) * 4;
    ctx.drawImage(fullImg, sx, sy, 132, 132, mapLeft, mapTop, mapSize, mapSize);
  } else if (full) {
    // The file exists but its pixels are still loading (or failed): keep
    // the previous complete frame instead of a black map with overlays.
    return;
  } else if (!mapImages || mapState === 'loading') {
    ctx.fillStyle = '#111';
    ctx.fillRect(0, 0, cv.width, cv.height);
    ctx.fillStyle = '#888';
    ctx.font = '14px monospace';
    ctx.fillText('Loading static map…', 20, 280);
    return;
  } else if (mapImages.every((entry) => entry.level !== level)) {
    ctx.fillStyle = '#111';
    ctx.fillRect(0, 0, cv.width, cv.height);
    ctx.fillStyle = '#888';
    ctx.font = '14px monospace';
    ctx.fillText('Static map missing, run daemon with pack', 20, 280);
    return;
  } else {
    // Deep ocean or cluster edge: no file covers the window. Paint black
    // (what a live render of empty tiles shows) and keep the overlays.
    ctx.fillStyle = '#111';
    ctx.fillRect(0, 0, cv.width, cv.height);
    ctx.imageSmoothingEnabled = false;
    const overlap = mapImages.find((entry) => entry.level === level
      && originX < entry.x0 + entry.w && originX + R * 2 + 1 > entry.x0
      && originZ < entry.z0 + entry.h && originZ + R * 2 + 1 > entry.z0);
    const overlapImg = overlap ? mapImgs[overlap.file] : null;
    if (overlapImg && !overlapImg.broken && overlapImg.complete && overlapImg.naturalWidth > 0) {
      const ix0 = Math.max(originX, overlap.x0), ix1 = Math.min(originX + R * 2 + 1, overlap.x0 + overlap.w);
      const iz0 = Math.max(originZ, overlap.z0), iz1 = Math.min(originZ + R * 2 + 1, overlap.z0 + overlap.h);
      ctx.drawImage(overlapImg,
        (ix0 - overlap.x0) * 4, (overlap.z0 + overlap.h - iz1) * 4, (ix1 - ix0) * 4, (iz1 - iz0) * 4,
        mapLeft + (ix0 - originX) * TILE, mapTop + (originZ + R * 2 + 1 - iz1) * TILE,
        (ix1 - ix0) * TILE, (iz1 - iz0) * TILE);
    }
  }
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
  const route = window.showOverlay ? (last.route || []) : [];
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
  if (window.showOverlay && last.route_target) {
    const [target_x, target_z] = last.route_target;
    const sx = cx + (target_x - pos.px) * TILE, sy = cy - (target_z - pos.pz) * TILE;
    ctx.fillStyle = '#3af';
    ctx.beginPath();
    ctx.arc(sx, sy, 4, 0, Math.PI * 2);
    ctx.fill();
  }
  if (window.showOverlay && last.route_action) {
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

  if (window.showOverlay) {
    for (const obj of last.objs || []) {
      dot(obj.x, obj.z, '#f00', esc(obj.name) || ('object #' + obj.id));
    }
  }
  // dots, labels and the route, over the client-rendered map image
  if (window.showOverlay) {
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
    dot(pos.px, pos.pz, '#fff', (last.account_name || last.account || '?') + ' lvl ' + (last.combat ?? '?'), true);
    for (const [lx, ly, color, font, str] of labels) {
      if (lx < -100 || ly < -12 || lx > cv.width + 100 || ly > cv.height + 12) continue;
      ctx.font = font;
      ctx.fillStyle = color;
      ctx.fillText(str, lx, ly);
    }
  }
}

function updatePanels() {
  if (!last) return;
  const [level, x, z] = last.position || [0,0,0];
  const modal = last.modal_name || (last.modal ? (Array.isArray(last.modal) ? last.modal.join(' ') : JSON.stringify(last.modal)) : null);
  const hint = last.hint_name || (last.hint ? (Array.isArray(last.hint) ? last.hint.join(' ') : JSON.stringify(last.hint)) : null);
  const lines = [];
  lines.push(`${last.account_name || last.account} @ ${x},${z},${level} (combat ${last.combat ?? '?'}, energy ${last.energy ?? '?'}, hp ${last.hitpoints ?? '?'})`);
  lines.push(`intent: ${last.intention || '-'}`);
  lines.push(`status: ${last.status || '?'}${last.paused ? ' (paused)' : ''}`);
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
  syncStatePre(document.getElementById('infotext'), lines, '(no state)');

  setText(document.getElementById('dialog'), last.modal_name ? `window: ${last.modal_name}` : '(no window open)');
  syncStatePre(document.getElementById('chat'), last.chat || [], '(no messages)');

  const fmt = (items, empty) => (items && items.length)
    ? items.map(i => (i.count > 1 ? i.count + 'x ' : '') + (i.name || 'object'))
    : [empty];
  // One `inv` entry per backpack slot (session_heartbeat slot_items):
  // occupied slots, not the sum of the stacks.
  const occupied = (last.inv || []).filter(i => (i.count || 0) > 0).length;
  setText(document.getElementById('invtitle'), `Inventory (${occupied}/28)`);
  syncStatePre(document.getElementById('inv'),
    ['Backpack:'].concat(fmt(last.inv, '(empty)'), ['', 'Worn:'], fmt(last.worn, '(nothing)')),
    '(empty)');
  syncStatePre(document.getElementById('skills'),
    (last.skills || []).map(s => `${SKILLS[s.id] || ('stat' + s.id)} ${s.level} (${s.xp}xp)`),
    '(no skills)');

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
  syncStatePre(document.getElementById('scenery'),
    [`Scenery (${sceneryCount}):`].concat(locLines.slice(0, 60).length ? locLines.slice(0, 60) : ['(none)'],
      [''], [`Ground items (${(last.objs || []).length}):`], objLines.length ? objLines : ['(none)']),
    '(none)');
}

connectSocket();
loadStaticMap();
paint();
document.getElementById("rtbtn").onclick = () => {
  rt = !rt;
  try { localStorage.setItem("spectate-rt", rt ? "1" : "0"); } catch (e) { /* private mode */ }
  syncRtBtn();
};
syncRtBtn();
// Session controls act on the followed session (selected from the drawer):
// open uses the account dropdown, run/stop/pause/close use the selection.
// Scripts come from /api/scripts.
function ctlMsg(text, good) {
  const el = document.getElementById("ctlmsg");
  if (el) { el.textContent = text || ""; el.style.color = good ? "#46c46f" : "#d9a441"; }
}
function parseArgs(text) {
  const parts = (text || "").trim().split(/\s+/).filter(Boolean);
  return parts.map(part => {
    try { return JSON.parse(part); } catch (e) { return part; }
  });
}
async function ctlCall(label, done, path, body) {
  ctlMsg(label + " …", false);
  try {
    const response = await fetch(path, {method: "POST", headers: {"Content-Type": "application/json"}, body: JSON.stringify(body)});
    const reply = await response.json();
    ctlMsg(reply.ok ? done : ("error: " + (reply.error || "?")), !!reply.ok);
  } catch (e) {
    ctlMsg("error: " + e, false);
  }
}
async function loadScripts() {
  try {
    const response = await fetch("/api/scripts");
    const reply = await response.json();
    const scripts = reply.scripts || [];
    document.getElementById("script").innerHTML = scripts.map(s => `<option value="${esc(s)}">${esc(s)}</option>`).join("");
  } catch (e) { /* read-only fallback */ }
}
async function loadAccounts() {
  try {
    const response = await fetch("/api/accounts");
    const reply = await response.json();
    const picker = document.getElementById("account");
    const current = picker.value;
    picker.innerHTML = `<option value="">account…</option>` + (reply.accounts || []).map(a => `<option value="${esc(a)}">${esc(a)}</option>`).join("");
    if (current && [...picker.options].some(o => o.value === current)) picker.value = current;
  } catch (e) { /* read-only fallback */ }
}
// One listener on the stable <table>: the rows are re-rendered 1x/s, so
// a click often spans two different row nodes (press on one, release on
// its replacement) and never fires on the row. Selecting on mousedown
// registers before any re-render can swap the node; click stays as a
// fallback (keyboard, touch edge cases). select() is idempotent.
["mousedown", "click"].forEach((kind) => {
  document.getElementById("players").addEventListener(kind, (event) => {
    if (kind === "mousedown") { listPressed = true; listPressedAt = Date.now(); }
    const row = event.target.closest("tr[data-account]");
    if (!row) return;
    event.preventDefault();
    const account = row.getAttribute("data-account");
    if (account !== selected) {
      select(account);
      for (const other of document.querySelectorAll("#players tr[data-account]")) {
        other.classList.toggle("cur", other === row);
      }
    }
    toggleList(true); // user picked: drawer closes (auto-follow on load must not)
  });
});
// Release anywhere ends the press; the deferred render runs after the
// click event of this release (setTimeout), so header sorting still fires.
document.addEventListener("mouseup", () => {
  if (!listPressed) return;
  listPressed = false;
  setTimeout(() => {
    if (listPressed || !pendingJobs) return;
    const jobs = pendingJobs;
    pendingJobs = null;
    renderList(jobs);
  }, 0);
});
document.getElementById("drawer-close").onclick = () => toggleList(true);
document.getElementById("btn-open").onclick = () => { const typed = (document.getElementById("account").value || "").trim(); if (!typed) return ctlMsg("pick a bot first", false); ctlCall("opening", "open", "/api/open", {account: typed}); };
document.getElementById("btn-close").onclick = () => { if (!selected) return ctlMsg("follow a session first", false); ctlCall("closing", "closed", "/api/close", {account: selected}); };
document.getElementById("btn-run").onclick = () => {
  if (!selected) return ctlMsg("follow a session first", false);
  const script = document.getElementById("script").value;
  if (!script) return ctlMsg("pick a script", false);
  ctlCall("starting " + script, "started", "/api/run", {account: selected, script: script, args: parseArgs(document.getElementById("args").value)});
};
document.getElementById("btn-stop").onclick = () => { if (!selected) return ctlMsg("follow a session first", false); ctlCall("stopping", "stopped", "/api/unload", {account: selected}); };
document.getElementById("btn-pause-toggle").onclick = () => {
  if (!selected) return ctlMsg("follow a session first", false);
  if (last && last.paused) ctlCall("resuming", "", "/api/resume", {account: selected});
  else ctlCall("pausing", "", "/api/pause", {account: selected});
};
loadScripts();
loadAccounts();
window.spectator = { select: select, toggle: toggleList };
})();
