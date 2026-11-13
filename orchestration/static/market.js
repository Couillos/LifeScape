// Market front: item list, OHLC candles over volume bars, book and last
// trades. Canvas maison via the shared LifeChart module (chart.js).
// Two views: 24 h of hourly candles (default, D42), or 6 months of daily
// candles. Fixed time slots (24 or 183) ending on the live window; no gap
// (user rule 2026-11-13): every candle opens at the previous close and a
// trade-less slot is a flat grey candle at the last close (the server
// history is already chained, `LifeChart.carryFlat` keeps the live tail
// chained). The newest candle stretches live with every WS tick (the
// `market` field of the 1 Hz frame).
const PAGE_SIZE = 25;
let items = [];
let selected = null;
let view = 'hour';
let candles = [];
// Last trade price before the history window: the close the leading
// trade-less hours carry (one flat candle per empty hour).
let prevClose = null;
let trades = [];
// `to` of the loaded history: its candles already hold every print
// before it, so only prints at or after it fold into the live tail.
let historyTo = 0;
let pageIndex = 0;
let query = "";
let liveClock = 0;
let recent = [];
let recentHavePseudos = false;
// Names seen in the deal feed: a traded good with no open order is not in
// `items` (list = open books only) but must still show and open by name.
const knownNames = new Map();

async function getJson(url) {
  const response = await fetch(url);
  return response.json();
}

async function loadList() {
  try {
    const payload = await getJson('/api/market/list?limit=2000&active=1');
    // `active=1` keeps every good with at least one open order (bid or
    // ask) or at least one past execution, sorted server-side by descending 24h volume, then by the
    // open quantity (remaining units, bids and asks).
    if (payload.ok) items = payload.items;
  } catch (e) { /* daemon down: keep the last list */ }
  if (selected === null && items.length) selected = items[0].item;
  renderList();
}

async function loadDetail() {
  if (selected === null) return;
  const res = view === 'hour' ? '1h' : '1d';
  // Same windows as the drawn slots: the server chains candles from the
  // first slot (opened by `prev_close`) up to the live one.
  const to = Math.floor(Date.now() / 1000);
  const from = currentSlots(to).slots[0];
  try {
    const payload = await getJson('/api/market/history?item=' + selected + '&res=' + res + '&from=' + from + '&to=' + to);
    if (payload.ok) {
      candles = payload.candles;
      prevClose = payload.prev_close === undefined ? null : payload.prev_close;
      trades = payload.trades;
      historyTo = to;
    }
  } catch (e) { /* keep the last frame */ }
  try {
    const book = await getJson('/api/market/book?item=' + selected + '&depth=8');
    if (book.ok) renderBook(book);
  } catch (e) { /* keep the last book */ }
  renderTrades();
  draw();
}

function filtered() {
  const needle = query.trim().toLowerCase();
  if (!needle) return items;
  return items.filter(item => item.name.toLowerCase().includes(needle));
}

function changeText(promille) {
  if (!promille) return '±0.0%';
  const sign = promille > 0 ? '+' : '';
  return sign + (promille / 10).toFixed(1) + '%';
}

// Item table columns (shared TableSort, table.js): raw values sort, fmt
// only changes the display. Default order = the API order (most traded
// first); a header click sorts the whole filtered list, then pages.
const ITEM_COLUMNS = [
  { key: 'name', label: 'Item', kind: 'str' },
  { key: 'price', label: 'Prix', kind: 'num' },
  { key: 'change', label: '24 h', kind: 'num', fmt: (value) => {
    const cls = value > 0 ? 'up' : (value < 0 ? 'down' : '');
    return '<span class="' + cls + '">' + changeText(value) + '</span>';
  } },
  { key: 'volume', label: 'Volume 24 h', kind: 'num' },
];

function itemRow(item) {
  return { id: item.item, name: item.name, price: item.price, change: item.change24h, volume: item.volume24h };
}

function renderList() {
  const matches = TableSort.sortedRows('items', ITEM_COLUMNS, filtered().map(itemRow));
  const pages = Math.max(1, Math.ceil(matches.length / PAGE_SIZE));
  pageIndex = Math.min(Math.max(pageIndex, 0), pages - 1);
  const slice = matches.slice(pageIndex * PAGE_SIZE, pageIndex * PAGE_SIZE + PAGE_SIZE);
  if (slice.length) {
    TableSort.render('items', ITEM_COLUMNS, slice, {
      rowAttrs: (row) => ({ 'data-id': row.id, class: selected === row.id ? 'cur' : '' }),
      onSort: () => { pageIndex = 0; renderList(); },
    });
  } else {
    document.getElementById('items').innerHTML = '<tr><td class="left muted">' + (query ? 'no match' : 'no traded item yet') + '</td></tr>';
  }
  document.getElementById('pageinfo').textContent = 'page ' + (pageIndex + 1) + '/' + pages + ' · ' + matches.length + ' item(s)';
}

// One listener on the stable <table>: rows are rebuilt on every render.
document.getElementById('items').addEventListener('click', (event) => {
  const row = event.target.closest('tr[data-id]');
  if (row) pick(Number(row.getAttribute('data-id')));
});

function page(delta) {
  pageIndex += delta;
  renderList();
}

function onSearch() {
  query = document.getElementById('search').value;
  pageIndex = 0;
  renderList();
  const match = items.find(item => item.name.toLowerCase() === query.trim().toLowerCase());
  if (match) pick(match.id !== undefined ? match.id : match.item);
}

function pick(id) {
  selected = id;
  renderList();
  loadDetail();
}

function setView(next) {
  view = next;
  document.getElementById('v-day').className = next === 'day' ? 'cur' : '';
  document.getElementById('v-hour').className = next === 'hour' ? 'cur' : '';
  loadDetail();
}

function aggregateLevels(levels, descending) {
  const totals = new Map();
  for (const level of levels || []) {
    totals.set(level[0], (totals.get(level[0]) || 0) + level[1]);
  }
  const rows = [...totals.entries()];
  rows.sort((left, right) => descending ? right[0] - left[0] : left[0] - right[0]);
  let cumul = 0;
  return rows.map(([price, qty]) => ({ price, qty, cumul: cumul += qty }));
}

function depthRows(rows, side) {
  return rows.map(row => '<tr class="' + side + '"><td>' + row.price + '</td><td>' + row.qty + '</td><td>' + row.cumul + '</td></tr>');
}

// Exchange-style ladder: asks on top (best ask right above the spread),
// the spread line, then bids (best bid right below it). Cumuls grow away
// from the spread on both sides.
function renderBook(book) {
  const bids = aggregateLevels(book.buys, true);
  const asks = aggregateLevels(book.sells, false);
  const out = ['<tr><th>price</th><th>qty</th><th>cumul</th></tr>'];
  if (!bids.length && !asks.length) {
    out.push('<tr><td colspan="3" class="muted">empty book</td></tr>');
  } else {
    out.push(...depthRows(asks, 'ask').reverse());
    let spread = '—';
    if (bids.length && asks.length) {
      const gap = asks[0].price - bids[0].price;
      spread = 'spread ' + gap + ' · mid ' + ((asks[0].price + bids[0].price) / 2);
    } else if (asks.length) {
      spread = 'no bids';
    } else {
      spread = 'no asks';
    }
    out.push('<tr class="spread"><td colspan="3">' + spread + '</td></tr>');
    out.push(...depthRows(bids, 'bid'));
  }
  document.getElementById('book').innerHTML = out.join('');
}

function renderTrades() {
  const rows = ['<tr><th>qty</th><th>price</th><th>time</th></tr>'];
  const last = trades.slice(0, 12);
  for (const trade of last) {
    const date = new Date(trade.t * 1000);
    const label = String(date.getHours()).padStart(2, '0') + ':' + String(date.getMinutes()).padStart(2, '0');
    rows.push('<tr><td>' + trade.q + '</td><td>' + trade.p + '</td><td>' + label + '</td></tr>');
  }
  document.getElementById('trades').innerHTML = rows.join('');
}

function statusLabel(status) {
  // Failed handovers never reach the feed (backend filter, user rule
  // 2026-11-13): the market shows trades that happened or are under way
  // (matched, convened, both at the square, trading).
  return status === 'settled' ? 'settled' : 'pending';
}

function renderRecent() {
  const rows = ['<tr><th>item</th><th>qty</th><th>price</th><th>buyer</th><th>seller</th><th>status</th><th>time</th></tr>'];
  for (const trade of recent.slice(0, 15)) {
    const date = new Date(trade.t * 1000);
    const label = String(date.getHours()).padStart(2, '0') + ':' + String(date.getMinutes()).padStart(2, '0');
    // The item opens its market (chart, book, last trades) on click.
    rows.push('<tr><td class="item-link" data-id="' + Number(trade.item) + '" title="open this item">' + itemName(trade.item) + '</td><td>' + trade.q + '</td><td>' + trade.p + '</td><td>' + trade.buyer + '</td><td>' + trade.seller + '</td><td>' + statusLabel(trade.status) + '</td><td>' + label + '</td></tr>');
  }
  document.getElementById('recent').innerHTML = rows.join('');
}

// One listener on the stable table: rows are rebuilt every 10 s.
document.getElementById('recent').addEventListener('click', (event) => {
  const cell = event.target.closest('td.item-link[data-id]');
  if (!cell) return;
  pick(Number(cell.getAttribute('data-id')));
  document.getElementById('chart')?.scrollIntoView({ behavior: 'smooth', block: 'start' });
});

function itemName(id) {
  const item = items.find(entry => entry.item === id);
  if (item) return item.name;
  return knownNames.has(id) ? knownNames.get(id) : ('#' + id);
}

async function loadRecent() {
  // Preferred feed: backend `recent` op (deals with buyer/seller).
  // Falls back to the anonymous prints of `trades` (no item = all)
  // when the op is unknown or the daemon predates it.
  try {
    const payload = await getJson('/api/market/recent?limit=20');
    if (payload.ok && Array.isArray(payload.deals)) {
      recentHavePseudos = true;
      for (const deal of payload.deals) {
        if (deal.name) knownNames.set(deal.item, deal.name);
      }
      recent = payload.deals.map(deal => ({ item: deal.item, q: deal.qty, p: deal.price, t: deal.t, buyer: deal.buyer, seller: deal.seller, status: deal.status }));
      renderRecent();
      return;
    }
  } catch (e) { /* fall through to the anonymous feed */ }
  try {
    const payload = await getJson('/api/market/trades?limit=20');
    if (payload.ok) {
      recentHavePseudos = false;
      recent = payload.trades.slice().reverse().map(trade => ({ item: trade.item, q: trade.q, p: trade.p, t: trade.t, buyer: '—', seller: '—', status: 'settled' }));
      renderRecent();
    }
  } catch (e) { /* keep the last frame */ }
}

// Fixed slots for the current view: 24 hourly or 183 daily windows,
// aligned on multiples of the width since epoch (same buckets as the
// store candles), the last one holding `now`: the live candle. (It used
// to end one window early, so the current hour never showed: live
// 2026-11-13, Bronze axe's chart closed at 20 while its last trades read
// 8, 8, 8, 6.)
function currentSlots(now) {
  const hourly = view === 'hour';
  const width = hourly ? 3600 : 86400;
  const count = hourly ? 24 : 183;
  const at = now === undefined ? Math.floor(Date.now() / 1000) : now;
  return { slots: LifeChart.liveSlots(at, width, count), width };
}

function draw() {
  const canvas = document.getElementById('chart');
  const item = items.find(entry => entry.item === selected);
  document.getElementById('title').textContent = item ? item.name : (selected === null ? 'Pick an item' : itemName(selected));
  document.getElementById('refline').textContent = item ? ' ref ' + item.price + ' · 24h ' + changeText(item.change24h) + ' · vol ' + item.volume24h : '';

  const { slots, width } = currentSlots();
  const byTime = new Map();
  for (const row of candles) byTime.set(row.t, { ...row });

  // Live tail: fold the newest WS prints into a copy of the candles.
  // Oldest first so the close ends on the freshest print; a print past
  // the last stored candle opens the live slot, then `carryFlat` below
  // opens it at the previous close like every other candle.
  const fresh = trades
    .filter(trade => slots.length && trade.t >= historyTo && trade.t >= slots[slots.length - 1] - width && trade.t < slots[slots.length - 1] + width)
    .slice()
    .sort((a, b) => a.t - b.t);
  for (const trade of fresh) {
    const start = Math.floor(trade.t / width) * width;
    let tail = byTime.get(start);
    if (!tail) {
      tail = { t: start, o: trade.p, h: trade.p, l: trade.p, c: trade.p, v: 0, vb: 0, vs: 0, n: 0 };
      byTime.set(start, tail);
    }
    // Split by the print's taker side (`s`: buy, sell, null = unknown).
    LifeChart.foldPrint(tail, trade);
  }

  const rows = LifeChart.carryFlat(LifeChart.fillSlots(slots, [...byTime.values()]), prevClose);
  LifeChart.draw(canvas, {
    slots,
    width,
    kind: 'candles',
    rows,
    live: true,
    leftTitle: 'price',
    rightTitle: 'volume',
    // Volume pane: buy volume (a buyer took the asks) green above the
    // baseline, sell volume (a seller hit the bids) red below; volume
    // settled before the side was recorded stacks grey above the green.
    legend: [
      { swatch: 'up', color: LifeChart.UP, label: 'candle' },
      { swatch: 'bar', color: LifeChart.UP, label: 'buy volume' },
      { swatch: 'bar', color: LifeChart.DOWN, label: 'sell volume' },
    ],
    emptyText: 'No history yet for this item.',
    tooltipFor: (slot, row) => LifeChart.candleTooltip(slot, width, row),
  });
}

// Live prints ride the dashboard WS: same socket, `market` field of the
// 1 Hz frame. Anything newer than the loaded trades stretches the last
// candle without a reload.
function watchLive() {
  const scheme = location.protocol === "https:" ? "wss://" : "ws://";
  let socket = null;
  try {
    socket = new WebSocket(scheme + location.host + "/ws");
  } catch (e) { return; }
  socket.onmessage = event => {
    let frame = null;
    try { frame = JSON.parse(event.data); } catch (e) { return; }
    const fresh = frame && frame.market && frame.market.trades;
    if (!fresh || !fresh.length) return;
    let moved = false;
    for (const trade of fresh) {
      if (!recentHavePseudos && (!recent.length || trade.t > recent[0].t)) {
        recent.unshift({ item: trade.item, q: trade.q, p: trade.p, t: trade.t, buyer: '—', seller: '—', status: 'settled' });
        recent = recent.slice(0, 30);
        renderRecent();
      }
      if (trade.item !== selected) continue;
      if (trade.t > liveClock) liveClock = trade.t;
      if (!trades.length || trade.t > trades[0].t || (trade.t === trades[0].t && trade.p !== trades[0].p)) {
        trades.unshift(trade);
        moved = true;
      }
    }
    if (moved) {
      trades = trades.slice(0, 60);
      renderTrades();
      draw();
    }
  };
  socket.onclose = () => setTimeout(watchLive, 5000);
}

let resizeTimer = null;
window.addEventListener('resize', () => {
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(() => { if (candles.length) draw(); }, 150);
});

loadList().then(() => { loadDetail(); loadRecent(); });
setInterval(loadList, 60000);
setInterval(loadDetail, 30000);
setInterval(loadRecent, 10000);
watchLive();
