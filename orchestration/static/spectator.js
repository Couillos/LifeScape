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
// the map image is rendered by the server with the client's own minimap
// passes. Frames are atomic: draw() only paints when the image for the
// exact origin/level of `last` is cached, otherwise it keeps the previous
// complete frame until the image lands (never map xor overlays).
const mapCache = new Map();
const mapPending = new Set();
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
  if (!pill) return;
  if (!last) { pill.style.display = "none"; return; }
  pill.style.display = "";
  pill.textContent = scriptState(last);
  pill.className = "pill " + scriptState(last);
}

function toggleList(force) {
  const hidden = force !== undefined ? force : !document.body.classList.contains("list-hidden");
  document.body.classList.toggle("list-hidden", hidden);
  document.getElementById("toggle").setAttribute("aria-expanded", String(!hidden));
  localStorage.setItem("list-hidden", hidden ? "1" : "0");
}
if (localStorage.getItem("list-hidden") === "0") toggleList(false);

function renderList(jobs) {
  document.getElementById("count").textContent = jobs.length;
  const ul = document.getElementById("players");
  if (!jobs.length) { ul.innerHTML = '<li class="muted">Nobody supervised.</li>'; return jobs; }
  ul.innerHTML = jobs.map(job => {
    const [, x, z] = job.position || [0,0,0];
    const cur = job.account === selected ? "cur" : "";
    const paused = job.paused ? " ⏸ paused" : "";
    return `<li><a href="#" data-account="${esc(job.account)}" class="${cur}">
      <span class="dot ${statusClass(job.status)}"></span><b>${esc(job.account_name || job.account)}</b> lvl ${job.combat ?? "-"} (${x},${z})${job.modal ? " [window]" : ""}${paused}
      <br><span class="muted">${esc(job.intention || job.status)} · ${esc(job.script)}${paused} · hp ${job.hitpoints ?? "-"}</span></a></li>`;
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
  last = null;
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
        // never auto-follow the staff account: it is only followed on click
        const first = data.jobs.find(j => j.account !== "admin") || data.jobs[0];
        select(first.account);
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
      last = merged;
      // Session gap, not a dead pipe: the socket stays live (stamp below
      // keeps ticking) while the worker relogs. Say so with the stale
      // age instead of showing a frozen frame as if it were live.
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
  pre.textContent = lines.join("\n") || "(no log)";
  if (follow) {
    pre.scrollTop = pre.scrollHeight;
  } else {
    pre.scrollTop = Math.max(0, pre.scrollHeight - pre.clientHeight - before);
  }
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
  // Ground, walls and mapscene sprites come from the server image: it is
  // built with the client's own passes, so the canvas is the client minimap
  // instead of a redraw that drifts on overlays and sprite order.
  const originX = pos.px - R;
  const originZ = pos.pz - R;
  const mapKey = originX + ',' + originZ + ',' + level;
  const mapLeft = cx - R * TILE;
  const mapTop = cy - (R + 1) * TILE;
  const mapSize = (R * 2 + 1) * TILE;
  let mapImg = null, mapDx = 0, mapDz = 0;
  const exact = mapCache.get(mapKey);
  if (exact) {
    mapImg = exact.img;
  } else {
    if (!mapPending.has(mapKey)) {
      mapPending.add(mapKey);
      const image = new Image();
      image.onload = () => {
        mapPending.delete(mapKey);
        mapCache.set(mapKey, {img: image, ox: originX, oz: originZ, lvl: level});
        if (mapCache.size > 48) mapCache.delete(mapCache.keys().next().value);
        paint();
      };
      image.onerror = () => { mapPending.delete(mapKey); };
      image.src = 'minimap.png?x=' + originX + '&z=' + originZ + '&level=' + level + '&size=' + (R * 2 + 1);
    }
    // En mouvement l'origine exacte n'est jamais en cache quand son image
    // arrive : on réutilise la carte même niveau la plus proche en la
    // décalant, sinon un bot qui marche ne dessine plus rien jusqu'à l'arrêt.
    let best = null, bestDist = 1e9;
    for (const entry of mapCache.values()) {
      if (entry.lvl !== level) continue;
      const dx = entry.ox - originX, dz = entry.oz - originZ;
      if (Math.abs(dx) > R * 2 || Math.abs(dz) > R * 2) continue;
      const dist = Math.abs(dx) + Math.abs(dz);
      if (dist < bestDist) { bestDist = dist; best = entry; }
    }
    if (!best) {
      // Aucune carte à décaler (premier chargement) : on garde l'ancien
      // frame complet plutôt qu'un fond noir avec des overlays orphelins.
      return;
    }
    mapImg = best.img;
    mapDx = best.ox - originX;
    mapDz = best.oz - originZ;
  }
  ctx.fillStyle = '#111';
  ctx.fillRect(0, 0, cv.width, cv.height);
  // The window origin tracks the player tile, so the blit rect is fixed:
  // no fractional slide between the map and the integer-tile overlays.
  // (Fallback ci-dessus : la carte de secours est décalée pour rester
  // alignée, le bord manquant reste noir une seconde.)
  ctx.imageSmoothingEnabled = false;
  ctx.drawImage(mapImg, mapLeft + mapDx * TILE, mapTop - mapDz * TILE, mapSize, mapSize);
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
  document.getElementById('infotext').textContent = lines.join('\n');

  document.getElementById('dialog').textContent = last.modal_name ? `window: ${last.modal_name}` : '(no window open)';
  document.getElementById('chat').textContent = (last.chat || []).join('\n') || '(no messages)';

  const fmt = (items, empty) => (items && items.length)
    ? items.map(i => (i.count > 1 ? i.count + 'x ' : '') + (i.name || 'object')).join('\n')
    : empty;
  document.getElementById('inv').textContent = 'Backpack:\n' + fmt(last.inv, '(empty)') + '\n\nWorn:\n' + fmt(last.worn, '(nothing)');
  document.getElementById('skills').textContent = (last.skills || []).map(s => `${SKILLS[s.id] || ('stat' + s.id)} ${s.level} (${s.xp}xp)`).join('\n') || '(no skills)';

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
    `Scenery (${sceneryCount}):\n` + (locLines.slice(0, 60).join('\n') || '(none)') +
    `\n\nGround items (${(last.objs || []).length}):\n` + (objLines.join('\n') || '(none)');
}

connectSocket();
paint();
document.getElementById("rtbtn").onclick = () => {
  rt = !rt;
  try { localStorage.setItem("spectate-rt", rt ? "1" : "0"); } catch (e) { /* private mode */ }
  syncRtBtn();
};
syncRtBtn();
// Session controls: open/run/stop/pause/resume/close on the typed account,
// defaulting to the followed bot. Scripts come from /api/scripts.
function ctlAccount() {
  const input = document.getElementById("account");
  const typed = (input && input.value || "").trim();
  return typed || selected || "";
}
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
// One listener on the stable <ul>: the rows are re-rendered 5x/s, so a
// click often spans two different row nodes (press on one, release on
// its replacement) and never fires on the link. Selecting on mousedown
// registers before any re-render can swap the node; click stays as a
// fallback (keyboard, touch edge cases). select() is idempotent.
["mousedown", "click"].forEach((kind) => {
  document.getElementById("players").addEventListener(kind, (event) => {
    const link = event.target.closest("a[data-account]");
    if (!link) return;
    event.preventDefault();
    select(link.getAttribute("data-account"));
    toggleList(true); // user picked: drawer closes (auto-follow on load must not)
  });
});
document.getElementById("drawer-close").onclick = () => toggleList(true);
document.getElementById("btn-open").onclick = () => { const a = ctlAccount(); if (!a) return ctlMsg("pick a bot first", false); ctlCall("opening", "open", "/api/open", {account: a}); };
document.getElementById("btn-close").onclick = () => { if (!selected) return ctlMsg("follow a session first", false); ctlCall("closing", "closed", "/api/close", {account: selected}); };
document.getElementById("btn-run").onclick = () => {
  const a = ctlAccount(); if (!a) return ctlMsg("pick a bot first", false);
  const script = document.getElementById("script").value;
  if (!script) return ctlMsg("pick a script", false);
  ctlCall("starting " + script, "started", "/api/run", {account: a, script: script, args: parseArgs(document.getElementById("args").value)});
};
document.getElementById("btn-stop").onclick = () => { if (!selected) return ctlMsg("follow a session first", false); ctlCall("stopping", "stopped", "/api/unload", {account: selected}); };
document.getElementById("btn-pause").onclick = () => { if (!selected) return ctlMsg("follow a session first", false); ctlCall("pausing", "", "/api/pause", {account: selected}); };
document.getElementById("btn-resume").onclick = () => { if (!selected) return ctlMsg("follow a session first", false); ctlCall("resuming", "", "/api/resume", {account: selected}); };
loadScripts();
loadAccounts();
window.spectator = { select: select, toggle: toggleList };
})();
