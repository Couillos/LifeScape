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

if (failures) {
  console.log(failures + ' failure(s)');
  process.exit(1);
}
