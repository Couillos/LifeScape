// Item holdings front: live item list plus the held-count curve over
// 24h / 7 days (hourly points) or 30 days / 6 months (daily points)
// (§6.5, D42, D48) drawn by the shared chart.js (D49). Read-only
// dashboard page: bots never read it. Under the curve of a selected
// item, its sources and sinks by provenance over the same period
// (`/api/items/flows`, task report lines, staff out).
//
// Pure helpers (ItemFlows.*) touch no DOM: `node --test items.test.js`.
const ItemFlows = (() => {
  'use strict';

  // Readable provenance (SDK item ledger attribution): + entered a
  // pack, - left it.
  const LABELS = {
    produced: 'produit',
    loot: 'butin',
    ground_spawn: 'ramassé au sol',
    shop_bought: 'acheté au PNJ',
    shop_sold: 'vendu au PNJ',
    traded: 'échangé entre bots',
    consumed: 'consommé',
    alch_low: 'low alch',
    alch_high: 'high alch',
    lost: 'perdu',
    death: 'mort',
    fee: 'frais',
    unattributed: 'non attribué',
  };
  // Flow window per chart period, in hours (24h / 7d / 30d / 6M).
  const PERIOD_HOURS = { day: 24, week: 168, month: 720, half: 4380 };

  function esc(text) {
    return String(text === null || text === undefined ? '' : text)
      .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }

  // A trade between bots moves objects, it creates or removes none: its
  // in and out rows read as such (user 2026-11-18).
  function label(provenance, quantity) {
    if (provenance === 'traded' && Number(quantity) > 0) return 'acheté à un bot';
    if (provenance === 'traded' && Number(quantity) < 0) return 'vendu à un bot';
    return LABELS[provenance] || String(provenance || '?');
  }

  function hoursFor(period) {
    return PERIOD_HOURS[period] || 24;
  }

  function flowsUrl(itemId, period) {
    return '/api/items/flows?item=' + encodeURIComponent(itemId) + '&hours=' + hoursFor(period);
  }

  // Signed quantity: green when it entered packs, red when it left.
  function quantityCell(quantity) {
    const amount = Number(quantity) || 0;
    if (amount > 0) return '<span class="up">+' + amount + '</span>';
    if (amount < 0) return '<span class="down">−' + Math.abs(amount) + '</span>';
    return '<span class="muted">0</span>';
  }

  // Table body of one item's flows (payload of /api/items/flows).
  function tableHtml(payload) {
    const flows = (payload && payload.flows) || [];
    if (!flows.length) return '<tr><td class="left muted">aucun flux enregistré sur la période</td></tr>';
    let net = 0;
    let lines = 0;
    const rows = flows.map((flow) => {
      net += Number(flow.quantity) || 0;
      lines += Number(flow.lines) || 0;
      return '<tr><td class="left" title="' + esc(flow.provenance) + '">' + esc(label(flow.provenance, flow.quantity)) + '</td><td>'
        + quantityCell(flow.quantity) + '</td><td>' + esc(flow.lines) + '</td></tr>';
    });
    return '<tr><th class="left">Provenance / destination</th><th>Quantité</th><th>Rapports</th></tr>'
      + rows.join('')
      + '<tr class="net"><td class="left">net</td><td>' + quantityCell(net) + '</td><td>' + lines + '</td></tr>';
  }

  // No flows to show (route missing, error, network): the table stays
  // empty, no message.
  function unavailableHtml() {
    return '';
  }

  return { LABELS, PERIOD_HOURS, esc, label, hoursFor, flowsUrl, quantityCell, tableHtml, unavailableHtml };
})();

// Supply indices (user 2026-11-18): families of objects summed into one
// count and one curve (`indices` of /api/items, `index=` of the
// history route). Pure helpers, no DOM: `node --test items.test.js`.
const ItemIndices = (() => {
  'use strict';

  // Table body of the indices (payload `indices`), the current one bold.
  function tableHtml(indices, currentKey) {
    if (!indices || !indices.length) return '<tr><td class="left muted">no index</td></tr>';
    const rows = indices.map((index) => '<tr data-index="' + ItemFlows.esc(index.key) + '"'
      + (index.key === currentKey ? ' class="cur"' : '') + '><td class="left">' + ItemFlows.esc(index.label)
      + '</td><td>' + ItemFlows.esc(index.count) + '</td><td>' + changeCell(index.change24h) + '</td></tr>');
    return '<tr><th class="left">Index</th><th>Qty</th><th>24 h</th></tr>' + rows.join('');
  }

  // History URL of the curve shown: an item, else an index, else the total.
  function historyUrl(res, itemId, indexKey) {
    let url = '/api/items/history?res=' + res;
    if (itemId !== null && itemId !== undefined) url += '&item=' + encodeURIComponent(itemId);
    else if (indexKey) url += '&index=' + encodeURIComponent(indexKey);
    return url;
  }

  function find(indices, key) {
    return (indices || []).find((index) => index.key === key) || null;
  }

  // Items of the list kept by an index filter (its object ids).
  function keep(items, index) {
    if (!index) return items;
    const ids = new Set(index.ids || []);
    return items.filter((item) => ids.has(item.id));
  }

  return { tableHtml, historyUrl, find, keep };
})();

// Item icons (icons.js); absent (daemon without the icon route): none.
const Icons = typeof LifeIcons !== 'undefined' ? LifeIcons : { item: () => '', skill: () => '' };

const PAGE_SIZE = 25;
let list = null;
let selected = null;
// Supply index key: filters the item list and, with no item picked,
// draws the index curve.
let indexKey = null;
let pageIndex = 0;
let query = "";
// Chart periods: the API serves every retained point of a resolution
// (hourly 8 days, daily 400 days); the slots pick the window shown.
const PERIODS = {
  day: { res: '1h', count: 24 },
  week: { res: '1h', count: 168 },
  month: { res: '1d', count: 30 },
  half: { res: '1d', count: 183 },
};
let period = 'day';
let curve = null;
let flowsToken = 0;

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
    if (indexKey !== null && !ItemIndices.find(list.indices, indexKey)) indexKey = null;
    renderList();
  }
}

async function loadCurve() {
  const res = PERIODS[period].res;
  try {
    const payload = await getJson(ItemIndices.historyUrl(res, selected, indexKey));
    if (payload.ok) curve = payload;
  } catch (e) { /* daemon down: keep the last curve */ }
  draw();
}

// Flows of the selected item over the chart period; stale answers
// (selection or period changed meanwhile) are dropped.
async function loadFlows() {
  const section = document.getElementById('flowsbox');
  if (selected === null) {
    section.style.display = 'none';
    return;
  }
  section.style.display = '';
  const token = ++flowsToken;
  let html;
  try {
    const response = await fetch(ItemFlows.flowsUrl(selected, period));
    const payload = response.status === 404 ? null : await response.json();
    html = payload && payload.ok ? ItemFlows.tableHtml(payload) : ItemFlows.unavailableHtml(response.status, payload && payload.error);
  } catch (e) {
    html = ItemFlows.unavailableHtml(0, null); /* daemon down or not JSON */
  }
  if (token !== flowsToken) return;
  document.getElementById('flows').innerHTML = html;
}

function filtered() {
  if (!list) return [];
  const inIndex = ItemIndices.keep(list.items, ItemIndices.find(list.indices, indexKey));
  const needle = query.trim().toLowerCase();
  if (!needle) return inIndex;
  return inIndex.filter(item => item.name.toLowerCase().includes(needle));
}

function currentIndex() {
  return list ? ItemIndices.find(list.indices, indexKey) : null;
}

function title() {
  if (selected === null) {
    const index = currentIndex();
    return index ? index.label + ' (index)' : 'Total items';
  }
  const found = ((list && list.items) || []).find(item => item.id === selected);
  return found ? found.name : ('obj ' + selected);
}

function liveCount() {
  if (!list) return 0;
  if (selected === null) {
    const index = currentIndex();
    return index ? index.count : (list.liveTotal || 0);
  }
  const found = list.items.find(item => item.id === selected);
  return found ? found.count : 0;
}

// Item table (shared TableSort, table.js): raw values sort, fmt only
// changes the display. `change` is the 24 h change of the held count in
// thousandths (now vs the hourly point 24 h ago); null = no baseline.
const ITEM_COLUMNS = [
  { key: 'name', label: 'Item', kind: 'str', fmt: (value, row) => Icons.item(row.id, row.count) + ItemFlows.esc(value) },
  { key: 'count', label: 'Qty', kind: 'num' },
  { key: 'change', label: '24 h', kind: 'num', fmt: (value) => changeCell(value) },
];

function changeCell(promille) {
  if (promille === null || promille === undefined) return '<span class="muted">—</span>';
  const cls = promille > 0 ? 'up' : (promille < 0 ? 'down' : '');
  const sign = promille > 0 ? '+' : '';
  return '<span class="' + cls + '">' + sign + (promille / 10).toFixed(1) + '%</span>';
}

function itemRow(item) {
  return { id: item.id, name: item.name, count: item.count, change: item.change24h === undefined ? null : item.change24h };
}

function renderList() {
  const matches = TableSort.sortedRows('items', ITEM_COLUMNS, filtered().map(itemRow));
  const pages = Math.max(1, Math.ceil(matches.length / PAGE_SIZE));
  pageIndex = Math.min(Math.max(pageIndex, 0), pages - 1);
  const slice = matches.slice(pageIndex * PAGE_SIZE, pageIndex * PAGE_SIZE + PAGE_SIZE);
  document.getElementById('indices').innerHTML = ItemIndices.tableHtml(list.indices, indexKey);
  const total = document.getElementById('total');
  total.className = selected === null && indexKey === null ? 'cur' : '';
  total.innerHTML = 'Total <span class="qty">' + (list.liveTotal || 0) + '</span> ' + changeCell(list.totalChange24h);
  if (slice.length) {
    TableSort.render('items', ITEM_COLUMNS, slice, {
      rowAttrs: (row) => ({ 'data-id': row.id, class: selected === row.id ? 'cur' : '' }),
      onSort: () => { pageIndex = 0; renderList(); },
    });
  } else {
    document.getElementById('items').innerHTML = '<tr><td class="left muted">no match</td></tr>';
  }
  document.getElementById('pageinfo').textContent = `page ${pageIndex + 1}/${pages} · ${matches.length} item(s)`;
}

// One listener on the stable <table>: rows are rebuilt on every render.
if (typeof document !== 'undefined') {
  document.getElementById('items').addEventListener('click', (event) => {
    const row = event.target.closest('tr[data-id]');
    if (row) pick(Number(row.getAttribute('data-id')));
  });
  document.getElementById('indices').addEventListener('click', (event) => {
    const row = event.target.closest('tr[data-index]');
    if (row) pickIndex(row.getAttribute('data-index'));
  });
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

// Total row: no item, no index. An item keeps the index filter.
function pick(id) {
  selected = id;
  if (id === null) indexKey = null;
  renderList();
  loadCurve();
  loadFlows();
}

// An index row: its curve and its members in the list; again = off.
function pickIndex(key) {
  indexKey = indexKey === key ? null : key;
  selected = null;
  pageIndex = 0;
  renderList();
  loadCurve();
  loadFlows();
}

function setPeriod(next) {
  period = next;
  for (const key of Object.keys(PERIODS)) {
    document.getElementById('period-' + key).className = key === period ? 'cur' : '';
  }
  loadCurve();
  loadFlows();
}

// Title over the curve: the picked item's icon before its name.
function titleHtml() {
  return (selected === null ? '' : Icons.item(selected, liveCount())) + ItemFlows.esc(title());
}

function draw() {
  const canvas = document.getElementById('chart');
  const heading = document.getElementById('title');
  const headingHtml = titleHtml();
  if (heading._html !== headingHtml) { heading._html = headingHtml; heading.innerHTML = headingHtml; }
  const live = liveCount();
  const { res, count } = PERIODS[period];
  const width = res === '1h' ? 3600 : 86400;
  const now = Math.floor(Date.now() / 1000);
  const from = Math.floor(now / width) * width - (count - 1) * width;
  const slots = LifeChart.slotStarts(from, width, count);
  // Fixed slots (§6.1): sparse measures align, empty slots stay gaps.
  const aligned = LifeChart.fillSlots(slots, ((curve && curve.res === res) ? curve.points : []) || []);
  const rows = aligned.map(row => row ? row.v : null);
  LifeChart.draw(canvas, {
    slots, width, kind: 'line', rows,
    leftTitle: 'Nombre d’objets',
    legend: [{ swatch: 'line', color: LifeChart.UP, label: title() + ' — en direct : ' + live }],
    live: true,
    emptyText: 'No point yet (the sampler records hourly).',
  });
}

let resizeTimer = null;
if (typeof module !== 'undefined' && module.exports) {
  module.exports = Object.assign({}, ItemFlows, { ItemIndices });
} else {
  window.addEventListener('resize', () => {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => { if (curve || list) draw(); }, 150);
  });

  loadList().then(loadCurve).then(loadFlows);
  setInterval(loadList, 60000);
  setInterval(loadCurve, 60000);
  setInterval(loadFlows, 60000);
}
