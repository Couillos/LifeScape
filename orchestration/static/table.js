// Shared sortable-table renderer (D46): every dashboard table sorts by
// header click (up/down arrow), numeric or alphabetic, missing values
// last in both directions, sort state kept across refreshes. No
// dependency. The rates page and the lives page (P1.5) share it: pass an
// explicit column list, rows stay plain objects.
//
//   TableSort.render(id, columns, rows, options)
//     columns: [{key, label, kind}]  kind: 'num' | 'str' | 'auto'
//     rows: [{key: value, ...}]     values may be null/undefined/'' (missing)
//     fmt (optional per column): function (value, row) -> display HTML;
//       sorting always uses the raw value, never the formatted text.
//     options (optional): {rowAttrs, onSort}
//       rowAttrs: function (row) -> {attribute: value} on the <tr>
//         (values escaped; empty/null values dropped), e.g. data-* + class.
//       onSort: function () called after a header click instead of the
//         default re-render (paged tables re-sort the whole list first).
//   TableSort.sortedRows(id, columns, rows): rows in the table's current
//     sort order (input order while no header was clicked).
var TableSort = (function () {
  var state = {}; // table id -> {key, dir}

  function isMissing(value) {
    return value === null || value === undefined || value === '';
  }

  function kindOf(column, rows) {
    if (column.kind === 'num' || column.kind === 'str') return column.kind;
    for (var i = 0; i < rows.length; i++) {
      var value = rows[i][column.key];
      if (isMissing(value)) continue;
      if (typeof value !== 'number') return 'str';
    }
    return 'num';
  }

  // Pure comparator core (node-testable): missing always last, numeric
  // or alphabetic on the raw values, direction applies to present values
  // only.
  function compareValues(left, right, numeric, dir) {
    var leftMissing = isMissing(left);
    var rightMissing = isMissing(right);
    if (leftMissing && rightMissing) return 0;
    if (leftMissing) return 1;
    if (rightMissing) return -1;
    var order;
    if (numeric) {
      order = left - right;
    } else {
      var a = String(left);
      var b = String(right);
      order = a < b ? -1 : a > b ? 1 : 0;
    }
    return order * dir;
  }

  function sortData(columns, rows, key, dir) {
    var column = null;
    for (var i = 0; i < columns.length; i++) {
      if (columns[i].key === key) column = columns[i];
    }
    var data = rows.slice();
    if (!column) return data;
    var numeric = kindOf(column, data) === 'num';
    data.sort(function (x, y) {
      return compareValues(x[key], y[key], numeric, dir);
    });
    return data;
  }

  function escapeHtml(text) {
    return String(text).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }

  // Opening <tr> with escaped attributes; missing values are dropped.
  function rowOpenTag(attrs) {
    var html = '<tr';
    for (var name in attrs) {
      if (!Object.prototype.hasOwnProperty.call(attrs, name) || isMissing(attrs[name])) continue;
      html += ' ' + name + '="' + escapeHtml(attrs[name]) + '"';
    }
    return html + '>';
  }

  function stateOf(id) {
    return state[id] || (state[id] = { key: null, dir: 1 });
  }

  function sortedRows(id, columns, rows) {
    var current = stateOf(id);
    return current.key ? sortData(columns, rows, current.key, current.dir) : rows.slice();
  }

  function render(id, columns, rows, options) {
    var table = document.getElementById(id);
    if (!table) return;
    var settings = options || {};
    var current = stateOf(id);
    var data = sortedRows(id, columns, rows);
    var head = '<tr>';
    for (var i = 0; i < columns.length; i++) {
      var column = columns[i];
      var arrow = '';
      if (current.key === column.key) arrow = current.dir === 1 ? ' ▲' : ' ▼';
      head += '<th data-key="' + column.key + '" class="' + (column.kind === 'num' ? '' : 'left') + '">' +
        escapeHtml(column.label) + arrow + '</th>';
    }
    head += '</tr>';
    var body = '';
    for (var r = 0; r < data.length; r++) {
      body += settings.rowAttrs ? rowOpenTag(settings.rowAttrs(data[r])) : '<tr>';
      for (var c = 0; c < columns.length; c++) {
        var cell = data[r][columns[c].key];
        var html = columns[c].fmt ? columns[c].fmt(cell, data[r]) : escapeHtml(isMissing(cell) ? '-' : cell);
        body += '<td class="' + (columns[c].kind === 'num' ? '' : 'left') + '">' + html + '</td>';
      }
      body += '</tr>';
    }
    table.innerHTML = head + (body || '<tr><td class="left muted" colspan="' + columns.length + '">-</td></tr>');
    var headers = table.getElementsByTagName('th');
    for (var h = 0; h < headers.length; h++) {
      (function (header) {
        header.style.cursor = 'pointer';
        header.onclick = function () {
          var key = header.getAttribute('data-key');
          if (current.key === key) {
            current.dir = -current.dir;
          } else {
            current.key = key;
            current.dir = 1;
          }
          if (settings.onSort) settings.onSort();
          else render(id, columns, rows, options);
        };
      })(headers[h]);
    }
  }

  return { render: render, sortData: sortData, sortedRows: sortedRows, rowOpenTag: rowOpenTag, _state: state };
})();

if (typeof module !== 'undefined' && module.exports) module.exports = TableSort;
