// Rates front: yield, XP, profit and trading tables per 6000 ticks,
// refreshed every 30 s. Read-only dashboard page: bots never read it.
// Sorting rides the shared TableSort helper (table.js, D46); when the
// table.js route is not served yet, tables render unsorted instead of
// blank.
let payload = null;
let filter = "";
const sorter = (typeof TableSort !== 'undefined') ? TableSort : null;

function bandLabel(band) {
  return band + "-" + (band + 4);
}

function number(value) {
  if (!isFinite(value)) return "-";
  return Math.round(value * 10) / 10;
}

function srcHtml(prior) {
  return prior ? '<span class="prior">prior</span>' : 'measured';
}

function priceHtml(prior) {
  return prior ? '<span class="prior">prior</span>' : 'market';
}

const YIELD_COLS = [
  { key: 'method', label: 'method', kind: 'str' },
  { key: 'item', label: 'item', kind: 'str' },
  { key: 'band', label: 'band', kind: 'num', fmt: (v) => bandLabel(v) },
  { key: 'count', label: 'n', kind: 'num' },
  { key: 'mean', label: 'mean/6000', kind: 'num', fmt: (v) => number(v) },
  { key: 'stddev', label: 'stddev', kind: 'num', fmt: (v) => number(v) },
  { key: 'ema', label: 'ema', kind: 'num', fmt: (v) => number(v) },
  { key: 'prior', label: 'src', kind: 'str', fmt: (v) => srcHtml(v) },
];
const XP_COLS = [
  { key: 'method', label: 'method', kind: 'str' },
  { key: 'skill', label: 'skill', kind: 'str' },
  { key: 'band', label: 'band', kind: 'num', fmt: (v) => bandLabel(v) },
  { key: 'count', label: 'n', kind: 'num' },
  { key: 'mean', label: 'mean/6000', kind: 'num', fmt: (v) => number(v) },
  { key: 'stddev', label: 'stddev', kind: 'num', fmt: (v) => number(v) },
  { key: 'ema', label: 'ema', kind: 'num', fmt: (v) => number(v) },
  { key: 'prior', label: 'src', kind: 'str', fmt: (v) => srcHtml(v) },
];
const PROFIT_COLS = [
  { key: 'method', label: 'method', kind: 'str' },
  { key: 'band', label: 'band', kind: 'num', fmt: (v) => bandLabel(v) },
  { key: 'revenue', label: 'revenue', kind: 'num', fmt: (v) => number(v) },
  { key: 'input_cost', label: 'inputs', kind: 'num', fmt: (v) => number(v) },
  { key: 'friction', label: 'friction', kind: 'num', fmt: (v) => number(v) },
  { key: 'trend_bonus', label: 'trend bonus', kind: 'num', fmt: (v) => number(v) },
  { key: 'profit', label: 'profit', kind: 'num', fmt: (v) => number(v) },
  { key: 'price_prior', label: 'price', kind: 'str', fmt: (v) => priceHtml(v) },
];
const TRADING_COLS = [
  { key: 'item', label: 'item', kind: 'str' },
  { key: 'profit_per_6000', label: 'profit/6000', kind: 'num', fmt: (v) => number(v) },
  { key: 'deals_settled', label: 'settled', kind: 'num' },
  { key: 'fill_rate', label: 'fill', kind: 'num', fmt: (v) => (isFinite(v) ? Math.round(v * 100) + '%' : '-') },
  { key: 'stock_value', label: 'stock', kind: 'num', fmt: (v) => number(v) },
];
const ENGAGED_COLS = [
  { key: 'script', label: 'script', kind: 'str' },
  { key: 'bots', label: 'bots', kind: 'num' },
];

async function load() {
  try {
    const response = await fetch('/api/rates/table');
    const body = await response.json();
    if (body.ok) payload = body;
  } catch (e) { /* daemon down: keep the last frame */ }
  render();
}

function matches(row) {
  const needle = filter.trim().toLowerCase();
  if (!needle) return true;
  return (row.method + " " + (row.item || row.skill || "")).toLowerCase().includes(needle);
}

function onSearch() {
  filter = document.getElementById('search').value;
  render();
}

// Plain unsorted fallback if the shared table.js fails to load:
// same cells, no arrows, never blank.
function plainRender(id, columns, rows) {
  const table = document.getElementById(id);
  if (!table) return;
  let html = '<tr>' + columns.map((c) => '<th class="left">' + c.label + '</th>').join('') + '</tr>';
  for (const row of rows) {
    html += '<tr>' + columns.map((c) => {
      const raw = row[c.key];
      const cell = c.fmt ? c.fmt(raw, row) : (raw === null || raw === undefined || raw === '' ? '-' : raw);
      return '<td class="left">' + cell + '</td>';
    }).join('') + '</tr>';
  }
  table.innerHTML = rows.length ? html : html + '<tr><td class="left muted" colspan="' + columns.length + '">-</td></tr>';
}

function renderTable(id, columns, rows) {
  if (sorter) sorter.render(id, columns, rows);
  else plainRender(id, columns, rows);
}

function render() {
  if (!payload) return;
  const yields = (payload.rates || []).filter(matches);
  const xp = (payload.xp || []).filter(matches);
  const profit = (payload.profit || []).filter(matches);
  const trading = payload.trading || [];
  document.getElementById('summary').textContent =
    yields.length + ' yield row(s) · ' + xp.length + ' xp row(s) · ' + profit.length + ' profit row(s) · ' +
    trading.length + ' trading row(s)';

  renderTable('yields', YIELD_COLS, yields);
  renderTable('xp', XP_COLS, xp);
  renderTable('profit', PROFIT_COLS, profit);
  renderTable('trading', TRADING_COLS, trading);

  const engaged = payload.engaged || {};
  const engagedRows = Object.keys(engaged).sort().map((name) => ({ script: name, bots: engaged[name] }));
  renderTable('engaged', ENGAGED_COLS, engagedRows);
}

load();
setInterval(load, 30000);
