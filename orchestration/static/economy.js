// Economy front: money supply, deals, top activities, trends, market makers.
// Read-only dashboard page: bots never read it.
function number(value) {
  if (!isFinite(value)) return "-";
  return Math.round(value * 10) / 10;
}

function scoreClass(score) {
  if (score > 0.005) return 'up';
  if (score < -0.005) return 'down';
  return '';
}

async function load() {
  let payload = null;
  try {
    const response = await fetch('/api/economy');
    const body = await response.json();
    if (body.ok) payload = body;
  } catch (e) { /* daemon down: keep the last frame */ }
  if (!payload) return;
  const date = new Date(payload.clock * 1000);
  document.getElementById('clock').textContent = 'updated ' +
    String(date.getHours()).padStart(2, '0') + ':' + String(date.getMinutes()).padStart(2, '0') +
    ':' + String(date.getSeconds()).padStart(2, '0');

  const holdings = payload.holdings || {};
  const money = holdings.liveTotal !== undefined ? holdings.liveTotal : holdings.live_total;
  document.getElementById('money').textContent =
    money !== undefined ? money + ' items across all accounts' : 'no holdings yet';
  const top = (holdings.items || []).slice(0, 12);
  document.getElementById('holdings').innerHTML =
    '<tr><th class="left">item</th><th>count</th></tr>' +
    top.map(row => '<tr><td class="left">' + row.name + '</td><td>' + row.count + '</td></tr>').join('');

  const deals = payload.deals || {};
  const failPct = ((deals.fail_rate || 0) * 100).toFixed(1) + '%';
  document.getElementById('deals').innerHTML =
    '<tr><th class="left">deals</th><th>settled</th><th>failed</th><th>fail rate</th></tr>' +
    '<tr><td class="left">' + (deals.total || 0) + '</td><td>' + (deals.settled || 0) +
    '</td><td>' + (deals.failed || 0) + '</td><td>' + failPct + '</td></tr>';

  const activities = (payload.activities || []).slice(0, 10);
  document.getElementById('activities').innerHTML =
    '<tr><th class="left">method</th><th>band</th><th>profit</th></tr>' +
    activities.map(row => '<tr><td class="left">' + row.method + '</td><td>' +
      row.band + '-' + (row.band + 4) + '</td><td>' + number(row.profit) + '</td></tr>').join('');

  const trends = payload.trends || [];
  document.getElementById('trends').innerHTML =
    '<tr><th class="left">item</th><th>shortage</th><th>momentum</th><th>score</th></tr>' +
    trends.map(row => '<tr><td class="left">' + row.name + '</td><td>' + number(row.shortage) +
      '</td><td>' + number(row.momentum) + '</td><td class="' + scoreClass(row.score) + '">' +
      number(row.score) + '</td></tr>').join('');

  // Market makers (plan marché Q6): one row per maker account.
  const makers = payload.makers || [];
  const pct = value => (value === null || value === undefined) ? '-' : Math.round(value * 1000) / 10 + '%';
  const spread = value => (value === null || value === undefined) ? '-' : 'x' + number(value);
  document.getElementById('makers').innerHTML =
    '<tr><th class="left">bot</th><th>slice</th><th>phase</th><th>target</th><th>capital</th>' +
    '<th>P&amp;L</th><th>spread</th><th>book share</th><th>quotes</th><th class="left">stock</th></tr>' +
    makers.map(row => '<tr><td class="left">' + row.account + '</td><td>' + row.slice + '/' + row.slices +
      '</td><td>' + row.phase + '</td><td>' + row.capital_target + '</td><td>' + row.capital +
      '</td><td class="' + (row.pnl >= 0 ? 'up' : 'down') + '">' + row.pnl + '</td><td>' + spread(row.mean_spread) +
      '</td><td>' + pct(row.book_share) + '</td><td>' + row.open_orders + '</td><td class="left">' +
      (row.stock || []).slice(0, 4).map(line => line.name + ' ' + line.units).join(', ') + '</td></tr>').join('');
}

load();
setInterval(load, 30000);
