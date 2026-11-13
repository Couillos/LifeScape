// Pure tests for chart.js (PRD2 §6.1/§6.2): fixed slots, empty slots,
// graduations, tooltip. Zero dependency: `node --test chart.test.js`.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const LifeChart = require('./chart.js');

test('slotStarts builds fixed windows from an aligned start', () => {
  assert.deepEqual(LifeChart.slotStarts(1_000 * 3600, 3600, 24).length, 24);
  assert.deepEqual(LifeChart.slotStarts(1_000 * 3600, 3600, 3), [1_000 * 3600, 1_001 * 3600, 1_002 * 3600]);
  assert.deepEqual(LifeChart.slotStarts(200 * 86400, 86400, 183).length, 183);
});

test('fillSlots aligns sparse candles and keeps empty slots null', () => {
  const slots = [100, 200, 300];
  const rows = LifeChart.fillSlots(slots, [{ t: 100, c: 1 }, { t: 300, c: 3 }]);
  assert.equal(rows.length, 3);
  assert.equal(rows[0].c, 1);
  assert.equal(rows[1], null);
  assert.equal(rows[2].c, 3);
});

test('fillSlots keeps every slot empty when nothing was traded', () => {
  assert.deepEqual(LifeChart.fillSlots([100, 200], []), [null, null]);
});

test('niceTicks graduates a price range with few round ticks', () => {
  const ticks = LifeChart.niceTicks(4, 9, 4);
  assert.ok(ticks.length >= 2 && ticks.length <= 6, `got ${ticks}`);
  assert.ok(ticks[0] <= 4 && ticks[ticks.length - 1] >= 9, `got ${ticks}`);
  const step = ticks[1] - ticks[0];
  assert.ok(ticks.every((tick) => Math.abs(tick / step - Math.round(tick / step)) < 1e-9), `got ${ticks}`);
});

test('niceTicks widens a flat range instead of collapsing', () => {
  const ticks = LifeChart.niceTicks(7, 7, 4);
  assert.ok(ticks[0] < 7 && ticks[ticks.length - 1] > 7, `got ${ticks}`);
});

test('niceTicks graduates a volume range from zero', () => {
  const ticks = LifeChart.niceTicks(0, 900, 2);
  assert.equal(ticks[0], 0);
  assert.ok(ticks[ticks.length - 1] >= 900, `got ${ticks}`);
  assert.ok(ticks.length <= 4, `got ${ticks}`);
});

test('candleTooltip shows period, O/H/L/C, volume, buy/sell volume and trade count', () => {
  const lines = LifeChart.candleTooltip(1_700_000_000, 3600, { o: 4, h: 9, l: 3, c: 8, v: 120, n: 11 });
  assert.equal(lines.length, 6);
  assert.match(lines[0], /\d\d\/\d\d \d\d:\d\d–\d\d:\d\d/);
  assert.equal(lines[1], 'O 4 H 9 L 3 C 8');
  assert.equal(lines[2], 'Volume 120');
  assert.deepEqual(lines.slice(3), ['Buy vol 0', 'Sell vol 0', 'Trades 11'], 'no split in the row: 0/0');
});

test('candleTooltip tolerates rows without an execution count', () => {
  const lines = LifeChart.candleTooltip(1_700_000_000, 86400, { o: 4, h: 4, l: 4, c: 4, v: 5 });
  assert.match(lines[0], /^\d\d\/\d\d$/);
  assert.equal(lines[5], 'Trades —');
});

test('carryFlat draws one flat candle per empty slot at the last close', () => {
  const traded = { o: 2, h: 5, l: 2, c: 4, v: 10, n: 2 };
  const rows = LifeChart.carryFlat([null, traded, null, null], 3);
  assert.deepEqual(rows[0], { o: 3, h: 3, l: 3, c: 3, v: 0, vb: 0, vs: 0, n: 0 }, 'leading slot carries the previous close');
  assert.deepEqual(rows[1], { o: 3, h: 5, l: 2, c: 4, v: 10, n: 2 }, 'opens at the previous close');
  assert.deepEqual(rows[2], { o: 4, h: 4, l: 4, c: 4, v: 0, vb: 0, vs: 0, n: 0 }, 'empty slot carries the last close');
  assert.deepEqual(rows[3], rows[2]);
  assert.deepEqual(LifeChart.carryFlat([null, null], null), [null, null], 'no known price: nothing to carry');
});

test('carryFlat opens every candle at the previous close (Wizards hat, Bronze axe)', () => {
  // Wizards hat: 135, 101, empty hour, 59 -> 101 and 59 open red.
  const hat = LifeChart.carryFlat([
    { o: 135, h: 135, l: 135, c: 135, v: 1 },
    { o: 101, h: 101, l: 101, c: 101, v: 1 },
    null,
    { o: 59, h: 59, l: 59, c: 59, v: 2 },
  ], null);
  assert.deepEqual(hat.map(row => [row.o, row.h, row.l, row.c]), [[135, 135, 135, 135], [135, 135, 101, 101], [101, 101, 101, 101], [101, 101, 59, 59]]);
  assert.equal(LifeChart.candleColor(hat[1]), LifeChart.DOWN);
  assert.equal(LifeChart.candleColor(hat[2]), LifeChart.FLAT);
  assert.equal(LifeChart.candleColor(hat[3]), LifeChart.DOWN);
  // Bronze axe: 16 16 12 20 then 8 8 8 6 -> the second hour opens at 20.
  const axe = LifeChart.carryFlat([{ o: 16, h: 20, l: 12, c: 20, v: 4 }, { o: 8, h: 8, l: 6, c: 6, v: 4 }], null);
  assert.deepEqual([axe[1].o, axe[1].h, axe[1].l, axe[1].c], [20, 20, 6, 6]);
  const input = { o: 8, h: 8, l: 6, c: 6, v: 4 };
  LifeChart.carryFlat([{ o: 1, h: 1, l: 1, c: 20, v: 1 }, input], null);
  assert.equal(input.o, 8, 'never mutates the rows it reads');
});

test('liveSlots ends on the slot holding now (the live candle)', () => {
  const hour = 3600;
  const now = 1_000 * hour + 1_234;
  const slots = LifeChart.liveSlots(now, hour, 24);
  assert.equal(slots.length, 24);
  assert.equal(slots[23], 1_000 * hour, 'the current hour is drawn');
  assert.equal(slots[0], 977 * hour);
  assert.equal(LifeChart.liveSlots(200 * 86400 + 5, 86400, 183)[182], 200 * 86400);
});

test('carryFlat flat candles carry a zero buy/sell split', () => {
  const rows = LifeChart.carryFlat([null], 5);
  assert.deepEqual(rows[0], { o: 5, h: 5, l: 5, c: 5, v: 0, vb: 0, vs: 0, n: 0 });
});

test('candleTooltip lists buy and sell volume', () => {
  const lines = LifeChart.candleTooltip(1_700_000_000, 3600, { o: 4, h: 9, l: 3, c: 8, v: 120, vb: 70, vs: 40, n: 11 });
  assert.ok(lines.includes('Buy vol 70'), `got ${lines}`);
  assert.ok(lines.includes('Sell vol 40'), `got ${lines}`);
  assert.equal(lines[lines.length - 1], 'Trades 11');
});

test('volumeSplit keeps the unknown side apart, never guessed', () => {
  assert.deepEqual(LifeChart.volumeSplit({ v: 120, vb: 70, vs: 40 }), { buy: 70, sell: 40, unknown: 10 });
  assert.deepEqual(LifeChart.volumeSplit({ v: 9 }), { buy: 0, sell: 0, unknown: 9 }, 'old payload: all unknown');
  assert.deepEqual(LifeChart.volumeSplit({ v: 0, vb: 0, vs: 0 }), { buy: 0, sell: 0, unknown: 0 });
});

test('volumeScaleMax: one scale for both sides, grey stacked on the buy side', () => {
  const rows = [null, { v: 10, vb: 3, vs: 2 }, { v: 12, vb: 0, vs: 12 }];
  assert.equal(LifeChart.volumeScaleMax(rows), 12, 'sell 12 outgrows buy 3 + unknown 5');
  assert.equal(LifeChart.volumeScaleMax([{ v: 9, vb: 4, vs: 1 }]), 8, 'buy 4 + unknown 4 above the baseline');
  assert.equal(LifeChart.volumeScaleMax([null]), 0);
});

test('foldPrint splits the live tail by taker side', () => {
  const tail = { o: 5, h: 5, l: 5, c: 5, v: 0, vb: 0, vs: 0, n: 0 };
  LifeChart.foldPrint(tail, { q: 4, p: 7, s: 'buy' });
  LifeChart.foldPrint(tail, { q: 2, p: 3, s: 'sell' });
  LifeChart.foldPrint(tail, { q: 1, p: 6, s: null });
  assert.deepEqual(tail, { o: 5, h: 7, l: 3, c: 6, v: 7, vb: 4, vs: 2, n: 3 });
  const old = { o: 2, h: 2, l: 2, c: 2, v: 3, n: 1 };
  LifeChart.foldPrint(old, { q: 1, p: 2, s: 'buy' });
  assert.deepEqual(old, { o: 2, h: 2, l: 2, c: 2, v: 4, vb: 1, vs: 0, n: 2 }, 'rows without a split start at zero');
});
