// Offline checks for items.js pure helpers (ItemFlows): readable
// provenance, period windows, signed quantities, flow table, the
// discreet line when the route is missing.
var ItemFlows = require('./items.js');

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

check('label produced', ItemFlows.label('produced'), 'produit');
check('label shop bought', ItemFlows.label('shop_bought'), 'acheté au PNJ');
check('label traded', ItemFlows.label('traded'), 'échangé entre joueurs');
check('label unattributed', ItemFlows.label('unattributed'), 'non attribué');
check('label unknown kept raw', ItemFlows.label('mystery'), 'mystery');
check('every provenance labelled', ['produced', 'loot', 'ground_spawn', 'shop_bought', 'shop_sold', 'traded', 'consumed', 'lost', 'death', 'fee', 'unattributed']
  .filter(function (key) { return !ItemFlows.LABELS[key]; }), []);

check('period hours', ['day', 'week', 'month', 'half'].map(ItemFlows.hoursFor), [24, 168, 720, 4380]);
check('unknown period falls back to 24 h', ItemFlows.hoursFor('nope'), 24);
check('flows url', ItemFlows.flowsUrl(995, 'week'), '/api/items/flows?item=995&hours=168');

check('quantity in', ItemFlows.quantityCell(12), '<span class="up">+12</span>');
check('quantity out', ItemFlows.quantityCell(-7), '<span class="down">−7</span>');
check('quantity zero', ItemFlows.quantityCell(0), '<span class="muted">0</span>');

var html = ItemFlows.tableHtml({ ok: true, flows: [
  { item: 'Logs', provenance: 'produced', quantity: 15, lines: 2 },
  { item: 'Logs', provenance: 'shop_sold', quantity: -10, lines: 1 },
] });
check('table header', html.indexOf('<th class="left">Provenance / destination</th><th>Quantité</th><th>Rapports</th>') >= 0, true);
check('table readable row', html.indexOf('>produit</td><td><span class="up">+15</span></td><td>2</td>') >= 0, true);
check('table outgoing row', html.indexOf('>vendu au PNJ</td><td><span class="down">−10</span></td><td>1</td>') >= 0, true);
check('table net row', html.indexOf('<tr class="net"><td class="left">net</td><td><span class="up">+5</span></td><td>3</td></tr>') >= 0, true);
check('table escapes', ItemFlows.tableHtml({ flows: [{ provenance: '<b>', quantity: 1, lines: 1 }] }).indexOf('<b>') < 0, true);
check('empty table', ItemFlows.tableHtml({ ok: true, flows: [] }), '<tr><td class="left muted">aucun flux enregistré sur la période</td></tr>');

check('no flows: an empty table, no message', ItemFlows.unavailableHtml(404, null), '');
check('error: empty too', ItemFlows.unavailableHtml(200, 'bad hours'), '');

var ItemIndices = ItemFlows.ItemIndices;
var indices = [
  { key: 'cooking', label: 'Cooking', count: 40, change24h: 1000, ids: [317, 318] },
  { key: 'food', label: 'Food', count: 5, change24h: null, ids: [315] },
];
var indexHtml = ItemIndices.tableHtml(indices, 'food');
check('index header', indexHtml.indexOf('<th class="left">Index</th><th>Qty</th><th>24 h</th>') >= 0, true);
check('index row', indexHtml.indexOf('<tr data-index="cooking"><td class="left">Cooking</td><td>40</td><td><span class="up">+100.0%</span></td></tr>') >= 0, true);
check('current index bold, no baseline dash', indexHtml.indexOf('<tr data-index="food" class="cur"><td class="left">Food</td><td>5</td><td><span class="muted">—</span></td></tr>') >= 0, true);
check('no index', ItemIndices.tableHtml([], null), '<tr><td class="left muted">no index</td></tr>');
check('history url total', ItemIndices.historyUrl('1h', null, null), '/api/items/history?res=1h');
check('history url index', ItemIndices.historyUrl('1d', null, 'melee_equipment'), '/api/items/history?res=1d&index=melee_equipment');
check('history url item wins', ItemIndices.historyUrl('1h', 995, 'food'), '/api/items/history?res=1h&item=995');
check('find index', ItemIndices.find(indices, 'food').label, 'Food');
check('find unknown', ItemIndices.find(indices, 'nope'), null);
check('index keeps its members', ItemIndices.keep([{ id: 317 }, { id: 995 }, { id: 318 }], indices[0]).map(function (item) { return item.id; }), [317, 318]);
check('no index keeps all', ItemIndices.keep([{ id: 995 }], null).length, 1);

if (failures) {
  console.log(failures + ' FAILURE(S)');
  process.exit(1);
}
console.log('items.js: all flow and index checks pass');
