// Offline checks for xp.js pure helpers: number formats, freshness,
// chart lines (hidden series, gaps), scale max, hover slot and tooltip,
// table ordering. Run: `node xp.test.js` (from orchestration/static).
var XpPage = require('./xp.js');

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

var snapshot = {
  generated_at: 1000, next_at: 2800, bucket_seconds: 3600,
  skills: ['attack', 'mining'], slots: [0, 3600, 7200], fleet_ticks: [100, 0, 300],
  series: {
    attack: { fleet_rate: [0.5, null, 1.25], efficiency: [1, null, 2] },
    mining: { fleet_rate: [0.25, null, 0.5], efficiency: [0.5, null, 0.7] },
  },
  table: [
    { skill: 'attack', periods: { '24h': { xp_per_hour: 100, efficiency: 1.5 } } },
    { skill: 'mining', periods: { '24h': { xp_per_hour: 900, efficiency: 0.5 } } },
    { skill: 'magic', periods: {} },
  ],
};

check('thousands use spaces', XpPage.formatNumber(1234567), '1 234 567');
check('null number', XpPage.formatNumber(null), '-');
check('rate small', XpPage.formatRate(0.5123), '0,5');
check('rate mid', XpPage.formatRate(42.37), '42,4');
check('rate big', XpPage.formatRate(9312.4), '9 312');
check('rate null', XpPage.formatRate(null), '-');
check('share', XpPage.formatShare(0.0345), '3,5 %');
check('share large', XpPage.formatShare(0.5), '50 %');
check('change up', XpPage.formatChange(380), '+380 %');
check('change down', XpPage.formatChange(-12.5), '-12,5 %');
check('change null', XpPage.formatChange(null), '-');
check('change class', [XpPage.changeClass(5), XpPage.changeClass(-5), XpPage.changeClass(0.2), XpPage.changeClass(null)], ['up', 'down', '', '']);
check('span', [XpPage.span(30), XpPage.span(600), XpPage.span(7500)], ['30 s', '10 min', '2 h 05']);
check('freshness', XpPage.freshness(snapshot, 1600), 'mis à jour il y a 10 min · prochaine mise à jour dans 20 min');
check('freshness due', XpPage.freshness(snapshot, 3000), 'mis à jour il y a 33 min · prochaine mise à jour imminente');

var lines = XpPage.chartLines(snapshot, XpPage.chartView(snapshot, '7d'), 'fleet_rate', { attack: true });
check('hidden series removed', lines.map(function (line) { return line.skill; }), ['mining']);
lines = XpPage.chartLines(snapshot, XpPage.chartView(snapshot, '7d'), 'efficiency', {});
check('mode picks the series', lines[0].values, [1, null, 2]);
check('max over visible lines', XpPage.chartMax(lines), 2);
check('max with nothing', XpPage.chartMax([]), 0);
check('slot at left edge', XpPage.slotAt(10, 50, 100, 3), 0);
check('slot at middle', XpPage.slotAt(100, 50, 100, 3), 1);
check('slot past right edge', XpPage.slotAt(900, 50, 100, 3), 2);
var tip = XpPage.tooltipLines(snapshot, lines, 2, '01/01 02:00–03:00');
check('tooltip sorted, with fleet ticks', tip.map(function (row) { return row.text; }),
  ['01/01 02:00–03:00 · 300 ticks de travail', 'attack 2,0', 'mining 0,7']);
check('tooltip skips gaps', XpPage.tooltipLines(snapshot, lines, 1, 'x').length, 1);
check('rows by xp per hour', XpPage.skillRows(snapshot, '24h').map(function (row) { return row.skill; }), ['mining', 'attack', 'magic']);
check('method text', XpPage.methodText({ method: 'chop:willow', xp_rate: 6.3, ticks: 12000, runs: 8 }), 'chop:willow · 6,3 · 12 000 ticks · 8 runs');
check('method text none', XpPage.methodText(null), '-');
check('escape', XpPage.esc('<a "x">'), '&lt;a &quot;x&quot;&gt;');

var big = {
  bucket_seconds: 3600,
  slots: [], fleet_ticks: [], fleet_xp: [], fleet_rate: [], series: { mining: { efficiency: [] } },
  history_chart: { bucket_seconds: 21600, slots: [0, 21600], fleet_ticks: [5, 6], fleet_xp: [10, 20], fleet_rate: [1, 2], series: {} },
};
for (var index = 0; index < 168; index++) {
  big.slots.push(index * 3600); big.fleet_ticks.push(index); big.fleet_xp.push(index * 2); big.fleet_rate.push(index * 3);
  big.series.mining.efficiency.push(index);
}
var day = XpPage.chartView(big, '24h');
check('24h keeps the last 24 hourly slots', [day.slots.length, day.slots[0], day.slots[23]], [24, 144 * 3600, 167 * 3600]);
check('24h slices every series', [day.fleet_xp[0], day.fleet_rate[23], day.fleet_ticks.length, day.series.mining.efficiency[0]], [288, 501, 24, 144]);
check('24h keeps the hourly width', day.bucket_seconds, 3600);
check('7d is the whole week', XpPage.chartView(big, '7d').slots.length, 168);
check('all reads the history series and its width', [XpPage.chartView(big, 'all').bucket_seconds, XpPage.chartView(big, 'all').fleet_xp], [21600, [10, 20]]);
check('all is empty before the first refresh with a history series', XpPage.chartView({ bucket_seconds: 3600, slots: [1], series: {} }, 'all').slots, []);
var fleet = XpPage.fleetLines(day);
check('fleet lines carry the two series', [fleet.xp[0].values.length, fleet.rate[0].values[23], fleet.rate[0].skill], [24, 501, 'XP / 6000 ticks flotte']);
check('rate label', XpPage.RATE_LABEL, 'XP / 6000 ticks');

if (failures > 0) {
  console.log(failures + ' FAILURE(S)');
  process.exit(1);
}
console.log('all ok');
