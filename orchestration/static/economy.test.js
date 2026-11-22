// Pure tests for economy.js: cancelled / failed transactions block.
// Zero dependency: `node --test economy.test.js`.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const Economy = require('./economy.js');

const outcomes = {
  deals: [{ key: '1h', label: '1 h', settled: 6, lost: 2, open: 1, lost_percent: 25,
    reasons: [{ reason: 'timeout', count: 1 }, { reason: 'no_show', count: 1 }] }],
  orders: [{ key: '24h', label: '24 h', settled: 1, lost: 3, open: 4, lost_percent: 75,
    reasons: [{ reason: 'need_met', count: 3 }] }],
};

test('percentLabel shows a dash when nothing is final', () => {
  assert.equal(Economy.percentLabel(null), '-');
  assert.equal(Economy.percentLabel(25), '25.0 %');
});

test('reasonLabel translates known reasons and keeps unknown ones', () => {
  assert.equal(Economy.reasonLabel('need_met'), 'besoin satisfait');
  assert.equal(Economy.reasonLabel('brand_new'), 'brand_new');
  assert.equal(Economy.reasonLabel(''), 'sans motif (anciens ordres)');
});

test('outcomesHtml renders both blocks with percentages and reason shares', () => {
  const html = Economy.outcomesHtml(outcomes);
  assert.match(html, /Échanges entre bots/);
  assert.match(html, /Ordres posés au marché/);
  assert.match(html, /25\.0 %/);
  assert.match(html, /75\.0 %/);
  assert.match(html, /besoin satisfait 3 <span class="muted">\(100 %\)/);
  assert.match(html, /délai dépassé 1 <span class="muted">\(50 %\)/);
  assert.equal(Economy.outcomesHtml(undefined), '');
});
