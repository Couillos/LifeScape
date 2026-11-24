// Offline checks for spectator.js pure helpers: error lines and the
// game tabs (backpack, worn, stats) draw lists.
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

check('error age hours', SpectatorPanels.errorAge({ at_unix: 1000 }, 1000 + 2 * 3600 + 13 * 60 + 5), 'il y a 2 h 13 min');
check('error age minutes', SpectatorPanels.errorAge({ at_unix: 1000 }, 1300), 'il y a 5 min');
check('error age seconds', SpectatorPanels.errorAge({ at_unix: 1000 }, 1045), 'il y a 45 s');
check('error age without time', SpectatorPanels.errorAge({ type: 'Blocked' }, 1045), '');
check('error line carries its age', SpectatorPanels.errorLine('last error', { at_unix: 1000, type: 'X' }, 1300), 'last error: {"at_unix":1000,"type":"X"} (il y a 5 min)');
check('no error, no line', SpectatorPanels.errorLine('last error', null, 5), null);
// ---- game tabs ----
var P = SpectatorPanels;
check('xp table: level 2 at 83', P.LEVEL_XP[0], 83);
check('xp table: level 10 at 1154', P.LEVEL_XP[8], 1154);
check('xp table: level 99 at 13034431', P.LEVEL_XP[97], 13034431);
check('xp table: 99 entries, the last one past 99', [P.LEVEL_XP.length, P.LEVEL_XP[98]], [99, 14391160]);
check('stack count under 100000 as is', P.invNumber(99999), '99999');
check('stack count in K from 100000', P.invNumber(100000), '100K');
check('stack count in K floors', P.invNumber(9999999), '9999K');
check('stack count in M from 10 million', P.invNumber(10000000), '10M');
check('stack count M floors', P.invNumber(2147483647), '2147M');
check('%1 value', P.inf(13034431), '13034431');
check('%1 value capped', P.inf(999999999), '*');

var pack = { x: 16, y: 8, cols: 4, rows: 7, mx: 10, my: 4, off: [] };
for (var i = 0; i < 20; i++) pack.off.push([0, 0]);
check('backpack slot 0', P.slotXY(pack, 0), { x: 16, y: 8 });
check('backpack slot 5: second row, second column', P.slotXY(pack, 5), { x: 58, y: 44 });
check('backpack slot 27: past the 20 offsets', P.slotXY(pack, 27), { x: 142, y: 224 });
check('backpack hover: inside slot 5', P.slotAt(pack, 58 + 31, 44 + 31), 5);
check('backpack hover: in the margin between slots', P.slotAt(pack, 50, 20), -1);
var worn = { x: 24, y: 44, cols: 3, rows: 5, mx: 24, my: 8, off: [[56, -38], [-41, 1], [-56, 1], [0, 0], [0, 0], [0, 0], [0, 0], [0, 0], [0, 0], [0, 0], [0, 0], [-55, 40], [112, -40], [41, -159]] };
check('worn head: grid slot 0 plus its offset', P.slotXY(worn, 0), { x: 80, y: 6 });
check('worn ammo: slot 13 plus its offset', P.slotXY(worn, 13), { x: 121, y: 45 });

check('items in their slot, gaps kept', P.placeItems([{ id: 1, slot: 0 }, { id: 2, slot: 5 }], 6, null).map(function (it) { return it && it.id; }), [1, null, null, null, null, 2]);
check('older worker pack: list order, packed', P.placeItems([{ id: 7 }, { id: 8 }], 3, null).map(function (it) { return it && it.id; }), [7, 8, null]);
check('older worker worn: the wearpos', P.placeItems([{ id: 1161 }, { id: 882 }], 14, { 1161: 0, 882: 13 }).map(function (it) { return it && it.id; }),
  [1161, null, null, null, null, null, null, null, null, null, null, null, null, 882]);
check('slot taken or out of range: first free slot', P.placeItems([{ id: 1, slot: 0 }, { id: 2, slot: 0 }, { id: 3, slot: 40 }], 3, null).map(function (it) { return it && it.id; }), [1, 2, 3]);
check('garbage entries skipped', P.placeItems([null, 'x', { name: 'no id' }], 2, null), [null, null]);

var stack = new Set([995]);
check('count shown for a stackable at 1', P.showsCount(stack, 995, 1), true);
check('count hidden for a single plain item', P.showsCount(stack, 1161, 1), false);
check('count shown for a plain item above 1', P.showsCount(stack, 1161, 2), true);

var hovers = [{ x: 0, y: 2, w: 64, h: 31, layer: 'a' }, { x: 64, y: 2, w: 64, h: 31, layer: 'b' }, { x: 60, y: 2, w: 10, h: 10, layer: 'c' }];
check('hover layer under the point', P.hoverLayer(hovers, 10, 10), 'a');
check('hover layer: the last match wins', P.hoverLayer(hovers, 65, 5), 'c');
check('hover layer: none', P.hoverLayer(hovers, 10, 40), null);

var snap = { combat: 72, quest_points: 12, skills: [{ id: 0, level: 45, base: 40, xp: 37224 }, { id: 3, level: 4, base: 10, xp: 1154 }, { id: 18, level: 50, base: 50, xp: 101333 }, { id: 20, level: 99, base: 99, xp: 13100000 }] };
check('stat_level: current level', P.ifValue({ op: 'stat_level', arg: 0 }, snap), 45);
check('stat_base_level: base', P.ifValue({ op: 'stat_base_level', arg: 3 }, snap), 10);
check('stat_xp', P.ifValue({ op: 'stat_xp', arg: 3 }, snap), 1154);
check('stat_xp_remaining: xp of the next level', P.ifValue({ op: 'stat_xp_remaining', arg: 0 }, snap), 41171);
check('stat_xp_remaining at 99: the client reads entry 98', P.ifValue({ op: 'stat_xp_remaining', arg: 20 }, snap), 14391160);
check('total level: base levels of the used stats (slayer out)', P.ifValue({ op: 'total_level' }, snap), 149);
check('combat level', P.ifValue({ op: 'combat_level' }, snap), 72);
check('quest points', P.ifValue({ op: 'pushvar', arg: 101, varp: 'qp' }, snap), 12);
check('quest points missing (older worker)', P.ifValue({ op: 'pushvar', arg: 101, varp: 'qp' }, { skills: [] }), null);
check('stat missing', P.ifValue({ op: 'stat_level', arg: 7 }, snap), null);
check('text filled', P.ifText({ text: 'Total Lvl: %1' }, 149), 'Total Lvl: 149');
check('text without value left out', P.ifText({ text: 'QP: %1' }, null), null);

var font = { height: 10, chars: { 49: ['f:49', 1, 2, 6], 50: ['f:50', 0, 1, 7], 32: [null, 0, 0, 3] } };
check('string width skips tags', P.stringWidth(font, '@red@1 2'), 16);
check('glyphs: shadow then colour, baseline minus height', P.glyphs(font, '1', 10, 20, 0xffff00, true, false),
  [{ glyph: 'f:49', x: 12, y: 13, colour: 0 }, { glyph: 'f:49', x: 11, y: 12, colour: 0xffff00 }]);
check('glyphs: centred, colour tag, space advances only', P.glyphs(font, '1 @lre@2', 20, 10, 0xffffff, false, true),
  [{ glyph: 'f:49', x: 13, y: 2, colour: 0xffffff }, { glyph: 'f:50', x: 21, y: 1, colour: 0xff9040 }]);

var index = {
  items: { wear: { 1161: 0 } },
  tabs: {
    images: {},
    fonts: { p11: font, p12: font },
    tabs: {
      inventory: { w: 190, h: 261, image: 'inventory', inv: { x: 16, y: 8, cols: 4, rows: 7, mx: 10, my: 4, off: pack.off, empty: [] }, texts: [], hovers: [], layers: {} },
      wornitems: { w: 190, h: 200, image: 'wornitems', inv: { x: 24, y: 44, cols: 3, rows: 5, mx: 24, my: 8, off: worn.off, empty: ['w:0', 'w:1'] }, texts: [], hovers: [], layers: {} },
      stats: { w: 190, h: 261, image: 'stats', inv: null,
        texts: [{ font: 'p11', x: 39, y: 15, centre: true, shadow: false, colour: 0xffff00, text: '%1', op: 'stat_level', arg: 0 }],
        hovers: [{ x: 0, y: 2, w: 64, h: 31, layer: '9' }],
        layers: { 9: { x: 2, y: 216, image: 'stats:layer9', texts: [{ font: 'p12', x: 66, y: 19, centre: false, shadow: false, colour: 0xffff00, text: '%1', op: 'stat_xp', arg: 0 }] } } },
    },
  },
};
var cellOf = function (id, count) { return id * 10 + (count > 1 ? 1 : 0); };
check('backpack draw: panel, icon in its slot, the stack count at +9', P.tabDraw(index, 'inventory', { inv: [{ id: 995, count: 12, slot: 5 }] }, null, cellOf, stack),
  [{ image: 'inventory', x: 0, y: 0 }, { item: 9951, x: 58, y: 44 }, { glyph: 'f:49', x: 60, y: 46, colour: 0 }, { glyph: 'f:49', x: 59, y: 45, colour: 0xffff00 },
    { glyph: 'f:50', x: 65, y: 45, colour: 0 }, { glyph: 'f:50', x: 64, y: 44, colour: 0xffff00 }]);
check('worn draw: empty slots show their sprite, a worn one its item', P.tabDraw(index, 'wornitems', { worn: [{ id: 1161, count: 1, slot: 0 }] }, null, cellOf, stack),
  [{ image: 'wornitems', x: 0, y: 0 }, { item: 11610, x: 80, y: 6 }, { image: 'w:1', x: 39, y: 45 }]);
check('stats draw: level text, no layer', P.tabDraw(index, 'stats', { skills: [{ id: 0, level: 1, base: 1, xp: 0 }] }, null, cellOf, stack),
  [{ image: 'stats', x: 0, y: 0 }, { glyph: 'f:49', x: 37, y: 7, colour: 0xffff00 }]);
check('stats draw: hovered layer on top with its texts', P.tabDraw(index, 'stats', { skills: [{ id: 0, level: 1, base: 1, xp: 2 }] }, '9', cellOf, stack).slice(2),
  [{ image: 'stats:layer9', x: 2, y: 216 }, { glyph: 'f:50', x: 68, y: 226, colour: 0xffff00 }]);
check('item label: name in the menu colour', P.itemLabel({ id: 1161, name: 'Adamant full helm', count: 1 }, stack), '@lre@Adamant full helm');
check('item label: a stack with its full count', P.itemLabel({ id: 995, name: 'Coins', count: 4290 }, stack), '@lre@Coins@whi@ x 4,290');

if (failures) {
  console.log(failures + ' failure(s)');
  process.exit(1);
}
