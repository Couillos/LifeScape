// Pure tests for brain.js (Lives page, "Cerveau" tab): ranks from deps,
// no overlapping boxes in a layer, collapsed done subtrees, filed edges.
// Zero dependency: `node orchestration/static/brain.test.js`.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const BrainLayout = require('./brain.js');

function task(id, deps, extra) {
  return Object.assign({ id: id, kind: 'reach_level', label: id, status: 'pending', progress: [0, 1], deps: deps, filed: [], alternatives: [] }, extra || {});
}

// n4 (top) needs n2; n2 needs n1 and n3 (n3 filed when n2 was blocked);
// n5 needs n1 directly and n2: the longest path wins for n1.
const NODES = [
  task('n1', []),
  task('n2', ['n1', 'n3'], { filed: ['n3'], alternatives: [{ method: 'a', est_ticks: 1, est_coins: 0, chosen: true }, { method: 'b', est_ticks: 2, est_coins: 0, chosen: false }] }),
  task('n3', [], { kind: 'obtain_items', item: 'net' }),
  task('n4', ['n2']),
  task('n5', ['n1', 'n2']),
];

test('ranks follow the longest dependency path from a top task', () => {
  const rank = BrainLayout.ranks(NODES);
  assert.equal(rank.n4, 0);
  assert.equal(rank.n5, 0);
  assert.equal(rank.n2, 1);
  assert.equal(rank.n3, 2);
  assert.equal(rank.n1, 2, 'n5 → n1 is shorter than n5 → n2 → n1');
  for (const node of NODES) {
    for (const dep of node.deps) assert.ok(rank[dep] > rank[node.id], dep + ' right of ' + node.id);
  }
});

test('a dependency cycle never loops forever', () => {
  const rank = BrainLayout.ranks([task('n1', ['n2']), task('n2', ['n1']), task('n3', ['n3'])]);
  assert.equal(typeof rank.n1, 'number');
  assert.equal(typeof rank.n2, 'number');
  assert.equal(rank.n3, 0, 'self dependency ignored');
});

function assertNoOverlap(model) {
  const byCol = {};
  for (const box of model.boxes) (byCol[box.col] = byCol[box.col] || []).push(box);
  for (const col of Object.keys(byCol)) {
    const boxes = byCol[col].slice().sort((left, right) => left.y - right.y);
    for (let index = 1; index < boxes.length; index++) {
      assert.ok(boxes[index - 1].y + boxes[index - 1].h <= boxes[index].y, 'column ' + col + ': ' + boxes[index - 1].id + ' overlaps ' + boxes[index].id);
    }
    const columnX = boxes[0].x;
    for (const box of boxes) assert.equal(box.x, columnX, 'one x per column');
  }
  const cols = Object.keys(byCol).map(Number).sort((left, right) => left - right);
  for (let index = 1; index < cols.length; index++) {
    const left = byCol[cols[index - 1]];
    const right = byCol[cols[index]];
    const leftEdge = Math.max(...left.map((box) => box.x + box.w));
    assert.ok(leftEdge < right[0].x, 'columns never overlap');
  }
}

test('layout without ambition: goal, then tasks by rank, no overlap', () => {
  const model = BrainLayout.layout({ ambition: null, goal: { label: 'fishing 20' }, nodes: NODES }, { collapsed: [] });
  const col = {};
  for (const box of model.boxes) col[box.id] = box.col;
  assert.equal(col.goal, 0);
  assert.equal(col.n4, 1);
  assert.equal(col.n2, 2);
  assert.equal(col.n1, 3);
  assertNoOverlap(model);
  const kinds = model.edges.map((edge) => edge.from + '>' + edge.to + ':' + edge.kind).sort();
  assert.ok(kinds.includes('n2>n3:filed'), 'orange edge to the filed sub-task');
  assert.ok(kinds.includes('n2>n1:dep'));
  assert.ok(kinds.includes('goal>n4:goal') && kinds.includes('goal>n5:goal'), 'goal feeds every top task');
  assert.ok(model.width > 0 && model.height > 0);
});

test('layout with ambition: ambition, milestones, goal, then tasks', () => {
  const ambition = {
    kind: 'skiller', label: 'Master fisher',
    milestones: [{ id: 'm1', label: 'fishing 10', status: 'done' }, { id: 'm2', label: 'fishing 20', status: 'active' }, { id: 'm3', label: 'cook 20', status: 'pending' }],
  };
  const model = BrainLayout.layout({ ambition: ambition, goal: { label: 'fishing 20', origin_milestone: 'm2' }, nodes: NODES }, { collapsed: [] });
  const col = {};
  for (const box of model.boxes) col[box.id] = box.col;
  assert.deepEqual([col.ambition, col['m:m1'], col['m:m3'], col.goal, col.n4], [0, 1, 1, 2, 3]);
  assertNoOverlap(model);
  assert.ok(model.edges.some((edge) => edge.from === 'm:m2' && edge.to === 'goal' && edge.kind === 'origin'));
  assert.equal(model.edges.filter((edge) => edge.kind === 'plan').length, 3);
});

test('done subtrees collapse and reopen', () => {
  const nodes = [
    task('n1', [], { status: 'done' }),
    task('n2', ['n1'], { status: 'done' }),
    task('n3', ['n2'], { status: 'active' }),
    task('n4', ['n1'], { status: 'pending' }),
  ];
  assert.deepEqual(BrainLayout.collapsibleDone(nodes), ['n2']);
  const shut = BrainLayout.visibleNodes(nodes, ['n2']).map((node) => node.id).sort();
  assert.deepEqual(shut, ['n1', 'n2', 'n3', 'n4'], 'n1 stays: open n4 still needs it');
  const lonely = [task('n1', [], { status: 'done' }), task('n2', ['n1'], { status: 'done' }), task('n3', ['n2'])];
  assert.deepEqual(BrainLayout.visibleNodes(lonely, ['n2']).map((node) => node.id), ['n2', 'n3']);
  assert.deepEqual(BrainLayout.visibleNodes(lonely, []).map((node) => node.id), ['n1', 'n2', 'n3']);
  const model = BrainLayout.layout({ goal: {}, nodes: lonely }, { collapsed: ['n2'] });
  assert.ok(!model.boxes.some((box) => box.id === 'n1'));
  assert.ok(!model.edges.some((edge) => edge.to === 'n1'), 'no edge to a hidden box');
});

test('abandoned tasks can be hidden with what only they reach', () => {
  const nodes = [task('n1', []), task('n2', ['n1'], { status: 'abandoned' }), task('n3', [])];
  assert.deepEqual(BrainLayout.visibleNodes(nodes, [], true).map((node) => node.id), ['n3']);
});

test('badges match wishes, orders and deals by object', () => {
  const payload = {
    wishes: [{ item: 'net' }, { item: 'iron_pickaxe' }],
    orders: [{ item: 'net', side: 'buy', remaining: 1, price: 5 }, { item: 'logs', side: 'sell', remaining: 9, price: 3 }],
    deals: [{ item: 'net', status: 'convened' }],
  };
  assert.deepEqual(BrainLayout.badgesFor(NODES[2], payload).map((badge) => badge.kind), ['wish', 'buy', 'deal']);
  assert.deepEqual(BrainLayout.badgesFor(NODES[0], payload), [], 'no object, no badge');
  assert.equal(BrainLayout.escapeHtml('<a "b">&'), '&lt;a &quot;b&quot;&gt;&amp;');
});
