// Guides front (plan rendements R4): money making and training guides
// generated from the measured yields and the game priors, priced at the
// current market price. One script for both pages: the page's body says
// which guide (data-guide="money" | "training"). Read-only dashboard
// pages: bots never read them (R7 later). Filters ride the API query
// (skill, level, family, quests); the table shows the best lines first
// (profit or effective XP), a click on a line unfolds its flows, stages
// and other XP. Sorting rides the shared TableSort helper (table.js).
var GuidesView = (function () {
  var SKILLS = ['attack', 'strength', 'defence', 'hitpoints', 'ranged', 'prayer', 'magic', 'cooking', 'woodcutting',
    'fishing', 'firemaking', 'crafting', 'smithing', 'mining', 'runecraft'];
  var FAMILIES = ['farm', 'recipe', 'chain', 'market'];
  var DEFAULT_LIMIT = 300;

  function number(value) {
    if (value === null || value === undefined || !isFinite(value)) return '-';
    var abs = Math.abs(value);
    if (abs >= 1e6) return (Math.round(value / 1e5) / 10) + 'M';
    if (abs >= 1e4) return (Math.round(value / 100) / 10) + 'k';
    return String(Math.round(value * 10) / 10);
  }

  function escapeHtml(text) {
    return String(text === null || text === undefined ? '' : text)
      .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }

  function bandText(band) {
    return band > 0 ? band + '-' + (band + 4) : '-';
  }

  function ticksText(ticks) {
    if (ticks === null || ticks === undefined || !isFinite(ticks)) return '-';
    if (ticks >= 6000) return (Math.round(ticks / 600) / 10) + ' × 6000';
    return String(Math.round(ticks));
  }

  // "mining 5000, smithing 200": a skill → XP map, best first.
  function xpText(map) {
    var keys = Object.keys(map || {}).filter(function (skill) { return map[skill] > 0.05; });
    keys.sort(function (left, right) { return map[right] - map[left]; });
    return keys.map(function (skill) { return skill + ' ' + number(map[skill]); }).join(', ') || '-';
  }

  // "smithing 30, combat 12, quest doric, rune_pickaxe, 5k capital".
  function prereqText(prereqs) {
    if (!prereqs) return '-';
    var parts = [];
    var levels = prereqs.levels || {};
    Object.keys(levels).sort().forEach(function (skill) { parts.push(skill + ' ' + levels[skill]); });
    if (prereqs.combat > 3) parts.push('combat ' + prereqs.combat);
    (prereqs.quests || []).forEach(function (quest) { parts.push('quest ' + quest); });
    (prereqs.tools || []).forEach(function (tool) { parts.push(tool); });
    var stock = prereqs.stock || {};
    Object.keys(stock).sort().forEach(function (item) { parts.push('stock ' + number(stock[item]) + ' ' + item); });
    if (prereqs.capital > 0) parts.push(number(prereqs.capital) + ' capital');
    if (prereqs.ironman === false) parts.push('no ironman');
    return parts.join(', ') || '-';
  }

  function confidenceText(confidence) {
    if (!confidence) return '-';
    var source = confidence.measured ? 'measured' : '<span class="prior">estimated</span>';
    return source + ' · n ' + (confidence.runs || 0) + ' · ' + escapeHtml(confidence.read || '');
  }

  // Query string of a filter object (empty values left out).
  function queryOf(filter) {
    var parts = [];
    ['skill', 'level', 'family', 'quests'].forEach(function (key) {
      var value = filter[key];
      if (value !== undefined && value !== null && String(value).trim() !== '') parts.push(key + '=' + encodeURIComponent(String(value).trim()));
    });
    return parts.join('&');
  }

  // Rows the page shows: accessible ones when asked, the text search,
  // then the best `limit` (the API already sorts best first).
  function visibleRows(rows, options) {
    var needle = (options.search || '').trim().toLowerCase();
    var out = rows.filter(function (row) {
      if (options.accessibleOnly && !row.accessible) return false;
      if (!needle) return true;
      return [row.method, row.style, row.site, row.tool, row.family, row.skill, row.supply].filter(Boolean).join(' ').toLowerCase().indexOf(needle) >= 0;
    });
    return options.limit > 0 ? out.slice(0, options.limit) : out;
  }

  var MONEY_COLS = [
    { key: 'family', label: 'family', kind: 'str' },
    { key: 'method', label: 'method', kind: 'str', fmt: function (v, row) { return escapeHtml(v) + (row.style ? ' <span class="muted">' + escapeHtml(row.style) + '</span>' : ''); } },
    { key: 'site', label: 'site', kind: 'str', fmt: function (v) { return v ? escapeHtml(v) : '<span class="muted">best</span>'; } },
    { key: 'tool', label: 'tool', kind: 'str' },
    { key: 'band', label: 'band', kind: 'num', fmt: bandText },
    { key: 'profit_6000', label: 'profit/6000', kind: 'num', fmt: number },
    { key: 'revenue_6000', label: 'revenue', kind: 'num', fmt: number },
    { key: 'cost_6000', label: 'cost', kind: 'num', fmt: number },
    { key: 'extra_ticks_6000', label: '+ticks/6000', kind: 'num', fmt: number },
    { key: 'xp_text', label: 'XP/6000 (bonus)', kind: 'str' },
    { key: 'prereq_text', label: 'prerequisites', kind: 'str' },
    { key: 'confidence_text', label: 'confidence', kind: 'str' },
    { key: 'price_confidence', label: 'prices', kind: 'str' },
    { key: 'blocked_text', label: 'blocked', kind: 'str' },
  ];

  var TRAINING_COLS = [
    { key: 'skill', label: 'skill', kind: 'str' },
    { key: 'band', label: 'band', kind: 'num', fmt: bandText },
    { key: 'method', label: 'method', kind: 'str', fmt: function (v, row) { return escapeHtml(v) + (row.style ? ' <span class="muted">' + escapeHtml(row.style) + '</span>' : ''); } },
    { key: 'supply', label: 'supply', kind: 'str' },
    { key: 'site', label: 'site', kind: 'str', fmt: function (v) { return v ? escapeHtml(v) : '<span class="muted">best</span>'; } },
    { key: 'tool', label: 'tool', kind: 'str' },
    { key: 'xp_6000', label: 'XP/6000', kind: 'num', fmt: number },
    { key: 'effective_xp_6000', label: 'effective XP/6000', kind: 'num', fmt: number },
    { key: 'coins_6000', label: 'coins/6000', kind: 'num', fmt: number },
    { key: 'coins_per_xp', label: 'coins/XP', kind: 'num', fmt: function (v) { return isFinite(v) ? String(Math.round(v * 100) / 100) : '-'; } },
    { key: 'converted_ticks_6000', label: 'coin ticks/6000', kind: 'num', fmt: number },
    { key: 'ticks_to_band_end', label: 'ticks to band end', kind: 'num', fmt: ticksText },
    { key: 'prereq_text', label: 'prerequisites', kind: 'str' },
    { key: 'confidence_text', label: 'confidence', kind: 'str' },
    { key: 'price_confidence', label: 'prices', kind: 'str' },
    { key: 'blocked_text', label: 'blocked', kind: 'str' },
  ];

  // Flattened display fields of an API row (sortable as text).
  function decorate(row) {
    row.xp_text = xpText(row.xp_6000 && typeof row.xp_6000 === 'object' ? row.xp_6000 : row.other_xp_6000);
    row.prereq_text = prereqText(row.prereqs);
    row.confidence_text = confidenceText(row.confidence);
    row.blocked_text = blockedText(row.blocked);
    return row;
  }

  // Why no bot can work a row whatever its levels (stock not held, an
  // input nobody makes, offers or sells).
  function blockedText(reasons) {
    return (reasons || []).join('; ') || '-';
  }

  function detailHtml(row) {
    var html = '<table class="detail"><tr><th class="left">item</th><th class="left">cause</th><th>/6000</th><th>unit price</th><th>value/6000</th><th class="left">outlet</th></tr>';
    (row.flows || []).forEach(function (line) {
      html += '<tr><td class="left">' + escapeHtml(line.display || line.item) + '</td><td class="left">' + escapeHtml(line.cause) + '</td><td>' + number(line.per_6000) + '</td><td>' +
        number(line.unit_price) + (line.traded ? '' : ' <span class="prior">' + escapeHtml(line.basis || 'untraded') + '</span>') + '</td><td>' + number(line.value_6000) + '</td><td class="left">' + escapeHtml(line.outlet) + '</td></tr>';
    });
    if (row.other_drops && row.other_drops.items > 0) {
      html += '<tr><td class="left">other drops (' + row.other_drops.items + ' objects)</td><td class="left">loot</td><td>' + number(row.other_drops.units_6000) + '</td><td>-</td><td>' + number(row.other_drops.value_6000) + '</td><td class="left">market</td></tr>';
    }
    html += '</table>';
    if (row.stages && row.stages.length) {
      html += '<table class="detail"><tr><th class="left">stage</th><th class="left">product</th><th>ticks/unit</th><th>profit/6000 if sold here</th></tr>';
      row.stages.forEach(function (stage) {
        html += '<tr><td class="left">' + escapeHtml(stage.method) + '</td><td class="left">' + escapeHtml(stage.product) + '</td><td>' + number(stage.ticks_per_unit) + '</td><td>' + number(stage.profit_6000) + '</td></tr>';
      });
      html += '</table>';
    }
    if (row.other_xp_6000 && Object.keys(row.other_xp_6000).length) html += '<div class="muted">other XP/6000: ' + escapeHtml(xpText(row.other_xp_6000)) + '</div>';
    if (row.quest_xp > 0) html += '<div class="muted">quest XP ' + number(row.quest_xp) + ', measured duration ' + ticksText(row.quest_ticks) + '</div>';
    return html;
  }

  function start(kind) {
    var state = { payload: null, rows: [], open: {} };
    var columns = kind === 'money' ? MONEY_COLS : TRAINING_COLS;
    var sorter = (typeof TableSort !== 'undefined') ? TableSort : null;

    function value(id) { var node = document.getElementById(id); return node ? node.value : ''; }
    function checked(id) { var node = document.getElementById(id); return node ? node.checked : false; }

    function render() {
      if (!state.payload) return;
      var options = { search: value('search'), accessibleOnly: checked('accessible'), limit: parseInt(value('limit'), 10) || 0 };
      var rows = visibleRows(state.rows, options);
      document.getElementById('summary').textContent = state.rows.length + ' line(s), ' + rows.length + ' shown · built in ' + (state.payload.built_ms || 0) + ' ms' +
        (state.payload.market_wait_ticks ? ' · market wait ' + Math.round(state.payload.market_wait_ticks) + ' ticks/lot' : ' · market wait unmeasured');
      var keyOf = function (row) { return [row.family, row.skill, row.method, row.style, row.site, row.tool, row.band, row.supply].join('|'); };
      var rowAttrs = function (row) { return { 'data-key': keyOf(row), class: row.accessible ? 'line' : 'line locked' }; };
      if (sorter) sorter.render('guide', columns, rows, { rowAttrs: rowAttrs });
      var table = document.getElementById('guide');
      var byKey = {};
      rows.forEach(function (row) { byKey[keyOf(row)] = row; });
      Array.prototype.forEach.call(table.querySelectorAll('tr.line'), function (line) {
        var key = line.getAttribute('data-key');
        line.style.cursor = 'pointer';
        line.onclick = function () { state.open[key] = !state.open[key]; render(); };
        if (state.open[key] && byKey[key]) {
          var detail = document.createElement('tr');
          detail.innerHTML = '<td class="left" colspan="' + columns.length + '">' + detailHtml(byKey[key]) + '</td>';
          line.parentNode.insertBefore(detail, line.nextSibling);
        }
      });
      var gaps = (state.payload.unrated || []).map(function (gap) { return escapeHtml(gap.method) + ': ' + escapeHtml(gap.reason); });
      document.getElementById('unrated').innerHTML = gaps.length ? gaps.join('<br>') : '-';
    }

    function load() {
      var filter = { skill: value('skill'), level: value('level'), family: value('family'), quests: value('quests') };
      document.getElementById('summary').textContent = 'building…';
      fetch('/api/guides/' + kind + '?' + queryOf(filter)).then(function (response) { return response.json(); }).then(function (body) {
        if (!body.ok) { document.getElementById('summary').textContent = body.error || 'error'; return; }
        state.payload = body;
        state.rows = (body.rows || []).map(decorate);
        render();
      }).catch(function () { document.getElementById('summary').textContent = 'daemon unreachable'; });
    }

    var skill = document.getElementById('skill');
    if (skill) skill.innerHTML = '<option value="">all skills</option>' + SKILLS.map(function (name) { return '<option>' + name + '</option>'; }).join('');
    var family = document.getElementById('family');
    if (family) family.innerHTML = '<option value="">all families</option>' + FAMILIES.map(function (name) { return '<option>' + name + '</option>'; }).join('');
    var limit = document.getElementById('limit');
    if (limit && !limit.value) limit.value = DEFAULT_LIMIT;
    ['skill', 'level', 'family', 'quests'].forEach(function (id) { var node = document.getElementById(id); if (node) node.onchange = load; });
    ['search', 'accessible', 'limit'].forEach(function (id) { var node = document.getElementById(id); if (node) node.oninput = render; if (node) node.onchange = render; });
    load();
    setInterval(load, 60000);
  }

  return { start: start, queryOf: queryOf, visibleRows: visibleRows, prereqText: prereqText, xpText: xpText, number: number };
})();

if (typeof module !== 'undefined' && module.exports) {
  module.exports = GuidesView;
} else if (typeof document !== 'undefined' && document.body && document.body.getAttribute('data-guide')) {
  GuidesView.start(document.body.getAttribute('data-guide'));
}
