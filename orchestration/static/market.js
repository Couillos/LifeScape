// Market front: item list, OHLC candles over volume bars, book and last
// trades. Canvas maison via the shared LifeChart module (chart.js).
// Two views: 24 h of hourly candles (default, D42), or 6 months of daily
// candles. Fixed time slots (24 or 183): empty slots show a gap, never a
// fake flat line. The newest candle stretches live with every WS tick
// (the `market` field of the 1 Hz frame).
const PAGE_SIZE = 25;
let items = [];
let selected = null;
let view = 'hour';
let candles = [];
let trades = [];
let pageIndex = 0;
let query = "";
let liveClock = 0;

async function getJson(url) {
  const response = await fetch(url);
  return response.json();
}

async function loadList() {
  try {
    const payload = await getJson('/api/market/list?limit=2000');
    if (payload.ok) items = payload.items;
  } catch (e) { /* daemon down: keep the last list */ }
  if (selected === null && items.length) selected = items[0].item;
  renderList();
}

async function loadDetail() {
  if (selected === null) return;
  const hourly = view === 'hour';
  const res = hourly ? '1h' : '1d';
  const width = hourly ? 3600 : 86400;
  const count = hourly ? 24 : 183;
  const span = count * width;
  const to = Math.floor(Date.now() / 1000);
  const from = to - span;
  try {
    const payload = await getJson('/api/market/history?item=' + selected + '&res=' + res + '&from=' + from + '&to=' + to);
    if (payload.ok) {
      candles = payload.candles;
      trades = payload.trades;
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

function renderList() {
  const matches = filtered();
  const pages = Math.max(1, Math.ceil(matches.length / PAGE_SIZE));
  pageIndex = Math.min(Math.max(pageIndex, 0), pages - 1);
  const slice = matches.slice(pageIndex * PAGE_SIZE, pageIndex * PAGE_SIZE + PAGE_SIZE);
  const rows = [];
  for (const item of slice) {
    const cls = item.change24h > 0 ? 'up' : (item.change24h < 0 ? 'down' : '');
    rows.push('<li class="' + (selected === item.item ? 'cur' : '') + '" data-id="' + item.item +
      '" onclick="pick(' + item.item + ')">' + item.name +
      ' <span class="qty">(' + item.price + ' · <span class="' + cls + '">' + changeText(item.change24h) + '</span> · ' + item.volume24h + ')</span></li>');
  }
  document.getElementById('items').innerHTML = rows.join('') || '<li class="muted">no match</li>';
  document.getElementById('pageinfo').textContent = 'page ' + (pageIndex + 1) + '/' + pages + ' · ' + matches.length + ' item(s)';
}

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

function renderBook(book) {
  const rows = ['<tr><th>bid</th><th>ask</th></tr>'];
  const depth = Math.max(book.buys.length, book.sells.length);
  for (let i = 0; i < depth; i++) {
    const bid = book.buys[i] ? book.buys[i][0] + ' × ' + book.buys[i][1] : '';
    const ask = book.sells[i] ? book.sells[i][0] + ' × ' + book.sells[i][1] : '';
    rows.push('<tr><td>' + bid + '</td><td>' + ask + '</td></tr>');
  }
  document.getElementById('book').innerHTML = rows.join('');
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

// Fixed slots for the current view: 24 hourly or 183 daily windows,
// aligned on multiples of the width since epoch (same buckets as the
// store candles). Sparse history maps on; empty slots stay null.
function currentSlots() {
  const hourly = view === 'hour';
  const width = hourly ? 3600 : 86400;
  const count = hourly ? 24 : 183;
  const to = Math.floor(Date.now() / 1000);
  const from = Math.floor((to - count * width) / width) * width;
  return { slots: LifeChart.slotStarts(from, width, count), width };
}

function draw() {
  const canvas = document.getElementById('chart');
  const item = items.find(entry => entry.item === selected);
  document.getElementById('title').textContent = item ? item.name : 'Pick an item';
  document.getElementById('refline').textContent = item ? ' ref ' + item.price + ' · 24h ' + changeText(item.change24h) + ' · vol ' + item.volume24h : '';

  const { slots, width } = currentSlots();
  const byTime = new Map();
  for (const row of candles) byTime.set(row.t, { ...row });

  // Live tail: fold the newest WS prints into a copy of the candles.
  // Oldest first so the close ends on the freshest print; a print past
  // the last stored candle opens the live slot with its own OHLC.
  const fresh = trades
    .filter(trade => slots.length && trade.t >= slots[slots.length - 1] - width && trade.t < slots[slots.length - 1] + width)
    .slice()
    .sort((a, b) => a.t - b.t);
  for (const trade of fresh) {
    const start = Math.floor(trade.t / width) * width;
    let tail = byTime.get(start);
    if (!tail) {
      tail = { t: start, o: trade.p, h: trade.p, l: trade.p, c: trade.p, v: 0, n: 0 };
      byTime.set(start, tail);
    }
    tail.h = Math.max(tail.h, trade.p);
    tail.l = Math.min(tail.l, trade.p);
    tail.c = trade.p;
    tail.v += trade.q;
    tail.n = (tail.n || 0) + 1;
  }

  const rows = LifeChart.fillSlots(slots, [...byTime.values()]);
  LifeChart.draw(canvas, {
    slots,
    width,
    kind: 'candles',
    rows,
    live: true,
    leftTitle: 'Prix (pièces)',
    rightTitle: 'Volume (quantité échangée)',
    legend: [
      { swatch: 'up', color: '#4caf50', label: 'Chandelle : ouverture → clôture, mèche : plus bas / plus haut — échelle de gauche, en pièces' },
      { swatch: 'bar', color: '#4caf50', label: 'Bâton : quantité échangée sur la période — échelle de droite' },
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

loadList().then(loadDetail);
setInterval(loadList, 60000);
setInterval(loadDetail, 30000);
watchLive();
