// Rates front (plan rendements R2): per key (planner method, style,
// site, tool tier, band) the signed flows (products and consumption),
// the XP-bearing units, the XP derived from them, the confidence (n,
// age, dispersion, failures, rejected samples, prior or measured),
// then profit and trading tables per 6000 ticks,
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

function keyLabel(value) {
  return value === '*' ? '<span class="muted">all</span>' : (value || '-');
}

function bandText(band) {
  return band > 0 ? bandLabel(band) : '-';
}

function ageText(secs) {
  if (!isFinite(secs) || secs <= 0) return '-';
  if (secs < 3600) return Math.round(secs / 60) + ' min';
  return Math.round(secs / 360) / 10 + ' h';
}

const KEY_COLS = [
  { key: 'method', label: 'method', kind: 'str' },
  { key: 'style', label: 'style', kind: 'str' },
  { key: 'site', label: 'site', kind: 'str', fmt: (v) => keyLabel(v) },
  { key: 'tool', label: 'tool', kind: 'str', fmt: (v) => keyLabel(v) },
  { key: 'band', label: 'band', kind: 'num', fmt: (v) => bandText(v) },
];
const CONFIDENCE_COLS = [
  { key: 'runs', label: 'n', kind: 'num' },
  { key: 'prior', label: 'src', kind: 'str', fmt: (v) => srcHtml(v) },
  { key: 'read', label: 'read', kind: 'str' },
  { key: 'age_secs', label: 'age', kind: 'num', fmt: (v) => ageText(v) },
  { key: 'dispersion', label: 'disp.', kind: 'num', fmt: (v) => number(v) },
  { key: 'failure_rate', label: 'fail', kind: 'num', fmt: (v) => (isFinite(v) ? Math.round(v * 100) + '%' : '-') },
  { key: 'rejected', label: 'rejected', kind: 'num' },
];
const YIELD_COLS = KEY_COLS.concat([
  { key: 'item', label: 'item', kind: 'str', fmt: (v, row) => (row.error ? '<span class="prior">' + row.error + '</span>' : v) },
  { key: 'cause', label: 'cause', kind: 'str' },
  { key: 'per_6000', label: '/6000', kind: 'num', fmt: (v) => number(v) },
]).concat(CONFIDENCE_COLS);
const UNIT_COLS = KEY_COLS.concat([
  { key: 'kind', label: 'unit', kind: 'str' },
  { key: 'subject', label: 'subject', kind: 'str' },
  { key: 'per_6000', label: '/6000', kind: 'num', fmt: (v) => number(v) },
  { key: 'runs', label: 'n', kind: 'num' },
  { key: 'prior', label: 'src', kind: 'str', fmt: (v) => srcHtml(v) },
]);
const XP_COLS = KEY_COLS.concat([
  { key: 'skill', label: 'skill', kind: 'str' },
  { key: 'per_6000', label: 'XP/6000', kind: 'num', fmt: (v) => number(v) },
  { key: 'runs', label: 'n', kind: 'num' },
  { key: 'prior', label: 'src', kind: 'str', fmt: (v) => srcHtml(v) },
]);
const PROFIT_COLS = [
  { key: 'method', label: 'method', kind: 'str' },
  { key: 'style', label: 'style', kind: 'str' },
  { key: 'band', label: 'band', kind: 'num', fmt: (v) => bandText(v) },
  { key: 'revenue', label: 'revenue', kind: 'num', fmt: (v) => number(v) },
  { key: 'input_cost', label: 'inputs', kind: 'num', fmt: (v) => number(v) },
  { key: 'friction', label: 'friction', kind: 'num', fmt: (v) => number(v) },
  { key: 'trend_bonus', label: 'trend bonus', kind: 'num', fmt: (v) => number(v) },
  { key: 'profit', label: 'profit', kind: 'num', fmt: (v) => number(v) },
  { key: 'price_prior', label: 'price', kind: 'str', fmt: (v) => priceHtml(v) },
  { key: 'prior', label: 'yields', kind: 'str', fmt: (v) => srcHtml(v) },
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
  return [row.method, row.style, row.site, row.tool, row.item, row.skill, row.subject].filter(Boolean).join(" ").toLowerCase().includes(needle);
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
  const units = (payload.units || []).filter(matches);
  const xp = (payload.xp || []).filter(matches);
  const profit = (payload.profit || []).filter(matches);
  const trading = payload.trading || [];
  document.getElementById('summary').textContent =
    yields.length + ' flow row(s) · ' + units.length + ' unit row(s) · ' + xp.length + ' xp row(s) · ' + profit.length + ' profit row(s) · ' +
    trading.length + ' trading row(s)';

  renderTable('yields', YIELD_COLS, yields);
  renderTable('units', UNIT_COLS, units);
  renderTable('xp', XP_COLS, xp);
  renderTable('profit', PROFIT_COLS, profit);
  renderTable('trading', TRADING_COLS, trading);

  const engaged = payload.engaged || {};
  const engagedRows = Object.keys(engaged).sort().map((name) => ({ script: name, bots: engaged[name] }));
  renderTable('engaged', ENGAGED_COLS, engagedRows);
}

load();
setInterval(load, 30000);
