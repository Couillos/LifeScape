// Hiscores front (2026-11-14): skill rankings in the spirit of the 2004
// hiscores (category column, Rank / Name / Level / XP table, 25 per
// page, search by name or rank, personal scores) plus the wealth
// rankings (coins, total value at the market current price).
// The daemon computes the snapshot every 30 min; this page only reads
// `/api/hiscores` (re-read every 5 min, cheap: cached bytes).
// State lives in the URL hash: `#cat=attack&page=2` or `#player=t01`.
//
// Pure helpers (Hiscores.*) touch no DOM: `node hiscores.test.js`.
const Hiscores = (() => {
  'use strict';

  const PER_PAGE = 25;
  const WEALTH = [
    { key: 'coins', label: 'Coins', unit: 'gp', hint: 'pack + bank' },
    { key: 'value', label: 'Total value', unit: 'gp', hint: 'coins + items (pack, bank, worn) at the market current price' },
  ];

  function esc(text) {
    return String(text === null || text === undefined ? '' : text)
      .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }

  // 1234567 -> "1,234,567" (the hiscores always showed full numbers).
  function formatNumber(value) {
    if (value === null || value === undefined || !isFinite(value)) return '-';
    const negative = value < 0;
    const digits = String(Math.floor(Math.abs(value)));
    let out = '';
    for (let index = 0; index < digits.length; index++) {
      if (index > 0 && (digits.length - index) % 3 === 0) out += ',';
      out += digits[index];
    }
    return (negative ? '-' : '') + out;
  }

  // 12_400_000 -> "12.4M", 15_300 -> "15.3K", 950 -> "950".
  function shortCoins(value) {
    if (value === null || value === undefined || !isFinite(value)) return '-';
    const abs = Math.abs(value);
    const cut = (number, suffix) => {
      const text = number >= 100 ? String(Math.floor(number)) : number.toFixed(1).replace(/\.0$/, '');
      return (value < 0 ? '-' : '') + text + suffix;
    };
    if (abs >= 1e9) return cut(abs / 1e9, 'B');
    if (abs >= 1e6) return cut(abs / 1e6, 'M');
    if (abs >= 1e4) return cut(abs / 1e3, 'K');
    return formatNumber(value);
  }

  function capitalize(text) {
    return text ? text.charAt(0).toUpperCase() + text.slice(1) : '';
  }

  // Every category of the left column, in display order.
  function categories(snapshot) {
    const skills = (snapshot && snapshot.skills) || [];
    const list = [{ key: 'overall', label: 'Overall', group: 'skills' }];
    for (const name of skills) list.push({ key: name, label: capitalize(name), group: 'skills' });
    for (const spec of WEALTH) list.push({ key: spec.key, label: spec.label, group: 'wealth', hint: spec.hint });
    return list;
  }

  function isWealth(category) {
    return WEALTH.some((spec) => spec.key === category);
  }

  // One player's cell for a category: {rank, level, xp} for skills,
  // {rank, amount} for wealth, null when unranked (market down).
  function cellOf(player, category, snapshot) {
    if (category === 'overall') {
      const cell = player.overall || [];
      return { rank: cell[0], level: cell[1], xp: cell[2] };
    }
    if (isWealth(category)) {
      const cell = player[category];
      return Array.isArray(cell) ? { rank: cell[0], amount: cell[1] } : null;
    }
    const slot = ((snapshot && snapshot.skills) || []).indexOf(category);
    const cell = slot >= 0 && player.skills ? player.skills[slot] : null;
    return cell ? { rank: cell[0], level: cell[1], xp: cell[2] } : null;
  }

  // Ranking rows of a category, best first (ranks come from the daemon).
  function rankingRows(snapshot, category) {
    const players = (snapshot && snapshot.players) || [];
    const rows = [];
    for (const player of players) {
      const cell = cellOf(player, category, snapshot);
      if (!cell || cell.rank === undefined || cell.rank === null) continue;
      rows.push(Object.assign({ account: player.account, name: player.name || player.account, iron: !!player.iron, source: player.source }, cell));
    }
    rows.sort((left, right) => left.rank - right.rank);
    return rows;
  }

  // Page `page` (1-based, clamped) of `rows`.
  function paginate(rows, page, perPage) {
    const size = perPage || PER_PAGE;
    const pages = Math.max(1, Math.ceil(rows.length / size));
    const current = Math.min(Math.max(1, Math.floor(page) || 1), pages);
    return { rows: rows.slice((current - 1) * size, current * size), page: current, pages: pages };
  }

  // Page holding `rank` (1-based).
  function pageOfRank(rank, perPage) {
    const size = perPage || PER_PAGE;
    return Math.max(1, Math.ceil((parseInt(rank, 10) || 1) / size));
  }

  // Name search: account or displayed name, case-blind, spaces and
  // underscores alike; an exact match wins over a prefix match.
  function findPlayer(snapshot, query) {
    const fold = (text) => String(text || '').trim().toLowerCase().replace(/[\s_]+/g, ' ');
    const wanted = fold(query);
    if (!wanted) return null;
    const players = (snapshot && snapshot.players) || [];
    const exact = players.find((player) => fold(player.account) === wanted || fold(player.name) === wanted);
    if (exact) return exact;
    return players.find((player) => fold(player.account).startsWith(wanted) || fold(player.name).startsWith(wanted)) || null;
  }

  function parseHash(hash) {
    const state = { cat: 'overall', page: 1, player: '', rank: 0 };
    const text = String(hash || '').replace(/^#/, '');
    for (const pair of text.split('&')) {
      if (!pair) continue;
      const cut = pair.indexOf('=');
      const key = cut < 0 ? pair : pair.slice(0, cut);
      const value = cut < 0 ? '' : decodeURIComponent(pair.slice(cut + 1));
      if (key === 'cat' && /^[a-z]+$/.test(value)) state.cat = value;
      else if (key === 'page') state.page = Math.max(1, parseInt(value, 10) || 1);
      else if (key === 'player') state.player = value;
      else if (key === 'rank') state.rank = Math.max(0, parseInt(value, 10) || 0);
    }
    return state;
  }

  function formatHash(state) {
    if (state.player) return '#player=' + encodeURIComponent(state.player);
    const parts = ['cat=' + encodeURIComponent(state.cat || 'overall')];
    if (state.page && state.page > 1) parts.push('page=' + state.page);
    if (state.rank) parts.push('rank=' + state.rank);
    return '#' + parts.join('&');
  }

  // Seconds -> "12 min", "2 h 05", "40 s".
  function span(seconds) {
    const value = Math.max(0, Math.round(seconds));
    if (value < 60) return value + ' s';
    if (value < 3600) return Math.round(value / 60) + ' min';
    const hours = Math.floor(value / 3600);
    const minutes = Math.floor((value % 3600) / 60);
    return hours + ' h ' + String(minutes).padStart(2, '0');
  }

  // "updated 12 min ago · next in 18 min" from the snapshot clock.
  function freshness(snapshot, nowSeconds) {
    if (!snapshot || !snapshot.generated_at) return '';
    const ago = nowSeconds - snapshot.generated_at;
    let text = 'updated ' + span(ago) + ' ago';
    if (snapshot.next_at) {
      const left = snapshot.next_at - nowSeconds;
      text += left > 0 ? ' · next update in ' + span(left) : ' · next update due now';
    }
    return text;
  }

  // UTC clock of a unix time: "2026-11-14 16:30 UTC".
  function utcStamp(seconds) {
    if (!seconds) return '-';
    return new Date(seconds * 1000).toISOString().slice(0, 16).replace('T', ' ') + ' UTC';
  }

  return {
    PER_PAGE, WEALTH, esc, formatNumber, shortCoins, capitalize, categories, isWealth, cellOf,
    rankingRows, paginate, pageOfRank, findPlayer, parseHash, formatHash, span, freshness, utcStamp,
  };
})();

if (typeof module !== 'undefined' && module.exports) module.exports = Hiscores;

if (typeof document !== 'undefined') {
  const page = Hiscores;
  const esc = page.esc;
  let snapshot = null;
  let loadError = '';
  let state = page.parseHash(location.hash);

  const nameCell = (row) => {
    let html = '<a href="' + page.formatHash({ player: row.account }) + '">' + esc(row.name) + '</a>';
    if (row.iron) html += ' <span class="tag" title="Ironman: plays alone">iron</span>';
    if (row.source === 'save') html += ' <span class="tag off" title="Offline: read from the save file">offline</span>';
    return html;
  };

  function renderCategories() {
    const list = page.categories(snapshot);
    let html = '';
    let group = '';
    for (const entry of list) {
      if (entry.group !== group) {
        group = entry.group;
        html += '<li class="head">' + (group === 'skills' ? 'Skills' : 'Wealth') + '</li>';
      }
      const current = !state.player && state.cat === entry.key;
      html += '<li><a class="cat' + (current ? ' cur' : '') + '" href="' + page.formatHash({ cat: entry.key })
        + '"' + (entry.hint ? ' title="' + esc(entry.hint) + '"' : '') + '>' + esc(entry.label) + '</a></li>';
    }
    document.getElementById('cats').innerHTML = html;
  }

  function pager(current, pages) {
    const link = (target, label, enabled) => enabled
      ? '<a class="btn" href="' + page.formatHash({ cat: state.cat, page: target }) + '">' + label + '</a>'
      : '<span class="btn off">' + label + '</span>';
    return link(1, '« Top', current > 1) + link(current - 1, '‹ Up ' + page.PER_PAGE, current > 1)
      + '<span class="pageno">page ' + current + ' / ' + pages + '</span>'
      + link(current + 1, 'Down ' + page.PER_PAGE + ' ›', current < pages);
  }

  function renderRanking() {
    const category = page.categories(snapshot).find((entry) => entry.key === state.cat) || page.categories(snapshot)[0];
    state.cat = category.key;
    const wealth = page.isWealth(category.key);
    const rows = page.rankingRows(snapshot, category.key);
    const paged = page.paginate(rows, state.rank ? page.pageOfRank(state.rank) : state.page);
    state.page = paged.page;
    let html = '<h2 class="scroll-title">' + esc(category.label) + ' Hiscores</h2>';
    if (wealth) html += '<p class="note">' + esc(category.hint) + '</p>';
    if (wealth && category.key === 'value' && snapshot && snapshot.market === false) {
      html += '<p class="note warn">Market down at the last snapshot: total value not ranked.</p>';
    }
    html += '<table class="ranks"><thead><tr><th>Rank</th><th class="left">Name</th>'
      + (wealth ? '<th>' + esc(category.label) + '</th>' : '<th>Level</th><th>XP</th>') + '</tr></thead><tbody>';
    for (const row of paged.rows) {
      const mark = state.rank && row.rank === state.rank ? ' class="hit"' : '';
      html += '<tr' + mark + '><td>' + page.formatNumber(row.rank) + '</td><td class="left">' + nameCell(row) + '</td>';
      html += wealth
        ? '<td title="' + page.formatNumber(row.amount) + ' gp">' + page.formatNumber(row.amount) + '</td>'
        : '<td>' + page.formatNumber(row.level) + '</td><td>' + page.formatNumber(row.xp) + '</td>';
      html += '</tr>';
    }
    if (!paged.rows.length) html += '<tr><td colspan="4" class="left note">No ranked player yet.</td></tr>';
    html += '</tbody></table><div class="pager">' + pager(paged.page, paged.pages) + '</div>';
    document.getElementById('board').innerHTML = html;
  }

  function renderPlayer() {
    const player = page.findPlayer(snapshot, state.player);
    if (!player) {
      document.getElementById('board').innerHTML = '<h2 class="scroll-title">Personal scores</h2><p class="note warn">No player named “'
        + esc(state.player) + '” in the hiscores.</p><p><a href="' + page.formatHash({ cat: 'overall' }) + '">Back to the overall ranking</a></p>';
      return;
    }
    let html = '<h2 class="scroll-title">Personal scores for ' + esc(player.name) + '</h2>';
    if (player.iron || player.source === 'save') {
      html += '<p class="note">' + (player.iron ? 'Ironman. ' : '') + (player.source === 'save' ? 'Offline: read from the save file.' : '') + '</p>';
    }
    html += '<table class="ranks"><thead><tr><th class="left">Skill</th><th>Rank</th><th>Level</th><th>XP</th></tr></thead><tbody>';
    for (const entry of page.categories(snapshot).filter((item) => item.group === 'skills')) {
      const cell = page.cellOf(player, entry.key, snapshot) || {};
      const href = page.formatHash({ cat: entry.key, rank: cell.rank });
      html += '<tr><td class="left"><a href="' + href + '">' + esc(entry.label) + '</a></td><td>' + page.formatNumber(cell.rank)
        + '</td><td>' + page.formatNumber(cell.level) + '</td><td>' + page.formatNumber(cell.xp) + '</td></tr>';
    }
    html += '</tbody></table>';
    html += '<h3 class="scroll-sub">Wealth</h3><table class="ranks"><thead><tr><th class="left">Ranking</th><th>Rank</th><th>Amount</th></tr></thead><tbody>';
    for (const spec of page.WEALTH) {
      const cell = page.cellOf(player, spec.key, snapshot);
      const href = cell ? page.formatHash({ cat: spec.key, rank: cell.rank }) : page.formatHash({ cat: spec.key });
      html += '<tr><td class="left"><a href="' + href + '" title="' + esc(spec.hint) + '">' + esc(spec.label) + '</a></td><td>'
        + (cell ? page.formatNumber(cell.rank) : '-') + '</td><td>' + (cell ? page.formatNumber(cell.amount) + ' gp' : '-') + '</td></tr>';
    }
    html += '</tbody></table>';
    const top = player.top || [];
    html += '<h3 class="scroll-sub">Most valuable holdings</h3>';
    if (top.length) {
      html += '<table class="ranks"><thead><tr><th class="left">Item</th><th>Count</th><th>Unit price</th><th>Value</th></tr></thead><tbody>';
      for (const item of top) {
        html += '<tr><td class="left">' + esc(item.name) + '</td><td>' + page.formatNumber(item.count) + '</td><td>'
          + page.formatNumber(item.price) + '</td><td>' + page.formatNumber(item.value) + '</td></tr>';
      }
      html += '</tbody></table>';
    } else {
      html += '<p class="note">Nothing with a market price.</p>';
    }
    html += '<p><a href="' + page.formatHash({ cat: 'overall', rank: (player.overall || [])[0] }) + '">Back to the overall ranking</a></p>';
    document.getElementById('board').innerHTML = html;
  }

  function renderStatus() {
    const now = Date.now() / 1000;
    const status = document.getElementById('status');
    if (!snapshot) {
      status.textContent = loadError || 'loading…';
      return;
    }
    const ago = page.freshness(snapshot, now).split(' · ')[0];
    status.textContent = '· ' + ((snapshot.players || []).length) + ' players' + (ago ? ' · ' + ago : '');
    const errors = (snapshot.errors || []).filter(Boolean);
    const box = document.getElementById('errors');
    box.style.display = errors.length ? 'block' : 'none';
    box.textContent = errors.join(' · ');
  }

  function render() {
    renderStatus();
    if (!snapshot) {
      document.getElementById('board').innerHTML = '<p class="note">' + esc(loadError || 'Loading the hiscores…') + '</p>';
      document.getElementById('cats').innerHTML = '';
      return;
    }
    renderCategories();
    if (state.player) renderPlayer();
    else renderRanking();
  }

  async function reload() {
    try {
      const response = await fetch('/api/hiscores', { cache: 'no-store' });
      const data = await response.json();
      if (!data.ok) {
        loadError = data.error || 'hiscores unavailable';
        if (!snapshot) render();
        return;
      }
      const changed = !snapshot || snapshot.generated_at !== data.generated_at || snapshot.next_at !== data.next_at;
      snapshot = data;
      loadError = '';
      if (changed) render();
      else renderStatus();
    } catch (error) {
      loadError = 'hiscores unreachable: ' + error;
      renderStatus();
    }
  }

  window.addEventListener('hashchange', () => {
    state = page.parseHash(location.hash);
    render();
    window.scrollTo(0, 0);
  });
  document.getElementById('byname').addEventListener('submit', (event) => {
    event.preventDefault();
    const query = document.getElementById('name').value;
    const player = page.findPlayer(snapshot, query);
    location.hash = page.formatHash({ player: player ? player.account : query.trim() });
  });
  document.getElementById('byrank').addEventListener('submit', (event) => {
    event.preventDefault();
    const rank = parseInt(document.getElementById('rank').value, 10) || 1;
    location.hash = page.formatHash({ cat: state.player ? 'overall' : state.cat, rank: rank });
  });

  render();
  reload();
  setInterval(reload, 300000);
  setInterval(renderStatus, 30000);
}
