// LifeChart: shared canvas charts, no dependency (PRD2 §6.1).
// Fixed time slots (96 quarter-hour, 168 hourly, 30 or 183 daily, empty slot when no datum),
// graduated left and right scales with titles, always-visible legend,
// hover and touch tooltip with crosshair, live end.
// Series: `candles` (market OHLCV) and `line` (holdings curve).
//
// Pure helpers (slotStarts, liveSlots, fillSlots, carryFlat, candleColor, niceTicks, axisLabel, candleTooltip,
// volumeSplit, volumeScaleMax, foldPrint) touch no
// DOM, so `node --test chart.test.js` covers them; draw() needs a canvas.
const LifeChart = (() => {
  'use strict';

  // Fixed slot starts: `count` windows of `width` seconds from `from`.
  function slotStarts(from, width, count) {
    const slots = [];
    for (let i = 0; i < count; i++) slots.push(from + i * width);
    return slots;
  }

  // The `count` slots ending on the one holding `now` (the live candle):
  // the last slot is the current window, never the previous one.
  function liveSlots(now, width, count) {
    const live = Math.floor(now / width) * width;
    return slotStarts(live - (count - 1) * width, width, count);
  }

  // Aligns sparse rows (`{t, ...}`) onto fixed slots: row or null per
  // slot. Empty slots stay null so the graph shows a gap, never a fake
  // flat line. Last row wins on duplicate timestamps.
  function fillSlots(slots, rows) {
    const byTime = new Map();
    for (const row of rows || []) byTime.set(row.t, row);
    return slots.map((start) => byTime.get(start) || null);
  }

  // Graduated ticks covering [lo, hi]: round 1/2/2.5/5 steps, at most
  // `maxCount` intervals. Zero range widens by one unit each side.
  function niceTicks(lo, hi, maxCount) {
    let low = lo, high = hi;
    if (!(high > low)) { low -= 1; high += 1; }
    const count = Math.max(1, maxCount || 4);
    const raw = (high - low) / count;
    const mag = Math.pow(10, Math.floor(Math.log10(raw)));
    let step = mag;
    for (const mult of [1, 2, 2.5, 5, 10]) {
      if (mult * mag >= raw) { step = mult * mag; break; }
    }
    const ticks = [];
    const start = Math.floor(low / step) * step;
    const end = Math.ceil(high / step - 1e-9) * step;
    for (let value = start; value <= end + 1e-9; value += step) {
      ticks.push(Math.round(value * 1e9) / 1e9);
    }
    return ticks;
  }

  // Least pixels between two axis labels (time axis, value axes).
  const LABEL_GAP_X = 14;
  const LABEL_GAP_Y = 13;

  // Slots between two time labels so `count` labels `labelW` wide fit
  // in `plotW` with `LABEL_GAP_X` between them.
  function labelEvery(count, plotW, labelW) {
    if (count <= 0) return 1;
    const fit = Math.max(1, Math.floor(plotW / (labelW + LABEL_GAP_X)));
    return Math.max(1, Math.ceil(count / fit));
  }

  // Tick intervals a pane `height` px tall holds `spacing` px apart,
  // between 1 and `max`.
  function tickCount(height, spacing, max) {
    return Math.max(1, Math.min(max, Math.floor(height / spacing)));
  }

  // True (and noted) when `y` sits at least `LABEL_GAP_Y` from every
  // label already drawn on that side.
  function spaced(taken, y) {
    if (taken.some((other) => Math.abs(other - y) < LABEL_GAP_Y)) return false;
    taken.push(y);
    return true;
  }

  function pad2(value) {
    return String(value).padStart(2, '0');
  }

  // Human period label for one slot: `12/11 14:00–15:00` hourly
  // (`12/11 14:15–14:30` for a quarter),
  // `12/11` daily (local time, like the rest of the dashboard).
  function periodLabel(slotStart, width) {
    const open = new Date(slotStart * 1000);
    if (width >= 86400) return pad2(open.getDate()) + '/' + pad2(open.getMonth() + 1);
    const close = new Date((slotStart + width) * 1000);
    return pad2(open.getDate()) + '/' + pad2(open.getMonth() + 1) + ' ' +
      pad2(open.getHours()) + ':' + pad2(open.getMinutes()) + '–' +
      pad2(close.getHours()) + ':' + pad2(close.getMinutes());
  }

  // Time-axis label for one slot: `dd/mm` daily, `HH:MM` hourly or
  // quarter-hourly within
  // a day, `dd/mm HH:MM` hourly over several days (7-day view: bare
  // hours would repeat across days).
  function axisLabel(slotStart, width, slotCount) {
    const date = new Date(slotStart * 1000);
    const day = pad2(date.getDate()) + '/' + pad2(date.getMonth() + 1);
    if (width >= 86400) return day;
    const hour = pad2(date.getHours()) + ':' + pad2(date.getMinutes());
    return slotCount * width > 86400 ? day + ' ' + hour : hour;
  }

  // Tooltip lines for one candle: period, O/H/L/C, volume, trade count
  // (PRD2 §6.2). Tolerates rows without `n` (old payloads).
  function candleTooltip(slotStart, width, row) {
    const split = volumeSplit(row);
    return [
      periodLabel(slotStart, width),
      'O ' + row.o + ' H ' + row.h + ' L ' + row.l + ' C ' + row.c,
      'Volume ' + row.v,
      'Buy vol ' + split.buy,
      'Sell vol ' + split.sell,
      'Trades ' + (row.n === undefined || row.n === null ? '—' : row.n),
    ];
  }

  // Volume of one candle by taker side: `vb` (a buyer took the asks),
  // `vs` (a seller hit the bids). What is left of `v` was settled before
  // the server recorded the side: unknown, drawn grey, never guessed.
  // Rows without `vb`/`vs` (old payloads) are all unknown.
  function volumeSplit(row) {
    const buy = row.vb > 0 ? row.vb : 0;
    const sell = row.vs > 0 ? row.vs : 0;
    const total = row.v > 0 ? row.v : 0;
    return { buy, sell, unknown: Math.max(0, total - buy - sell) };
  }

  // Shared scale of the mirrored volume pane: the taller of the upper
  // stack (buy + unknown) and the lower bar (sell), over present rows.
  function volumeScaleMax(rows) {
    let most = 0;
    for (const row of rows || []) {
      if (!row) continue;
      const split = volumeSplit(row);
      most = Math.max(most, split.buy + split.unknown, split.sell);
    }
    return most;
  }

  // Folds one live print (`{q, p, s}`, `s` = taker side or null) into a
  // candle in place: OHLC, total volume, buy or sell share, count.
  function foldPrint(candle, trade) {
    candle.h = Math.max(candle.h, trade.p);
    candle.l = Math.min(candle.l, trade.p);
    candle.c = trade.p;
    candle.v = (candle.v || 0) + trade.q;
    candle.vb = (candle.vb || 0) + (trade.s === 'buy' ? trade.q : 0);
    candle.vs = (candle.vs || 0) + (trade.s === 'sell' ? trade.q : 0);
    candle.n = (candle.n || 0) + 1;
    return candle;
  }

  function lineTooltip(slotStart, width, value) {
    return [periodLabel(slotStart, width), 'Value ' + value];
  }

  // One candle per slot, always, never a gap (user rule 2026-11-13): a
  // slot without trades shows a flat candle at the last known close
  // (volume 0); every traded candle opens at the previous close (its
  // high/low widen to include it), so a price move between two windows
  // shows as that candle's colour, never as a jump. The close before the
  // window (`previousClose`) carries the leading slots and opens the
  // first candle. Slots before any known price stay empty. The server
  // history is already chained; this keeps the live tail (WS prints
  // folded in the browser) chained too. Rows are copied, never mutated.
  function carryFlat(rows, previousClose) {
    let close = previousClose === undefined ? null : previousClose;
    return rows.map((row) => {
      if (row) {
        const chained = close === null
          ? row
          : { ...row, o: close, h: Math.max(row.h, close), l: Math.min(row.l, close) };
        close = row.c;
        return chained;
      }
      if (close === null) return null;
      return { o: close, h: close, l: close, c: close, v: 0, vb: 0, vs: 0, n: 0 };
    });
  }

  // Vivid pair kept apart by luminance as well as hue (deuteranopia:
  // bright green vs deep red stay distinct even when hues merge).
  const UP = '#00e676';
  const DOWN = '#ff1744';
  // Volume pane (under the candles): buy volume UP above a mid baseline,
  // sell volume DOWN below it, mirrored on one scale.

  // Flat trade-less candle (`carryFlat`): neutral grey, neither up nor
  // down. Also the volume of unknown taker side (`volumeSplit`), stacked
  // above the green bar.
  const FLAT = '#888';

  function candleColor(row) {
    if (!(row.v > 0) && row.o === row.c && row.h === row.l) return FLAT;
    return row.c >= row.o ? UP : DOWN;
  }

  // Full render. Model:
  // { slots, width, kind: 'candles'|'line', rows (aligned, null = empty),
  //   leftTitle, rightTitle, legend: [{swatch, label}], live, emptyText,
  //   tooltipFor(slot, row) -> string[] }
  // Canvas height for a width: the 720:340 ratio, capped by `maxHeight`
  // (a page that must fit without scrolling, e.g. errors).
  function chartHeight(width, maxHeight) {
    const natural = Math.round(width * 340 / 720);
    return maxHeight ? Math.min(natural, maxHeight) : natural;
  }

  function draw(canvas, model) {
    const ctx = canvas.getContext('2d');
    const ratio = (typeof window !== 'undefined' && window.devicePixelRatio) || 1;
    const width = Math.max(320, canvas.clientWidth || 720);
    const height = chartHeight(width, model.maxHeight);
    canvas.width = Math.round(width * ratio);
    canvas.height = Math.round(height * ratio);
    canvas.style.height = height + 'px';
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    ctx.fillStyle = '#111';
    ctx.fillRect(0, 0, width, height);

    const slots = model.slots || [];
    const rows = model.rows || [];
    const padLeft = 64, padRight = 64, padBottom = 24;
    ctx.font = '11px monospace';
    // Legend first (always visible): wrap entries, count lines, size
    // the top pad so wrapped legend rows never cover the plot.
    let legendLines = 0;
    {
      let used = 0;
      const avail = Math.max(10, width - padLeft - padRight - 8);
      for (const entry of model.legend || []) {
        const need = 18 + 4 + ctx.measureText(entry.label || '').width + 16;
        if (used + need > avail && used > 0) { legendLines++; used = 0; }
        used += need;
      }
      if ((model.legend || []).length) legendLines++;
    }
    const padTop = 8 + legendLines * 14 + 6;
    const plotX = padLeft, plotW = Math.max(10, width - padLeft - padRight);
    const plotY = padTop, plotH = Math.max(10, height - padTop - padBottom);
    // Candles: price pane on top (2/3), volume pane below (1/3), a gap
    // between them so bars never cover a candle. Lines use the full plot.
    const volTop = plotY + plotH * (2 / 3);
    const priceH = model.kind === 'candles' ? Math.max(10, volTop - plotY - 8) : plotH;

    // Ranges over present rows only; empty slots never flatten the scale.
    let lo = Infinity, hi = -Infinity, volMax = 0;
    for (const row of rows) {
      if (!row) continue;
      if (model.kind === 'candles') {
        lo = Math.min(lo, row.l); hi = Math.max(hi, row.h);
      } else {
        lo = Math.min(lo, row); hi = Math.max(hi, row);
      }
    }
    if (!(hi > lo)) { lo -= 1; hi += 1; }
    if (model.kind === 'candles') volMax = volumeScaleMax(rows);
    if (volMax <= 0) volMax = 1;
    // Graduated ticks first: the scales stretch to their extremes so
    // gridlines never land outside the plot.
    // As many price intervals as the pane height holds labels apart.
    const priceTicks = niceTicks(lo, hi, tickCount(priceH, LABEL_GAP_Y * 2, 4));
    lo = Math.min(lo, priceTicks[0]);
    hi = Math.max(hi, priceTicks[priceTicks.length - 1]);
    let volTicks = [];
    if (model.kind === 'candles') {
      // One interval when the half-pane holds no two labels apart.
      volTicks = niceTicks(0, volMax, (plotY + plotH - volTop) / 2 >= LABEL_GAP_Y * 3 ? 2 : 1);
      volMax = Math.max(volMax, volTicks[volTicks.length - 1]);
    }

    const yOf = (price) => plotY + priceH - ((price - lo) / (hi - lo)) * priceH;
    // Mirrored volume pane: baseline in the middle, one scale both ways.
    const volBase = (volTop + plotY + plotH) / 2;
    const volHalf = Math.max(2, (plotY + plotH - volTop) / 2 - 2);
    const volUp = (vol) => volBase - (vol / volMax) * volHalf;
    const volDown = (vol) => volBase + (vol / volMax) * volHalf;
    const slotW = slots.length ? plotW / slots.length : plotW;
    const xOf = (i) => plotX + slotW * (i + 0.5);
    const barW = Math.max(1, Math.min(12, slotW * 0.55));

    // Volume bars in their own pane (bottom third, right scale): buy
    // volume green above the baseline, unknown side grey stacked on it,
    // sell volume red below.
    if (model.kind === 'candles') {
      ctx.strokeStyle = '#333';
      ctx.beginPath(); ctx.moveTo(plotX, volTop); ctx.lineTo(plotX + plotW, volTop); ctx.stroke();
      ctx.strokeStyle = '#555';
      ctx.beginPath(); ctx.moveTo(plotX, volBase); ctx.lineTo(plotX + plotW, volBase); ctx.stroke();
      for (let i = 0; i < rows.length; i++) {
        const row = rows[i];
        if (!row || !(row.v > 0)) continue;
        const split = volumeSplit(row);
        const left = xOf(i) - barW / 2;
        if (split.buy > 0) {
          ctx.fillStyle = UP;
          ctx.fillRect(left, volUp(split.buy), barW, volBase - volUp(split.buy));
        }
        if (split.unknown > 0) {
          ctx.fillStyle = FLAT;
          ctx.fillRect(left, volUp(split.buy + split.unknown), barW, volUp(split.buy) - volUp(split.buy + split.unknown));
        }
        if (split.sell > 0) {
          ctx.fillStyle = DOWN;
          ctx.fillRect(left, volBase, barW, volDown(split.sell) - volBase);
        }
      }
    }

    // Graduated gridlines, left price scale and right volume scale.
    ctx.lineWidth = 1;
    ctx.font = '10px monospace';
    ctx.strokeStyle = '#333';
    // Labels never closer than `LABEL_GAP_Y` on one side (gridlines
    // stay): the price scale's foot never collides with the volume pane.
    const leftTaken = [];
    for (const tick of priceTicks) {
      const y = yOf(tick);
      ctx.beginPath(); ctx.moveTo(plotX, y); ctx.lineTo(plotX + plotW, y); ctx.stroke();
      ctx.fillStyle = '#888';
      if (spaced(leftTaken, y)) ctx.fillText(String(Math.round(tick * 100) / 100), 6, y + 3);
    }
    if (model.kind === 'candles') {
      ctx.fillStyle = '#888';
      // Graduated both ways from the baseline (0 once, on it).
      const rightTaken = [volTop];
      for (const tick of volTicks) {
        const label = String(Math.round(tick * 100) / 100);
        const marks = tick > 0 ? [volUp(tick), volDown(tick)] : [volBase];
        for (const y of marks) {
          if (spaced(rightTaken, y)) ctx.fillText(label, plotX + plotW + 6, y + 3);
        }
      }
    }

    // Series.
    if (model.kind === 'candles') {
      for (let i = 0; i < rows.length; i++) {
        const row = rows[i];
        if (!row) continue;
        const x = xOf(i);
        ctx.strokeStyle = candleColor(row);
        ctx.fillStyle = candleColor(row);
        ctx.lineWidth = 1;
        ctx.beginPath(); ctx.moveTo(x, yOf(row.h)); ctx.lineTo(x, yOf(row.l)); ctx.stroke();
        const top = yOf(Math.max(row.o, row.c));
        const bottom = yOf(Math.min(row.o, row.c));
        if (row.h === row.l && row.o === row.c) {
          ctx.beginPath(); ctx.moveTo(x - barW / 2, top); ctx.lineTo(x + barW / 2, top); ctx.stroke();
        } else {
          ctx.fillRect(x - barW / 2, top, barW, Math.max(1, bottom - top));
        }
      }
      ctx.lineWidth = 1;
    } else {
      ctx.strokeStyle = UP;
      ctx.lineWidth = 2;
      ctx.beginPath();
      let pen = false;
      for (let i = 0; i < rows.length; i++) {
        if (rows[i] === null || rows[i] === undefined) { pen = false; continue; }
        const x = xOf(i), y = yOf(rows[i]);
        if (!pen) { ctx.moveTo(x, y); pen = true; } else ctx.lineTo(x, y);
      }
      ctx.stroke();
      ctx.lineWidth = 1;
    }

    // Live end: bright dot on the last present point.
    let liveIndex = -1;
    for (let i = rows.length - 1; i >= 0; i--) {
      if (rows[i] !== null && rows[i] !== undefined) { liveIndex = i; break; }
    }
    if (model.live && liveIndex >= 0) {
      const last = rows[liveIndex];
      const y = model.kind === 'candles' ? yOf(last.c) : yOf(last);
      ctx.fillStyle = '#fff';
      ctx.beginPath();
      ctx.arc(xOf(liveIndex), y, 4, 0, Math.PI * 2);
      ctx.fill();
    }

    // Time axis on fixed slots.
    ctx.fillStyle = '#888';
    ctx.font = '10px monospace';
    // As many labels as fit side by side at their measured width, each
    // centred on its slot and kept inside the plot.
    const sample = slots.length ? ctx.measureText(axisLabel(slots[0], model.width, slots.length)).width : 0;
    const every = labelEvery(slots.length, plotW, sample);
    let lastRight = -Infinity;
    for (let i = 0; i < slots.length; i += every) {
      const label = axisLabel(slots[i], model.width, slots.length);
      const textW = ctx.measureText(label).width;
      const left = Math.min(Math.max(xOf(i) - textW / 2, plotX), plotX + plotW - textW);
      if (left < lastRight + LABEL_GAP_X) continue;
      ctx.fillText(label, left, plotY + plotH + 14);
      lastRight = left + textW;
    }

    // Axis titles, rotated.
    ctx.save();
    ctx.fillStyle = '#888';
    ctx.font = '11px monospace';
    ctx.translate(12, plotY + priceH / 2);
    ctx.rotate(-Math.PI / 2);
    ctx.textAlign = 'center';
    ctx.fillText(model.leftTitle || '', 0, 0);
    ctx.restore();
    if (model.kind === 'candles') {
      ctx.save();
      ctx.fillStyle = '#888';
      ctx.font = '11px monospace';
      ctx.translate(width - 12, (volTop + plotY + plotH) / 2);
      ctx.rotate(Math.PI / 2);
      ctx.textAlign = 'center';
      ctx.fillText(model.rightTitle || '', 0, 0);
      ctx.restore();
    }
    ctx.textAlign = 'left';

    // Legend, always visible: swatches plus labels, wrapped in the top pad.
    let legendX = plotX + 4, legendLine = 0, legendY = 14;
    ctx.font = '11px monospace';
    for (const entry of model.legend || []) {
      const label = entry.label || '';
      const swatchW = 18;
      const textW = ctx.measureText(label).width;
      if (legendX + swatchW + 4 + textW > plotX + plotW - 4 && legendX > plotX + 4) {
        legendX = plotX + 4; legendLine++; legendY = 14 + legendLine * 14;
      }
      ctx.fillStyle = entry.color || '#888';
      if (entry.swatch === 'tick' || entry.swatch === 'line') {
        ctx.fillRect(legendX, legendY - 8, swatchW, 2);
      } else {
        ctx.fillRect(legendX, legendY - 11, swatchW, 7);
      }
      ctx.fillStyle = '#ccc';
      ctx.fillText(label, legendX + swatchW + 4, legendY - 3);
      legendX += swatchW + 4 + textW + 16;
    }

    const hasData = liveIndex >= 0;
    if (!hasData) {
      ctx.fillStyle = '#888';
      ctx.font = '14px monospace';
      ctx.fillText(model.emptyText || 'No data yet.', plotX + 20, plotY + plotH / 2);
    }

    // Hover and touch: nearest-slot crosshair plus tooltip box.
    const geometry = { plotX, plotW, plotY, plotH, priceH, slotW, count: slots.length };
    canvas._lifeModel = model;
    canvas._lifeGeometry = geometry;
    if (!canvas._lifeBound) {
      canvas._lifeBound = true;
      // The listeners bind once: they read the geometry of the CURRENT
      // draw (a period switch changes the slot count and width; the first
      // draw's 24 slots made 7 d / 30 d / 6 months point past the plot).
      const showAt = (event, touch) => {
        const current = canvas._lifeGeometry;
        if (!current) return;
        const box = canvas.getBoundingClientRect();
        const point = touch || (event.touches && event.touches[0]) || event;
        if (!point || point.clientX === undefined) return;
        drawOverlay(canvas, Math.round(((point.clientX - box.left) - current.plotX - current.slotW / 2) / current.slotW));
        if (event.preventDefault && event.touches) event.preventDefault();
      };
      canvas.addEventListener('mousemove', (event) => showAt(event, event));
      canvas.addEventListener('mouseleave', () => draw(canvas, canvas._lifeModel));
      canvas.addEventListener('touchstart', (event) => showAt(event), { passive: false });
      canvas.addEventListener('touchmove', (event) => showAt(event), { passive: false });
    }
  }

  // Crosshair and tooltip overlay for one slot: full redraw, then the
  // marker. Out-of-range or empty slots clear back to the plain graph.
  function drawOverlay(canvas, index) {
    const model = canvas._lifeModel, geometry = canvas._lifeGeometry;
    if (!model || !geometry) return;
    drawWithoutRebind(canvas, model, geometry);
    if (index < 0 || index >= model.slots.length) return;
    // Long views hold many empty slots (a short history): the nearest
    // slot with data answers instead of a blank hover.
    index = nearestFilled(model.rows, index);
    const row = model.rows[index];
    if (row === null || row === undefined) return;
    const ctx = canvas.getContext('2d');
    const ratio = (typeof window !== 'undefined' && window.devicePixelRatio) || 1;
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    const x = geometry.plotX + geometry.slotW * (index + 0.5);
    const base = overlayRange(model);
    const ticks = niceTicks(base.lo, base.hi, 4);
    const lo = Math.min(base.lo, ticks[0]);
    const hi = Math.max(base.hi, ticks[ticks.length - 1]);
    const y = model.kind === 'candles'
      ? geometry.plotY + (geometry.priceH || geometry.plotH) - ((row.c - lo) / (hi - lo)) * (geometry.priceH || geometry.plotH)
      : 0;
    ctx.strokeStyle = '#777';
    ctx.setLineDash([4, 3]);
    ctx.beginPath(); ctx.moveTo(x, geometry.plotY); ctx.lineTo(x, geometry.plotY + geometry.plotH); ctx.stroke();
    if (model.kind === 'candles') {
      ctx.beginPath(); ctx.moveTo(geometry.plotX, y); ctx.lineTo(geometry.plotX + geometry.plotW, y); ctx.stroke();
    }
    ctx.setLineDash([]);
    const lines = model.tooltipFor
      ? model.tooltipFor(model.slots[index], row)
      : (model.kind === 'candles'
        ? candleTooltip(model.slots[index], model.width, row)
        : lineTooltip(model.slots[index], model.width, row));
    ctx.font = '11px monospace';
    const boxW = Math.max(...lines.map((line) => ctx.measureText(line).width)) + 16;
    const boxH = lines.length * 14 + 10;
    let boxX = x + 12;
    if (boxX + boxW > geometry.plotX + geometry.plotW) boxX = x - boxW - 12;
    let boxY = (model.kind === 'candles' ? y : geometry.plotY) - boxH - 8;
    if (boxY < geometry.plotY) boxY = geometry.plotY + 4;
    ctx.fillStyle = 'rgba(0,0,0,0.88)';
    ctx.strokeStyle = '#555';
    ctx.fillRect(boxX, boxY, boxW, boxH);
    ctx.strokeRect(boxX, boxY, boxW, boxH);
    ctx.fillStyle = '#eee';
    lines.forEach((line, i) => ctx.fillText(line, boxX + 8, boxY + 16 + i * 14));
  }

  // Index of the slot with data nearest to `index` (itself when filled,
  // -1 when every slot is empty).
  function nearestFilled(rows, index) {
    for (let step = 0; step < rows.length; step++) {
      for (const at of [index - step, index + step]) {
        if (at >= 0 && at < rows.length && rows[at] !== null && rows[at] !== undefined) return at;
      }
    }
    return -1;
  }

  function overlayRange(model) {
    let lo = Infinity, hi = -Infinity;
    for (const row of model.rows) {
      if (!row) continue;
      if (model.kind === 'candles') { lo = Math.min(lo, row.l); hi = Math.max(hi, row.h); }
      else { lo = Math.min(lo, row); hi = Math.max(hi, row); }
    }
    if (!(hi > lo)) { lo -= 1; hi += 1; }
    return { lo, hi };
  }

  // Same as draw() without touching listeners: used under the overlay so
  // hovering never stacks duplicate handlers.
  function drawWithoutRebind(canvas, model, geometry) {
    const keep = canvas._lifeBound;
    canvas._lifeBound = true;
    draw(canvas, model);
    canvas._lifeBound = keep;
    canvas._lifeModel = model;
    canvas._lifeGeometry = geometry;
  }

  return {
    slotStarts, liveSlots, fillSlots, carryFlat, candleColor, niceTicks, periodLabel, axisLabel, candleTooltip, lineTooltip, nearestFilled, labelEvery, tickCount, spaced,
    volumeSplit, volumeScaleMax, foldPrint, draw, chartHeight, UP, DOWN, FLAT,
  };
})();

if (typeof module !== 'undefined' && module.exports) module.exports = LifeChart;
