// Brain tab of the Lives page ("Cerveau"): one bot's long-term wishes and
// every sub-task they imply, as a layered SVG graph (vanilla, no library).
//
// Columns left → right: ambition → milestones → current goal → task DAG.
// A task's column is its longest dependency path from a top task (one no
// other task depends on): a task always sits right of every task that
// needs it, so prerequisites and filed sub-tasks read as "to get X, first
// Y". Method + costed alternatives ride inside each task box as chips.
// Orange edges = sub-tasks a blocked task filed. Badges = wishes, open
// orders and deals on the task whose object they serve (matched by debug
// name). Done subtrees collapse (click a box to toggle). Pan = drag,
// zoom = wheel (viewBox), "Ajuster" refits.
//
// Pure part (BrainLayout.*) is node-testable: `node brain.test.js`.
var BrainLayout = (function () {
  var COL_GAP = 70;
  var ROW_GAP = 14;
  var MARGIN = 20;
  var WIDTHS = { ambition: 220, milestone: 200, goal: 200, node: 220 };

  function nodeIndex(nodes) {
    var byId = {};
    nodes.forEach(function (node) { byId[node.id] = node; });
    return byId;
  }

  // Parents of each node: the nodes that list it in `deps`.
  function parentsOf(nodes) {
    var byId = nodeIndex(nodes);
    var parents = {};
    nodes.forEach(function (node) { parents[node.id] = []; });
    nodes.forEach(function (node) {
      (node.deps || []).forEach(function (dep) {
        if (byId[dep] && dep !== node.id && parents[dep].indexOf(node.id) < 0) parents[dep].push(node.id);
      });
    });
    return parents;
  }

  // Rank = longest dependency path from a top node (no parent): 0 for a
  // top node, 1 + max(parent rank) otherwise. A cycle (corrupt file) is
  // cut at the revisited edge, never an infinite loop.
  function ranks(nodes) {
    var parents = parentsOf(nodes);
    var memo = {};
    var visiting = {};
    function rankOf(nodeId) {
      if (memo[nodeId] !== undefined) return memo[nodeId];
      if (visiting[nodeId]) return 0;
      visiting[nodeId] = true;
      var best = 0;
      (parents[nodeId] || []).forEach(function (parent) { best = Math.max(best, rankOf(parent) + 1); });
      visiting[nodeId] = false;
      memo[nodeId] = best;
      return best;
    }
    var out = {};
    nodes.forEach(function (node) { out[node.id] = rankOf(node.id); });
    return out;
  }

  // Every node reachable through deps from `rootId` (itself excluded).
  function subtree(rootId, byId) {
    var seen = {};
    var stack = (byId[rootId] && byId[rootId].deps || []).slice();
    while (stack.length) {
      var next = stack.pop();
      if (seen[next] || !byId[next] || next === rootId) continue;
      seen[next] = true;
      stack = stack.concat(byId[next].deps || []);
    }
    return Object.keys(seen);
  }

  // Done nodes whose whole subtree is done and non-empty: the default
  // collapsed set (the past stays one box until clicked open).
  function collapsibleDone(nodes) {
    var byId = nodeIndex(nodes);
    return nodes.filter(function (node) {
      if (node.status !== 'done') return false;
      var below = subtree(node.id, byId);
      return below.length > 0 && below.every(function (belowId) { return byId[belowId].status === 'done'; });
    }).map(function (node) { return node.id; });
  }

  // Nodes left visible once `collapsed` boxes hide their subtrees: a node
  // is hidden when it has parents and every parent is collapsed or hidden.
  // `hideAbandoned` drops abandoned nodes (and anything only they reach).
  function visibleNodes(nodes, collapsed, hideAbandoned) {
    var parents = parentsOf(nodes);
    var rank = ranks(nodes);
    var order = nodes.slice().sort(function (left, right) { return rank[left.id] - rank[right.id]; });
    var hidden = {};
    var shut = {};
    (collapsed || []).forEach(function (shutId) { shut[shutId] = true; });
    order.forEach(function (node) {
      if (hideAbandoned && node.status === 'abandoned') { hidden[node.id] = true; return; }
      var ups = parents[node.id];
      if (!ups.length) return;
      var open = ups.some(function (parent) { return !hidden[parent] && !shut[parent]; });
      if (!open) hidden[node.id] = true;
    });
    return nodes.filter(function (node) { return !hidden[node.id]; });
  }

  function nodeHeight(node) {
    var chips = Math.min((node.alternatives || []).length, 3);
    return 46 + chips * 13;
  }

  function idNumber(nodeId) {
    var parsed = parseInt(String(nodeId).replace(/^n/, ''), 10);
    return isNaN(parsed) ? 0 : parsed;
  }

  // Full layout. payload: /api/brain shape. options: {collapsed: [ids],
  // hideAbandoned}. Returns {boxes, edges, width, height}; boxes carry
  // {id, type, col, x, y, w, h, data}. Columns are stacked top-down with
  // a fixed gap (no overlap by construction); task columns are ordered by
  // the mean row of their parents (fewer crossings).
  function layout(payload, options) {
    options = options || {};
    var boxes = [];
    var edges = [];
    var columns = [];
    var ambition = payload.ambition && typeof payload.ambition === 'object' ? payload.ambition : null;
    var goalId = 'goal';
    if (ambition) {
      columns.push([{ id: 'ambition', type: 'ambition', h: 74, data: ambition }]);
      var milestones = (ambition.milestones || []).map(function (stone) {
        return { id: 'm:' + stone.id, type: 'milestone', h: 48, data: stone };
      });
      if (milestones.length) columns.push(milestones);
      milestones.forEach(function (stone) { edges.push({ from: 'ambition', to: stone.id, kind: 'plan' }); });
    }
    columns.push([{ id: goalId, type: 'goal', h: 58, data: payload.goal || {} }]);
    var origin = payload.goal && payload.goal.origin_milestone;
    if (ambition && origin) edges.push({ from: 'm:' + origin, to: goalId, kind: 'origin' });

    var shown = visibleNodes(payload.nodes || [], options.collapsed || [], options.hideAbandoned);
    var rank = ranks(shown);
    var parents = parentsOf(shown);
    var taskCols = [];
    shown.forEach(function (node) {
      var level = rank[node.id];
      while (taskCols.length <= level) taskCols.push([]);
      taskCols[level].push({ id: node.id, type: 'node', h: nodeHeight(node), data: node });
    });
    var firstTask = columns.length;
    taskCols.forEach(function (column) { columns.push(column); });

    var rowOf = {};
    var shownIds = {};
    shown.forEach(function (node) { shownIds[node.id] = true; });
    function sortKey(item) {
      var ups = parents[item.id] || [];
      if (!ups.length) return idNumber(item.id) / 1e6;
      var sum = 0;
      ups.forEach(function (parent) { sum += rowOf[parent] || 0; });
      return sum / ups.length;
    }
    var left = MARGIN;
    var height = 0;
    columns.forEach(function (items, colIndex) {
      if (colIndex >= firstTask) {
        items.sort(function (first, second) {
          var diff = sortKey(first) - sortKey(second);
          return diff !== 0 ? diff : idNumber(first.id) - idNumber(second.id);
        });
      }
      var width = 0;
      var top = MARGIN;
      items.forEach(function (item, rowIndex) {
        item.col = colIndex;
        item.w = WIDTHS[item.type];
        item.x = left;
        item.y = top;
        rowOf[item.id] = rowIndex;
        top += item.h + ROW_GAP;
        width = Math.max(width, item.w);
        boxes.push(item);
      });
      height = Math.max(height, top);
      left += width + COL_GAP;
    });
    shown.forEach(function (node) {
      if (!(parents[node.id] || []).length) edges.push({ from: goalId, to: node.id, kind: 'goal' });
      var filed = node.filed || [];
      (node.deps || []).forEach(function (dep) {
        if (!shownIds[dep] || dep === node.id) return;
        edges.push({ from: node.id, to: dep, kind: filed.indexOf(dep) >= 0 ? 'filed' : 'dep' });
      });
    });
    return { boxes: boxes, edges: edges, width: left - COL_GAP + MARGIN, height: height + MARGIN };
  }


  // Badges of one task: wishes, open orders and deals on its object.
  function badgesFor(node, payload) {
    var item = node.item;
    if (!item) return [];
    var out = [];
    (payload.wishes || []).forEach(function (wish) {
      if (wish.item === item) out.push({ kind: 'wish', text: 'souhait' });
    });
    (payload.orders || []).forEach(function (order) {
      if (order.item === item) out.push({ kind: order.side === 'sell' ? 'sell' : 'buy', text: (order.side === 'sell' ? 'vente ' : 'achat ') + order.remaining + '@' + order.price });
    });
    (payload.deals || []).forEach(function (deal) {
      if (deal.item === item) out.push({ kind: 'deal', text: 'deal ' + deal.status });
    });
    return out;
  }

  function escapeHtml(text) {
    return String(text === null || text === undefined ? '' : text)
      .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }

  return {
    parentsOf: parentsOf, ranks: ranks, collapsibleDone: collapsibleDone, visibleNodes: visibleNodes,
    layout: layout, badgesFor: badgesFor, escapeHtml: escapeHtml, nodeHeight: nodeHeight,
  };
})();

if (typeof module !== 'undefined' && module.exports) module.exports = BrainLayout;

// -- DOM view (browser only) ------------------------------------------------
var BrainView = (function () {
  if (typeof document === 'undefined') return null;
  var SVG = 'http://www.w3.org/2000/svg';
  // Same palette as the .st-* chips of life.html (fill, text).
  var STATUS = {
    pending: ['#333', '#ccc'], ready: ['#1d3f5e', '#9fd0ff'], active: ['#1d5e2a', '#c8ffd2'],
    done: ['#2c4a2c', '#9ccc9c'], blocked: ['#5e3c1d', '#ffd9a0'], failed: ['#5e1d1d', '#ffb0b0'],
    abandoned: ['#222', '#777'],
  };
  var EDGE = { dep: '#666', goal: '#888', plan: '#4a7', origin: '#ff0', filed: '#f90' };
  var BADGE = { wish: '#7a4fbf', buy: '#1d5e5e', sell: '#5e5e1d', deal: '#2a5ea8' };
  var account = null;
  var payload = null;
  var version = null;
  var dragged = false;    // last mousedown moved the view (no click toggle)
  var opened = {};        // ids the operator opened by hand
  var view = null;        // current viewBox {x, y, w, h}
  var full = null;        // fitted viewBox of the last layout
  var active = false;
  var timer = null;

  function svgNode(name, attrs, parent) {
    var node = document.createElementNS(SVG, name);
    for (var key in attrs) node.setAttribute(key, attrs[key]);
    if (parent) parent.appendChild(node);
    return node;
  }

  function text(parent, x, y, content, attrs) {
    var node = svgNode('text', Object.assign({ x: x, y: y, fill: '#eee', 'font-size': 11, 'font-family': 'Arial, Helvetica, sans-serif' }, attrs || {}), parent);
    node.textContent = content;
    return node;
  }

  function clip(content, max) {
    content = String(content || '');
    return content.length > max ? content.slice(0, max - 1) + '…' : content;
  }

  function statusColors(status) {
    return STATUS[status] || STATUS.pending;
  }

  // Milestone targets are objects ({skill, level} | {item, count, skill} |
  // {style} | {amount}), see planner/src/ambition.rs.
  function targetText(target) {
    if (target === null || target === undefined) return '?';
    if (typeof target !== 'object') return String(target);
    if (target.level !== undefined) return (target.skill || '?') + ' ' + target.level;
    if (target.item !== undefined) return target.count + ' × ' + target.item;
    if (target.style !== undefined) return 'équipement ' + target.style;
    if (target.amount !== undefined) return target.amount + ' pièces';
    return JSON.stringify(target);
  }

  function progressText(progress) {
    if (!progress || progress.length < 2 || !progress[1]) return '';
    return progress[0] + '/' + progress[1];
  }

  function tipFor(box) {
    var data = box.data || {};
    var lines = [];
    if (box.type === 'ambition') {
      lines.push('Ambition: ' + (data.label || data.kind || '?'));
      if (data.why) lines.push('Pourquoi: ' + data.why);
      if (data.groups && data.groups.length) lines.push('Groupes: ' + data.groups.join(', '));
      (data.revisions || []).slice(-3).forEach(function (rev) {
        lines.push('Révision @' + rev.tick + ': ' + rev.from + ' → ' + rev.to + (rev.why ? ' (' + rev.why + ')' : ''));
      });
    } else if (box.type === 'milestone') {
      lines.push('Jalon ' + data.id + ': ' + (data.label || ''));
      lines.push('Statut: ' + (data.status || '?') + ' · ' + (data.kind || '') + ' → ' + targetText(data.target));
      if (progressText(data.progress)) lines.push('Progrès: ' + progressText(data.progress));
      if (data.blocked_count) lines.push('Blocages: ' + data.blocked_count);
    } else if (box.type === 'goal') {
      lines.push('But courant: ' + (data.label || '?'));
      if (data.origin_milestone) lines.push('Issu du jalon ' + data.origin_milestone);
    } else {
      lines.push(data.id + ' · ' + data.label + ' · ' + data.status);
      if (progressText(data.progress)) lines.push('Progrès: ' + progressText(data.progress));
      if (data.block) lines.push('Bloqué: ' + data.block);
      if (data.detail && data.detail !== data.block) lines.push('Détail: ' + data.detail);
      if (data.last_error) lines.push('Dernière erreur: ' + data.last_error);
      lines.push('Échecs ' + data.failures + ' · blocages ' + data.blocks + ' · relances ' + data.revives +
        (data.spent_ticks ? ' · ' + data.spent_ticks + ' ticks' : ''));
      (data.alternatives || []).forEach(function (alt) {
        lines.push((alt.chosen ? '▸ ' : '  ') + alt.method + ' · ' + alt.est_ticks + 't' + (alt.est_coins ? ' · ' + alt.est_coins + 'c' : ''));
      });
      if (data.filed && data.filed.length) lines.push('Sous-tâches déposées: ' + data.filed.join(', '));
    }
    return lines;
  }

  function drawBox(layer, box) {
    var data = box.data || {};
    var group = svgNode('g', { 'data-id': box.id, cursor: box.type === 'node' ? 'pointer' : 'default' }, layer);
    var status = box.type === 'node' || box.type === 'milestone' ? data.status : (box.type === 'goal' ? 'active' : 'ready');
    var colors = statusColors(status);
    var stroke = box.type === 'goal' ? '#ff0' : (box.type === 'ambition' ? '#4a7' : '#444');
    svgNode('rect', { x: box.x, y: box.y, width: box.w, height: box.h, rx: 5, fill: colors[0], stroke: stroke, 'stroke-width': box.type === 'node' ? 1 : 1.5 }, group);
    var title;
    var sub;
    if (box.type === 'ambition') {
      title = clip(data.label || data.kind, 30);
      sub = clip(data.why || data.kind || '', 36);
    } else if (box.type === 'milestone') {
      title = clip(data.label, 28);
      sub = (data.status || '') + (progressText(data.progress) ? ' · ' + progressText(data.progress) : '') + (data.blocked_count ? ' · ⚠' + data.blocked_count : '');
    } else if (box.type === 'goal') {
      title = 'But: ' + clip(data.label, 24);
      sub = data.origin_milestone ? 'jalon ' + data.origin_milestone : 'sans ambition';
    } else {
      title = data.id + ' ' + clip(data.label, 24);
      sub = data.status + (progressText(data.progress) ? ' · ' + progressText(data.progress) : '') +
        (data.failures ? ' · ✗' + data.failures : '') + (data.blocks ? ' · ⛔' + data.blocks : '');
    }
    text(group, box.x + 8, box.y + 16, title, { fill: colors[1], 'font-weight': 700, 'text-decoration': status === 'abandoned' ? 'line-through' : 'none' });
    text(group, box.x + 8, box.y + 31, clip(sub, 38), { fill: '#bbb', 'font-size': 10 });
    if (box.type === 'node') {
      var alts = (data.alternatives || []).slice(0, 3);
      alts.forEach(function (alt, index) {
        var chipTop = box.y + 40 + index * 13;
        svgNode('rect', { x: box.x + 8, y: chipTop, width: box.w - 16, height: 11, rx: 5, fill: alt.chosen ? '#04A800' : '#111', opacity: alt.chosen ? 0.35 : 0.8 }, group);
        text(group, box.x + 12, chipTop + 9, clip(alt.method, 26) + ' · ' + alt.est_ticks + 't' + (alt.est_coins ? ' ' + alt.est_coins + 'c' : ''), { 'font-size': 9, fill: alt.chosen ? '#c8ffd2' : '#888' });
      });
      var badges = BrainLayout.badgesFor(data, payload);
      var badgeLeft = box.x + box.w;
      badges.slice(0, 3).forEach(function (badge) {
        var width = badge.text.length * 5.5 + 8;
        badgeLeft -= width + 3;
        svgNode('rect', { x: badgeLeft, y: box.y - 7, width: width, height: 13, rx: 6, fill: BADGE[badge.kind] || '#444' }, group);
        text(group, badgeLeft + 4, box.y + 3, badge.text, { 'font-size': 9 });
      });
      var collapsible = BrainLayout.collapsibleDone(payload.nodes || []).indexOf(data.id) >= 0;
      if (collapsible) {
        var closed = currentCollapsed().indexOf(data.id) >= 0;
        text(group, box.x + box.w - 16, box.y + 16, closed ? '+' : '−', { fill: '#ff0', 'font-weight': 700, 'font-size': 13 });
      }
      group.addEventListener('click', function () { toggle(data.id); });
    }
    group.addEventListener('mousemove', function (event) { showTip(event, tipFor(box)); });
    group.addEventListener('mouseleave', hideTip);
  }

  function boxAnchor(boxes, boxId, side) {
    var box = boxes.find(function (candidate) { return candidate.id === boxId; });
    if (!box) return null;
    return { x: box.x + (side === 'out' ? box.w : 0), y: box.y + Math.min(box.h / 2, 20) };
  }

  function drawEdges(layer, model) {
    model.edges.forEach(function (edge) {
      var from = boxAnchor(model.boxes, edge.from, 'out');
      var target = boxAnchor(model.boxes, edge.to, 'in');
      if (!from || !target) return;
      var bend = Math.max(30, (target.x - from.x) / 2);
      var path = 'M' + from.x + ',' + from.y + ' C' + (from.x + bend) + ',' + from.y + ' ' + (target.x - bend) + ',' + target.y + ' ' + target.x + ',' + target.y;
      svgNode('path', {
        d: path, fill: 'none', stroke: EDGE[edge.kind] || '#666',
        'stroke-width': edge.kind === 'filed' || edge.kind === 'origin' ? 2 : 1.2,
        'stroke-dasharray': edge.kind === 'plan' ? '4 3' : 'none', 'marker-end': 'url(#brain-arrow-' + edge.kind + ')',
      }, layer);
    });
  }

  function currentCollapsed() {
    var base = BrainLayout.collapsibleDone((payload && payload.nodes) || []);
    return base.filter(function (nodeId) { return !opened[nodeId]; });
  }

  function toggle(nodeId) {
    if (dragged) return;
    if (BrainLayout.collapsibleDone(payload.nodes || []).indexOf(nodeId) < 0) return;
    opened[nodeId] = !opened[nodeId];
    draw(false);
  }

  function showTip(event, lines) {
    var tip = document.getElementById('brain-tip');
    if (!tip) return;
    tip.innerHTML = lines.map(BrainLayout.escapeHtml).join('<br>');
    tip.style.display = 'block';
    tip.style.left = (event.clientX + 14) + 'px';
    tip.style.top = (event.clientY + 14) + 'px';
  }

  function hideTip() {
    var tip = document.getElementById('brain-tip');
    if (tip) tip.style.display = 'none';
  }

  function applyView(svg) {
    svg.setAttribute('viewBox', view.x + ' ' + view.y + ' ' + view.w + ' ' + view.h);
  }

  function draw(refit) {
    var svg = document.getElementById('brain-svg');
    if (!svg || !payload) return;
    while (svg.firstChild) svg.removeChild(svg.firstChild);
    var defs = svgNode('defs', {}, svg);
    Object.keys(EDGE).forEach(function (kind) {
      var marker = svgNode('marker', { id: 'brain-arrow-' + kind, viewBox: '0 0 10 10', refX: 9, refY: 5, markerWidth: 6, markerHeight: 6, orient: 'auto' }, defs);
      svgNode('path', { d: 'M0,0 L10,5 L0,10 z', fill: EDGE[kind] }, marker);
    });
    var hide = document.getElementById('brain-hide-abandoned');
    var model = BrainLayout.layout(payload, { collapsed: currentCollapsed(), hideAbandoned: hide && hide.checked });
    drawEdges(svgNode('g', {}, svg), model);
    var layer = svgNode('g', {}, svg);
    model.boxes.forEach(function (box) { drawBox(layer, box); });
    full = { x: 0, y: 0, w: Math.max(model.width, 400), h: Math.max(model.height, 200) };
    if (refit || !view) view = Object.assign({}, full);
    applyView(svg);
  }

  function panelHtml() {
    var esc = BrainLayout.escapeHtml;
    var out = [];
    if (payload.errors && payload.errors.length) {
      out.push('<h3>Sources en erreur</h3>' + payload.errors.map(function (error) { return '<div class="brain-err">' + esc(error) + '</div>'; }).join(''));
    }
    var live = payload.live || {};
    var levels = live.levels || {};
    out.push('<h3>Live</h3><div>combat ' + esc(live.combat === null || live.combat === undefined ? '?' : live.combat) +
      ' · ' + esc(live.coins === null || live.coins === undefined ? '?' : live.coins) + ' pièces</div>' +
      '<div class="muted">' + Object.keys(levels).sort().map(function (skill) { return esc(skill) + ' ' + levels[skill]; }).join(' · ') + '</div>' +
      '<div class="muted">porté: ' + (live.worn && live.worn.length ? live.worn.map(esc).join(', ') : '-') + '</div>');
    var bans = payload.banned_skills || [];
    out.push('<h3>Compétences bannies</h3>' + (bans.length ? bans.map(function (ban) {
      return '<div><span class="chip ' + (ban.active ? 'st-blocked' : 'st-done') + '">' + esc(ban.skill) + '</span> ' +
        (ban.active ? esc(ban.ticks_left) + ' ticks' + (ban.until_tick !== null && ban.until_tick !== undefined ? ' (→ tick ' + esc(ban.until_tick) + ')' : '') : 'expiré') +
        '<div class="alt">' + esc(ban.cause) + '</div></div>';
    }).join('') : '<div class="muted">aucune</div>'));
    var wishes = payload.wishes || [];
    out.push('<h3>Souhaits</h3>' + (wishes.length ? wishes.map(function (wish) {
      var bid = wish.bid ? ' · offre ' + esc(wish.bid.price) + (wish.bid.ceiling ? ' (plafond ' + esc(wish.bid.ceiling) + ')' : '') : ' · pas d\'offre';
      return '<div>' + esc(wish.item) + ' <span class="alt">' + esc(wish.slot) + ' +' + esc(wish.gain) + bid + '</span></div>';
    }).join('') : '<div class="muted">aucun</div>'));
    var orders = payload.orders || [];
    out.push('<h3>Ordres ouverts</h3>' + (orders.length ? orders.map(function (order) {
      return '<div>' + esc(order.side) + ' ' + esc(order.remaining) + ' × ' + esc(order.name || order.item) + ' @ ' + esc(order.price) +
        (order.ceiling ? ' <span class="alt">plafond ' + esc(order.ceiling) + '</span>' : '') + ' <span class="alt">' + esc(order.status) + '</span></div>';
    }).join('') : '<div class="muted">aucun</div>'));
    var deals = payload.deals || [];
    out.push('<h3>Deals</h3>' + (deals.length ? deals.map(function (deal) {
      return '<div>' + esc(deal.role) + ' ' + esc(deal.qty) + ' × ' + esc(deal.name || deal.item) + ' @ ' + esc(deal.price) +
        ' <span class="alt">avec ' + esc(deal.partner) + ' · ' + esc(deal.status) + '</span></div>';
    }).join('') : '<div class="muted">aucun</div>'));
    var history = (payload.goal_history || []).slice(-15).reverse();
    out.push('<h3>Historique des buts</h3>' + (history.length ? history.map(function (entry) {
      var date = new Date(entry.at * 1000);
      var pad = function (number) { return String(number).padStart(2, '0'); };
      var chip = { next: 'st-ready', done: 'st-done', stuck: 'st-failed', unban: 'st-pending' }[entry.kind] || 'st-pending';
      return '<div><span class="alt">' + pad(date.getDate()) + '/' + pad(date.getMonth() + 1) + ' ' + pad(date.getHours()) + ':' + pad(date.getMinutes()) +
        '</span> <span class="chip ' + chip + '">' + esc(entry.kind) + '</span> <span class="alt">' + esc(entry.outcome) + '</span></div>';
    }).join('') : '<div class="muted">aucun</div>'));
    return out.join('');
  }

  async function poll() {
    if (!active || !account) return;
    var requested = account;
    var next = null;
    var failure = null;
    try {
      next = await (await fetch('/api/brain/' + encodeURIComponent(requested))).json();
    } catch (error) {
      failure = 'daemon injoignable';
    }
    if (requested !== account) return;
    var status = document.getElementById('brain-status');
    if (!next || !next.ok) {
      if (status) status.textContent = failure || (next && next.error) || 'erreur';
      return;
    }
    var fresh = next.version !== version;
    payload = next;
    if (status) {
      var now = new Date();
      status.textContent = 'version ' + next.version + ' · relu ' + String(now.getHours()).padStart(2, '0') + ':' +
        String(now.getMinutes()).padStart(2, '0') + ':' + String(now.getSeconds()).padStart(2, '0');
    }
    var panel = document.getElementById('brain-panel');
    if (panel) panel.innerHTML = panelHtml();
    if (fresh) {
      var first = version === null;
      version = next.version;
      draw(first);
    }
  }

  function select(name) {
    if (name === account) return;
    account = name;
    payload = null;
    version = null;
    opened = {};
    view = null;
    var svg = document.getElementById('brain-svg');
    if (svg) while (svg.firstChild) svg.removeChild(svg.firstChild);
    var title = document.getElementById('brain-title');
    if (title) title.textContent = name ? 'Cerveau de ' + name : 'Choisir un bot';
    poll();
  }

  function setActive(flag) {
    active = flag;
    if (timer) { clearInterval(timer); timer = null; }
    if (active) {
      timer = setInterval(poll, 5000);
      if (payload) draw(false);
      poll();
    }
  }

  function bindPanZoom() {
    var svg = document.getElementById('brain-svg');
    if (!svg) return;
    var drag = null;
    svg.addEventListener('wheel', function (event) {
      if (!view) return;
      event.preventDefault();
      var rect = svg.getBoundingClientRect();
      var ratioX = (event.clientX - rect.left) / rect.width;
      var ratioY = (event.clientY - rect.top) / rect.height;
      var scale = event.deltaY > 0 ? 1.15 : 1 / 1.15;
      var zoomW = Math.min(Math.max(view.w * scale, 120), full.w * 6);
      var zoomH = zoomW * view.h / view.w;
      view = { x: view.x + (view.w - zoomW) * ratioX, y: view.y + (view.h - zoomH) * ratioY, w: zoomW, h: zoomH };
      applyView(svg);
    }, { passive: false });
    svg.addEventListener('mousedown', function (event) {
      if (!view) return;
      drag = { x: event.clientX, y: event.clientY, view: Object.assign({}, view) };
      dragged = false;
    });
    window.addEventListener('mousemove', function (event) {
      if (!drag) return;
      if (Math.abs(event.clientX - drag.x) + Math.abs(event.clientY - drag.y) > 4) dragged = true;
      var rect = svg.getBoundingClientRect();
      view = Object.assign({}, drag.view, {
        x: drag.view.x - (event.clientX - drag.x) * drag.view.w / rect.width,
        y: drag.view.y - (event.clientY - drag.y) * drag.view.h / rect.height,
      });
      applyView(svg);
    });
    window.addEventListener('mouseup', function () { drag = null; });
    var fit = document.getElementById('brain-fit');
    if (fit) fit.onclick = function () { if (full) { view = Object.assign({}, full); applyView(svg); } };
    var hide = document.getElementById('brain-hide-abandoned');
    if (hide) hide.onchange = function () { draw(false); };
    var collapseAll = document.getElementById('brain-collapse');
    if (collapseAll) collapseAll.onclick = function () { opened = {}; draw(false); };
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bindPanZoom);
  else bindPanZoom();

  return { select: select, setActive: setActive };
})();
