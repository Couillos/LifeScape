// Errors front: every failure the daemon saw (task errors, nav dossiers,
// failed deals, brain abandons), KPIs, hourly trend, groups by
// signature, drill-down with coordinates and links. Read-only page,
// refreshed every 30 s. Filters live in the URL hash (shareable links).
//
// Pure helpers (ErrorsPage.*) touch no DOM: `node errors.test.js`.
const ErrorsPage = (() => {
  'use strict';

  const SOURCES = ['task', 'nav', 'deal', 'brain'];
  const FILTER_KEYS = ['source', 'kind', 'account', 'script', 'zone', 'signature'];
  const HOURS = [1, 6, 24, 72];

  function esc(text) {
    return String(text === null || text === undefined ? '' : text)
      .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }

  // Query string for the API: window + non-empty filters + extras.
  function queryString(state, extra) {
    const parts = ['hours=' + encodeURIComponent(state.hours || 24)];
    for (const key of FILTER_KEYS) {
      if (state[key]) parts.push(key + '=' + encodeURIComponent(state[key]));
    }
    for (const key in (extra || {})) parts.push(key + '=' + encodeURIComponent(extra[key]));
    return parts.join('&');
  }

  // State from a `#hours=24&source=nav` hash (unknown keys ignored,
  // unknown window back to 24 h).
  function parseHash(hash) {
    const state = { hours: 24 };
    for (const key of FILTER_KEYS) state[key] = '';
    for (const pair of String(hash || '').replace(/^#/, '').split('&')) {
      if (!pair) continue;
      const cut = pair.indexOf('=');
      const key = cut < 0 ? pair : pair.slice(0, cut);
      let value = cut < 0 ? '' : pair.slice(cut + 1);
      try { value = decodeURIComponent(value); } catch (e) { /* keep raw */ }
      if (key === 'hours') {
        const hours = parseInt(value, 10);
        state.hours = HOURS.indexOf(hours) >= 0 ? hours : 24;
      } else if (FILTER_KEYS.indexOf(key) >= 0) {
        state[key] = value;
      }
    }
    return state;
  }

  function pct(rate) {
    if (rate === null || rate === undefined || !isFinite(rate)) return '-';
    return (Math.round(rate * 1000) / 10).toFixed(1) + '%';
  }

  // Trend against the previous window: signed percent, 'n/a' without one.
  function trendText(trend) {
    if (trend === null || trend === undefined || !isFinite(trend)) return 'n/a';
    return (trend > 0 ? '+' : '') + trend + '%';
  }

  function posText(pos) {
    if (!pos || pos.length !== 3) return '-';
    return pos[0] + ', ' + pos[1] + ', ' + pos[2];
  }

  function spectateHref(account) {
    return '/#' + encodeURIComponent(account);
  }

  function lifeHref(account) {
    return '/life#' + encodeURIComponent(account);
  }

  function dossierHref(name) {
    return '/api/errors/dossier/' + encodeURIComponent(name);
  }

  // `fishing n4 · gather:fishing/shrimp` (method only when it says more).
  function taskText(row) {
    let text = row.script || '';
    if (row.node_id) text += (text ? ' ' : '') + row.node_id;
    if (row.method && row.method !== row.script) text += (text ? ' · ' : '') + row.method;
    return text;
  }

  // Hourly buckets to chart slots + values for one series ('total' or
  // a source). Zero stays zero: an hour without errors is real.
  function chartSeries(hourly, series) {
    const buckets = hourly || [];
    return {
      slots: buckets.map((bucket) => bucket.t),
      rows: buckets.map((bucket) => (bucket[series] === undefined ? 0 : bucket[series])),
    };
  }

  // `just now`, `12 min ago`, `3 h ago`, `2 d ago`.
  function ago(clock, now) {
    const span = Math.max(0, now - clock);
    if (span < 60) return 'just now';
    if (span < 3600) return Math.floor(span / 60) + ' min ago';
    if (span < 86400) return Math.floor(span / 3600) + ' h ago';
    return Math.floor(span / 86400) + ' d ago';
  }

  return { SOURCES, FILTER_KEYS, esc, queryString, parseHash, pct, trendText, posText, spectateHref, lifeHref, dossierHref, taskText, chartSeries, ago };
})();

if (typeof module !== 'undefined' && module.exports) module.exports = ErrorsPage;

if (typeof document !== 'undefined') {
  const page = ErrorsPage;
  const esc = page.esc;
  const sorter = (typeof TableSort !== 'undefined') ? TableSort : null;
  let state = page.parseHash(location.hash);
  let summary = null;
  let listed = null;
  let series = 'total';

  const SELECTS = [
    { key: 'source', label: 'all sources', options: 'sources' },
    { key: 'kind', label: 'all types', options: 'kinds' },
    { key: 'account', label: 'all accounts', options: 'accounts' },
    { key: 'script', label: 'all tasks', options: 'scripts' },
    { key: 'zone', label: 'all zones', options: 'zones' },
  ];

  const chip = (source) => '<span class="chip src-' + esc(source) + '">' + esc(source) + '</span>';
  const now = () => Math.floor(Date.now() / 1000);
  const localTime = (clock) => {
    const date = new Date(clock * 1000);
    return String(date.getHours()).padStart(2, '0') + ':' + String(date.getMinutes()).padStart(2, '0');
  };

  function render(id, columns, rows, options) {
    if (sorter) {
      sorter.render(id, columns, rows, options);
      return;
    }
    const head = '<tr>' + columns.map((c) => '<th class="' + (c.kind === 'num' ? '' : 'left') + '">' + esc(c.label) + '</th>').join('') + '</tr>';
    document.getElementById(id).innerHTML = head + rows.map((row) => '<tr>' + columns.map((c) =>
      '<td class="' + (c.kind === 'num' ? '' : 'left') + '">' + (c.fmt ? c.fmt(row[c.key], row) : esc(row[c.key])) + '</td>').join('') + '</tr>').join('');
  }

  function writeHash() {
    const hash = page.queryString(state);
    if (location.hash.replace(/^#/, '') !== hash) history.replaceState(null, '', '#' + hash);
  }

  function setFilter(changes) {
    Object.assign(state, changes);
    writeHash();
    reload();
  }

  async function getJson(url) {
    const response = await fetch(url);
    return response.json();
  }

  async function loadSummary() {
    try {
      const body = await getJson('/api/errors/summary?' + page.queryString(state));
      if (body.ok) {
        summary = body;
        document.getElementById('status').textContent = 'updated ' + localTime(now());
      } else {
        document.getElementById('status').textContent = 'error: ' + body.error;
      }
    } catch (e) {
      document.getElementById('status').textContent = 'daemon unreachable, last frame kept';
    }
    renderSummary();
  }

  async function loadList() {
    try {
      const body = await getJson('/api/errors/list?' + page.queryString(state, { limit: 300 }));
      if (body.ok) listed = body;
    } catch (e) { /* keep the last list */ }
    renderList();
  }

  function reload() {
    loadSummary();
    loadList();
  }

  function renderSelects() {
    const options = (summary && summary.options) || {};
    for (const spec of SELECTS) {
      const select = document.getElementById('f-' + spec.key);
      const values = (options[spec.options] || []).slice();
      if (state[spec.key] && values.indexOf(state[spec.key]) < 0) values.unshift(state[spec.key]);
      select.innerHTML = '<option value="">' + esc(spec.label) + '</option>' +
        values.map((value) => '<option' + (value === state[spec.key] ? ' selected' : '') + ' value="' + esc(value) + '">' + esc(value) + '</option>').join('');
    }
    document.getElementById('f-hours').value = String(state.hours);
    const sig = document.getElementById('sigchip');
    sig.style.display = state.signature ? 'inline-block' : 'none';
    sig.textContent = state.signature ? 'signature: ' + state.signature + '  ×' : '';
    sig.title = state.signature ? state.signature + ' (click to clear)' : '';
  }

  function tile(value, label, cls) {
    return '<div class="tile"><div class="value' + (cls ? ' ' + cls : '') + '">' + esc(value) + '</div><div class="label">' + esc(label) + '</div></div>';
  }

  function renderTiles() {
    const kpis = (summary && summary.kpis) || {};
    const trend = kpis.trend_pct;
    const trendClass = trend > 0 ? 'up' : (trend < 0 ? 'down' : '');
    let html = tile(kpis.total || 0, 'errors (' + (summary ? summary.hours : state.hours) + ' h)') +
      tile(kpis.per_hour === undefined ? '-' : kpis.per_hour, 'errors / hour') +
      tile(page.trendText(trend), 'vs previous window (' + (kpis.previous_total || 0) + ')', trendClass) +
      tile(kpis.accounts || 0, 'accounts hit') +
      tile(kpis.signatures || 0, 'distinct signatures') +
      tile(page.pct(kpis.run_failure_rate), 'task failure rate (' + (kpis.runs_failed || 0) + '/' + (kpis.runs || 0) + ' runs)') +
      tile(page.pct(kpis.deal_failure_rate), 'deal failure rate (' + (kpis.deals_failed || 0) + '/' + ((kpis.deals_failed || 0) + (kpis.deals_settled || 0)) + ')');
    for (const source of page.SOURCES) html += tile(kpis[source] || 0, source + ' errors');
    document.getElementById('tiles').innerHTML = html;
  }

  function renderChart() {
    const box = document.getElementById('series');
    box.innerHTML = ['total'].concat(page.SOURCES).map((name) =>
      '<button data-series="' + name + '" class="' + (name === series ? 'cur' : '') + '">' + name + '</button>').join(' ');
    if (typeof LifeChart === 'undefined') return;
    const hourly = (summary && summary.hourly) || [];
    const data = page.chartSeries(hourly, series);
    const bySlot = new Map(hourly.map((bucket) => [bucket.t, bucket]));
    LifeChart.draw(document.getElementById('chart'), {
      slots: data.slots,
      width: 3600,
      kind: 'line',
      rows: data.rows,
      leftTitle: 'errors / hour',
      legend: [{ swatch: 'line', color: '#4caf50', label: series === 'total' ? 'all sources' : series }],
      live: true,
      emptyText: 'No error in this window.',
      tooltipFor: (slot, value) => {
        const bucket = bySlot.get(slot) || {};
        return [LifeChart.periodLabel(slot, 3600), series + ' ' + value]
          .concat(page.SOURCES.map((source) => source + ' ' + (bucket[source] || 0)));
      },
    });
  }

  const GROUP_COLS = [
    { key: 'count', label: 'count', kind: 'num' },
    { key: 'share', label: 'share', kind: 'num', fmt: (v) => page.pct(v) },
    { key: 'source', label: 'source', kind: 'str', fmt: (v) => chip(v) },
    { key: 'kind', label: 'type', kind: 'str' },
    { key: 'signature', label: 'signature', kind: 'str', fmt: (v) => '<span class="sig" title="' + esc(v) + '">' + esc(v) + '</span>' },
    { key: 'accounts', label: 'accounts', kind: 'num', fmt: (v, row) => '<span title="' + esc((row.top_accounts || []).join(', ')) + '">' + esc(v) + '</span>' },
    { key: 'top_zone', label: 'top zone', kind: 'str' },
    { key: 'top_script', label: 'top task', kind: 'str' },
    { key: 'last_seen', label: 'last seen', kind: 'num', fmt: (v) => esc(page.ago(v, now())) },
  ];
  const KIND_COLS = [
    { key: 'key', label: 'source/type', kind: 'str' },
    { key: 'count', label: 'count', kind: 'num' },
  ];
  const KEY_COLS = (label) => [
    { key: 'key', label: label, kind: 'str' },
    { key: 'count', label: 'errors', kind: 'num' },
  ];
  const SCRIPT_COLS = [
    { key: 'script', label: 'task', kind: 'str' },
    { key: 'runs', label: 'runs', kind: 'num' },
    { key: 'failed', label: 'failed', kind: 'num' },
    { key: 'blocked', label: 'blocked', kind: 'num' },
    { key: 'failure_rate', label: 'fail %', kind: 'num', fmt: (v) => page.pct(v) },
    { key: 'errors', label: 'errors', kind: 'num' },
  ];
  const ROW_COLS = [
    { key: 'time', label: 'time (UTC)', kind: 'str' },
    { key: 'account', label: 'account', kind: 'str', fmt: (v, row) => v
      ? esc(v) + ' <a href="' + page.spectateHref(v) + '" title="spectate this bot">watch</a> <a href="' + page.lifeHref(v) + '" title="life">life</a>' +
        (row.peer ? ' <span class="muted">↔ ' + esc(row.peer) + '</span>' : '')
      : '-' },
    { key: 'source', label: 'source', kind: 'str', fmt: (v) => chip(v) },
    { key: 'kind', label: 'type', kind: 'str' },
    { key: 'task', label: 'task', kind: 'str', fmt: (v, row) => '<span title="' + esc(row.args || '') + '">' + esc(v || '-') + '</span>' },
    { key: 'message', label: 'message', kind: 'str', fmt: (v, row) => '<span class="msg" title="' + esc(v + '\n\nsignature: ' + row.signature) + '">' + esc(v) + '</span>' },
    { key: 'pos_text', label: 'x, z, level', kind: 'str' },
    { key: 'zone', label: 'zone', kind: 'str' },
    { key: 'detail', label: 'detail', kind: 'str' },
    { key: 'dossier', label: 'dossier', kind: 'str', fmt: (v) => v ? '<a href="' + page.dossierHref(v) + '" target="_blank" rel="noopener">json</a>' : '-' },
  ];

  function renderSummary() {
    renderSelects();
    renderTiles();
    renderChart();
    if (!summary) return;
    render('groups', GROUP_COLS, summary.groups || [], {
      rowAttrs: (row) => ({ 'data-pick': 'group', 'data-index': (summary.groups || []).indexOf(row),
        class: row.signature === state.signature && row.kind === state.kind ? 'cur' : '' }),
    });
    render('kinds', KIND_COLS, summary.by_kind || [], { rowAttrs: (row) => ({ 'data-pick': 'kind', 'data-key': row.key }) });
    render('scripts', SCRIPT_COLS, summary.scripts || [], { rowAttrs: (row) => ({ 'data-pick': 'script', 'data-key': row.script }) });
    render('zones', KEY_COLS('zone'), summary.by_zone || [], { rowAttrs: (row) => ({ 'data-pick': 'zone', 'data-key': row.key }) });
    render('accounts', KEY_COLS('account'), summary.by_account || [], { rowAttrs: (row) => ({ 'data-pick': 'account', 'data-key': row.key }) });
  }

  function renderList() {
    const rows = ((listed && listed.rows) || []).map((row) => Object.assign({}, row, { task: page.taskText(row), pos_text: page.posText(row.pos) }));
    render('rows', ROW_COLS, rows);
    document.getElementById('listinfo').textContent = listed
      ? '(newest first, ' + rows.length + ' of ' + listed.total + (state.signature ? ', one signature' : '') + ')'
      : '';
  }

  // Row picks: a group narrows to its signature, a breakdown row to its key.
  for (const id of ['groups', 'kinds', 'scripts', 'zones', 'accounts']) {
    document.getElementById(id).addEventListener('click', (event) => {
      if (event.target.closest('a') || event.target.closest('th')) return;
      const row = event.target.closest('tr[data-pick]');
      if (!row || !summary) return;
      const pick = row.getAttribute('data-pick');
      const key = row.getAttribute('data-key') || '';
      if (pick === 'group') {
        const group = (summary.groups || [])[parseInt(row.getAttribute('data-index'), 10)];
        if (group) setFilter({ source: group.source, kind: group.kind, signature: group.signature });
      } else if (pick === 'kind') {
        const cut = key.indexOf('/');
        setFilter({ source: key.slice(0, cut), kind: key.slice(cut + 1) });
      } else {
        setFilter({ [pick]: key });
      }
    });
  }
  for (const spec of SELECTS) {
    document.getElementById('f-' + spec.key).addEventListener('change', (event) => setFilter({ [spec.key]: event.target.value }));
  }
  document.getElementById('f-hours').addEventListener('change', (event) => setFilter({ hours: parseInt(event.target.value, 10) || 24 }));
  document.getElementById('sigchip').addEventListener('click', () => setFilter({ signature: '' }));
  document.getElementById('clear').addEventListener('click', () => {
    const cleared = {};
    for (const key of page.FILTER_KEYS) cleared[key] = '';
    setFilter(cleared);
  });
  document.getElementById('series').addEventListener('click', (event) => {
    const button = event.target.closest('button[data-series]');
    if (!button) return;
    series = button.getAttribute('data-series');
    renderChart();
  });
  window.addEventListener('resize', () => renderChart());

  writeHash();
  reload();
  setInterval(reload, 30000);
}
