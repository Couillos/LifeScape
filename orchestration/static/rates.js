// Rates front: yield, XP and profit tables per (method x band),
// refreshed every 30 s. Read-only dashboard page: bots never read it.
let payload = null;
let filter = "";

function bandLabel(band) {
  return band + "-" + (band + 4);
}

function number(value) {
  if (!isFinite(value)) return "-";
  return Math.round(value * 10) / 10;
}

async function load() {
  try {
    const response = await fetch('/api/rates/table');
    const body = await response.json();
    if (body.ok) payload = body;
  } catch (e) { /* daemon down: keep the last frame */ }
  render();
}

function matches(row) {
  const needle = filter.trim().toLowerCase();
  if (!needle) return true;
  return (row.method + " " + (row.item || row.skill || "")).toLowerCase().includes(needle);
}

function onSearch() {
  filter = document.getElementById('search').value;
  render();
}

function render() {
  if (!payload) return;
  const yields = (payload.rates || []).filter(matches);
  const xp = (payload.xp || []).filter(matches);
  const profit = (payload.profit || []).filter(matches);
  document.getElementById('summary').textContent =
    yields.length + ' yield row(s) · ' + xp.length + ' xp row(s) · ' + profit.length + ' profit row(s)';

  const yieldRows = ['<tr><th class="left">method</th><th class="left">item</th><th>band</th>' +
    '<th>n</th><th>mean/6000</th><th>stddev</th><th>ema</th><th class="left">src</th></tr>'];
  for (const row of yields) {
    const src = row.prior ? '<span class="prior">prior</span>' : 'measured';
    yieldRows.push('<tr><td class="left">' + row.method + '</td><td class="left">' + row.item + '</td>' +
      '<td>' + bandLabel(row.band) + '</td><td>' + row.count + '</td><td>' + number(row.mean) + '</td>' +
      '<td>' + number(row.stddev) + '</td><td>' + number(row.ema) + '</td><td class="left">' + src + '</td></tr>');
  }
  document.getElementById('yields').innerHTML = yieldRows.join('');

  const xpRows = ['<tr><th class="left">method</th><th class="left">skill</th><th>band</th>' +
    '<th>n</th><th>mean/6000</th><th>stddev</th><th>ema</th><th class="left">src</th></tr>'];
  for (const row of xp) {
    const src = row.prior ? '<span class="prior">prior</span>' : 'measured';
    xpRows.push('<tr><td class="left">' + row.method + '</td><td class="left">' + row.skill + '</td>' +
      '<td>' + bandLabel(row.band) + '</td><td>' + row.count + '</td><td>' + number(row.mean) + '</td>' +
      '<td>' + number(row.stddev) + '</td><td>' + number(row.ema) + '</td><td class="left">' + src + '</td></tr>');
  }
  document.getElementById('xp').innerHTML = xpRows.join('');

  const profitRows = ['<tr><th class="left">method</th><th>band</th><th>revenue</th>' +
    '<th>inputs</th><th>friction</th><th>trend bonus</th><th>profit</th><th class="left">price</th></tr>'];
  for (const row of profit) {
    const price = row.price_prior ? '<span class="prior">prior</span>' : 'market';
    profitRows.push('<tr><td class="left">' + row.method + '</td><td>' + bandLabel(row.band) + '</td>' +
      '<td>' + number(row.revenue) + '</td><td>' + number(row.input_cost) + '</td>' +
      '<td>' + number(row.friction) + '</td><td>' + number(row.trend_bonus) + '</td>' +
      '<td>' + number(row.profit) + '</td><td class="left">' + price + '</td></tr>');
  }
  document.getElementById('profit').innerHTML = profitRows.join('');

  const engaged = payload.engaged || {};
  const names = Object.keys(engaged).sort();
  const engagedRows = ['<tr><th class="left">script</th><th>bots</th></tr>'];
  for (const name of names) {
    engagedRows.push('<tr><td class="left">' + name + '</td><td>' + engaged[name] + '</td></tr>');
  }
  document.getElementById('engaged').innerHTML =
    names.length ? engagedRows.join('') : '<tr><td class="left muted" colspan="2">no running bot</td></tr>';
}

load();
setInterval(load, 30000);
