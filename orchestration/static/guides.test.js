// Offline checks for guides.js (plan rendements R4): query string of the
// filters, visible rows (accessible, search, top N), prerequisite text.
var GuidesView = require('./guides.js');

var failures = 0;
function check(name, actual, expected) {
  var ok = JSON.stringify(actual) === JSON.stringify(expected);
  if (!ok) {
    failures++;
    console.log('FAIL ' + name + ': got ' + JSON.stringify(actual) + ', want ' + JSON.stringify(expected));
  } else {
    console.log('ok ' + name);
  }
}

check('query leaves empty filters out', GuidesView.queryOf({ skill: 'mining', level: '', family: undefined, quests: ' doric ' }), 'skill=mining&quests=doric');
var rows = [
  { method: 'gather:mining/coal', accessible: true },
  { method: 'smelt:steel_bar', accessible: false },
  { method: 'gather:mining/iron_ore', accessible: true },
];
check('accessible only', GuidesView.visibleRows(rows, { accessibleOnly: true, limit: 0 }).map(function (row) { return row.method; }), ['gather:mining/coal', 'gather:mining/iron_ore']);
check('search', GuidesView.visibleRows(rows, { search: 'steel', limit: 0 }).map(function (row) { return row.method; }), ['smelt:steel_bar']);
check('top N keeps the best first', GuidesView.visibleRows(rows, { limit: 2 }).length, 2);
check('prerequisites', GuidesView.prereqText({ levels: { smithing: 30 }, combat: 12, quests: ['doric'], tools: ['hammer'], capital: 15000, ironman: false }), 'smithing 30, combat 12, quest doric, hammer, 15k capital, no ironman');
check('stock prerequisite', GuidesView.prereqText({ levels: {}, combat: 3, quests: [], tools: [], capital: 0, ironman: true, stock: { rune_platebody: 28 } }), 'stock 28 rune_platebody');
check('xp text best first', GuidesView.xpText({ mining: 5000, smithing: 200, magic: 0 }), 'mining 5000, smithing 200');

if (failures) {
  console.log(failures + ' failure(s)');
  process.exit(1);
}
