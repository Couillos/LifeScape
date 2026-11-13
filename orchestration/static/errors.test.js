// Offline checks for errors.js pure helpers: query/hash round trip,
// percent and trend text, links, task label, chart series, ages.
var ErrorsPage = require('./errors.js');

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

var state = ErrorsPage.parseHash('#hours=6&source=nav&zone=Varrock%20Square&bogus=1');
check('hash window', state.hours, 6);
check('hash filter decoded', state.zone, 'Varrock Square');
check('hash unknown key ignored', state.bogus, undefined);
check('hash bad window falls back', ErrorsPage.parseHash('#hours=5').hours, 24);
check('empty hash', ErrorsPage.parseHash('').source, '');
check('query keeps only set filters', ErrorsPage.queryString(state), 'hours=6&source=nav&zone=Varrock%20Square');
check('query extras', ErrorsPage.queryString({ hours: 24 }, { limit: 300 }), 'hours=24&limit=300');
check('round trip', ErrorsPage.parseHash('#' + ErrorsPage.queryString(state)), state);
check('signature survives the hash', ErrorsPage.parseHash('#' + ErrorsPage.queryString({ hours: 24, signature: 'no route (#,#,#) & more' })).signature, 'no route (#,#,#) & more');

check('pct', ErrorsPage.pct(0.2534), '25.3%');
check('pct null', ErrorsPage.pct(null), '-');
check('trend up', ErrorsPage.trendText(12.5), '+12.5%');
check('trend down', ErrorsPage.trendText(-3), '-3%');
check('trend none', ErrorsPage.trendText(null), 'n/a');

check('pos', ErrorsPage.posText([3200, 3420, 0]), '3200, 3420, 0');
check('pos missing', ErrorsPage.posText(null), '-');
check('spectate link', ErrorsPage.spectateHref('fiona_park'), '/#fiona_park');
check('life link', ErrorsPage.lifeHref('a b'), '/life#a%20b');
check('dossier link', ErrorsPage.dossierHref('t01-5.json'), '/api/errors/dossier/t01-5.json');
check('escape', ErrorsPage.esc('<a href="x">&</a>'), '&lt;a href=&quot;x&quot;&gt;&amp;&lt;/a&gt;');

check('task full', ErrorsPage.taskText({ script: 'fishing', node_id: 'n4', method: 'gather:fishing/shrimp' }), 'fishing n4 · gather:fishing/shrimp');
check('task same method', ErrorsPage.taskText({ script: 'market', node_id: '', method: 'market' }), 'market');
check('task empty', ErrorsPage.taskText({}), '');

var hourly = [{ t: 3600, total: 3, nav: 2, task: 1 }, { t: 7200, total: 0, nav: 0, task: 0 }];
check('series total', ErrorsPage.chartSeries(hourly, 'total'), { slots: [3600, 7200], rows: [3, 0] });
check('series source, zero kept', ErrorsPage.chartSeries(hourly, 'nav').rows, [2, 0]);
check('series unknown is zero', ErrorsPage.chartSeries(hourly, 'deal').rows, [0, 0]);
check('series empty', ErrorsPage.chartSeries(null, 'total'), { slots: [], rows: [] });

check('ago now', ErrorsPage.ago(1000, 1030), 'just now');
check('ago min', ErrorsPage.ago(1000, 1000 + 12 * 60), '12 min ago');
check('ago hours', ErrorsPage.ago(0, 3 * 3600 + 5), '3 h ago');
check('ago days', ErrorsPage.ago(0, 2 * 86400), '2 d ago');

if (failures) {
  console.log(failures + ' FAILURE(S)');
  process.exit(1);
}
console.log('errors.js: all checks pass');
