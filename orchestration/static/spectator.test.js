// Offline checks for spectator.js pure helpers: skills table rows.
var SpectatorPanels = require('./spectator.js');

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

check('skill rows', SpectatorPanels.skillRows([{ id: 0, level: 40, base: 40, xp: 37224 }, { id: 3, level: 4, base: 10, xp: 1154 }]),
  [{ skill: 'attack', level: 40, base: 40, xp: 37224 }, { skill: 'hitpoints', level: 10, base: 10, xp: 1154 }]);
check('level column is the base level (hitpoints = level, never hp left)', SpectatorPanels.skillRows([{ id: 3, level: 4, base: 10, xp: 1154 }])[0].level, 10);
check('level falls back to current without base', SpectatorPanels.skillRows([{ id: 0, level: 40, xp: 37224 }])[0].level, 40);
check('unknown stat id', SpectatorPanels.skillRows([{ id: 30, level: 1, base: 1, xp: 0 }]), [{ skill: 'stat30', level: 1, base: 1, xp: 0 }]);
check('totals sum base levels, not current', SpectatorPanels.skillTotals(SpectatorPanels.skillRows([{ id: 0, level: 45, base: 40, xp: 37224 }, { id: 3, level: 4, base: 10, xp: 1154 }])),
  { skill: 'Total', level: 50, xp: 38378 });
check('totals without base', SpectatorPanels.skillTotals([{ skill: 'attack', level: 40, xp: 37224 }]), { skill: 'Total', level: null, xp: 37224 });
check('totals of no skills', SpectatorPanels.skillTotals([]), { skill: 'Total', level: 0, xp: 0 });
check('no skills', SpectatorPanels.skillRows(undefined), []);
check('21 skill names', SpectatorPanels.SKILLS.length, 21);

check('error age hours', SpectatorPanels.errorAge({ at_unix: 1000 }, 1000 + 2 * 3600 + 13 * 60 + 5), 'il y a 2 h 13 min');
check('error age minutes', SpectatorPanels.errorAge({ at_unix: 1000 }, 1300), 'il y a 5 min');
check('error age seconds', SpectatorPanels.errorAge({ at_unix: 1000 }, 1045), 'il y a 45 s');
check('error age without time', SpectatorPanels.errorAge({ type: 'Blocked' }, 1045), '');
check('error line carries its age', SpectatorPanels.errorLine('last error', { at_unix: 1000, type: 'X' }, 1300), 'last error: {"at_unix":1000,"type":"X"} (il y a 5 min)');
check('no error, no line', SpectatorPanels.errorLine('last error', null, 5), null);
check('inventory lines carry the item id for the icon', SpectatorPanels.inventoryLines(
  [{ id: 444, count: 1, name: 'Gold ore' }, { id: 995, count: 250, name: 'Coins' }], [{ id: 1161, count: 1, name: 'Adamant full helm' }]),
  [{ text: 'Backpack:' }, { id: 444, count: 1, text: 'Gold ore' }, { id: 995, count: 250, text: '250x Coins' }, { text: '' }, { text: 'Worn:' }, { id: 1161, count: 1, text: 'Adamant full helm' }]);
check('inventory lines when empty', SpectatorPanels.inventoryLines([], null),
  [{ text: 'Backpack:' }, { text: '(empty)' }, { text: '' }, { text: 'Worn:' }, { text: '(nothing)' }]);

if (failures) {
  console.log(failures + ' failure(s)');
  process.exit(1);
}
