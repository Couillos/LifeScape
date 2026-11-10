// Offline checks for table.js (D46): numeric vs alphabetic order,
// missing values last in both directions, sort kept per table.
var TableSort = require('./table.js');

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

var numCols = [{ key: 'mean', label: 'mean', kind: 'num' }];
var rows = [{ mean: 300 }, { mean: null }, { mean: 100 }, { mean: 200 }];
check('numeric asc', TableSort.sortData(numCols, rows, 'mean', 1).map(function (r) { return r.mean; }), [100, 200, 300, null]);
check('numeric desc, missing still last', TableSort.sortData(numCols, rows, 'mean', -1).map(function (r) { return r.mean; }), [300, 200, 100, null]);

var strCols = [{ key: 'item', label: 'item', kind: 'str' }];
var items = [{ item: 'Logs' }, { item: '' }, { item: 'Feather' }, { item: 'Coal' }];
check('alpha asc', TableSort.sortData(strCols, items, 'item', 1).map(function (r) { return r.item; }), ['Coal', 'Feather', 'Logs', '']);
check('alpha desc, missing still last', TableSort.sortData(strCols, items, 'item', -1).map(function (r) { return r.item; }), ['Logs', 'Feather', 'Coal', '']);

var autoCols = [{ key: 'band', label: 'band', kind: 'auto' }, { key: 'method', label: 'method', kind: 'auto' }];
var mixed = [{ band: 6, method: 'fishing' }, { band: 1, method: 'woodcut' }, { band: null, method: 'mining' }];
check('auto numeric', TableSort.sortData(autoCols, mixed, 'band', 1).map(function (r) { return r.band; }), [1, 6, null]);
check('auto string', TableSort.sortData(autoCols, mixed, 'method', 1).map(function (r) { return r.method; }), ['fishing', 'mining', 'woodcut']);

check('unknown key keeps order', TableSort.sortData(numCols, rows, 'nope', 1).map(function (r) { return r.mean; }), [300, null, 100, 200]);
check('empty rows', TableSort.sortData(numCols, [], 'mean', 1), []);

if (failures) {
  console.log(failures + ' FAILURE(S)');
  process.exit(1);
}
console.log('table.js: all sort checks pass');
