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

check('skill rows', SpectatorPanels.skillRows([{ id: 0, level: 40, xp: 37224 }, { id: 3, level: 10, xp: 1154 }]),
  [{ skill: 'attack', level: 40, xp: 37224 }, { skill: 'hitpoints', level: 10, xp: 1154 }]);
check('unknown stat id', SpectatorPanels.skillRows([{ id: 30, level: 1, xp: 0 }]), [{ skill: 'stat30', level: 1, xp: 0 }]);
check('no skills', SpectatorPanels.skillRows(undefined), []);
check('21 skill names', SpectatorPanels.SKILLS.length, 21);

if (failures) {
  console.log(failures + ' failure(s)');
  process.exit(1);
}
