// Life front: one bot's goal, task graph, engagement and history.
// Read-only dashboard page: bots never read it.
let lives = [];
let selected = null;
let query = "";

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
  renderList();
}

function filtered() {
  const needle = query.trim().toLowerCase();
  if (!needle) return lives;
  return lives.filter(row => row.account.toLowerCase().includes(needle));
}

function nodeSummary(row) {
  const counts = row.nodes || {};
  const order = ['active', 'ready', 'pending', 'blocked', 'failed', 'done', 'abandoned'];
  return order.filter(key => counts[key]).map(key => key + ':' + counts[key]).join(' ');
}

function renderList() {
  const rows = filtered().map(row =>
    '<li class="' + (selected === row.account ? 'cur' : '') + '" data-id="' + row.account + '"' +
    ' onclick="pick(\'' + row.account + '\')">' + row.account +
    ' <span class="muted">' + goalText(row.goal) + ' · ' + nodeSummary(row) + '</span></li>');
  document.getElementById('bots').innerHTML = rows.join('') || '<li class="muted">no life file</li>';
}

function onSearch() {
  query = document.getElementById('search').value;
  renderList();
}

function pick(account) {
  selected = account;
  try { history.replaceState(null, '', '#' + account); } catch (e) { /* file:// */ }
  renderList();
  loadDetail();
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

loadLives().then(loadDetail);
setInterval(loadLives, 60000);
setInterval(loadDetail, 30000);
