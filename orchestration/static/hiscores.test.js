// Offline checks for hiscores.js pure helpers: number formats,
// categories, ranking rows, paging, name search, hash round trip,
// freshness text.
var Hiscores = require('./hiscores.js');

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

check('thousands', Hiscores.formatNumber(1234567), '1,234,567');
check('small number', Hiscores.formatNumber(999), '999');
check('zero', Hiscores.formatNumber(0), '0');
check('negative', Hiscores.formatNumber(-12000), '-12,000');
check('missing', Hiscores.formatNumber(null), '-');
check('short millions', Hiscores.shortCoins(12400000), '12.4M');
check('short thousands', Hiscores.shortCoins(15300), '15.3K');
check('short under 10k', Hiscores.shortCoins(9500), '9,500');

var snapshot = {
  ok: true, generated_at: 1000, next_at: 2800, skills: ['attack', 'defence'], market: true,
  players: [
    { account: 'olivia_king', name: 'Olivia king', iron: false, source: 'live', overall: [1, 40, 9000],
      skills: [[2, 30, 8000], [1, 10, 1000]], coins: [2, 50], value: [1, 900], top: [] },
    { account: 'Ironxmt9nxcy', name: 'Ironxmt9nxcy', iron: true, source: 'save', overall: [2, 35, 9500],
      skills: [[1, 32, 9400], [2, 3, 100]], coins: [1, 300], value: [2, 300], top: [] },
  ],
};

var cats = Hiscores.categories(snapshot).map(function (c) { return c.key; });
check('categories: overall, skills, wealth', cats, ['overall', 'attack', 'defence', 'coins', 'value']);
check('category labels', Hiscores.categories(snapshot)[1].label, 'Attack');
check('wealth category', Hiscores.isWealth('value'), true);
check('skill is not wealth', Hiscores.isWealth('attack'), false);

check('overall rows by rank', Hiscores.rankingRows(snapshot, 'overall').map(function (r) { return [r.rank, r.account, r.level, r.xp]; }),
  [[1, 'olivia_king', 40, 9000], [2, 'Ironxmt9nxcy', 35, 9500]]);
check('skill rows by rank', Hiscores.rankingRows(snapshot, 'attack').map(function (r) { return r.account; }), ['Ironxmt9nxcy', 'olivia_king']);
check('coins rows', Hiscores.rankingRows(snapshot, 'coins').map(function (r) { return [r.rank, r.amount]; }), [[1, 300], [2, 50]]);
check('row flags', Hiscores.rankingRows(snapshot, 'coins')[0].iron, true);
var down = JSON.parse(JSON.stringify(snapshot));
down.players.forEach(function (p) { p.value = null; });
check('market down: no value rows', Hiscores.rankingRows(down, 'value'), []);
check('unknown skill: no rows', Hiscores.rankingRows(snapshot, 'slayer'), []);
check('cell of a skill', Hiscores.cellOf(snapshot.players[0], 'defence', snapshot), { rank: 1, level: 10, xp: 1000 });

var many = [];
for (var i = 1; i <= 60; i++) many.push(i);
var second = Hiscores.paginate(many, 2);
check('page 2 of 25', [second.rows[0], second.rows[24], second.page, second.pages], [26, 50, 2, 3]);
check('page clamped high', Hiscores.paginate(many, 9).page, 3);
check('page clamped low', Hiscores.paginate(many, 0).page, 1);
check('empty paging', Hiscores.paginate([], 1), { rows: [], page: 1, pages: 1 });
check('page of rank 25', Hiscores.pageOfRank(25), 1);
check('page of rank 26', Hiscores.pageOfRank(26), 2);

check('search by account', Hiscores.findPlayer(snapshot, 'olivia_king').account, 'olivia_king');
check('search by shown name, any case', Hiscores.findPlayer(snapshot, 'OLIVIA KING').account, 'olivia_king');
check('search by prefix', Hiscores.findPlayer(snapshot, 'ironx').account, 'Ironxmt9nxcy');
check('search miss', Hiscores.findPlayer(snapshot, 'nobody'), null);
check('search empty', Hiscores.findPlayer(snapshot, '  '), null);

var state = Hiscores.parseHash('#cat=attack&page=3&bogus=1');
check('hash category and page', [state.cat, state.page, state.player], ['attack', 3, '']);
check('hash player decoded', Hiscores.parseHash('#player=olivia%20king').player, 'olivia king');
check('hash bad page', Hiscores.parseHash('#page=x').page, 1);
check('hash rejects odd category', Hiscores.parseHash('#cat=<b>').cat, 'overall');
check('format player hash', Hiscores.formatHash({ player: 'a b' }), '#player=a%20b');
check('format page hash', Hiscores.formatHash({ cat: 'coins', page: 2 }), '#cat=coins&page=2');
check('format first page hash', Hiscores.formatHash({ cat: 'coins', page: 1 }), '#cat=coins');
check('format rank hash', Hiscores.formatHash({ cat: 'attack', rank: 30 }), '#cat=attack&rank=30');
check('hash round trip', Hiscores.parseHash(Hiscores.formatHash({ cat: 'value', page: 4 })).page, 4);

check('span seconds', Hiscores.span(40), '40 s');
check('span minutes', Hiscores.span(720), '12 min');
check('span hours', Hiscores.span(7500), '2 h 05');
check('freshness', Hiscores.freshness(snapshot, 1720), 'updated 12 min ago · next update in 18 min');
check('freshness overdue', Hiscores.freshness(snapshot, 3000), 'updated 33 min ago · next update due now');
check('utc stamp', Hiscores.utcStamp(1794671690), '2026-11-14 15:54 UTC');
check('escape', Hiscores.esc('<a "b">'), '&lt;a &quot;b&quot;&gt;');

if (failures) {
  console.log(failures + ' FAILURE(S)');
  process.exit(1);
}
console.log('all ok');
