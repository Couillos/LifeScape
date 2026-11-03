// Detailed tile mode for the LifeScape spectator.
//
// Flat mode paints one mapped colour per tile plus plain scenery markers.
// Detail mode renders everything the snapshot carries:
//   - ground tiles y×y from the true cache colours. Textures are
//     downsampled by the SERVER (detail_table) and arrive over the
//     websocket; the canvas paints them asynchronously as the table
//     arrives, with no HTTP texture request and no main-thread stall.
//     y comes from #detaily (1..32): y=1 is the true average colour,
//     higher y looks closer to the in-game texture, still 2D.
//   - houses and walls as oriented segments (snapshot angle 0=W 1=N
//     2=E 3=S, straight-wall shapes 0-3), solid blocks for other
//     obstacles, doorway markers for actionable non-blocking locs
//     (open doors, ladders: the live snapshot already reflects the
//     open/closed state, so doors follow it with no extra channel).
//   - ground items as red dots with their name, no texture.
//   - flowers (locs and ground items with "flower" in the name) as
//     small dots overlaid on the ground, never labelled.
//
// Front RAM cache: identical tiles share one prerendered TILE×TILE
// canvas, keyed by floor id + resolution, so a repeated grass/water
// tile is painted once no matter how many times it appears. Vector
// overlays (walls, doors, items) are drawn per frame: they move.
(() => {
  const DEFAULT_Y = 8;
  const WALL_SHAPES = new Set([0, 1, 2, 3]); // straight walls; else solid

  const tileCache = new Map(); // "floorId:y" -> prerendered canvas
  let table = null; // {y, flo, tex} from the websocket, null until it arrives
  let detailOn = false;
  let detailY = DEFAULT_Y;
  let resend = null; // assigned by the main script: resend follow + prefs

  try {
    detailOn = localStorage.getItem("spectate-detail") === "1";
    const savedY = parseInt(localStorage.getItem("spectate-detail-y") || String(DEFAULT_Y), 10);
    if (savedY >= 1 && savedY <= 32) detailY = savedY;
  } catch (storageError) { /* private mode: stay on defaults */ }

  function syncControls() {
    const button = document.getElementById("detailbtn");
    const slider = document.getElementById("detaily");
    const value = document.getElementById("detailval");
    if (button) button.textContent = detailOn ? "Detail: on" : "Detail: off";
    if (slider) {
      slider.style.display = detailOn ? "" : "none";
      slider.value = String(detailY);
    }
    if (value) {
      value.style.display = detailOn ? "" : "none";
      value.textContent = String(detailY);
    }
  }

  function isFlower(name) {
    return /flower/i.test(name || "");
  }

  function hexToRgb(hex) {
    const value = parseInt(String(hex || "").slice(1), 16);
    return [value >> 16 & 255, value >> 8 & 255, value & 255];
  }

  function buildTile(floorId, fallbackCss) {
    const info = table && table.flo ? table.flo[floorId] : null;
    if (!info) return null; // table not here yet: caller paints flat meanwhile
    const canvas = document.createElement("canvas");
    canvas.width = 16;
    canvas.height = 16;
    const paint = canvas.getContext("2d");
    const pixels = info.texture && table.tex ? table.tex[info.texture] : null;
    if (!pixels) {
      paint.fillStyle = info.color || fallbackCss;
      paint.fillRect(0, 0, 16, 16);
      return canvas;
    }
    const resolution = table.y;
    const cells = document.createElement("canvas");
    cells.width = resolution;
    cells.height = resolution;
    const sample = cells.getContext("2d");
    const image = sample.createImageData(resolution, resolution);
    for (let index = 0; index < resolution * resolution; index++) {
      const [red, green, blue] = hexToRgb(pixels[index]);
      image.data[index * 4] = red;
      image.data[index * 4 + 1] = green;
      image.data[index * 4 + 2] = blue;
      image.data[index * 4 + 3] = 255;
    }
    sample.putImageData(image, 0, 0);
    paint.imageSmoothingEnabled = false;
    paint.drawImage(cells, 0, 0, 16, 16);
    return canvas;
  }

  function drawTile(ctx, left, top, size, floorId, fallbackCss) {
    if (!detailOn || !table) return false;
    const key = floorId + ":" + table.y;
    let tile = tileCache.get(key);
    if (!tile) {
      tile = buildTile(floorId, fallbackCss);
      if (!tile) return false;
      tileCache.set(key, tile);
    }
    ctx.drawImage(tile, left, top, size, size);
    return true;
  }

  function wallBar(ctx, sx, sy, half, angle) {
    // Straight wall on the edge its angle faces: 0=W 1=N 2=E 3=S.
    ctx.fillStyle = '#7d7d7d';
    if (angle === 0) ctx.fillRect(sx - half, sy - half + 1, 5, 16 - 2);
    else if (angle === 2) ctx.fillRect(sx + half - 5, sy - half + 1, 5, 16 - 2);
    else if (angle === 1) ctx.fillRect(sx - half + 1, sy - half, 16 - 2, 5);
    else ctx.fillRect(sx - half + 1, sy + half - 5, 16 - 2, 5);
  }

  function dot(ctx, sx, sy, radius, color) {
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.arc(sx, sy, radius, 0, Math.PI * 2);
    ctx.fill();
  }

  function drawScenery(ctx, view, locs, objs, flora, labels) {
    const labelled = new Set();
    let labelCount = 0;
    const pushLabel = (sx, sy, text) => {
      labels.push([sx + 6, sy + 4, '#ccc', '10px monospace', String(text).slice(0, 30)]);
    };
    for (const loc of locs || []) {
      const sx = view.cx + (loc.x - view.px) * view.tile;
      const sy = view.cy - (loc.z - view.pz) * view.tile;
      if (sx < -30 || sy < -12 || sx > view.w + 30 || sy > view.h + 12) continue;
      if (isFlower(loc.name)) {
        dot(ctx, sx, sy, 2.5, '#e78ac3'); // overlaid on the ground, no name
        continue;
      }
      if (loc.block && WALL_SHAPES.has(loc.shape)) {
        wallBar(ctx, sx, sy, view.half, (loc.angle || 0) & 3);
      } else if (loc.block) {
        ctx.fillStyle = '#565656';
        ctx.fillRect(sx - view.half, sy - view.half, view.tile, view.tile);
        ctx.strokeStyle = '#333333';
        ctx.strokeRect(sx - view.half + 0.5, sy - view.half + 0.5, view.tile - 1, view.tile - 1);
      } else if (loc.act) {
        ctx.fillStyle = '#8a6d3b'; // open door, ladder, gate: live state
        ctx.fillRect(sx - 4, sy - 4, 8, 8);
      } else {
        ctx.fillStyle = '#2a5a3a';
        ctx.fillRect(sx - 3, sy - 3, 6, 6);
      }
      if (loc.act && labelCount < 14 && Math.abs(loc.x - view.px) + Math.abs(loc.z - view.pz) <= 6 && !labelled.has(loc.name)) {
        labelled.add(loc.name);
        labelCount += 1;
        labels.push([sx + 5, sy - 4, '#8d9', '9px monospace', String(loc.name || ('scenery #' + loc.id)).slice(0, 22)]);
      }
    }
    for (const weed of flora || []) {
      const sx = view.cx + (weed.x - view.px) * view.tile;
      const sy = view.cy - (weed.z - view.pz) * view.tile;
      if (sx < -20 || sy < -20 || sx > view.w + 20 || sy > view.h + 20) continue;
      dot(ctx, sx, sy, 2.5, '#e78ac3');
    }
    for (const obj of objs || []) {
      const sx = view.cx + (obj.x - view.px) * view.tile;
      const sy = view.cy - (obj.z - view.pz) * view.tile;
      if (sx < -20 || sy < -20 || sx > view.w + 20 || sy > view.h + 20) continue;
      if (isFlower(obj.name)) {
        dot(ctx, sx, sy, 2.5, '#e78ac3'); // no texture, no name
        continue;
      }
      dot(ctx, sx, sy, 3, '#ff3b30');
      pushLabel(sx, sy, obj.name || ('object #' + obj.id));
    }
  }

  document.getElementById("detailbtn").onclick = () => {
    detailOn = !detailOn;
    try { localStorage.setItem("spectate-detail", detailOn ? "1" : "0"); } catch (storageError) { /* private mode */ }
    if (resend) resend(); // server (re)sends the texture table over the socket
    syncControls();
  };
  document.getElementById("detaily").oninput = (event) => {
    const next = parseInt(event.target.value, 10) || 1;
    detailY = Math.min(32, Math.max(1, next));
    try { localStorage.setItem("spectate-detail-y", String(detailY)); } catch (storageError) { /* private mode */ }
    if (resend) resend(); // server downsamples at the new resolution
    syncControls();
  };

  syncControls();

  window.SpectateDetail = {
    drawTile,
    drawScenery,
    prefs() { return {on: detailOn, y: detailY}; },
    setTable(detail) { table = detail; tileCache.clear(); },
    set onPrefsChange(callback) { resend = callback; },
    get on() { return detailOn; },
  };
})();
