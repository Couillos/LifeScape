// Page « XP / tick » (2026-11-22). Le daemon calcule un instantané toutes
// les 30 min (comme les hiscores, `xp_service`); cette page lit seulement
// `/api/xp` (octets en cache). Tous les taux sont des rapports de sommes
// (somme des XP / somme des ticks de travail), jamais des moyennes de taux.
//
// Helpers purs (XpPage.*) sans DOM : `node xp.test.js`.
const XpPage = (() => {
  'use strict';

  const MODES = [
    {
      key: 'fleet_per_tick',
      label: 'Contribution à la flotte',
      note: 'XP gagnés dans la compétence pendant l’heure ÷ ticks de travail de toute la flotte pendant cette heure : '
        + 'la part de chaque compétence dans l’XP par tick de la flotte (les courbes s’additionnent).',
    },
    {
      key: 'efficiency',
      label: 'Efficacité de la compétence',
      note: 'XP gagnés dans la compétence ÷ ticks de travail des activités qui rapportent de l’XP dans cette compétence : '
        + 'ce que rapporte un tick passé à l’entraîner (heure sans activité = trou).',
    },
  ];
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

  // 1.5512 -> "1,55" (French decimal comma); null -> "-".
  function formatRate(value) {
    if (value === null || value === undefined || !isFinite(value)) return '-';
    const digits = value >= 10 ? 1 : value >= 1 ? 2 : 3;
    return value.toFixed(digits).replace('.', ',');
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

  // Chart lines: per visible skill, the aligned values (null = gap).
  function chartLines(snapshot, modeKey, hidden) {
    const lines = [];
    for (const skill of snapshot.skills || []) {
      if (hidden && hidden[skill]) continue;
      const values = ((snapshot.series || {})[skill] || {})[modeKey] || [];
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
    return best.method + ' · ' + formatRate(best.xp_per_tick) + '/tick · ' + formatNumber(best.ticks) + ' ticks · ' + best.runs + ' runs';
  }

  return {
    MODES, PERIOD_LABELS, COLORS, esc, formatNumber, formatRate, formatShare, formatChange, changeClass, span, freshness,
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
  const hidden = {};
  const byId = (id) => document.getElementById(id);

  const PAD = { left: 56, right: 16, top: 10, bottom: 24 };

  function drawChart(hoverSlot) {
    const canvas = byId('chart');
    const context = canvas.getContext('2d');
    const ratio = window.devicePixelRatio || 1;
    const width = Math.max(320, canvas.clientWidth || 720);
    const height = Math.round(Math.min(420, width * 0.42));
    canvas.width = Math.round(width * ratio);
    canvas.height = Math.round(height * ratio);
    canvas.style.height = height + 'px';
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.fillStyle = '#111';
    context.fillRect(0, 0, width, height);
    context.font = '11px monospace';
    const slots = (snapshot && snapshot.slots) || [];
    const lines = snapshot ? page.chartLines(snapshot, modeKey, hidden) : [];
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
      context.fillText(page.formatRate(tick), PAD.left - 6, y + 4);
    }
    context.textAlign = 'center';
    const every = LifeChart.labelEvery(slots.length, plotW, 80);
    for (let index = 0; index < slots.length; index += every) {
      context.fillText(LifeChart.axisLabel(slots[index], snapshot.bucket_seconds, slots.length), xOf(index), height - 8);
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
      const label = LifeChart.periodLabel(slots[hoverSlot], snapshot.bucket_seconds);
      const rows = page.tooltipLines(snapshot, lines, hoverSlot, label);
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

  function renderCards() {
    let html = '';
    for (const period of snapshot.periods || []) {
      const fleet = (snapshot.fleet || {})[period.key] || {};
      html += '<div class="card"><span class="muted">Flotte · ' + page.esc(page.PERIOD_LABELS[period.key] || period.key) + '</span>'
        + '<b>' + page.formatRate(fleet.xp_per_tick) + ' XP/tick</b>'
        + page.formatNumber(fleet.xp) + ' XP · ' + page.formatNumber(fleet.work_ticks) + ' ticks · ' + (fleet.bots || 0) + ' bots</div>';
    }
    byId('cards').innerHTML = html;
  }

  function renderControls() {
    byId('modes').innerHTML = page.MODES.map((entry) =>
      '<button class="toggle" data-mode="' + entry.key + '" aria-pressed="' + (entry.key === modeKey) + '">' + page.esc(entry.label) + '</button>').join('');
    byId('modenote').textContent = page.mode(modeKey).note;
    byId('legend').innerHTML = (snapshot.skills || []).map((skill) =>
      '<button class="legend' + (hidden[skill] ? ' off' : '') + '" data-skill="' + skill + '"><i style="background:' + (page.COLORS[skill] || '#ccc') + '"></i>' + page.esc(skill) + '</button>').join('');
    byId('periods').innerHTML = (snapshot.periods || []).map((period) =>
      '<button class="toggle" data-period="' + period.key + '" aria-pressed="' + (period.key === periodKey) + '">' + page.esc(page.PERIOD_LABELS[period.key] || period.key) + '</button>').join('');
  }

  function renderSkills() {
    const compare = periodKey !== 'all';
    let html = '<thead><tr><th class="left">Compétence</th><th>XP / tick (efficacité)</th><th>Contribution flotte / tick</th><th>XP / h</th>'
      + '<th>Part du temps de travail</th><th>Bots</th><th class="left">Meilleure méthode (XP/tick, échantillon)</th>'
      + (compare ? '<th>Évolution vs période précédente</th>' : '') + '</tr></thead><tbody>';
    for (const row of page.skillRows(snapshot, periodKey)) {
      html += '<tr' + (row.idle ? ' class="idle" title="Moins de ' + Math.round((snapshot.idle_share || 0.01) * 100) + ' % du temps de travail de la flotte"' : '') + '>'
        + '<td class="left">' + page.esc(row.skill) + '</td>'
        + '<td>' + page.formatRate(row.efficiency) + '</td><td>' + page.formatRate(row.fleet_per_tick) + '</td>'
        + '<td>' + page.formatNumber(row.xp_per_hour) + '</td><td>' + page.formatShare(row.share) + '</td><td>' + (row.bots || 0) + '</td>'
        + '<td class="left">' + page.esc(page.methodText(row.best_method)) + '</td>'
        + (compare ? '<td class="' + page.changeClass(row.change_percent) + '">' + page.formatChange(row.change_percent) + '</td>' : '') + '</tr>';
    }
    byId('skills').innerHTML = html + '</tbody>';
    byId('skillnote').textContent = 'Efficacité = XP ÷ ticks de travail des activités qui rapportent de l’XP dans la compétence (un combat compte ses ticks dans chaque compétence de combat nourrie, '
      + 'les parts se recoupent). XP / h = XP de toute la flotte par heure réelle. En rouge : moins de 1 % du temps de travail. '
      + 'Méthode classée à partir de ' + page.formatNumber(snapshot.min_method_ticks) + ' ticks. Les comptes staff et personnels sont exclus.';
  }

  function renderMethods() {
    let html = '<thead><tr><th class="left">Compétence</th><th class="left">Méthode</th><th>XP / tick</th><th>Ticks de travail</th><th>Exécutions</th><th>Bots</th></tr></thead><tbody>';
    for (const row of snapshot.top_methods || []) {
      html += '<tr><td class="left">' + page.esc(row.skill) + '</td><td class="left">' + page.esc(row.method) + '</td><td>' + page.formatRate(row.xp_per_tick)
        + '</td><td>' + page.formatNumber(row.ticks) + '</td><td>' + page.formatNumber(row.runs) + '</td><td>' + row.bots + '</td></tr>';
    }
    byId('methods').innerHTML = html + '</tbody>';
    byId('methodnote').textContent = 'Méthodes (méthode du planner) ayant au moins ' + page.formatNumber(snapshot.min_method_ticks) + ' ticks de travail sur 24 h, classées par XP par tick.';
  }

  function renderStatus() {
    const status = byId('status');
    if (!snapshot) { status.textContent = loadError || 'chargement…'; return; }
    status.textContent = '· ' + page.freshness(snapshot, Date.now() / 1000);
  }

  function render() {
    renderStatus();
    if (!snapshot) { drawChart(-1); return; }
    renderCards();
    renderControls();
    renderSkills();
    renderMethods();
    drawChart(-1);
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
    else if (target.dataset.skill) hidden[target.dataset.skill] = !hidden[target.dataset.skill];
    else return;
    render();
  });
  const canvas = byId('chart');
  const hover = (event) => {
    if (!snapshot) return;
    const rect = canvas.getBoundingClientRect();
    const point = event.touches ? event.touches[0] : event;
    drawChart(page.slotAt(point.clientX - rect.left, PAD.left, rect.width - PAD.left - PAD.right, (snapshot.slots || []).length));
  };
  canvas.addEventListener('mousemove', hover);
  canvas.addEventListener('touchstart', hover, { passive: true });
  canvas.addEventListener('touchmove', hover, { passive: true });
  canvas.addEventListener('mouseleave', () => drawChart(-1));
  window.addEventListener('resize', () => drawChart(-1));

  render();
  reload();
  setInterval(reload, 300000);
  setInterval(renderStatus, 30000);
}
