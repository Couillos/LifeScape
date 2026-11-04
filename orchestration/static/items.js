const PAGE_SIZE = 25;
let data = null;
let selected = null;
let pageIndex = 0;
let query = "";

async function load() {
  try {
    const response = await fetch('/api/items/history');
    const payload = await response.json();
    data = payload.data ?? payload;
  } catch (e) { return; }
  renderList();
  draw();
}

function filtered() {
  const needle = query.trim().toLowerCase();
  if (!needle) return data.items;
  return data.items.filter(item => item.name.toLowerCase().includes(needle));
}

function series() {
  if (!selected) return data.total;
  return data.items.find(item => item.id === selected)?.series ?? [];
}

function renderList() {
  const matches = filtered();
  const pages = Math.max(1, Math.ceil((matches.length + 1) / PAGE_SIZE));
  pageIndex = Math.min(Math.max(pageIndex, 0), pages - 1);
  const start = pageIndex * PAGE_SIZE;
  const slice = matches.slice(start, start + PAGE_SIZE);

  const rows = [];
  if (selected === null) rows.push(`<li class="cur" onclick="pick(null)">Total <span class="qty">(${data.liveTotal})</span></li>`);
  else rows.push(`<li onclick="pick(null)">Total <span class="qty">(${data.liveTotal})</span></li>`);
  for (const item of slice) {
    rows.push(`<li class="${selected === item.id ? 'cur' : ''}" data-id="${item.id}" onclick="pick(${item.id})">${item.name} <span class="qty">(${item.count})</span></li>`);
  }
  document.getElementById('items').innerHTML = rows.join('') || '<li class="muted">no match</li>';
  document.getElementById('pageinfo').textContent = `page ${pageIndex + 1}/${pages} · ${matches.length} item(s)`;
}

function page(delta) {
  pageIndex += delta;
  renderList();
}

function onSearch() {
  query = document.getElementById('search').value;
  pageIndex = 0;
  renderList();
  const match = data.items.find(item => item.name.toLowerCase() === query.trim().toLowerCase());
  if (match) pick(match.id);
}

function pick(id) {
  selected = id;
  renderList();
  draw();
}

function draw() {
  const canvas = document.getElementById('chart');
  const ctx = canvas.getContext('2d');
  // Back the canvas at device resolution so the chart stays crisp.
  const ratio = window.devicePixelRatio || 1;
  const width = Math.max(320, canvas.clientWidth);
  const height = Math.round(width * 340 / 720);
  canvas.width = Math.round(width * ratio);
  canvas.height = Math.round(height * ratio);
  canvas.style.height = height + 'px';
  ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
  ctx.fillStyle = '#111';
  ctx.fillRect(0, 0, width, height);

  const values = series();
  document.getElementById('title').textContent = selected === null
    ? 'Total items'
    : (data.items.find(item => item.id === selected)?.name ?? ('obj ' + selected));
  const live = selected === null
    ? data.liveTotal
    : (data.items.find(item => item.id === selected)?.count ?? 0);

  if (!values.length) {
    ctx.fillStyle = '#888';
    ctx.font = '14px monospace';
    ctx.fillText('No point yet (one every ' + data.intervalMinutes + ' min).', 20, height / 2);
    return;
  }

  const points = values.map((value, index) => ({ value, time: data.samples[index] ?? null, live: false }));
  points.push({ value: live, time: Date.now(), live: true });

  let min = Infinity, max = -Infinity;
  for (const point of points) { min = Math.min(min, point.value); max = Math.max(max, point.value); }
  if (min === max) { min -= 1; max += 1; }
  const pad = 40;
  const chartW = width - pad * 2, chartH = height - pad * 2;
  const yOf = value => pad + chartH - ((value - min) / (max - min)) * chartH;
  const slot = points.length > 1 ? chartW / (points.length - 1) : 0;

  ctx.strokeStyle = '#333';
  ctx.lineWidth = 1;
  ctx.font = '10px monospace';
  for (let step = 0; step <= 4; step++) {
    const y = pad + (chartH * step) / 4;
    ctx.beginPath(); ctx.moveTo(pad, y); ctx.lineTo(width - pad, y); ctx.stroke();
    ctx.fillStyle = '#888';
    ctx.fillText(String(Math.round(max - ((max - min) * step) / 4)), 4, y + 3);
  }

  ctx.strokeStyle = '#4caf50';
  ctx.lineWidth = 2;
  ctx.beginPath();
  points.forEach((point, index) => {
    const x = pad + slot * index, y = yOf(point.value);
    if (index === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
  });
  ctx.stroke();
  ctx.lineWidth = 1;

  points.forEach((point, index) => {
    const x = pad + slot * index, y = yOf(point.value);
    ctx.fillStyle = point.live ? '#fff' : '#4caf50';
    ctx.beginPath();
    ctx.arc(x, y, point.live ? 4 : 2.5, 0, Math.PI * 2);
    ctx.fill();
    if (point.live) {
      ctx.fillStyle = '#888';
      ctx.font = '10px monospace';
      ctx.fillText(String(point.value), x + 6, y + 3);
    }
  });

  const step = Math.max(1, Math.ceil(points.length / 8));
  ctx.fillStyle = '#888';
  ctx.font = '10px monospace';
  for (let index = 0; index < points.length; index += step) {
    const x = pad + slot * index, point = points[index];
    let label = 'live';
    if (!point.live && point.time) {
      const date = new Date(point.time * 1000);
      label = String(date.getHours()).padStart(2, '0') + ':' + String(date.getMinutes()).padStart(2, '0');
    }
    ctx.fillText(label, x - 14, height - pad + 14);
  }
}

let resizeTimer = null;
window.addEventListener('resize', () => {
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(() => { if (data) draw(); }, 150);
});

load();
setInterval(load, 60000);
