// Rates front (user 2026-11-17: the measures view listed ~18 000 flow
// rows, unusable). The API aggregates (rates_view, web_rates_view): one
// row per measured activity (method × style; or per exact key: site ×
// tool × band), filtered, sorted and paged server side, over an index of
// the live snapshot it caches (no recompute per request).
// Columns: runs, age, confidence (measured = n >= 3, else estimated:
// thin, the brain reads the prior), main products and consumption per
// 6000 work ticks, derived XP per skill, net value at the current price,
// failure rate. A click on a row unfolds its detail (every flow, rare
// pickups folded into "other drops", units, XP) and its keys as sub-rows
// (each unfolds the same way). Trading and engaged bots below, folded.
// Sorting rides the shared TableSort helper (table.js): a header click
// asks the server for that order (the page is a slice of the sorted
// rows). Read-only dashboard page: bots never read it.
var RatesView = (function () {
  var PAGE_SIZES = [25, 50, 100, 200];
  var DEFAULT_SORT = { key: 'runs', dir: -1 };

  function escapeHtml(text) {
    return String(text === null || text === undefined ? '' : text)
      .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }

  function number(value) {
    if (value === null || value === undefined || !isFinite(value)) return '-';
    var abs = Math.abs(value);
    if (abs >= 1e6) return (Math.round(value / 1e5) / 10) + 'M';
    if (abs >= 1e4) return (Math.round(value / 100) / 10) + 'k';
    return String(Math.round(value * 10) / 10);
  }

  function bandText(band, last) {
    if (!(band > 0)) return '-';
    var top = last > band ? last : band;
    return band + '-' + (top + 4);
  }

  function ageText(secs) {
    if (!isFinite(secs) || secs <= 0) return '-';
    if (secs < 3600) return Math.round(secs / 60) + ' min';
    if (secs < 172800) return Math.round(secs / 360) / 10 + ' h';
    return Math.round(secs / 8640) / 10 + ' d';
  }

  function percent(value) {
    return isFinite(value) && value !== null ? Math.round(value * 100) + '%' : '-';
  }

  // "measured", "estimated (thin)" or "measured 3/5 keys": plain text.
  function confidenceText(row) {
    if (!row) return '-';
    var keys = row.keys || 0;
    var measured = row.measured_keys || 0;
    if (measured === 0) return 'estimated (thin)';
    if (keys > 1 && measured < keys) return 'measured ' + measured + '/' + keys + ' keys';
    return 'measured';
  }

  function confidenceHtml(row) {
    var text = escapeHtml(confidenceText(row));
    return row && row.measured_keys === 0 ? '<span class="prior">' + text + '</span>' : text;
  }

  // "Bones 40, Feather 100": headline lines (item and units per 6000).
  function flowsText(lines) {
    if (!lines || !lines.length) return '-';
    return lines.map(function (line) { return line.item + ' ' + number(Math.abs(line.per_6000)); }).join(', ');
  }

  // "strength 5000, hitpoints 1700": a skill → XP map, most first.
  function xpText(map) {
    var keys = Object.keys(map || {}).filter(function (skill) { return map[skill] > 0.05; });
    keys.sort(function (left, right) { return map[right] - map[left]; });
    return keys.map(function (skill) { return skill + ' ' + number(map[skill]); }).join(', ') || '-';
  }

  // Query string of the filters, the sort and the page (empty values out).
  function queryOf(filters, sort, offset, limit) {
    var parts = [];
    var add = function (key, value) {
      if (value === undefined || value === null || value === false) return;
      var text = String(value).trim();
      if (text !== '') parts.push(key + '=' + encodeURIComponent(text));
    };
    add('group', filters.group);
    add('q', filters.q);
    add('skill', filters.skill);
    add('family', filters.family);
    add('band', filters.band);
    if (filters.measured) add('measured', '1');
    if (filters.min_runs > 0) add('min_runs', filters.min_runs);
    if (sort && sort.key) {
      add('sort', sort.key);
      add('dir', sort.dir === 1 ? 'asc' : 'desc');
    }
    if (offset > 0) add('offset', offset);
    add('limit', limit);
    return parts.join('&');
  }

  // "rows 51-100 of 86 (from 187) · page 2/2": the counts of a page.
  function countText(payload) {
    if (!payload) return '';
    var shown = (payload.rows || []).length;
    var first = shown ? payload.offset + 1 : 0;
    var unit = payload.group === 'key' ? 'key(s)' : 'activit(ies)';
    var pages = Math.max(1, Math.ceil((payload.matched || 0) / (payload.limit || 1)));
    var page = Math.floor((payload.offset || 0) / (payload.limit || 1)) + 1;
    return unit + ' ' + first + '-' + (payload.offset + shown) + ' of ' + payload.matched + ' matching (' + payload.total + ' in all) · page ' + page + '/' + pages;
  }

  // Server answer time and index age.
  function timingText(payload) {
    if (!payload) return '';
    return 'answered in ' + number(payload.took_ms) + ' ms · index ' + Math.round((payload.index_age_ms || 0) / 1000) + ' s old (built in ' + number(payload.index_build_ms) + ' ms)';
  }

  // Stable id of a row (activity, or key with site, tool, band).
  function rowId(row) {
    return [row.method, row.style, row.site || '', row.tool || '', row.site ? row.band : ''].join('|');
  }

  // Detail query of a row (a key row narrows to its key).
  function detailQuery(row, filters) {
    var parts = ['method=' + encodeURIComponent(row.method), 'style=' + encodeURIComponent(row.style || '')];
    if (row.site !== null && row.site !== undefined) {
      parts.push('site=' + encodeURIComponent(row.site));
      parts.push('tool=' + encodeURIComponent(row.tool || ''));
      parts.push('band=' + row.band);
    }
    var shared = queryOf({ q: filters.q, skill: filters.skill, family: filters.family, measured: filters.measured, min_runs: filters.min_runs }, null, 0, null);
    return parts.join('&') + (shared ? '&' + shared : '');
  }

  function methodHtml(value, row) {
    return escapeHtml(value) + (row.style ? ' <span class="muted">' + escapeHtml(row.style) + '</span>' : '');
  }

  // Columns of a group: activity rows show their spread (keys, sites,
  // bands), key rows their site, tool and band. Column keys are the
  // server's sort keys.
  function columnsFor(group) {
    var head = [
      { key: 'method', label: 'activity', kind: 'str', fmt: methodHtml },
      { key: 'family', label: 'family', kind: 'str' },
    ];
    var spread = group === 'key' ? [
      { key: 'site', label: 'site', kind: 'str', fmt: function (v) { return v ? escapeHtml(v) : '<span class="muted">none</span>'; } },
      { key: 'tool', label: 'tool', kind: 'str', fmt: function (v) { return v ? escapeHtml(v) : '-'; } },
      { key: 'band', label: 'band', kind: 'num', fmt: function (v) { return bandText(v); } },
    ] : [
      { key: 'band', label: 'bands', kind: 'num', fmt: function (v, row) { return bandText(v, row.band_max); } },
      { key: 'keys', label: 'keys', kind: 'num', fmt: function (v, row) { return v + (row.sites > 1 ? ' <span class="muted">(' + row.sites + ' sites)</span>' : ''); } },
    ];
    var tail = [
      { key: 'runs', label: 'runs', kind: 'num' },
      { key: 'age_secs', label: 'age', kind: 'num', fmt: ageText },
      { key: 'confidence', label: 'confidence', kind: 'num', fmt: function (v, row) { return confidenceHtml(row); } },
      { key: 'product_value_6000', label: 'main products /6000', kind: 'num', fmt: function (v, row) { return escapeHtml(flowsText(row.products)); } },
      { key: 'consumption_value_6000', label: 'main consumption /6000', kind: 'num', fmt: function (v, row) { return escapeHtml(flowsText(row.consumption)); } },
      { key: 'xp_total', label: 'XP/6000', kind: 'num', fmt: function (v, row) { return escapeHtml(xpText(row.xp)); } },
      { key: 'net_value_6000', label: 'value/6000', kind: 'num', fmt: number },
      { key: 'failure_rate', label: 'fail', kind: 'num', fmt: percent },
    ];
    return head.concat(spread, tail);
  }

  // Unfolded detail: every flow (with its value), units, XP, then the
  // keys as sub-rows (`openKeys`: id -> detail payload of an unfolded key).
  function detailHtml(detail, openKeys) {
    if (!detail) return '<span class="muted">loading…</span>';
    if (!detail.ok) return '<span class="prior">' + escapeHtml(detail.error || 'error') + '</span>';
    var summary = detail.summary;
    var html = '<table class="detail"><tr><th class="left">object</th><th class="left">cause</th><th>/6000</th><th>value/6000</th></tr>';
    (summary.flows || []).forEach(function (line) {
      html += '<tr><td class="left">' + escapeHtml(line.item) + '</td><td class="left">' + escapeHtml(line.cause) + '</td><td>' + number(line.per_6000) + '</td><td>' + number(line.value_6000) + '</td></tr>';
    });
    html += '</table>';
    var units = (summary.units || []).map(function (unit) { return escapeHtml(unit.kind + ' ' + unit.subject + ' ' + number(unit.per_6000)); });
    html += '<div class="muted">XP-bearing units/6000: ' + (units.length ? units.join(', ') : '-') + '</div>';
    html += '<div class="muted">XP/6000 (derived): ' + escapeHtml(xpText(summary.xp)) + ' · value/6000 ' + number(summary.net_value_6000) +
      ' · runs ' + summary.runs + ' · fail ' + percent(summary.failure_rate) + ' · rejected ' + (summary.rejected || 0) +
      (summary.dispersion !== null && summary.dispersion !== undefined ? ' · dispersion ' + number(summary.dispersion) : '') + '</div>';
    var keys = detail.keys || [];
    if (keys.length > 1) {
      html += '<table class="detail keys"><tr><th class="left">site</th><th class="left">tool</th><th>band</th><th>runs</th><th>age</th><th class="left">confidence</th><th class="left">main products</th><th class="left">main consumption</th><th class="left">XP/6000</th><th>value/6000</th><th>fail</th></tr>';
      keys.forEach(function (key) {
        var id = rowId(key);
        html += '<tr class="subkey" data-key="' + escapeHtml(id) + '"><td class="left">' + (key.site ? escapeHtml(key.site) : '<span class="muted">none</span>') + '</td><td class="left">' + escapeHtml(key.tool || '-') + '</td><td>' + bandText(key.band) +
          '</td><td>' + key.runs + '</td><td>' + ageText(key.age_secs) + '</td><td class="left">' + confidenceHtml(key) + '</td><td class="left">' + escapeHtml(flowsText(key.products)) + '</td><td class="left">' +
          escapeHtml(flowsText(key.consumption)) + '</td><td class="left">' + escapeHtml(xpText(key.xp)) + '</td><td>' + number(key.net_value_6000) + '</td><td>' + percent(key.failure_rate) + '</td></tr>';
        if (openKeys && openKeys.hasOwnProperty(id)) html += '<tr><td class="left" colspan="11">' + detailHtml(openKeys[id], null) + '</td></tr>';
      });
      html += '</table>';
    }
    return html;
  }

  function start() {
    var state = { payload: null, offset: 0, open: {}, details: {}, keyDetails: {} };
    var sorter = (typeof TableSort !== 'undefined') ? TableSort : null;
    if (sorter) sorter._state.rates = { key: DEFAULT_SORT.key, dir: DEFAULT_SORT.dir };

    function node(id) { return document.getElementById(id); }
    function value(id) { var found = node(id); return found ? found.value : ''; }

    function filters() {
      return {
        group: value('group') || 'activity',
        q: value('search'),
        skill: value('skill'),
        family: value('family'),
        band: value('band'),
        measured: node('measured') ? node('measured').checked : true,
        min_runs: parseInt(value('min_runs'), 10) || 0,
      };
    }

    function limit() { return parseInt(value('limit'), 10) || 50; }

    function fillSelect(id, label, options, render) {
      var select = node(id);
      if (!select) return;
      var current = select.value;
      select.innerHTML = '<option value="">' + label + '</option>' + options.map(function (option) {
        var text = render ? render(option) : option;
        return '<option value="' + escapeHtml(option) + '"' + (String(option) === current ? ' selected' : '') + '>' + escapeHtml(text) + '</option>';
      }).join('');
    }

    function render() {
      var payload = state.payload;
      if (!payload) return;
      node('summary').textContent = countText(payload);
      node('timing').textContent = timingText(payload);
      node('prev').disabled = payload.offset <= 0;
      node('next').disabled = payload.offset + (payload.rows || []).length >= payload.matched;
      var columns = columnsFor(payload.group);
      var rows = payload.rows || [];
      var options = { rowAttrs: function (row) { return { 'data-key': rowId(row), class: 'line' }; }, onSort: function () { state.offset = 0; load(); } };
      if (sorter) sorter.render('rates', columns, rows, options);
      var table = node('rates');
      var byId = {};
      rows.forEach(function (row) { byId[rowId(row)] = row; });
      Array.prototype.forEach.call(table.querySelectorAll('tr.line'), function (line) {
        var id = line.getAttribute('data-key');
        line.style.cursor = 'pointer';
        line.onclick = function () { toggle(id, byId[id]); };
        if (!state.open[id]) return;
        var detail = document.createElement('tr');
        detail.innerHTML = '<td class="left" colspan="' + columns.length + '">' + detailHtml(state.details[id], state.keyDetails[id]) + '</td>';
        line.parentNode.insertBefore(detail, line.nextSibling);
        Array.prototype.forEach.call(detail.querySelectorAll('tr.subkey'), function (sub) {
          var keyId = sub.getAttribute('data-key');
          sub.style.cursor = 'pointer';
          sub.onclick = function (event) { event.stopPropagation(); toggleKey(id, keyId); };
        });
      });
    }

    function fetchJson(url, done) {
      fetch(url).then(function (response) { return response.json(); }).then(done).catch(function () { done({ ok: false, error: 'daemon unreachable' }); });
    }

    function toggle(id, row) {
      state.open[id] = !state.open[id];
      if (state.open[id] && row) {
        state.details[id] = null;
        fetchJson('/api/rates/detail?' + detailQuery(row, filters()), function (body) { state.details[id] = body; render(); });
      }
      render();
    }

    function toggleKey(parentId, keyId) {
      var open = state.keyDetails[parentId] || (state.keyDetails[parentId] = {});
      if (open.hasOwnProperty(keyId)) {
        delete open[keyId];
        render();
        return;
      }
      var parent = state.details[parentId];
      var key = parent && parent.keys ? parent.keys.filter(function (candidate) { return rowId(candidate) === keyId; })[0] : null;
      if (!key) return;
      open[keyId] = null;
      fetchJson('/api/rates/detail?' + detailQuery(key, filters()), function (body) { open[keyId] = body; render(); });
      render();
    }

    function load() {
      var sort = sorter && sorter._state.rates && sorter._state.rates.key ? sorter._state.rates : DEFAULT_SORT;
      fetchJson('/api/rates/activities?' + queryOf(filters(), sort, state.offset, limit()), function (body) {
        if (!body.ok) {
          node('summary').textContent = body.error || 'error';
          return;
        }
        var facets = body.facets || {};
        fillSelect('skill', 'all skills', facets.skills || []);
        fillSelect('family', 'all families', facets.families || []);
        fillSelect('band', 'all bands', facets.bands || [], function (band) { return 'levels ' + bandText(band); });
        state.payload = body;
        render();
      });
    }

    // A new filter starts again at the first page, details folded.
    function refilter() {
      state.offset = 0;
      state.open = {};
      state.details = {};
      state.keyDetails = {};
      load();
    }

    function loadTrading() {
      fetchJson('/api/rates/trading', function (body) {
        if (!body.ok || !sorter) return;
        sorter.render('trading', TRADING_COLS, body.trading || []);
        var engaged = body.engaged || {};
        sorter.render('engaged', ENGAGED_COLS, Object.keys(engaged).sort().map(function (name) { return { script: name, bots: engaged[name] }; }));
      });
    }

    var sizes = node('limit');
    if (sizes) sizes.innerHTML = PAGE_SIZES.map(function (size) { return '<option' + (size === 50 ? ' selected' : '') + '>' + size + '</option>'; }).join('');
    var typing = null;
    if (node('search')) node('search').oninput = function () { clearTimeout(typing); typing = setTimeout(refilter, 250); };
    ['group', 'skill', 'family', 'band', 'measured', 'min_runs', 'limit'].forEach(function (id) { if (node(id)) node(id).onchange = refilter; });
    node('prev').onclick = function () { state.offset = Math.max(0, state.offset - limit()); load(); };
    node('next').onclick = function () { state.offset += limit(); load(); };
    load();
    loadTrading();
    setInterval(load, 60000);
    setInterval(loadTrading, 60000);
  }

  var TRADING_COLS = [
    { key: 'item', label: 'item', kind: 'str' },
    { key: 'profit_per_6000', label: 'profit/6000', kind: 'num', fmt: number },
    { key: 'deals_settled', label: 'settled', kind: 'num' },
    { key: 'fill_rate', label: 'fill', kind: 'num', fmt: percent },
    { key: 'stock_value', label: 'stock', kind: 'num', fmt: number },
  ];
  var ENGAGED_COLS = [
    { key: 'script', label: 'script', kind: 'str' },
    { key: 'bots', label: 'bots', kind: 'num' },
  ];

  return { start: start, queryOf: queryOf, countText: countText, timingText: timingText, confidenceText: confidenceText, confidenceHtml: confidenceHtml, flowsText: flowsText, xpText: xpText, bandText: bandText, ageText: ageText, rowId: rowId, detailQuery: detailQuery, columnsFor: columnsFor, detailHtml: detailHtml, number: number };
})();

if (typeof module !== 'undefined' && module.exports) {
  module.exports = RatesView;
} else if (typeof document !== 'undefined' && document.getElementById('rates')) {
  RatesView.start();
}
