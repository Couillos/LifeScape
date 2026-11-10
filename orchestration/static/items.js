// Item holdings front: live item list plus the 24h / 6-month curve
// (§6.5, D42, D48) drawn by the shared chart.js (D49). Read-only
// dashboard page: bots never read it.
const PAGE_SIZE = 25;
let list = null;
let selected = null;
let pageIndex = 0;
let query = "";
let res = "1h";
let curve = null;

async function getJson(url) {
  const response = await fetch(url);
  return response.json();
}

async function loadList() {
  try {
    const payload = await getJson('/api/items');
    if (payload.items) list = payload;
  } catch (e) { /* daemon down: keep the last list */ }
  if (list) {
    if (selected !== null && !list.items.some(item => item.id === selected)) selected = null;
    renderList();
  }
}

async function loadCurve() {
  const key = selected === null ? '' : '&item=' + encodeURIComponent(selected);
  try {
    const payload = await getJson('/api/items/history?res=' + res + key);
    if (payload.ok) curve = payload;
  } catch (e) { /* daemon down: keep the last curve */ }
  draw();
}

function filtered() {
  if (!list) return [];
  const needle = query.trim().toLowerCase();
  if (!needle) return list.items;
  return list.items.filter(item => item.name.toLowerCase().includes(needle));
}

function title() {
  if (selected === null) return 'Total items';
  const found = (list.items || []).find(item => item.id === selected);
  return found ? found.name : ('obj ' + selected);
}

function liveCount() {
  if (!list) return 0;
  if (selected === null) return list.liveTotal || 0;
  const found = list.items.find(item => item.id === selected);
  return found ? found.count : 0;
}

function renderList() {
  const matches = filtered();
  const pages = Math.max(1, Math.ceil((matches.length + 1) / PAGE_SIZE));
  pageIndex = Math.min(Math.max(pageIndex, 0), pages - 1);
  const start = pageIndex * PAGE_SIZE;
  const slice = matches.slice(start, start + PAGE_SIZE);

  const rows = [];
  if (selected === null) rows.push(`<li class="cur" onclick="pick(null)">Total <span class="qty">(${(list.liveTotal || 0)})</span></li>`);
  else rows.push(`<li onclick="pick(null)">Total <span class="qty">(${(list.liveTotal || 0)})</span></li>`);
  for (const item of slice) {
    rows.push(`<li class="${selected === item.id ? 'cur' : ''}" data-id="${item.id}" onclick="pick(${item.id})">${item.name} <span class="qty">(${item.count})</span></li>`);
  }
  document.getElementById('items').innerHTML = rows.join('') || '<li class="muted">no match</li>';
  document.getElementById('pageinfo').textContent = `page ${pageIndex + 1}/${pages} · ${matches.length} item(s)`;
}

function page(delta) {
  pageIndex += delta;
  renderList();
}

function onSearch() {
  query = document.getElementById('search').value;
  pageIndex = 0;
  renderList();
  const match = (list.items || []).find(item => item.name.toLowerCase() === query.trim().toLowerCase());
  if (match) pick(match.id);
}

function pick(id) {
  selected = id;
  renderList();
  loadCurve();
}

function setRes(next) {
  res = next;
  document.getElementById('res1h').className = res === '1h' ? 'cur' : '';
  document.getElementById('res1d').className = res === '1d' ? 'cur' : '';
  loadCurve();
}

function draw() {
  const canvas = document.getElementById('chart');
  document.getElementById('title').textContent = title();
  const live = liveCount();
  const width = res === '1h' ? 3600 : 86400;
  const count = res === '1h' ? 24 : 183;
  const now = Math.floor(Date.now() / 1000);
  const from = Math.floor(now / width) * width - (count - 1) * width;
  const slots = LifeChart.slotStarts(from, width, count);
  // Fixed slots (§6.1): sparse measures align, empty slots stay gaps.
  const aligned = LifeChart.fillSlots(slots, ((curve && curve.res === res) ? curve.points : []) || []);
  const rows = aligned.map(row => row ? row.v : null);
  LifeChart.draw(canvas, {
    slots, width, kind: 'line', rows,
    leftTitle: 'Nombre d’objets',
    legend: [{ swatch: 'line', color: '#4caf50', label: title() + ' — en direct : ' + live }],
    live: true,
    emptyText: 'No point yet (the sampler records hourly).',
  });
  document.getElementById('legend').textContent =
    res === '1h' ? 'one point per hour, last 24h' : 'last measure of each day, last 6 months';
}

let resizeTimer = null;
window.addEventListener('resize', () => {
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(() => { if (curve || list) draw(); }, 150);
});

loadList().then(loadCurve);
setInterval(loadList, 60000);
setInterval(loadCurve, 60000);
