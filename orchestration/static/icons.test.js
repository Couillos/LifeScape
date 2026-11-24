// Offline checks for icons.js pure helpers: slot markup and the CSS
// cut out of the sheets (background-position per id), cell 0 = no icon.
var LifeIcons = require('./icons.js');

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

check('item slot', LifeIcons.item(995), '<span class="ic ic-item ic-i995" aria-hidden="true"></span>');
check('item slot from a string id', LifeIcons.item('444'), '<span class="ic ic-item ic-i444" aria-hidden="true"></span>');
check('bad item id: neutral slot', LifeIcons.item('x"><b>'), '<span class="ic ic-item" aria-hidden="true"></span>');
check('missing item id: neutral slot', LifeIcons.item(undefined), '<span class="ic ic-item" aria-hidden="true"></span>');
check('stack slot: last count step reached', LifeIcons.item(995, 300), '<span class="ic ic-item ic-i995 ic-n250" aria-hidden="true"></span>');
check('stack slot: huge count', LifeIcons.item(995, 124069130), '<span class="ic ic-item ic-i995 ic-n10000" aria-hidden="true"></span>');
check('count 1: no step', LifeIcons.item(995, 1), '<span class="ic ic-item ic-i995" aria-hidden="true"></span>');
check('skill slot', LifeIcons.skill('Attack'), '<span class="ic ic-skill ic-s-attack" aria-hidden="true"></span>');
check('bad skill name: neutral slot', LifeIcons.skill('stat30'), '<span class="ic ic-skill" aria-hidden="true"></span>');

var index = {
  items: { file: 'items.png', hash: 'abc', cell: 32, cols: 3, slot: [0, 1, 0, 4], count: { 3: { 2: 5, 25: 7 } } },
  skills: { file: 'skills.png', hash: 'def', cell: 25, cols: 2, slot: { attack: 1, runecraft: 3, 'x-y': 2 } },
};
check('rules: one image per sheet, one position per drawn id, none for id 0/2 (cell 0 stays)', LifeIcons.rules(index),
  '.ic-item{background-image:url("/icons/items.png?v=abc")}'
  + '.ic-i1{background-position:-32px 0}'
  + '.ic-i3{background-position:-32px -32px}'
  + '.ic-i3.ic-n2{background-position:-64px -32px}'
  + '.ic-i3.ic-n25{background-position:-32px -64px}'
  + '.ic-skill{background-image:url("/icons/skills.png?v=def")}'
  + '.ic-s-attack{background-color:#3e3529;border-radius:3px;background-position:-25px 0}'
  + '.ic-s-runecraft{background-color:#3e3529;border-radius:3px;background-position:-25px -25px}');
check('rules without index', LifeIcons.rules(null), '');
check('rules without a sheet', LifeIcons.rules({ items: { file: 'items.png' } }), '');
check('base css sizes the slots before the index arrives', /\.ic-item\{width:32px;height:32px\}/.test(LifeIcons.BASE_CSS) && /\.ic-skill\{width:25px;height:25px\}/.test(LifeIcons.BASE_CSS), true);

if (failures) {
  console.log(failures + ' failure(s)');
  process.exit(1);
}
