// Offline checks for guides.js (plan rendements R4): query string of the
// filters and profile, visible rows (feasibility, search, top N),
// conditions and feasibility text (D22: nothing blocked).
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
check('query carries the profile', GuidesView.queryOf({ level: '30', levels: 'smithing:45', held: 'naturerune:100', coins: '2500' }), 'level=30&levels=smithing%3A45&held=naturerune%3A100&coins=2500');
var rows = [
  { method: 'gather:mining/coal', feasibility: { state: 'feasible' } },
  { method: 'smelt:steel_bar', feasibility: { state: 'infeasible' } },
  { method: 'gather:mining/iron_ore', feasibility: { state: 'after_prereqs' } },
];
check('feasible now', GuidesView.visibleRows(rows, { feasibility: 'now', limit: 0 }).map(function (row) { return row.method; }), ['gather:mining/coal']);
check('feasible or after prerequisites', GuidesView.visibleRows(rows, { feasibility: 'reach', limit: 0 }).map(function (row) { return row.method; }), ['gather:mining/coal', 'gather:mining/iron_ore']);
check('every line kept by default (nothing blocked)', GuidesView.visibleRows(rows, { limit: 0 }).length, 3);
check('search', GuidesView.visibleRows(rows, { search: 'steel', limit: 0 }).map(function (row) { return row.method; }), ['smelt:steel_bar']);
check('top N keeps the best first', GuidesView.visibleRows(rows, { limit: 2 }).length, 2);

// Conditions (D22): the API's structured list, each with its text; the
// impossible ones (no site within bank reach: danger::NEVER, never shown
// as combat 255) highlighted.
var conditions = [
  { kind: 'level', text: 'mining 85', impossible: false },
  { kind: 'site', text: 'no site within bank reach (impossible today)', impossible: true },
  { kind: 'market', text: 'buy coal 19/lot: asks 0, recent 5, shops 0', impossible: false },
];
check('conditions text', GuidesView.conditionsText(conditions), 'mining 85, no site within bank reach (impossible today), buy coal 19/lot: asks 0, recent 5, shops 0');
check('no condition', GuidesView.conditionsText([]), '-');
check('impossible condition highlighted', GuidesView.conditionsHtml([conditions[1], { kind: 'quest', text: 'quest <x>' }]), '<span class="impossible">no site within bank reach (impossible today)</span>, quest &lt;x&gt;');
check('conditions column renders through its formatter', GuidesView.columnFormats('conditions_text'), [true, true]);
check('feasible', GuidesView.feasibilityText({ state: 'feasible', pending: [], reasons: [] }), 'feasible');
check('after prerequisites', GuidesView.feasibilityText({ state: 'after_prereqs', pending: ['smithing 20 -> 30', 'buy hammer'], reasons: [] }), 'after_prereqs: smithing 20 -> 30; buy hammer');
check('infeasible with its reasons', GuidesView.feasibilityText({ state: 'infeasible', pending: [], reasons: ['coal: holds 0, asks 1, shops 0 of 19 per lot'] }), 'infeasible: coal: holds 0, asks 1, shops 0 of 19 per lot');
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


// A sort re-runs the guide render (click-to-unfold survives a sort).
var rerendered = function () {};
check('sort re-runs the guide render', GuidesView.sortOptions(null, rerendered).onSort === rerendered, true);

if (failures) {
  console.log(failures + ' failure(s)');
  process.exit(1);
}
