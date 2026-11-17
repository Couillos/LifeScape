// Economy front: deals, top activities, market makers, 100 % market lots
// (predicted vs realized).
// Read-only dashboard page: bots never read it.
function number(value) {
  if (!isFinite(value)) return "-";
  return Math.round(value * 10) / 10;
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

  const deals = payload.deals || {};
  const failPct = ((deals.fail_rate || 0) * 100).toFixed(1) + '%';
  document.getElementById('deals').innerHTML =
    '<tr><th class="left">deals</th><th>settled</th><th>failed</th><th>fail rate</th></tr>' +
    '<tr><td class="left">' + (deals.total || 0) + '</td><td>' + (deals.settled || 0) +
    '</td><td>' + (deals.failed || 0) + '</td><td>' + failPct + '</td></tr>';

  // Top activities: the money guide's catalogue (precomputed set, the
  // guides page's figures): priced lines, none impossible for every bot.
  const activities = (payload.activities || []).slice(0, 10);
  const band = value => value > 0 ? value + '-' + (value + 4) : '-';
  const confidence = row => !row.confidence ? '-' :
    (row.confidence.measured ? 'measured' : 'estimated') + ' · n ' + (row.confidence.runs || 0);
  document.getElementById('activities').innerHTML = payload.activities_pending ? '<tr><td class="muted">guides: first build running</td></tr>' :
    '<tr><th class="left">family</th><th class="left">method</th><th>band</th><th>profit/6000</th><th class="left">confidence</th></tr>' +
    activities.map(row => '<tr><td class="left">' + row.family + '</td><td class="left">' + row.method +
      (row.style ? ' <span class="muted">' + row.style + '</span>' : '') + '</td><td>' + band(row.band) + '</td><td>' +
      number(row.profit_6000) + '</td><td class="left">' + confidence(row) + '</td></tr>').join('');

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

  // 100 % market lots (plan rendements R5): one row per lot, newest first.
  const trades = payload.trades || [];
  const closed = payload.trades_closed || {};
  document.getElementById('trades_closed').textContent = (closed.lots || 0) + ' closed lots: predicted ' +
    (closed.predicted || 0) + ', realized ' + (closed.realized || 0);
  const made = row => row.made === null || row.made === undefined ? '-' : row.made;
  const craft = row => row.craft_yield === null || row.craft_yield === undefined ? '-' :
    number(row.craft_yield) + ' (guide ' + number(row.guide_per_6000) + ')';
  document.getElementById('trades').innerHTML =
    '<tr><th class="left">bot</th><th class="left">recipe</th><th>lot</th><th>status</th><th>bought</th>' +
    '<th>made</th><th>craft / 6000</th><th>sold</th><th>predicted</th><th>realized</th></tr>' +
    trades.map(row => '<tr><td class="left">' + row.account + '</td><td class="left">' +
      String(row.method).replace('trade:craft/', '') + '</td><td>' + row.lot + ' (' + row.bound + ')</td><td>' +
      row.status + '</td><td>' + row.bought_units + '/' + row.planned_units + ' for ' + row.bought_coins +
      '</td><td>' + made(row) + '</td><td>' + craft(row) + '</td><td>' + row.sold_units + ' for ' + row.sold_coins +
      '</td><td>' + row.predicted + '</td><td class="' + (row.realized >= 0 ? 'up' : 'down') + '">' + row.realized +
      '</td></tr>').join('');
}

load();
setInterval(load, 30000);
