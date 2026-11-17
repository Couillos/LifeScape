// Guides front (plan rendements R4): money making and training guides
// generated from the measured yields and the game priors, priced at the
// current market price. One script for both pages: the page's body says
// which guide (data-guide="money" | "training"). Read-only dashboard
// pages: the API serves the set the daemon precomputes (guides_store,
// its age shown), the bots will read the same (R7). Filters ride the API query
// (skill, level, family, quests); the table shows the best lines first
// (profit or effective XP), a click on a line unfolds its flows, stages
// and other XP. Sorting rides the shared TableSort helper (table.js).
// D22: no line is ever blocked. Each line lists its conditions (levels,
// site, quests, tools, stock, capital, market availability of what it
// buys) and the API judges it for the profile typed in (level, per-skill
// levels, quests, stock held, coins): the "feasibility" filter keeps the
// lines feasible now, or feasible after prerequisites.
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

  // "set built 42 s ago in 3.7 s, rebuilt every 300 s": the age of the
  // precomputed set the API read (guides_store), then what the read
  // approximates (a level under its band's last, quests on chains).
  function freshnessText(payload) {
    if (!payload) return '';
    var age = Math.round((payload.age_ms || 0) / 1000);
    var parts = ['set built ' + age + ' s ago in ' + (Math.round((payload.cycle_ms || 0) / 100) / 10) + ' s, rebuilt every ' + (payload.refresh_s || '?') + ' s'];
    (payload.approximate || []).forEach(function (note) { parts.push('≈ ' + note); });
    return parts.join(' · ');
  }

  // Milliseconds before asking again: the first set is still building.
  function retryDelay(body) {
    return body && body.pending ? 5000 : null;
  }

  // "mining 5000, smithing 200": a skill → XP map, best first.
  function xpText(map) {
    var keys = Object.keys(map || {}).filter(function (skill) { return map[skill] > 0.05; });
    keys.sort(function (left, right) { return map[right] - map[left]; });
    return keys.map(function (skill) { return skill + ' ' + number(map[skill]); }).join(', ') || '-';
  }

  // "smithing 30, no site within bank reach (impossible today), quest
  // doric, buy coal 19/lot: asks 0, recent 5, shops 0": the conditions of
  // a line (D22, the API's structured list, each with its text).
  function conditionsText(conditions) {
    if (!conditions || !conditions.length) return '-';
    return conditions.map(function (condition) { return condition.text || condition.kind; }).join(', ');
  }

  // Cell markup of the conditions: the impossible ones (no site within
  // bank reach, an input nothing makes) highlighted.
  function conditionsHtml(conditions) {
    if (!conditions || !conditions.length) return '-';
    return conditions.map(function (condition) {
      var text = escapeHtml(condition.text || condition.kind);
      return condition.impossible ? '<span class="impossible">' + text + '</span>' : text;
    }).join(', ');
  }

  // "after_prereqs: smithing 20 -> 30, buy hammer" or "infeasible:
  // coal: holds 0, asks 1, shops 0 of 19 per lot": the API's judgement
  // for the profile of the query.
  function feasibilityText(feasibility) {
    if (!feasibility) return '-';
    var parts = (feasibility.pending || []).concat(feasibility.reasons || []);
    return feasibility.state + (parts.length ? ': ' + parts.join('; ') : '');
  }

  // Plain text (sort key and search), never markup.
  function confidenceText(confidence) {
    if (!confidence) return '-';
    return (confidence.measured ? 'measured' : 'estimated') + ' · n ' + (confidence.runs || 0) + ' · ' + (confidence.read || '');
  }

  // Cell markup of the confidence column (estimates highlighted).
  function confidenceHtml(confidence) {
    if (!confidence) return '-';
    var source = confidence.measured ? 'measured' : '<span class="prior">estimated</span>';
    return source + ' · n ' + (confidence.runs || 0) + ' · ' + escapeHtml(confidence.read || '');
  }

  // Filters of the query: the view (skill, level, family) and the profile
  // the API judges each line for (level, levels, quests, held, coins).
  var QUERY_KEYS = ['skill', 'level', 'family', 'quests', 'levels', 'held', 'coins'];

  // Query string of a filter object (empty values left out).
  function queryOf(filter) {
    var parts = [];
    QUERY_KEYS.forEach(function (key) {
      var value = filter[key];
      if (value !== undefined && value !== null && String(value).trim() !== '') parts.push(key + '=' + encodeURIComponent(String(value).trim()));
    });
    return parts.join('&');
  }

  // Feasibility filter: '' every line, 'reach' feasible or after
  // prerequisites (no infeasible condition), 'now' feasible now.
  function keepFeasibility(row, wanted) {
    var state = row.feasibility ? row.feasibility.state : '';
    if (wanted === 'now') return state === 'feasible';
    if (wanted === 'reach') return state === 'feasible' || state === 'after_prereqs';
    return true;
  }

  // Rows the page shows: the feasibility for the profile, the text
  // search, then the best `limit` (the API already sorts best first).
  function visibleRows(rows, options) {
    var needle = (options.search || '').trim().toLowerCase();
    var out = rows.filter(function (row) {
      if (!keepFeasibility(row, options.feasibility || '')) return false;
      if (!needle) return true;
      return [row.method, row.style, row.site, row.tool, row.family, row.skill, row.supply].filter(Boolean).join(' ').toLowerCase().indexOf(needle) >= 0;
    });
    return options.limit > 0 ? out.slice(0, options.limit) : out;
  }

  // Unpriced line (an input nothing prices): no figure, said so.
  function pricedNumber(value, row) {
    return row && row.priced === false ? '<span class="muted">unpriced</span>' : number(value);
  }

  var CONDITION_COLS = [
    { key: 'conditions_text', label: 'conditions', kind: 'str', fmt: function (text, row) { return conditionsHtml(row.conditions); } },
    { key: 'feasibility_text', label: 'feasibility (profile)', kind: 'str' },
  ];

  var MONEY_COLS = [
    { key: 'family', label: 'family', kind: 'str' },
    { key: 'method', label: 'method', kind: 'str', fmt: function (v, row) { return escapeHtml(v) + (row.style ? ' <span class="muted">' + escapeHtml(row.style) + '</span>' : ''); } },
    { key: 'site', label: 'site', kind: 'str', fmt: function (v) { return v ? escapeHtml(v) : '<span class="muted">best</span>'; } },
    { key: 'tool', label: 'tool', kind: 'str' },
    { key: 'band', label: 'band', kind: 'num', fmt: bandText },
    { key: 'profit_6000', label: 'profit/6000', kind: 'num', fmt: pricedNumber },
    { key: 'revenue_6000', label: 'revenue', kind: 'num', fmt: pricedNumber },
    { key: 'cost_6000', label: 'cost', kind: 'num', fmt: pricedNumber },
    { key: 'extra_ticks_6000', label: '+ticks/6000', kind: 'num', fmt: number },
    { key: 'xp_text', label: 'XP/6000 (bonus)', kind: 'str' },
  ].concat(CONDITION_COLS, [
    { key: 'confidence_text', label: 'confidence', kind: 'str', fmt: function (v, row) { return confidenceHtml(row.confidence); } },
    { key: 'price_confidence', label: 'prices', kind: 'str' },
  ]);

  var TRAINING_COLS = [
    { key: 'skill', label: 'skill', kind: 'str' },
    { key: 'band', label: 'band', kind: 'num', fmt: bandText },
    { key: 'method', label: 'method', kind: 'str', fmt: function (v, row) { return escapeHtml(v) + (row.style ? ' <span class="muted">' + escapeHtml(row.style) + '</span>' : ''); } },
    { key: 'supply', label: 'supply', kind: 'str' },
    { key: 'site', label: 'site', kind: 'str', fmt: function (v) { return v ? escapeHtml(v) : '<span class="muted">best</span>'; } },
    { key: 'tool', label: 'tool', kind: 'str' },
    { key: 'xp_6000', label: 'XP/6000', kind: 'num', fmt: number },
    { key: 'effective_xp_6000', label: 'effective XP/6000', kind: 'num', fmt: pricedNumber },
    { key: 'coins_6000', label: 'coins/6000', kind: 'num', fmt: pricedNumber },
    { key: 'coins_per_xp', label: 'coins/XP', kind: 'num', fmt: function (value, row) { return row && row.priced === false ? '<span class="muted">unpriced</span>' : (value !== null && isFinite(value) ? String(Math.round(value * 100) / 100) : '-'); } },
    { key: 'converted_ticks_6000', label: 'coin ticks/6000', kind: 'num', fmt: pricedNumber },
    { key: 'ticks_to_band_end', label: 'ticks to band end', kind: 'num', fmt: ticksText },
  ].concat(CONDITION_COLS, [
    { key: 'confidence_text', label: 'confidence', kind: 'str', fmt: function (v, row) { return confidenceHtml(row.confidence); } },
    { key: 'price_confidence', label: 'prices', kind: 'str' },
  ]);

  // Flattened display fields of an API row (sortable as text).
  function decorate(row) {
    row.xp_text = xpText(row.xp_6000 && typeof row.xp_6000 === 'object' ? row.xp_6000 : row.other_xp_6000);
    row.conditions_text = conditionsText(row.conditions);
    row.feasibility_text = feasibilityText(row.feasibility);
    row.confidence_text = confidenceText(row.confidence);
    return row;
  }


  // The way an input or consumable came by (decisions 2026-11-16 §1/§3):
  // farmed by its source or bought, with what each way costs a unit.
  function routeText(route) {
    var way = route.route === 'farm' ? 'farm ' + (route.source || '?') : 'buy';
    var costs = [];
    if (route.farm_coins_per_unit !== null && route.farm_coins_per_unit !== undefined) costs.push('farm ' + number(route.farm_coins_per_unit) + 'gp');
    else if (route.farm_ticks_per_unit !== null && route.farm_ticks_per_unit !== undefined) costs.push('farm ' + number(route.farm_ticks_per_unit) + ' ticks');
    if (route.buy_coins_per_unit !== null && route.buy_coins_per_unit !== undefined) costs.push('buy ' + number(route.buy_coins_per_unit) + 'gp');
    return (route.display || route.item) + ' (' + route.cause + '): ' + way + (costs.length ? ' [' + costs.join(', ') + ']' : '');
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
    if (row.routes && row.routes.length) {
      html += '<div class="muted">ways: ' + row.routes.map(function (route) { return escapeHtml(routeText(route)); }).join('; ') + '</div>';
    }
    if (row.other_xp_6000 && Object.keys(row.other_xp_6000).length) html += '<div class="muted">other XP/6000: ' + escapeHtml(xpText(row.other_xp_6000)) + '</div>';
    if (row.quest_xp > 0) html += '<div class="muted">quest XP ' + number(row.quest_xp) + ', measured duration ' + ticksText(row.quest_ticks) + '</div>';
    html += '<div class="muted">conditions: ' + conditionsHtml(row.conditions) + '</div>';
    html += '<div class="muted">for this profile: ' + escapeHtml(feasibilityText(row.feasibility)) + '</div>';
    return html;
  }

  function start(kind) {
    var state = { payload: null, rows: [], open: {} };
    var columns = kind === 'money' ? MONEY_COLS : TRAINING_COLS;
    var sorter = (typeof TableSort !== 'undefined') ? TableSort : null;

    function value(id) { var node = document.getElementById(id); return node ? node.value : ''; }

    function render() {
      if (!state.payload) return;
      var options = { search: value('search'), feasibility: value('feasibility'), limit: parseInt(value('limit'), 10) || 0 };
      var rows = visibleRows(state.rows, options);
      document.getElementById('summary').textContent = state.rows.length + ' line(s), ' + rows.length + ' shown · ' + freshnessText(state.payload) +
        (state.payload.market_wait_ticks ? ' · market wait ' + Math.round(state.payload.market_wait_ticks) + ' ticks/lot' : ' · market wait unmeasured');
      var keyOf = function (row) { return [row.family, row.skill, row.method, row.style, row.site, row.tool, row.band, row.supply].join('|'); };
      var rowAttrs = function (row) { return { 'data-key': keyOf(row), class: row.feasibility && row.feasibility.state === 'infeasible' ? 'line locked' : 'line' }; };
      // onSort: a header click re-runs this render, so sorted lines keep
      // their click handlers and open details.
      if (sorter) sorter.render('guide', columns, rows, sortOptions(rowAttrs, render));
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
      var filter = {};
      QUERY_KEYS.forEach(function (key) { filter[key] = value(key); });
      document.getElementById('summary').textContent = 'loading…';
      fetch('/api/guides/' + kind + '?' + queryOf(filter)).then(function (response) { return response.json(); }).then(function (body) {
        if (!body.ok) {
          document.getElementById('summary').textContent = body.error || 'error';
          var delay = retryDelay(body);
          if (delay) setTimeout(load, delay);
          return;
        }
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
    QUERY_KEYS.forEach(function (id) { var node = document.getElementById(id); if (node) node.onchange = load; });
    ['search', 'feasibility', 'limit'].forEach(function (id) { var node = document.getElementById(id); if (node) node.oninput = render; if (node) node.onchange = render; });
    load();
    setInterval(load, 60000);
  }

  // Options of the guide table: a header click (sort) must re-run the
  // guide render, or sorted lines lose their click-to-unfold handlers.
  function sortOptions(rowAttrs, rerender) {
    return { rowAttrs: rowAttrs, onSort: rerender };
  }

  // Whether each guide (money, training) renders `key` through a formatter.
  function columnFormats(key) {
    return [MONEY_COLS, TRAINING_COLS].map(function (cols) { return cols.some(function (col) { return col.key === key && typeof col.fmt === 'function'; }); });
  }

  return { start: start, sortOptions: sortOptions, confidenceText: confidenceText, confidenceHtml: confidenceHtml, columnFormats: columnFormats, queryOf: queryOf, visibleRows: visibleRows, conditionsText: conditionsText, conditionsHtml: conditionsHtml, feasibilityText: feasibilityText, xpText: xpText, number: number, routeText: routeText, freshnessText: freshnessText, retryDelay: retryDelay };
})();

if (typeof module !== 'undefined' && module.exports) {
  module.exports = GuidesView;
} else if (typeof document !== 'undefined' && document.body && document.body.getAttribute('data-guide')) {
  GuidesView.start(document.body.getAttribute('data-guide'));
}
