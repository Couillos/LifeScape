// Pure tests for market.js: chart views and the live tail on their
// slots (24 h in 15-minute candles, user 2026-11-17). Zero dependency:
// `node --test market.test.js` (the page wiring only runs in a browser).
const { test } = require('node:test');
const assert = require('node:assert/strict');
global.LifeChart = require('./chart.js');
const Market = require('./market.js');

const QUARTER = 900;

test('the 24 h view draws 96 quarter-hour candles, the others keep their resolution', () => {
  assert.deepEqual(Market.VIEWS.hour, { res: '15m', width: 900, count: 96 });
  assert.deepEqual(Market.VIEWS.week, { res: '1h', width: 3600, count: 168 });
  assert.deepEqual(Market.VIEWS.month, { res: '1d', width: 86400, count: 30 });
  assert.deepEqual(Market.VIEWS.day, { res: '1d', width: 86400, count: 183 });
  const now = 20_500 * 86400 + 13 * 3600 + 7 * 60 + 12;
  const { slots, width } = Market.viewSlots('hour', now);
  assert.equal(width, QUARTER);
  assert.equal(slots.length, 96);
  assert.equal(slots[95], 20_500 * 86400 + 13 * 3600, 'the live quarter (13:00-13:15) closes the view');
  assert.equal(slots[95] - slots[0], 95 * QUARTER, '24 h span');
  assert.ok(slots.every((start, index) => index === 0 || start - slots[index - 1] === QUARTER));
});

test('carryFlat chains the 96 quarters: leading flats at prev_close, gaps flat at the last close', () => {
  const now = 20_500 * 86400 + 13 * 3600 + 7 * 60;
  const { slots, width } = Market.viewSlots('hour', now);
  const history = [
    { t: slots[10], o: 15, h: 15, l: 11, c: 11, v: 4, vb: 4, vs: 0, n: 2 },
    { t: slots[40], o: 20, h: 20, l: 20, c: 20, v: 1, vb: 0, vs: 1, n: 1 },
  ];
  const rows = Market.chartRows(slots, width, history, [], 12, now);
  assert.equal(rows.length, 96);
  assert.ok(rows.every(row => row !== null), 'no gap once a price is known');
  assert.deepEqual([rows[0].o, rows[0].c, rows[0].v], [12, 12, 0]);
  assert.deepEqual([rows[10].o, rows[10].h, rows[10].l, rows[10].c], [12, 15, 11, 11], 'opens at the previous close');
  assert.deepEqual([rows[11].o, rows[39].c], [11, 11]);
  assert.deepEqual([rows[40].o, rows[40].h, rows[40].c], [11, 20, 20]);
  assert.deepEqual([rows[95].o, rows[95].c, rows[95].v], [20, 20, 0], 'live quarter flat until a print');
  assert.equal(history[0].o, 15, 'history rows never mutated');
});

test('WS prints fold into the live quarter, not the live hour', () => {
  const now = 20_500 * 86400 + 13 * 3600 + 7 * 60;
  const { slots, width } = Market.viewSlots('hour', now);
  const loadedTo = slots[95] + 200;
  const history = [{ t: slots[94], o: 8, h: 8, l: 8, c: 8, v: 1, vb: 1, vs: 0, n: 1 }];
  const prints = [
    { t: slots[95] + 400, q: 3, p: 10, s: 'buy' },
    { t: slots[95] + 300, q: 2, p: 7, s: 'sell' },
    { t: loadedTo - 5, q: 9, p: 99, s: 'buy' },
  ];
  const rows = Market.chartRows(slots, width, history, prints, null, loadedTo);
  assert.deepEqual(rows[95], { t: slots[95], o: 8, h: 10, l: 7, c: 10, v: 5, vb: 3, vs: 2, n: 2 },
    'opens at the previous quarter close, closes on the freshest print; the already-loaded print is skipped');
  assert.deepEqual([rows[94].o, rows[94].c, rows[94].v], [8, 8, 1]);
  assert.equal(rows[93], null, 'nothing known before the first trade');
});

test('a market maker pseudo is gold, any other plain and escaped', () => {
  assert.equal(Market.partyHtml('jim_ford', true), '<span class="maker" title="market maker">jim_ford</span>');
  assert.equal(Market.partyHtml('eva_dale', false), 'eva_dale');
  assert.equal(Market.partyHtml('<b>', false), '&lt;b&gt;');
});
