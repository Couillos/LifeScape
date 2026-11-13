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

test('candleTooltip shows period, O/H/L/C, volume and trade count', () => {
  const lines = LifeChart.candleTooltip(1_700_000_000, 3600, { o: 4, h: 9, l: 3, c: 8, v: 120, n: 11 });
  assert.equal(lines.length, 4);
  assert.match(lines[0], /\d\d\/\d\d \d\d:\d\d–\d\d:\d\d/);
  assert.equal(lines[1], 'O 4 H 9 L 3 C 8');
  assert.equal(lines[2], 'Volume 120');
  assert.equal(lines[3], 'Trades 11');
});

test('candleTooltip tolerates rows without an execution count', () => {
  const lines = LifeChart.candleTooltip(1_700_000_000, 86400, { o: 4, h: 4, l: 4, c: 4, v: 5 });
  assert.match(lines[0], /^\d\d\/\d\d$/);
  assert.equal(lines[3], 'Trades —');
});

test('carryFlat draws one flat candle per empty slot at the last close', () => {
  const traded = { o: 2, h: 5, l: 2, c: 4, v: 10, n: 2 };
  const rows = LifeChart.carryFlat([null, traded, null, null], 3);
  assert.deepEqual(rows[0], { o: 3, h: 3, l: 3, c: 3, v: 0, n: 0 }, 'leading slot carries the previous close');
  assert.equal(rows[1], traded);
  assert.deepEqual(rows[2], { o: 4, h: 4, l: 4, c: 4, v: 0, n: 0 }, 'empty slot carries the last close');
  assert.deepEqual(rows[3], rows[2]);
  assert.deepEqual(LifeChart.carryFlat([null, null], null), [null, null], 'no known price: nothing to carry');
});
