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
check('shop condition prerequisite', GuidesView.prereqText({ levels: {}, combat: 3, quests: ['dragon'], quest_points: 32, tools: [], capital: 0, ironman: true }), 'quest dragon, 32 quest points');
check('stock prerequisite', GuidesView.prereqText({ levels: {}, combat: 3, quests: [], tools: [], capital: 0, ironman: true, stock: { rune_platebody: 28 } }), 'stock 28 rune_platebody');
check('route farmed', GuidesView.routeText({ item: 'iron_ore', display: 'Iron ore', cause: 'input', route: 'farm', source: 'gather:mining/iron_ore', farm_coins_per_unit: 12.5, buy_coins_per_unit: 20 }), 'Iron ore (input): farm gather:mining/iron_ore [farm 12.5gp, buy 20gp]');
check('route bought', GuidesView.routeText({ item: 'fishing_bait', cause: 'bait', route: 'buy', source: '', farm_coins_per_unit: null, farm_ticks_per_unit: null, buy_coins_per_unit: 4 }), 'fishing_bait (bait): buy [buy 4gp]');
check('xp text best first', GuidesView.xpText({ mining: 5000, smithing: 200, magic: 0 }), 'mining 5000, smithing 200');
check('freshness of the set', GuidesView.freshnessText({ age_ms: 42400, cycle_ms: 3720, refresh_s: 300, approximate: [] }), 'set built 42 s ago in 3.7 s, rebuilt every 300 s');
check('approximate read said', GuidesView.freshnessText({ age_ms: 0, cycle_ms: 0, refresh_s: 300, approximate: ['rates of level 30'] }), 'set built 0 s ago in 0 s, rebuilt every 300 s · ≈ rates of level 30');
check('pending set asks again', GuidesView.retryDelay({ ok: false, pending: true }), 5000);
check('an error is not retried', GuidesView.retryDelay({ ok: false, error: 'x' }), null);

// Confidence column: plain text for sorting, HTML only through the column
// formatter (never raw markup escaped into the cell).
check('confidence text is plain', GuidesView.confidenceText({ measured: false, runs: 0, read: 'PriorOnly' }), 'estimated · n 0 · PriorOnly');
check('confidence html marks estimates', GuidesView.confidenceHtml({ measured: false, runs: 2, read: 'Exact' }), '<span class="prior">estimated</span> · n 2 · Exact');
check('confidence html escapes the read label', GuidesView.confidenceHtml({ measured: true, runs: 3, read: '<b>' }), 'measured · n 3 · &lt;b&gt;');
check('confidence column renders through its formatter', GuidesView.columnFormats('confidence_text'), [true, true]);

if (failures) {
  console.log(failures + ' failure(s)');
  process.exit(1);
}
