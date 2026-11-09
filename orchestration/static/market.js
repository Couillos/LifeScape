// Market front: item list, price-over-volume canvas graph, book and last
// trades. Canvas maison, no library. Two views: 6 months of daily
// candles, or 24 h of hourly candles. The newest candle stretches live
// with every WS tick (the `market` field of the 1 Hz frame).
const PAGE_SIZE = 25;
let items = [];
let selected = null;
let view = 'day';
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
    const payload = await getJson('/api/market/list?limit=500');
    if (payload.ok) items = payload.items;
  } catch (e) { /* daemon down: keep the last list */ }
  if (selected === null && items.length) selected = items[0].item;
  renderList();
}

async function loadDetail() {
  if (selected === null) return;
  const res = view === 'day' ? '1d' : '1h';
  const span = view === 'day' ? 183 * 86400 : 86400;
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

function draw() {
  const canvas = document.getElementById('chart');
  const ctx = canvas.getContext('2d');
  const ratio = window.devicePixelRatio || 1;
  const width = Math.max(320, canvas.clientWidth);
  const height = Math.round(width * 340 / 720);
  canvas.width = Math.round(width * ratio);
  canvas.height = Math.round(height * ratio);
  canvas.style.height = height + 'px';
  ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
  ctx.fillStyle = '#111';
  ctx.fillRect(0, 0, width, height);

  const item = items.find(entry => entry.item === selected);
  document.getElementById('title').textContent = item ? item.name : 'Pick an item';
  document.getElementById('refline').textContent = item ? ' ref ' + item.price + ' · 24h ' + changeText(item.change24h) + ' · vol ' + item.volume24h : '';
  if (!candles.length) {
    ctx.fillStyle = '#888';
    ctx.font = '14px monospace';
    ctx.fillText('No history yet for this item.', 20, height / 2);
    return;
  }

  // Live tail: fold the newest WS prints into a copy of the last candle.
  const live = candles.map(row => ({ ...row }));
  const fresh = trades.filter(trade => live.length && trade.t >= live[live.length - 1].t);
  if (fresh.length) {
    const tail = live[live.length - 1];
    for (const trade of fresh) {
      tail.h = Math.max(tail.h, trade.p);
      tail.l = Math.min(tail.l, trade.p);
      tail.c = trade.p;
      tail.v += trade.q;
    }
  }

  let lo = Infinity, hi = -Infinity, volMax = 0;
  for (const row of live) { lo = Math.min(lo, row.l); hi = Math.max(hi, row.h); volMax = Math.max(volMax, row.v); }
  if (lo === hi) { lo -= 1; hi += 1; }
  if (volMax <= 0) volMax = 1;
  const pad = 44;
  const chartW = width - pad * 2, chartH = height - pad * 2;
  const yOf = price => pad + chartH - ((price - lo) / (hi - lo)) * chartH;
  const slot = chartW / live.length;
  const barW = Math.max(1, Math.min(10, slot * 0.55));

  // Volume bars first (right scale), price line over them (left scale).
  for (let i = 0; i < live.length; i++) {
    const barH = (live[i].v / volMax) * chartH * 0.45;
    ctx.fillStyle = '#2c4a2c';
    ctx.fillRect(pad + slot * i + (slot - barW) / 2, pad + chartH - barH, barW, barH);
  }

  ctx.strokeStyle = '#333';
  ctx.lineWidth = 1;
  ctx.font = '10px monospace';
  for (let step = 0; step <= 4; step++) {
    const y = pad + (chartH * step) / 4;
    ctx.beginPath(); ctx.moveTo(pad, y); ctx.lineTo(width - pad, y); ctx.stroke();
    ctx.fillStyle = '#888';
    ctx.fillText(String(Math.round(hi - ((hi - lo) * step) / 4)), 4, y + 3);
  }

  ctx.strokeStyle = '#4caf50';
  ctx.lineWidth = 2;
  ctx.beginPath();
  live.forEach((row, i) => {
    const x = pad + slot * i + slot / 2, y = yOf(row.c);
    if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
  });
  ctx.stroke();
  ctx.lineWidth = 1;

  const lastX = pad + slot * (live.length - 1) + slot / 2;
  ctx.fillStyle = '#fff';
  ctx.beginPath();
  ctx.arc(lastX, yOf(live[live.length - 1].c), 4, 0, Math.PI * 2);
  ctx.fill();

  ctx.fillStyle = '#888';
  const step = Math.max(1, Math.ceil(live.length / 8));
  for (let i = 0; i < live.length; i += step) {
    const date = new Date(live[i].t * 1000);
    const label = view === 'day'
      ? String(date.getDate()).padStart(2, '0') + '/' + String(date.getMonth() + 1).padStart(2, '0')
      : String(date.getHours()).padStart(2, '0') + ':' + String(date.getMinutes()).padStart(2, '0');
    ctx.fillText(label, pad + slot * i, height - pad + 14);
  }
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
