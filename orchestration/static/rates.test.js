// Offline checks for rates.js (user 2026-11-17: the page folds ~18 000
// flow rows into one row per activity): query of the filters, counts of
// a page, headline text, confidence, detail unfolding (flows, keys as
// sub-rows, an unfolded key).
var RatesView = require('./rates.js');

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

// Filters: empty values left out, measured as 1, sort and page.
check('default query', RatesView.queryOf({ group: 'activity', q: '', skill: '', family: '', band: '', measured: true, min_runs: 0 }, { key: 'runs', dir: -1 }, 0, 50),
  'group=activity&measured=1&sort=runs&dir=desc&limit=50');
check('every filter', RatesView.queryOf({ group: 'key', q: ' raw beef ', skill: 'strength', family: 'fight', band: '11', measured: false, min_runs: 5 }, { key: 'xp_total', dir: 1 }, 100, 25),
  'group=key&q=raw%20beef&skill=strength&family=fight&band=11&min_runs=5&sort=xp_total&dir=asc&offset=100&limit=25');

// Counts: "x rows of y".
var payload = { group: 'activity', offset: 50, limit: 50, matched: 86, total: 187, rows: new Array(36), took_ms: 1.234, index_age_ms: 12000, index_build_ms: 140 };
check('count text', RatesView.countText(payload), 'activit(ies) 51-86 of 86 matching (187 in all) · page 2/2');
check('empty page', RatesView.countText({ group: 'key', offset: 0, limit: 50, matched: 0, total: 3371, rows: [] }), 'key(s) 0-0 of 0 matching (3371 in all) · page 1/1');
check('timing text', RatesView.timingText(payload), 'answered in 1.2 ms · index 12 s old (built in 140 ms)');

// Headline columns.
check('flows text', RatesView.flowsText([{ item: 'Bones', per_6000: 40 }, { item: 'Feather', per_6000: 100.04 }]), 'Bones 40, Feather 100');
check('consumption shown as units out', RatesView.flowsText([{ item: 'Trout', per_6000: -5 }]), 'Trout 5');
check('no flow', RatesView.flowsText([]), '-');
check('xp text most first', RatesView.xpText({ hitpoints: 1700, strength: 5000, magic: 0 }), 'strength 5000, hitpoints 1700');
check('band range of an activity', RatesView.bandText(1, 16), '1-20');
check('band of a key', RatesView.bandText(6), '6-10');
check('no band', RatesView.bandText(0), '-');
check('age in days', RatesView.ageText(3 * 86400), '3 d');

// Confidence: measured (n >= 3), estimated (thin), partly measured.
check('measured', RatesView.confidenceText({ keys: 1, measured_keys: 1 }), 'measured');
check('thin', RatesView.confidenceText({ keys: 3, measured_keys: 0 }), 'estimated (thin)');
check('partly measured', RatesView.confidenceText({ keys: 5, measured_keys: 3 }), 'measured 3/5 keys');
check('thin highlighted', RatesView.confidenceHtml({ keys: 1, measured_keys: 0 }), '<span class="prior">estimated (thin)</span>');

// Rows and detail queries: an activity row asks every key, a key row
// narrows to its site, tool and band; the page's filters ride along.
var activity = { method: 'fight:melee/cows', style: 'melee:strength', site: null, tool: null, band: 1 };
var keyRow = { method: 'fight:melee/cows', style: 'melee:strength', site: '3255,3270,0', tool: 'iron_sword', band: 11 };
check('activity id', RatesView.rowId(activity), 'fight:melee/cows|melee:strength|||');
check('key id', RatesView.rowId(keyRow), 'fight:melee/cows|melee:strength|3255,3270,0|iron_sword|11');
check('activity detail query', RatesView.detailQuery(activity, { measured: true, min_runs: 0 }), 'method=fight%3Amelee%2Fcows&style=melee%3Astrength&measured=1');
check('key detail query', RatesView.detailQuery(keyRow, { q: 'beef', measured: false }), 'method=fight%3Amelee%2Fcows&style=melee%3Astrength&site=3255%2C3270%2C0&tool=iron_sword&band=11&q=beef');

// Columns: sort keys are the server's; key rows show site, tool, band.
var keys = function (group) { return RatesView.columnsFor(group).map(function (col) { return col.key; }); };
check('activity columns', keys('activity'), ['method', 'family', 'band', 'keys', 'runs', 'age_secs', 'confidence', 'product_value_6000', 'consumption_value_6000', 'xp_total', 'net_value_6000', 'failure_rate']);
check('key columns', keys('key').slice(2, 5), ['site', 'tool', 'band']);
var productsCol = RatesView.columnsFor('activity').filter(function (col) { return col.key === 'product_value_6000'; })[0];
check('products cell escapes', productsCol.fmt(0, { products: [{ item: '<b>', per_6000: 1 }] }), '&lt;b&gt; 1');

// Detail: every flow, units, XP; keys as sub-rows, an unfolded key's
// own detail under it.
var detail = {
  ok: true,
  summary: { flows: [{ item: 'Cowhide', cause: 'loot', per_6000: 50, value_6000: 5000 }, { item: 'other drops (2 objects)', cause: 'loot', per_6000: 0.5, value_6000: 3, folded: 2 }, { item: 'Trout', cause: 'food', per_6000: -5, value_6000: -100 }],
    units: [{ kind: 'damage', subject: 'cow', per_6000: 400 }], xp: { strength: 1600 }, net_value_6000: 4903, runs: 6, failure_rate: 0, rejected: 0, dispersion: null },
  keys: [keyRow, { method: 'fight:melee/cows', style: 'melee:strength', site: '3030,3300,0', tool: 'iron_sword', band: 11, runs: 2, keys: 1, measured_keys: 0 }],
};
var html = RatesView.detailHtml(detail, {});
check('detail lists every flow', (html.match(/<tr><td class="left">/g) || []).length, 3);
check('detail shows other drops', html.indexOf('other drops (2 objects)') >= 0, true);
check('detail shows units and XP', html.indexOf('damage cow 400') >= 0 && html.indexOf('strength 1600') >= 0, true);
check('keys as clickable sub-rows', (html.match(/class="subkey"/g) || []).length, 2);
var opened = {};
opened[RatesView.rowId(keyRow)] = { ok: true, summary: { flows: [{ item: 'Bones', cause: 'loot', per_6000: 33, value_6000: 0 }], units: [], xp: {}, runs: 4 }, keys: [keyRow] };
var unfolded = RatesView.detailHtml(detail, opened);
check('an unfolded key shows its own flows', unfolded.indexOf('Bones') >= 0, true);
check('a pending detail says so', RatesView.detailHtml(null), '<span class="muted">loading…</span>');
check('a detail error is shown', RatesView.detailHtml({ ok: false, error: 'rates: no measured key' }), '<span class="prior">rates: no measured key</span>');

if (failures) {
  console.log(failures + ' failure(s)');
  process.exit(1);
}
