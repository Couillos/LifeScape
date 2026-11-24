// Pure panel helpers (node-testable, spectator.test.js).
var SpectatorPanels = (function () {
  // "il y a 2 h 13 min" from the error's unix time (at_unix); '' without one.
  function errorAge(error, nowSeconds) {
    if (!error || typeof error.at_unix !== 'number') return '';
    const secs = Math.max(0, Math.floor(nowSeconds - error.at_unix));
    const hours = Math.floor(secs / 3600);
    const minutes = Math.floor((secs % 3600) / 60);
    if (hours > 0) return 'il y a ' + hours + ' h ' + String(minutes).padStart(2, '0') + ' min';
    if (minutes > 0) return 'il y a ' + minutes + ' min';
    return 'il y a ' + secs + ' s';
  }
  // Panel line for last_error / previous_error (with their age).
  function errorLine(label, error, nowSeconds) {
    if (!error) return null;
    const age = errorAge(error, nowSeconds);
    return label + ': ' + JSON.stringify(error) + (age ? ' (' + age + ')' : '');
  }

  // ---- Game tabs (2026-11-24): the backpack, worn and stats tabs drawn
  // like the rev 274 client (Client-TS `drawInterface`, `invNumber`,
  // `getIfVar`, PixFont), from the layout the icons generator reads in
  // the interface cache (`/icons/index.json` `tabs`, sheet tabs.png).
  // These helpers turn a snapshot into a draw list; the page only plots
  // it (one canvas per tab, redrawn when the list changes).

  // Client.levelExperience: entry i = xp at which level i + 2 starts.
  const LEVEL_XP = (function () {
    const table = [];
    let acc = 0;
    for (let i = 0; i < 99; i++) {
      const level = i + 1;
      acc += Math.floor(level + Math.pow(2, level / 7) * 300);
      table.push(Math.floor(acc / 4));
    }
    return table;
  })();
  // Skill.used: the stats the client's "Total Lvl" sums (not slayer 18,
  // not the unused ids).
  const USED_STATS = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 20];
  // PixFont.updateState colour tags.
  const TAG_COLOURS = { red: 0xff0000, gre: 0x00ff00, blu: 0x0000ff, yel: 0xffff00, cya: 0x00ffff, mag: 0xff00ff, whi: 0xffffff, bla: 0x000000,
    lre: 0xff9040, dre: 0x800000, dbl: 0x000080, or1: 0xffb000, or2: 0xff7000, or3: 0xff3000, gr1: 0xc0ff00, gr2: 0x80ff00, gr3: 0x40ff00 };
  const YELLOW = 0xffff00;

  // Client.invNumber: the stack count of an inventory slot.
  function invNumber(amount) {
    if (amount < 100000) return String(amount);
    if (amount < 10000000) return Math.floor(amount / 1000) + 'K';
    return Math.floor(amount / 1000000) + 'M';
  }
  // Client.inf: a `%1` value.
  function inf(value) {
    return value < 999999999 ? String(value) : '*';
  }
  // Top-left of an inventory slot (drawInterface TYPE_INV): grid pitch
  // 32 + margin, plus the slot's own offset for the first 20 slots.
  function slotXY(inv, slot) {
    let x = inv.x + (slot % inv.cols) * (inv.mx + 32);
    let y = inv.y + Math.floor(slot / inv.cols) * (inv.my + 32);
    if (slot < 20 && inv.off && inv.off[slot]) {
      x += inv.off[slot][0];
      y += inv.off[slot][1];
    }
    return { x: x, y: y };
  }
  // Slot under a point (the client's 32x32 hover square), -1 for none.
  function slotAt(inv, x, y) {
    for (let slot = 0; slot < inv.cols * inv.rows; slot++) {
      const at = slotXY(inv, slot);
      if (x >= at.x && y >= at.y && x < at.x + 32 && y < at.y + 32) return slot;
    }
    return -1;
  }
  // Items by slot: an entry's `slot` (worker heartbeat), else its wearpos
  // (`wear`: worn tab of an older worker), else the first free slot in
  // list order (backpack of an older worker: packed, gaps unknown).
  function placeItems(list, capacity, wear) {
    const slots = new Array(capacity).fill(null);
    const loose = [];
    for (const item of list || []) {
      if (!item || typeof item !== 'object' || item.id === undefined || item.id === null) continue;
      let slot = Number.isInteger(item.slot) ? item.slot : -1;
      if (slot < 0 && wear && Number.isInteger(wear[item.id])) slot = wear[item.id];
      if (slot >= 0 && slot < capacity && !slots[slot]) slots[slot] = item;
      else loose.push(item);
    }
    for (const item of loose) {
      const free = slots.indexOf(null);
      if (free >= 0) slots[free] = item;
    }
    return slots;
  }
  // The client prints a slot's count for a stackable obj (sprite width
  // 33) or any count other than 1.
  function showsCount(stack, id, count) {
    return count !== 1 || (stack ? stack.has(Number(id)) : false);
  }
  // Hover layer under a point (addComponentOptions: the last component
  // whose box holds the point wins), null for none.
  function hoverLayer(hovers, x, y) {
    let found = null;
    for (const hover of hovers || []) {
      if (x >= hover.x && y >= hover.y && x < hover.x + hover.w && y < hover.y + hover.h) found = hover.layer;
    }
    return found;
  }
  function skillOf(snapshot, id) {
    return (snapshot.skills || []).find(skill => skill.id === id) || null;
  }
  function baseOf(skill) {
    return skill ? (typeof skill.base === 'number' ? skill.base : skill.level) : null;
  }
  // Client.getIfVar for the ops the tabs use; null when the snapshot
  // does not carry the value (the text is then left out).
  function ifValue(text, snapshot) {
    const skill = text.arg !== undefined ? skillOf(snapshot, text.arg) : null;
    const num = (value) => (typeof value === 'number' ? value : null);
    switch (text.op) {
      case 'stat_level': return skill ? num(skill.level) : null;
      case 'stat_base_level': return num(baseOf(skill));
      case 'stat_xp': return skill ? num(skill.xp) : null;
      case 'stat_xp_remaining': {
        const base = baseOf(skill);
        return base >= 1 && base <= 99 ? LEVEL_XP[base - 1] : null;
      }
      case 'combat_level': return num(snapshot.combat);
      case 'total_level': {
        let total = 0;
        let seen = false;
        for (const id of USED_STATS) {
          const base = baseOf(skillOf(snapshot, id));
          if (typeof base === 'number') { total += base; seen = true; }
        }
        return seen ? total : null;
      }
      case 'pushvar': return text.varp === 'qp' ? num(snapshot.quest_points) : null;
      default: return null;
    }
  }
  function ifText(text, value) {
    return value === null || value === undefined ? null : text.text.split('%1').join(inf(value));
  }
  // PixFont.stringWid: tags take no room.
  function stringWidth(font, str) {
    let width = 0;
    for (let i = 0; i < str.length; i++) {
      if (str.charAt(i) === '@' && i + 4 < str.length && str.charAt(i + 4) === '@') { i += 4; continue; }
      const glyph = font.chars[str.charCodeAt(i)];
      if (glyph) width += glyph[3];
    }
    return width;
  }
  // PixFont.drawStringTag (centreStringTag when `centre`): one plot per
  // glyph, its black shadow first at +1,+1. `y` is the baseline.
  function glyphs(font, str, x, y, colour, shadow, centre) {
    if (centre) x -= Math.floor(stringWidth(font, str) / 2);
    y -= font.height;
    const out = [];
    for (let i = 0; i < str.length; i++) {
      if (str.charAt(i) === '@' && i + 4 < str.length && str.charAt(i + 4) === '@') {
        const tag = TAG_COLOURS[str.substring(i + 1, i + 4)];
        if (tag !== undefined) colour = tag;
        i += 4;
        continue;
      }
      const glyph = font.chars[str.charCodeAt(i)];
      if (!glyph) continue;
      const [image, offX, offY, advance] = glyph;
      if (image) {
        if (shadow) out.push({ glyph: image, x: x + offX + 1, y: y + offY + 1, colour: 0 });
        out.push({ glyph: image, x: x + offX, y: y + offY, colour: colour });
      }
      x += advance;
    }
    return out;
  }
  // Items a tab shows by slot (null = empty), from the snapshot.
  function tabItems(index, name, snapshot) {
    const tab = index.tabs.tabs[name];
    if (!tab || !tab.inv) return [];
    const worn = name === 'wornitems';
    return placeItems(worn ? snapshot.worn : snapshot.inv, tab.inv.cols * tab.inv.rows, worn ? index.items.wear : null);
  }
  // Draw list of one tab: the panel, then (inv) each slot's icon and
  // count or its empty-slot sprite, the `%1` texts, and the hover layer
  // `layer` on top. `cellOf(id, count)` = the item sheet cell.
  function tabDraw(index, name, snapshot, layer, cellOf, stack) {
    const tabs = index.tabs;
    const tab = tabs.tabs[name];
    const ops = [{ image: tab.image, x: 0, y: 0 }];
    if (tab.inv) {
      const items = tabItems(index, name, snapshot);
      items.forEach((item, slot) => {
        const at = slotXY(tab.inv, slot);
        if (item) {
          ops.push({ item: cellOf(item.id, item.count), x: at.x, y: at.y });
          if (showsCount(stack, item.id, item.count)) ops.push(...glyphs(tabs.fonts.p11, invNumber(item.count), at.x, at.y + 9, YELLOW, true, false));
        } else if (slot < 20 && tab.inv.empty[slot]) {
          ops.push({ image: tab.inv.empty[slot], x: at.x, y: at.y });
        }
      });
    }
    const texts = (list, dx, dy) => {
      for (const text of list) {
        const str = ifText(text, ifValue(text, snapshot));
        if (str !== null) ops.push(...glyphs(tabs.fonts[text.font], str, text.x + dx, text.y + dy, text.colour, text.shadow, text.centre));
      }
    };
    texts(tab.texts, 0, 0);
    const over = layer !== null && layer !== undefined ? tab.layers[layer] : null;
    if (over) {
      ops.push({ image: over.image, x: over.x, y: over.y });
      texts(over.texts, over.x, over.y);
    }
    return ops;
  }
  // Hover text of an item: its name in the client's menu colour (@lre@),
  // the full count after it for a stack.
  function itemLabel(item, stack) {
    const name = item.name || ('object #' + item.id);
    const count = showsCount(stack, item.id, item.count) ? '@whi@ x ' + Number(item.count).toLocaleString('en-US') : '';
    return '@lre@' + name + count;
  }
  return { errorAge: errorAge, errorLine: errorLine, LEVEL_XP: LEVEL_XP, invNumber: invNumber, inf: inf, slotXY: slotXY, slotAt: slotAt,
    placeItems: placeItems, showsCount: showsCount, hoverLayer: hoverLayer, ifValue: ifValue, ifText: ifText, stringWidth: stringWidth,
    glyphs: glyphs, tabItems: tabItems, tabDraw: tabDraw, itemLabel: itemLabel };
})();
if (typeof module !== 'undefined' && module.exports) module.exports = SpectatorPanels;

if (typeof document !== 'undefined') (() => {
// Everything stays inside this closure: a browser extension injected into
// the page shares the global scope, and generic globals (socket, select,
// connect...) collide with its bundle.

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
  { key: "script", label: "Script", kind: "str" },
  { key: "zone", label: "Zone", kind: "str" },
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
  document.getElementById("titletext").classList.toggle("sel", Boolean(selected));
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
  for (const id of ["infotext", "log", "chat", "scenery"]) {
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
        // `/#<account>` (links from the errors page) wins; otherwise
        // never auto-follow the staff account: it is only followed on click
        const wanted = decodeURIComponent((location.hash || "").replace(/^#/, ""));
        const first = data.jobs.find(j => j.account === wanted) ||
          data.jobs.find(j => j.account !== "admin") || data.jobs[0];
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

// Game tabs (backpack, worn, stats): one canvas each at the client's
// size, plotted from SpectatorPanels.tabDraw with the sheets of
// /icons/index.json (items.png, tabs.png: two images, no request per
// icon). A canvas is redrawn only when its draw list changed, so the
// 1 s frames never flicker; the hover (mouse, or a tap on touch) is kept
// per tab and re-applied to every frame. Index or sheets unreachable:
// the canvases stay blank.
const GameTabs = (() => {
  const tabs = [
    { name: "inventory", canvas: document.getElementById("tab-inv") },
    { name: "wornitems", canvas: document.getElementById("tab-worn") },
    { name: "stats", canvas: document.getElementById("tab-stats") },
  ];
  const tip = document.getElementById("tabtip");
  const tipCanvas = tip.querySelector("canvas");
  let assets = null; // { index, sheet, items, stack, tints }
  let snapshot = null;

  function loadImage(url) {
    return new Promise((resolve, reject) => {
      const image = new Image();
      image.onload = () => resolve(image);
      image.onerror = reject;
      image.src = url;
    });
  }
  if (typeof LifeIcons !== "undefined") LifeIcons.onIndex((index) => {
    if (!index || !index.tabs || !index.items) return;
    const url = (sheet) => "/icons/" + sheet.file + "?v=" + encodeURIComponent(sheet.hash || "");
    Promise.all([loadImage(url(index.tabs)), loadImage(url(index.items))]).then(([sheet, items]) => {
      assets = { index, sheet, items, stack: new Set(index.items.stack || []), tints: new Map() };
      for (const tab of tabs) draw(tab);
    }).catch(() => {});
  });

  // The tabs sheet with its white glyph masks in one colour (cached).
  function tint(colour) {
    let canvas = assets.tints.get(colour);
    if (!canvas) {
      canvas = document.createElement("canvas");
      canvas.width = assets.sheet.width;
      canvas.height = assets.sheet.height;
      const ctx = canvas.getContext("2d");
      ctx.drawImage(assets.sheet, 0, 0);
      ctx.globalCompositeOperation = "source-in";
      ctx.fillStyle = "#" + colour.toString(16).padStart(6, "0");
      ctx.fillRect(0, 0, canvas.width, canvas.height);
      assets.tints.set(colour, canvas);
    }
    return canvas;
  }
  function plot(ctx, ops) {
    const images = assets.index.tabs.images;
    const cols = assets.index.items.cols;
    const cell = assets.index.items.cell;
    for (const op of ops) {
      if (op.image) {
        const [sx, sy, w, h] = images[op.image];
        ctx.drawImage(assets.sheet, sx, sy, w, h, op.x, op.y, w, h);
      } else if (op.glyph) {
        const [sx, sy, w, h] = images[op.glyph];
        ctx.drawImage(tint(op.colour), sx, sy, w, h, op.x, op.y, w, h);
      } else if (op.item) {
        ctx.drawImage(assets.items, (op.item % cols) * cell, Math.floor(op.item / cols) * cell, cell, cell, op.x, op.y, cell, cell);
      }
    }
  }
  const cellOf = (id, count) => LifeIcons.itemCell(assets.index, id, count);

  function draw(tab) {
    if (!assets || !snapshot || !tab.canvas) return;
    const spec = assets.index.tabs.tabs[tab.name];
    if (!spec) return;
    const layer = tab.hover && spec.hovers.length ? SpectatorPanels.hoverLayer(spec.hovers, tab.hover.x, tab.hover.y) : null;
    const ops = SpectatorPanels.tabDraw(assets.index, tab.name, snapshot, layer, cellOf, assets.stack);
    const key = JSON.stringify(ops);
    if (key !== tab.key) {
      tab.key = key;
      if (tab.canvas.width !== spec.w) tab.canvas.width = spec.w;
      if (tab.canvas.height !== spec.h) tab.canvas.height = spec.h;
      const ctx = tab.canvas.getContext("2d");
      ctx.imageSmoothingEnabled = false;
      ctx.clearRect(0, 0, spec.w, spec.h);
      plot(ctx, ops);
    }
    if (spec.inv) syncTip(tab, spec);
  }

  // Item hover box: the client's b12 font on the frame colours of its
  // right-click menu (0x5d5447 border, black inside).
  let tipOwner = null;
  function syncTip(tab, spec) {
    const item = tab.hover ? SpectatorPanels.tabItems(assets.index, tab.name, snapshot)[SpectatorPanels.slotAt(spec.inv, tab.hover.x, tab.hover.y)] : null;
    if (!item) {
      if (tipOwner === tab) { tip.hidden = true; tipOwner = null; tip._key = null; }
      return;
    }
    tipOwner = tab;
    const label = SpectatorPanels.itemLabel(item, assets.stack);
    if (tip._key !== label) {
      tip._key = label;
      const font = assets.index.tabs.fonts.b12;
      const width = SpectatorPanels.stringWidth(font, label) + 8;
      const height = font.height + 8;
      tipCanvas.width = width;
      tipCanvas.height = height;
      const ctx = tipCanvas.getContext("2d");
      ctx.imageSmoothingEnabled = false;
      ctx.fillStyle = "#5d5447";
      ctx.fillRect(0, 0, width, height);
      ctx.fillStyle = "#000";
      ctx.fillRect(1, 1, width - 2, height - 2);
      plot(ctx, SpectatorPanels.glyphs(font, label, 4, height - 5, 0xffffff, true, false));
    }
    tip.hidden = false;
    const gap = 14;
    let left = tab.hover.clientX + gap;
    let top = tab.hover.touch ? tab.hover.clientY - tipCanvas.height - 2 * gap : tab.hover.clientY + gap;
    if (left + tipCanvas.width > window.innerWidth - 4) left = Math.max(4, tab.hover.clientX - gap - tipCanvas.width);
    if (top < 4) top = tab.hover.clientY + gap;
    tip.style.left = left + "px";
    tip.style.top = top + "px";
  }

  function pointAt(tab, event) {
    const box = tab.canvas.getBoundingClientRect();
    return {
      x: Math.floor((event.clientX - box.left) * tab.canvas.width / box.width),
      y: Math.floor((event.clientY - box.top) * tab.canvas.height / box.height),
      clientX: event.clientX,
      clientY: event.clientY,
      touch: event.pointerType === "touch" || event.pointerType === "pen",
    };
  }
  for (const tab of tabs) {
    if (!tab.canvas) continue;
    tab.canvas.addEventListener("pointermove", (event) => {
      if (event.pointerType === "touch") return;
      tab.hover = pointAt(tab, event);
      draw(tab);
    });
    tab.canvas.addEventListener("pointerleave", (event) => {
      if (event.pointerType === "touch" || event.pointerType === "pen") return;
      tab.hover = null;
      draw(tab);
    });
    // Touch: a tap shows what is under the finger until the next tap.
    tab.canvas.addEventListener("pointerdown", (event) => {
      if (event.pointerType === "mouse") return;
      for (const other of tabs) if (other !== tab && other.hover) { other.hover = null; draw(other); }
      tab.hover = pointAt(tab, event);
      draw(tab);
    });
  }
  document.addEventListener("pointerdown", (event) => {
    if (event.pointerType === "mouse" || tabs.some((tab) => tab.canvas === event.target)) return;
    for (const tab of tabs) if (tab.hover) { tab.hover = null; draw(tab); }
  });

  return {
    update(next) {
      snapshot = next;
      for (const tab of tabs) draw(tab);
    },
  };
})();

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
  const nowSeconds = Date.now() / 1000;
  const lastErrorLine = SpectatorPanels.errorLine('last error', last.last_error, nowSeconds);
  if (lastErrorLine) lines.push(lastErrorLine);
  const previousErrorLine = SpectatorPanels.errorLine('previous error', last.previous_error, nowSeconds);
  if (previousErrorLine) lines.push(previousErrorLine);
  lines.push(modal ? `window open: ${modal}` : 'no window open');
  if (hint) lines.push(`hint arrow: ${hint}`);
  if (last.flash !== null && last.flash !== undefined && last.flash !== -1) lines.push(`flashing tab: ${last.flash}`);
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

  syncStatePre(document.getElementById('chat'), last.chat || [], '(no messages)');

  // One `inv` entry per backpack slot (session_heartbeat slot_items):
  // occupied slots, not the sum of the stacks.
  const occupied = (last.inv || []).filter(i => (i.count || 0) > 0).length;
  setText(document.getElementById('invtitle'), `Inventory (${occupied}/28)`);
  GameTabs.update(last);

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
