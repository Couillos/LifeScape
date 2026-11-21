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

// -- Progression view (A1 to A4): presenters of the Cerveau panel ------------

const VIEW = {
  kind: 'miner', label: 'mineur : minerais et barres', rung: 3, drawn_at: 1795272845, revision_total: 2,
  given: {
    source: 'a4_rebalance', text: 'Donnée par le rééquilibrage A4.',
    numbers: { text: 'value 909 per 6000 ticks', value_6000: 909, holders: 14, driver: 'iron_ore', climb_ticks: 0, kept: false },
  },
  revision_rows: [
    { at_secs: 1795272845, from: 'priest', to: 'miner', why: 'rebalance', text: 'rééquilibrage A4 : le métier dont la demande non servie rapporte le plus', retired_rule: false },
    { at_secs: 1795252942, from: 'mage', to: 'priest', why: 'repair:coverage:prayer', text: 'règle retirée : réparation de couverture', retired_rule: true },
  ],
  milestones: [
    { id: 'm1', label: 'mining 70', state: 'done', state_text: 'fait', progress: [70, 70], origin: 'ladder', ticks_left: null },
    { id: 'm2', label: 'équiper le meilleur magic', state: 'waiting', state_text: 'en attente : équipement que personne ne fabrique (ni en stock, ni au marché), nouvel essai dans 900 ticks', progress: [0, 1], origin: 'ladder', ticks_left: 900 },
    { id: 'm3', label: 'mining 80', state: 'set_aside', state_text: 'mis de côté (bloqué 3 fois) : reprise possible dans 4000 ticks', progress: [70, 80], origin: 'ladder', ticks_left: 4000 },
    { id: 'm4', label: 'produire 200 iron_ore', state: 'open', state_text: 'ouvert', progress: [0, 200], origin: 'demand:iron_ore', ticks_left: null },
  ],
};

test('durations read in ticks and in minutes with the server tick length', () => {
  assert.equal(BrainLayout.durationText(900, 600), '900 ticks (≈ 9 min)');
  assert.equal(BrainLayout.durationText(40, 600), '40 ticks (≈ 24 s)');
  assert.equal(BrainLayout.durationText(72000, 600), '72 000 ticks (≈ 12 h)');
  assert.equal(BrainLayout.durationText(100, 0), '100 ticks', 'no tick length, no minutes');
});

test('the ambition shows how it was given, its value and the revisions', () => {
  const html = BrainLayout.ambitionHtml({ ambition_view: VIEW, tick_ms: 600 });
  assert.match(html, /mineur : minerais et barres/);
  assert.match(html, /Rééquilibrage du 21\/11/);
  assert.match(html, /valeur 909 pièces par 6000 ticks/);
  assert.match(html, /14 bots déjà sur ce métier/);
  assert.match(html, /portée par iron_ore/);
  assert.match(html, /règle retirée/, 'a retired rule is named as retired');
  assert.match(html, /Révisions \(2\)/);
  assert.doesNotMatch(html, /repair:coverage/, 'raw rule codes are not shown');
  assert.match(BrainLayout.ambitionHtml({ ambition_view: null }), /aucune ambition/);
});

test('every protected, pinned and operator source has its label', () => {
  for (const [source, label] of [['protected', 'Compte protégé'], ['pin', 'Épinglée'], ['operator', 'Type opérateur'], ['a4_kept', 'Gardée'], ['a4_draw', 'valeur du marché'], ['legacy', 'avant le 21/11']]) {
    const html = BrainLayout.ambitionHtml({ ambition_view: Object.assign({}, VIEW, { given: { source: source, text: 't', numbers: null } }) });
    assert.match(html, new RegExp(label, 'i'), source);
  }
});

test('milestones show their real state with the cooldown left', () => {
  const html = BrainLayout.milestonesHtml({ ambition_view: VIEW, tick_ms: 600 });
  assert.match(html, /1 ouverts · 1 en attente · 1 mis de côté · 1 faits/);
  assert.match(html, /st-waiting/);
  assert.match(html, /personne ne fabrique/);
  assert.match(html, /st-set_aside/);
  assert.match(html, /reprise possible dans 4000 ticks/);
  assert.match(html, /demande du marché/, 'a milestone born of a market demand is marked');
  assert.equal(BrainLayout.milestonesHtml({ ambition_view: null }), '');
  assert.equal(BrainLayout.milestoneShort(VIEW.milestones[2], 600), 'mis de côté · 4 000t (≈ 40 min)'.replace('4 000t', '4 000t'));
  assert.equal(BrainLayout.milestoneShort(VIEW.milestones[3], 600), 'ouvert');
});

test('the goal names its origin: milestone, buy offer, side goal, filler', () => {
  const goal = (origin, extra) => BrainLayout.goalHtml(Object.assign({ goal: { label: 'magic 70', origin: origin } }, extra || {}));
  assert.match(goal({ kind: 'milestone', text: 'jalon m1 : magic 70', raw: 'm1' }), /jalon m1 : magic 70/);
  assert.match(goal({ kind: 'bid', text: 'réponse à une offre d\'achat : sapphire', raw: 'bid:sapphire' }), /offre d&#39;achat|offre d'achat/);
  assert.match(goal({ kind: 'side', text: 'but d\'appoint : money', raw: 'side:money' }), /appoint/);
  assert.match(goal({ kind: 'none', text: 'sans origine (but libre)', raw: '' }, { filler: { method: 'gather:woodcut/yew_logs', parked: 'n1315' } }), /remplissage en cours : gather:woodcut\/yew_logs/);
  assert.equal(BrainLayout.originShort({ kind: 'milestone', raw: 'm3' }), 'jalon m3');
  assert.equal(BrainLayout.originShort({ kind: 'demand', raw: 'm4' }), 'jalon m4 · demande');
  assert.equal(BrainLayout.originShort({ kind: 'bid', raw: 'bid:x' }), 'offre d\'achat');
  assert.equal(BrainLayout.originShort(null), 'sans origine');
});

test('cooldowns list the avoided sites and methods with the time left', () => {
  const html = BrainLayout.coolingHtml({ tick_ms: 600, cooling: [{ method: 'gather:mining/iron_ore', site: '3295,3310,0', ticks_left: 600, cause: 'contended' }] });
  assert.match(html, /gather:mining\/iron_ore/);
  assert.match(html, /site 3295,3310,0/);
  assert.match(html, /600 ticks \(≈ 6 min\)/);
  assert.match(BrainLayout.coolingHtml({ cooling: [] }), /aucun/);
});

test('the value of time shows the lift of a lucrative climb', () => {
  const training = { skill: 'crafting', own_rate: 1.5, pull_per_tick: 10, time_value: 10, lifted: true,
    pending: [{ item: 'amulet_of_power', method: 'craft', climb_ticks: 120000, value_6000: 60000, gain_per_tick: 3, gates: [{ skill: 'crafting', level: 70, climb_ticks: 120000 }, { skill: 'mining', level: 40, climb_ticks: 0 }] }] };
  const html = BrainLayout.trainingHtml({ training: training, tick_ms: 600 });
  assert.match(html, /un tick vaut <b>10\.00<\/b>/);
  assert.match(html, /relevé par une montée lucrative/);
  assert.match(html, /amulet_of_power/);
  assert.match(html, /crafting 70/);
  assert.doesNotMatch(html, /mining 40/, 'a gate already reached is not a climb');
  assert.match(BrainLayout.trainingHtml({ training: null }), /pas d'entraînement/);
  assert.doesNotMatch(BrainLayout.trainingHtml({ training: Object.assign({}, training, { lifted: false }) }), /relevé/);
});

test('the graph reads the enriched ambition and drops no milestone', () => {
  const model = BrainLayout.layout({ ambition: { kind: 'x' }, ambition_view: VIEW, goal: { label: 'mining 80', origin_milestone: 'm3' }, nodes: [] }, { collapsed: [] });
  const ids = model.boxes.map((box) => box.id).sort();
  assert.deepEqual(ids, ['ambition', 'goal', 'm:m1', 'm:m2', 'm:m3', 'm:m4']);
  const stone = model.boxes.find((box) => box.id === 'm:m2');
  assert.equal(stone.data.state, 'waiting');
});

test('texts a life file carries are escaped in every section', () => {
  const evil = '<img src=x onerror=alert(1)>';
  const view = Object.assign({}, VIEW, { label: evil, given: { source: 'legacy', text: evil, numbers: { text: evil } },
    milestones: [{ id: 'm1', label: evil, state: 'open', state_text: evil, progress: [0, 1], origin: 'ladder' }] });
  const payload = { ambition_view: view, goal: { label: evil, origin: { kind: 'bid', text: evil } }, filler: { method: evil, parked: evil },
    cooling: [{ method: evil, site: evil, ticks_left: 1, cause: evil }], training: { skill: evil, own_rate: 1, pull_per_tick: 0, time_value: 1, lifted: false, pending: [] } };
  const html = BrainLayout.progressionHtml(payload);
  assert.doesNotMatch(html, /<img/);
  assert.match(html, /&lt;img/);
});
