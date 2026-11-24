// Item and skill icons of the dashboard (2026-11-24, D-icons in
// docs/decisions.md). Two sprite sheets and one index, written by
// `gamedata/tools/icons.mts` into orchestration/static/icons/:
//   items.png  every item icon, 32x32 cells (the client's own backpack
//              render), cell 0 empty;
//   skills.png the stat-tab icons, 25x25 cells, cell 0 empty;
//   index.json {items|skills: {file, hash, cell, cols, slot}}: `slot` maps
//              an item id (array index) or a skill name to its cell,
//              0 = no icon; items.count {id: {step: cell}}: the stack
//              icon from that count on (coins pile up, arrows bundle).
// Each sheet is fetched once (`?v=<hash>`, cached for good); the icons
// are cut client-side with background-position. A page writes an empty
// slot of the final size at once (LifeIcons.item / .skill), so the
// layout never moves; the CSS rules that paint the slots arrive with
// the index. An id without an icon keeps the empty cell 0, never
// another item's icon. Index unreachable: the slots fold away.
//
// Pure helpers (rules, item, skill) touch no DOM: `node --test icons.test.js`.
var LifeIcons = (function () {
  'use strict';

  var INDEX_URL = '/icons/index.json';
  var BASE_CSS = '.ic{display:inline-block;vertical-align:middle;flex:none;margin:-2px 4px -2px 0;'
    + 'background-repeat:no-repeat;background-position:0 0;image-rendering:pixelated}'
    + '.ic-item{width:32px;height:32px}.ic-skill{width:25px;height:25px}'
    + '.no-icons .ic{display:none}'
    // icon + name that may wrap (narrow cell): the name wraps beside the
    // icon, never under it.
    + '.iclabel{display:inline-flex;align-items:center}';

  // Skill names are plain lowercase words; anything else gets no class.
  function skillKey(name) {
    var key = String(name === null || name === undefined ? '' : name).toLowerCase();
    return /^[a-z]+$/.test(key) ? key : '';
  }

  // Count steps of the rev 274 cache (`countco`): a stack of `count`
  // shows the icon of the last step <= count. The generator checks that
  // the cache uses no other step.
  var COUNT_STEPS = [2, 3, 4, 5, 25, 100, 250, 1000, 10000];

  function countClass(count) {
    var number = Number(count);
    var step = 0;
    for (var i = 0; i < COUNT_STEPS.length; i++) if (number >= COUNT_STEPS[i]) step = COUNT_STEPS[i];
    return step ? ' ic-n' + step : '';
  }

  // Sheet cell of an item at a stack count (the cell the CSS rules pick),
  // 0 = no icon. Pages drawing on a canvas (spectator game tabs) cut the
  // same sheet with it.
  function itemCell(index, id, count) {
    var items = index && index.items;
    var key = itemKey(id);
    if (!items || !items.slot || !key) return 0;
    var slot = items.slot[Number(key)] || 0;
    var steps = (items.count && items.count[key]) || null;
    if (!slot || !steps) return slot;
    var number = Number(count);
    for (var i = 0; i < COUNT_STEPS.length; i++) {
      if (number >= COUNT_STEPS[i] && steps[COUNT_STEPS[i]]) slot = steps[COUNT_STEPS[i]];
    }
    return slot;
  }

  function itemKey(id) {
    var number = Number(id);
    return Number.isInteger(number) && number >= 0 ? String(number) : '';
  }

  function itemClass(id, count) {
    var key = itemKey(id);
    return 'ic ic-item' + (key ? ' ic-i' + key + countClass(count) : '');
  }

  // Empty slot of an item icon (HTML), `count` optional (a stack). Unknown
  // id: the neutral cell.
  function item(id, count) {
    return '<span class="' + itemClass(id, count) + '" aria-hidden="true"></span>';
  }

  function skill(name) {
    var key = skillKey(name);
    return '<span class="ic ic-skill' + (key ? ' ic-s-' + key : '') + '" aria-hidden="true"></span>';
  }

  // Same slots as DOM nodes (pages that build nodes, not HTML).
  function itemNode(id, count) {
    var span = document.createElement('span');
    span.className = itemClass(id, count);
    span.setAttribute('aria-hidden', 'true');
    return span;
  }

  function position(sheet, slot) {
    var x = (slot % sheet.cols) * sheet.cell;
    var y = Math.floor(slot / sheet.cols) * sheet.cell;
    return '{background-position:' + (x ? -x + 'px' : '0') + ' ' + (y ? -y + 'px' : '0') + '}';
  }

  // CSS of one sheet: its image once, then one position per slot (and
  // per stack step of the items whose icon changes with the count).
  function sheetRules(sheet, kind, prefix, keys) {
    if (!sheet || !sheet.file || !sheet.cell || !sheet.cols || !sheet.slot) return '';
    var url = '/icons/' + sheet.file + '?v=' + encodeURIComponent(sheet.hash || '');
    var css = '.ic-' + kind + '{background-image:url("' + url + '")}';
    for (var i = 0; i < keys.length; i++) {
      var slot = sheet.slot[keys[i]];
      if (!slot) continue;
      css += '.' + prefix + keys[i] + position(sheet, slot);
      var steps = (sheet.count && sheet.count[keys[i]]) || {};
      for (var s = 0; s < COUNT_STEPS.length; s++) {
        var stepSlot = steps[COUNT_STEPS[s]];
        if (stepSlot) css += '.' + prefix + keys[i] + '.ic-n' + COUNT_STEPS[s] + position(sheet, stepSlot);
      }
    }
    return css;
  }

  // Every rule of the index (pure).
  function rules(index) {
    if (!index) return '';
    var items = index.items || {};
    var skills = index.skills || {};
    var itemIds = [];
    for (var id = 0; id < ((items.slot && items.slot.length) || 0); id++) itemIds.push(String(id));
    var skillNames = Object.keys(skills.slot || {}).filter(skillKey);
    return sheetRules(items, 'item', 'ic-i', itemIds) + sheetRules(skills, 'skill', 'ic-s-', skillNames);
  }

  function addStyle(css) {
    var style = document.createElement('style');
    style.textContent = css;
    document.head.appendChild(style);
  }

  // The index once loaded (null when unreachable), for pages that draw
  // from the sheets themselves: `onIndex(callback)` runs at once when it
  // is already there.
  var loaded = { done: false, index: null, waiting: [] };
  function onIndex(callback) {
    if (loaded.done) callback(loaded.index);
    else loaded.waiting.push(callback);
  }
  function settle(index) {
    loaded.done = true;
    loaded.index = index;
    var waiting = loaded.waiting;
    loaded.waiting = [];
    for (var i = 0; i < waiting.length; i++) waiting[i](index);
  }

  // Browser: base sizes now (slots take their room at once), sheets when
  // the index arrives.
  function load() {
    addStyle(BASE_CSS);
    fetch(INDEX_URL, { cache: 'no-store' })
      .then(function (response) {
        if (!response.ok) throw new Error('icons ' + response.status);
        return response.json();
      })
      .then(function (index) { addStyle(rules(index)); settle(index); })
      .catch(function () { document.documentElement.classList.add('no-icons'); settle(null); });
  }

  if (typeof document !== 'undefined' && typeof fetch !== 'undefined') load();

  return { item: item, skill: skill, itemNode: itemNode, itemCell: itemCell, rules: rules, onIndex: onIndex, BASE_CSS: BASE_CSS };
})();

if (typeof module !== 'undefined' && module.exports) module.exports = LifeIcons;
