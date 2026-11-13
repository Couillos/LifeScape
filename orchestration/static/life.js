// Lives front: full-width table, one account per row (§6.4, D47),
// detail below. Sorting rides the shared table.js (D46). Read-only
// dashboard page: bots never read it.
let lives = [];
let selected = null;
let query = "";

// Column order matches LIVES_COLUMNS; TableSort keeps the sort state
// across refreshes, missing values last in both directions.
const COLS = [
  { key: "account", label: "Compte", kind: "str" },
  { key: "goal", label: "But", kind: "str" },
  { key: "profile", label: "Profil", kind: "str" },
  { key: "active_task", label: "Tâche active", kind: "str" },
  { key: "chain", label: "Chaîne en cours", kind: "str" },
  { key: "engagement", label: "Engagement", kind: "str" },
  { key: "active", label: "Actives", kind: "num" },
  { key: "ready", label: "Prêtes", kind: "num" },
  { key: "pending", label: "En attente", kind: "num" },
  { key: "blocked", label: "Bloquées", kind: "num" },
  { key: "failed", label: "Échouées", kind: "num" },
  { key: "done", label: "Faites", kind: "num" },
  { key: "updated", label: "Dernier changement", kind: "num", fmt: (v) => dateText(v) },
];

async function getJson(url) {
  const response = await fetch(url);
  return response.json();
}

function goalText(goal) {
  if (!goal || typeof goal !== 'object') return '-';
  const kind = goal.kind || '?';
  if (kind === 'max_skills') return 'max ' + (goal.skills || []).join(',') + ' → ' + goal.level;
  if (kind === 'obtain') return 'obtain ' + goal.count + ' × ' + goal.item;
  if (kind === 'complete_quest') return 'quest ' + goal.quest;
  if (kind === 'equip_best') return 'equip ' + goal.style;
  if (kind === 'access_area') return 'reach ' + goal.area;
  if (kind === 'make_money') return 'make money';
  if (kind === 'get_rich') return 'get rich' + (goal.merchant === true ? ' (merchant)' : goal.merchant === false ? ' (producer)' : '');
  return kind;
}

function profileText(profile) {
  if (!profile || typeof profile !== 'object') return '-';
  const bits = [];
  if (profile.ironman) bits.push('ironman');
  if (profile.archetype) bits.push(profile.archetype);
  return bits.join(' · ') || '-';
}

function engText(engagement) {
  if (!engagement || typeof engagement !== 'object' || !engagement.node) return '-';
  return engagement.node + ': ' + engagement.min_ticks + '–' + engagement.max_ticks + 't';
}

function dateText(stamp) {
  if (!stamp) return '-';
  const date = new Date(stamp * 1000);
  const pad = n => String(n).padStart(2, '0');
  return pad(date.getDate()) + '/' + pad(date.getMonth() + 1) + ' ' + pad(date.getHours()) + ':' + pad(date.getMinutes());
}

function renderIndex() {
  const rows = filtered().map(displayRow);
  TableSort.render("index", COLS, rows);
  // Row click selects the bot for the detail below; the account is the
  // first cell. Rebound after every render (sort rebuilds the rows).
  const table = document.getElementById("index");
  const trs = table.getElementsByTagName("tr");
  const known = new Set(rows.map((row) => row.account));
  for (let i = 1; i < trs.length; i++) {
    const first = trs[i].getElementsByTagName("td")[0];
    if (!first || !known.has(first.textContent)) continue;
    const account = first.textContent;
    if (account === selected) trs[i].className = "cur";
    trs[i].onclick = ((name) => () => pick(name))(account);
  }
  document.getElementById("count").textContent = rows.length + " account(s)";
}

// Sortable text rides beside the raw values: TableSort sorts the raw
// cell, fmt only changes the display.
function displayRow(row) {
  return {
    account: row.account,
    goal: goalText(row.goal),
    profile: profileText(row.profile),
    active_task: row.active_task || null,
    chain: row.chain && row.chain.length ? row.chain.join(" → ") : null,
    engagement: engText(row.engagement) === "-" ? null : engText(row.engagement),
    active: row.active,
    ready: row.ready,
    pending: row.pending,
    blocked: row.blocked,
    failed: row.failed,
    done: row.done,
    updated: row.updated || null,
  };
}

async function loadLives() {
  try {
    const payload = await getJson('/api/lives');
    if (payload.ok) lives = payload.lives;
  } catch (e) { /* daemon down: keep the last list */ }
  const fromHash = (location.hash || '').replace(/^#/, '');
  if (selected === null) {
    const known = lives.some(row => row.account === fromHash);
    selected = known ? fromHash : (lives.length ? lives[0].account : null);
  }
  renderIndex();
}

function filtered() {
  const needle = query.trim().toLowerCase();
  if (!needle) return lives;
  return lives.filter(row => row.account.toLowerCase().includes(needle));
}

function onSearch() {
  query = document.getElementById('search').value;
  renderIndex();
}

function kindText(node) {
  const kind = node.kind || '?';
  const params = node.params || {};
  if (kind === 'reach_level') return 'reach ' + params.skill + ' ' + params.level;
  if (kind === 'obtain_items') return 'obtain ' + params.count + ' × ' + params.item;
  if (kind === 'complete_quest') return 'quest ' + params.quest;
  if (kind === 'equip_best') return 'equip ' + params.style;
  if (kind === 'access_area') return 'reach ' + params.area;
  if (kind === 'space_cleanup') return 'cleanup bank';
  if (kind === 'make_money') return 'make money';
  return kind;
}

function statusChip(status) {
  const safe = String(status || 'pending').replace(/[^a-z_]/g, '');
  return '<span class="chip st-' + safe + '">' + safe + '</span>';
}

function pick(account) {
  selected = account;
  try { history.replaceState(null, '', '#' + account); } catch (e) { /* file:// */ }
  renderIndex();
  loadDetail();
  if (BrainView) BrainView.select(account);
}

// Detail / brain tabs share the selected bot; the brain polls only
// while its tab is shown. The chosen tab is a per-viewer convenience.
function showTab(name) {
  const brain = name === 'brain';
  document.getElementById('detail').hidden = brain;
  document.getElementById('brainpane').hidden = !brain;
  document.getElementById('tab-detail').classList.toggle('cur', !brain);
  document.getElementById('tab-brain').classList.toggle('cur', brain);
  try { localStorage.setItem('life-tab', name); } catch (e) { /* storage off */ }
  if (BrainView) BrainView.setActive(brain);
}

function altText(alternatives) {
  if (!alternatives || !alternatives.length) return '<span class="muted">-</span>';
  return alternatives.map(alt =>
    '<div class="alt">' + alt.method + ' · ' + alt.est_ticks + 't' +
    (alt.est_coins ? ' · ' + alt.est_coins + 'c' : '') + '</div>').join('');
}

async function loadDetail() {
  if (selected === null) return;
  let life = null;
  try {
    const payload = await getJson('/api/life/' + encodeURIComponent(selected));
    if (payload.ok) life = payload.life;
  } catch (e) { /* keep the last frame */ }
  if (!life) return;
  document.getElementById('title').textContent = life.account +
    (life.active ? ' · on ' + life.active : ' · idle');
  document.getElementById('goal').textContent = 'goal: ' + goalText(life.goal);
  const profile = life.profile || {};
  document.getElementById('profile').textContent =
    (profile.ironman ? 'ironman · ' : '') + (profile.archetype || '') +
    ' · market affinity ' + Math.round((profile.market_affinity || 0) * 100) + '%' +
    ' · patience ' + Math.round((profile.price_patience || 0) * 100) + '%';
  const engaged = life.engagement;
  document.getElementById('engaged').textContent = engaged
    ? engaged.node + ': ' + engaged.min_ticks + '–' + engaged.max_ticks + ' ticks (drawn ' + engaged.chosen + ')'
    : 'no engagement (between tasks)';

  const head = '<tr><th class="left">id</th><th class="left">task</th><th class="left">status</th>' +
    '<th class="left">method + alternatives</th><th>progress</th><th>ticks</th></tr>';
  const rows = (life.nodes || []).map(node => {
    const progress = node.progress && node.progress.target
      ? node.progress.current + '/' + node.progress.target : '-';
    const method = node.method ? '<div>' + node.method + '</div>' : '<span class="muted">undecided</span>';
    return '<tr><td class="left">' + node.id + '</td><td class="left">' + kindText(node) + '</td>' +
      '<td class="left">' + statusChip(node.status) + '</td>' +
      '<td class="left">' + method + altText(node.alternatives) + '</td>' +
      '<td>' + progress + '</td><td>' + (node.spent_ticks || 0) + '</td></tr>';
  });
  document.getElementById('nodes').innerHTML = head + rows.join('');

  const hhead = '<tr><th class="left">node</th><th class="left">outcome</th><th class="left">ended</th></tr>';
  const hrows = (life.history || []).slice(-20).reverse().map(entry => {
    const date = new Date(entry.ended * 1000);
    const label = String(date.getDate()).padStart(2, '0') + '/' + String(date.getMonth() + 1).padStart(2, '0') +
      ' ' + String(date.getHours()).padStart(2, '0') + ':' + String(date.getMinutes()).padStart(2, '0');
    return '<tr><td class="left">' + entry.node + '</td><td class="left">' + entry.outcome +
      '</td><td class="left">' + label + '</td></tr>';
  });
  document.getElementById('history').innerHTML = hhead +
    (hrows.join('') || '<tr><td class="left muted" colspan="3">no history yet</td></tr>');
}

loadLives().then(() => {
  loadDetail();
  if (BrainView && selected !== null) BrainView.select(selected);
  let tab = 'detail';
  try { tab = localStorage.getItem('life-tab') || 'detail'; } catch (e) { tab = 'detail'; }
  showTab(tab === 'brain' ? 'brain' : 'detail');
});
setInterval(loadLives, 60000);
setInterval(loadDetail, 30000);
