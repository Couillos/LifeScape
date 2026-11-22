// Page « XP » (2026-11-22). Le daemon calcule un instantané toutes
// les 30 min (comme les hiscores, `xp_service`); cette page lit seulement
// `/api/xp` (octets en cache). Tous les taux sont des rapports de sommes
// (somme des XP / somme des ticks de travail), jamais des moyennes de taux.
//
// Helpers purs (XpPage.*) sans DOM : `node xp.test.js`.
const XpPage = (() => {
  'use strict';

  const MODES = [
    {
      key: 'fleet_rate',
      label: 'Contribution à la flotte',
    },
    {
      key: 'efficiency',
      label: 'Efficacité de la compétence',
    },
  ];
  const RATE_LABEL = 'XP / 6000 ticks';
  const PERIOD_LABELS = { '24h': '24 h', '7d': '7 jours', all: 'Tout l’historique' };
  const COLORS = {
    attack: '#e6194b', strength: '#f58231', defence: '#9a6324', hitpoints: '#fabed4', ranged: '#3cb44b',
    prayer: '#ffe119', magic: '#4363d8', cooking: '#911eb4', woodcutting: '#aaffc3', firemaking: '#ff7f50',
    fishing: '#42d4f4', crafting: '#f032e6', smithing: '#bfbfbf', mining: '#808000', runecraft: '#00ffcc',
  };

  function esc(text) {
    return String(text === null || text === undefined ? '' : text)
      .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }

  function formatNumber(value) {
    if (value === null || value === undefined || !isFinite(value)) return '-';
    const digits = String(Math.floor(Math.abs(value)));
    let out = '';
    for (let index = 0; index < digits.length; index++) {
      if (index > 0 && (digits.length - index) % 3 === 0) out += ' ';
      out += digits[index];
    }
    return (value < 0 ? '-' : '') + out;
  }

  // Rates are per 6000 ticks: 9312.4 -> "9 312", 42.37 -> "42,4"; null -> "-".
  function formatRate(value) {
    if (value === null || value === undefined || !isFinite(value)) return '-';
    if (Math.abs(value) >= 100) return formatNumber(Math.round(value));
    return value.toFixed(1).replace('.', ',');
  }

  // Axis labels stay narrow: 4000000 -> "4 M", 250000 -> "250 k".
  function formatAxis(value) {
    if (value === null || value === undefined || !isFinite(value)) return '-';
    const size = Math.abs(value);
    if (size >= 1e6) return (value / 1e6).toFixed(size % 1e6 === 0 ? 0 : 1).replace('.', ',') + ' M';
    if (size >= 1e4) return formatNumber(Math.round(value / 1e3)) + ' k';
    return formatRate(value);
  }

  function formatShare(value) {
    if (value === null || value === undefined || !isFinite(value)) return '-';
    const percent = value * 100;
    return (percent >= 10 ? percent.toFixed(0) : percent.toFixed(1)).replace('.', ',') + ' %';
  }

  // 380 -> "+380 %", -12.5 -> "-12,5 %", null -> "-".
  function formatChange(value) {
    if (value === null || value === undefined || !isFinite(value)) return '-';
    const text = Math.abs(value) >= 100 ? String(Math.round(value)) : value.toFixed(1).replace('.', ',');
    return (value > 0 ? '+' : '') + text + ' %';
  }

  function changeClass(value) {
    if (value === null || value === undefined || !isFinite(value) || Math.abs(value) < 1) return '';
    return value > 0 ? 'up' : 'down';
  }

  function span(seconds) {
    const value = Math.max(0, Math.round(seconds));
    if (value < 60) return value + ' s';
    if (value < 3600) return Math.round(value / 60) + ' min';
    return Math.floor(value / 3600) + ' h ' + String(Math.floor((value % 3600) / 60)).padStart(2, '0');
  }

  function freshness(snapshot, nowSeconds) {
    if (!snapshot || !snapshot.generated_at) return '';
    let text = 'mis à jour il y a ' + span(nowSeconds - snapshot.generated_at);
    if (snapshot.next_at) {
      const left = snapshot.next_at - nowSeconds;
      text += left > 0 ? ' · prochaine mise à jour dans ' + span(left) : ' · prochaine mise à jour imminente';
    }
    return text;
  }

  function mode(key) {
    return MODES.find((entry) => entry.key === key) || MODES[0];
  }

  // Data of one chart timeframe: "24h" = last 24 hourly slots of the week
  // series, "7d" = the 7 day hourly series, "all" = the whole-history series
  // (bucket width chosen by the daemon). Same shape for the three.
  function chartView(snapshot, periodKey) {
    const week = {
      bucket_seconds: snapshot.bucket_seconds, slots: snapshot.slots || [], fleet_ticks: snapshot.fleet_ticks || [],
      fleet_xp: snapshot.fleet_xp || [], fleet_rate: snapshot.fleet_rate || [], series: snapshot.series || {},
    };
    if (periodKey === 'all') return snapshot.history_chart || { bucket_seconds: week.bucket_seconds, slots: [], fleet_ticks: [], fleet_xp: [], fleet_rate: [], series: {} };
    if (periodKey !== '24h') return week;
    const count = 24;
    const tail = (list) => list.slice(Math.max(0, list.length - count));
    const series = {};
    for (const skill of Object.keys(week.series)) {
      series[skill] = {};
      for (const key of Object.keys(week.series[skill])) series[skill][key] = tail(week.series[skill][key]);
    }
    return {
      bucket_seconds: week.bucket_seconds, slots: tail(week.slots), fleet_ticks: tail(week.fleet_ticks),
      fleet_xp: tail(week.fleet_xp), fleet_rate: tail(week.fleet_rate), series,
    };
  }

  // The two fleet lines: raw XP per bucket, XP per 6000 fleet ticks.
  function fleetLines(view) {
    return {
      xp: [{ skill: 'XP flotte', color: '#ff0', values: view.fleet_xp || [] }],
      rate: [{ skill: RATE_LABEL + ' flotte', color: '#04A800', values: view.fleet_rate || [] }],
    };
  }

  // Chart lines: per visible skill, the aligned values (null = gap).
  function chartLines(snapshot, view, modeKey, hidden) {
    const lines = [];
    for (const skill of snapshot.skills || []) {
      if (hidden && hidden[skill]) continue;
      const values = ((view.series || {})[skill] || {})[modeKey] || [];
      lines.push({ skill, color: COLORS[skill] || '#ccc', values });
    }
    return lines;
  }

  // Top of the value scale over the visible lines (0 when nothing is drawn).
  function chartMax(lines) {
    let max = 0;
    for (const line of lines) {
      for (const value of line.values) if (value !== null && value !== undefined && value > max) max = value;
    }
    return max;
  }

  // Slot nearest to a pixel x in a plot starting at `plotX`, `plotW` wide.
  function slotAt(x, plotX, plotW, slotCount) {
    if (slotCount <= 0) return -1;
    const ratio = (x - plotX) / plotW;
    return Math.max(0, Math.min(slotCount - 1, Math.round(ratio * (slotCount - 1))));
  }

  // Tooltip lines of one slot, highest value first, visible lines only.
  function tooltipLines(snapshot, lines, slot, label) {
    const rows = lines
      .map((line) => ({ skill: line.skill, color: line.color, value: line.values[slot] }))
      .filter((row) => row.value !== null && row.value !== undefined)
      .sort((left, right) => right.value - left.value);
    const out = [{ text: label + ' · ' + formatNumber((snapshot.fleet_ticks || [])[slot]) + ' ticks de travail', color: '#eee' }];
    for (const row of rows) out.push({ text: row.skill + ' ' + formatRate(row.value), color: row.color });
    return out;
  }

  // Table rows for one period, ordered by XP per hour (biggest first).
  function skillRows(snapshot, periodKey) {
    return (snapshot.table || [])
      .map((entry) => Object.assign({ skill: entry.skill }, (entry.periods || {})[periodKey] || {}))
      .sort((left, right) => (right.xp_per_hour || 0) - (left.xp_per_hour || 0) || left.skill.localeCompare(right.skill));
  }

  function methodText(best) {
    if (!best) return '-';
    return best.method + ' · ' + formatRate(best.xp_rate) + ' · ' + formatNumber(best.ticks) + ' ticks · ' + best.runs + ' runs';
  }

  return {
    MODES, RATE_LABEL, PERIOD_LABELS, chartView, fleetLines, COLORS, esc, formatNumber, formatRate, formatAxis, formatShare, formatChange, changeClass, span, freshness,
    mode, chartLines, chartMax, slotAt, tooltipLines, skillRows, methodText,
  };
})();

if (typeof module !== 'undefined' && module.exports) module.exports = XpPage;

if (typeof document !== 'undefined') {
  const page = XpPage;
  let snapshot = null;
  let loadError = '';
  let modeKey = page.MODES[0].key;
  let periodKey = '24h';
  let chartPeriodKey = '24h';
  let fleetPeriodKey = '24h';
  const hidden = {};
  const byId = (id) => document.getElementById(id);

  const PAD = { left: 56, right: 16, top: 10, bottom: 24 };

  // One canvas: `lines` over the slots of `view`, hover tooltip when
  // `hoverSlot >= 0`. Every chart (skills, fleet XP, fleet rate) uses it.
  function drawPlot(canvas, view, lines, hoverSlot, shortHeight) {
    const context = canvas.getContext('2d');
    const ratio = window.devicePixelRatio || 1;
    const width = Math.max(320, canvas.clientWidth || 720);
    const height = Math.round(Math.min(shortHeight ? 220 : 420, width * (shortHeight ? 0.25 : 0.42)));
    canvas.width = Math.round(width * ratio);
    canvas.height = Math.round(height * ratio);
    canvas.style.height = height + 'px';
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.fillStyle = '#111';
    context.fillRect(0, 0, width, height);
    context.font = '11px monospace';
    const slots = (view && view.slots) || [];
    const plotW = width - PAD.left - PAD.right, plotH = height - PAD.top - PAD.bottom;
    const ticks = LifeChart.niceTicks(0, page.chartMax(lines) || 1, LifeChart.tickCount(plotH, 40, 8));
    const top = ticks[ticks.length - 1] || 1;
    const xOf = (index) => PAD.left + (slots.length > 1 ? (index / (slots.length - 1)) * plotW : 0);
    const yOf = (value) => PAD.top + plotH - (value / top) * plotH;
    context.strokeStyle = '#2a2a2a';
    context.fillStyle = '#888';
    context.textAlign = 'right';
    for (const tick of ticks) {
      const y = yOf(tick);
      context.beginPath(); context.moveTo(PAD.left, y); context.lineTo(PAD.left + plotW, y); context.stroke();
      context.fillText(page.formatAxis(tick), PAD.left - 6, y + 4);
    }
    context.textAlign = 'center';
    const every = LifeChart.labelEvery(slots.length, plotW, 80);
    for (let index = 0; index < slots.length; index += every) {
      context.fillText(LifeChart.axisLabel(slots[index], view.bucket_seconds, slots.length), xOf(index), height - 8);
    }
    context.lineWidth = 1.5;
    for (const line of lines) {
      context.strokeStyle = line.color;
      context.beginPath();
      let pen = false;
      line.values.forEach((value, index) => {
        if (value === null || value === undefined) { pen = false; return; }
        if (!pen) { context.moveTo(xOf(index), yOf(value)); pen = true; } else context.lineTo(xOf(index), yOf(value));
      });
      context.stroke();
    }
    if (hoverSlot >= 0 && slots.length) {
      const x = xOf(hoverSlot);
      context.strokeStyle = '#666';
      context.beginPath(); context.moveTo(x, PAD.top); context.lineTo(x, PAD.top + plotH); context.stroke();
      const label = LifeChart.periodLabel(slots[hoverSlot], view.bucket_seconds);
      const rows = page.tooltipLines(view, lines, hoverSlot, label);
      const boxW = Math.max(...rows.map((row) => context.measureText(row.text).width)) + 16;
      const boxH = rows.length * 14 + 8;
      const boxX = x + 12 + boxW > width ? x - 12 - boxW : x + 12;
      context.fillStyle = 'rgba(0,0,0,0.88)';
      context.fillRect(boxX, PAD.top, boxW, boxH);
      context.strokeStyle = '#555';
      context.strokeRect(boxX, PAD.top, boxW, boxH);
      context.textAlign = 'left';
      rows.forEach((row, index) => { context.fillStyle = row.color; context.fillText(row.text, boxX + 8, PAD.top + 14 + index * 14); });
    }
  }

  // The three charts, each with its own timeframe and the same hover slot.
  const CHARTS = [
    { id: 'fleetxp', periodVar: () => fleetPeriodKey, lines: (view) => page.fleetLines(view).xp, short: true },
    { id: 'fleetrate', periodVar: () => fleetPeriodKey, lines: (view) => page.fleetLines(view).rate, short: true },
    { id: 'chart', periodVar: () => chartPeriodKey, lines: (view) => page.chartLines(snapshot, view, modeKey, hidden), short: false },
  ];

  function drawCharts(hoverChart, hoverSlot) {
    for (const chart of CHARTS) {
      const view = snapshot ? page.chartView(snapshot, chart.periodVar()) : null;
      drawPlot(byId(chart.id), view, view ? chart.lines(view) : [], chart === hoverChart ? hoverSlot : -1, chart.short);
    }
  }

  function renderControls() {
    byId('modes').innerHTML = page.MODES.map((entry) =>
      '<button class="toggle" data-mode="' + entry.key + '" aria-pressed="' + (entry.key === modeKey) + '">' + page.esc(entry.label) + '</button>').join('');
    byId('legend').innerHTML = (snapshot.skills || []).map((skill) =>
      '<button class="legend' + (hidden[skill] ? ' off' : '') + '" data-skill="' + skill + '"><i style="background:' + (page.COLORS[skill] || '#ccc') + '"></i>' + page.esc(skill) + '</button>').join('');
    const periodButtons = (attribute, current) => (snapshot.periods || []).map((period) =>
      '<button class="toggle" data-' + attribute + '="' + period.key + '" aria-pressed="' + (period.key === current) + '">' + page.esc(page.PERIOD_LABELS[period.key] || period.key) + '</button>').join('');
    byId('periods').innerHTML = periodButtons('period', periodKey);
    byId('chartperiods').innerHTML = periodButtons('chartperiod', chartPeriodKey);
    byId('fleetperiods').innerHTML = periodButtons('fleetperiod', fleetPeriodKey);
  }

  function renderSkills() {
    const compare = periodKey !== 'all';
    let html = '<thead><tr><th class="left">Compétence</th><th>XP (efficacité)</th><th>Contribution flotte (XP / 6000 ticks)</th><th>XP / h</th>'
      + '<th>Part du temps de travail</th><th>Bots</th><th class="left">Meilleure méthode (XP / 6000 ticks, échantillon)</th>'
      + (compare ? '<th>Évolution vs période précédente</th>' : '') + '</tr></thead><tbody>';
    for (const row of page.skillRows(snapshot, periodKey)) {
      html += '<tr' + (row.idle ? ' class="idle" title="Moins de ' + Math.round((snapshot.idle_share || 0.01) * 100) + ' % du temps de travail de la flotte"' : '') + '>'
        + '<td class="left">' + page.esc(row.skill) + '</td>'
        + '<td>' + page.formatRate(row.efficiency) + '</td><td>' + page.formatRate(row.fleet_rate) + '</td>'
        + '<td>' + page.formatNumber(row.xp_per_hour) + '</td><td>' + page.formatShare(row.share) + '</td><td>' + (row.bots || 0) + '</td>'
        + '<td class="left">' + page.esc(page.methodText(row.best_method)) + '</td>'
        + (compare ? '<td class="' + page.changeClass(row.change_percent) + '">' + page.formatChange(row.change_percent) + '</td>' : '') + '</tr>';
    }
    byId('skills').innerHTML = html + '</tbody>';
  }

  function renderMethods() {
    let html = '<thead><tr><th class="left">Compétence</th><th class="left">Méthode</th><th>XP</th><th>Ticks de travail</th><th>Exécutions</th><th>Bots</th></tr></thead><tbody>';
    for (const row of snapshot.top_methods || []) {
      html += '<tr><td class="left">' + page.esc(row.skill) + '</td><td class="left">' + page.esc(row.method) + '</td><td>' + page.formatRate(row.xp_rate)
        + '</td><td>' + page.formatNumber(row.ticks) + '</td><td>' + page.formatNumber(row.runs) + '</td><td>' + row.bots + '</td></tr>';
    }
    byId('methods').innerHTML = html + '</tbody>';
    byId('methodnote').textContent = 'Méthodes (méthode du planner) ayant au moins ' + page.formatNumber(snapshot.min_method_ticks) + ' ticks de travail sur 24 h, classées par XP / 6000 ticks.';
  }

  function renderStatus() {
    const status = byId('status');
    if (!snapshot) { status.textContent = loadError || 'chargement…'; return; }
    status.textContent = '· ' + page.freshness(snapshot, Date.now() / 1000);
  }

  function render() {
    renderStatus();
    if (!snapshot) { drawCharts(null, -1); return; }
    renderControls();
    renderSkills();
    renderMethods();
    drawCharts(null, -1);
  }

  async function reload() {
    try {
      const response = await fetch('/api/xp', { cache: 'no-store' });
      const body = await response.json();
      if (body.ok === false) throw new Error(body.error || 'instantané indisponible');
      snapshot = body;
      loadError = '';
      byId('errors').style.display = 'none';
    } catch (error) {
      loadError = 'xp inaccessible : ' + error;
      byId('errors').style.display = 'block';
      byId('errors').textContent = loadError;
    }
    render();
  }

  document.addEventListener('click', (event) => {
    const target = event.target.closest('button');
    if (!target || !snapshot) return;
    if (target.dataset.mode) modeKey = target.dataset.mode;
    else if (target.dataset.period) periodKey = target.dataset.period;
    else if (target.dataset.chartperiod) chartPeriodKey = target.dataset.chartperiod;
    else if (target.dataset.fleetperiod) fleetPeriodKey = target.dataset.fleetperiod;
    else if (target.dataset.skill) hidden[target.dataset.skill] = !hidden[target.dataset.skill];
    else return;
    render();
  });
  for (const chart of CHARTS) {
    const canvas = byId(chart.id);
    const hover = (event) => {
      if (!snapshot) return;
      const rect = canvas.getBoundingClientRect();
      const point = event.touches ? event.touches[0] : event;
      const view = page.chartView(snapshot, chart.periodVar());
      drawCharts(chart, page.slotAt(point.clientX - rect.left, PAD.left, rect.width - PAD.left - PAD.right, (view.slots || []).length));
    };
    canvas.addEventListener('mousemove', hover);
    canvas.addEventListener('touchstart', hover, { passive: true });
    canvas.addEventListener('touchmove', hover, { passive: true });
    canvas.addEventListener('mouseleave', () => drawCharts(null, -1));
  }
  window.addEventListener('resize', () => drawCharts(null, -1));

  render();
  reload();
  setInterval(reload, 300000);
  setInterval(renderStatus, 30000);
}
