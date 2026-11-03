// Scenery painter for the LifeScape spectator: houses and walls as
// oriented segments (snapshot angle 0=W 1=N 2=E 3=S, straight-wall
// shapes 0-3), solid blocks for other obstacles, doorway markers for
// actionable non-blocking locs (open doors, ladders: the live snapshot
// already reflects the open/closed state), ground items as red dots
// with their name, flowers as small dots overlaid on the ground,
// never labelled.
(() => {
  const WALL_SHAPES = new Set([0, 1, 2, 3]); // straight walls; else solid

  function isFlower(name) {
    return /flower/i.test(name || "");
  }

  function wallBar(ctx, sx, sy, half, angle) {
    // Straight wall on the edge its angle faces: 0=W 1=N 2=E 3=S.
    ctx.fillStyle = '#7d7d7d';
    if (angle === 0) ctx.fillRect(sx - half, sy - half + 1, 5, 16 - 2);
    else if (angle === 2) ctx.fillRect(sx + half - 5, sy - half + 1, 5, 16 - 2);
    else if (angle === 1) ctx.fillRect(sx - half + 1, sy - half, 16 - 2, 5);
    else ctx.fillRect(sx + half + 1, sy + half - 5, 16 - 2, 5);
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
      // label only actionable scenery, close and not already named: the map
      // is dense enough that every name would overlap.
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
      labels.push([sx + 6, sy + 4, '#ccc', '10px monospace', String(obj.name || ('object #' + obj.id)).slice(0, 30)]);
    }
  }

  window.SpectateScenery = { drawScenery };
})();
