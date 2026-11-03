// Scenery painter for the LifeScape spectator, drawn the way the game
// minimap draws it (Client.drawDetail, Client-TS branch 274).
//
// This paints into a 4 px per tile buffer, exactly like the client's
// 512x512 minimap image, and the caller scales that buffer up. So a wall
// is one pixel of a four-pixel tile, a diagonal is four pixels, and every
// line lands on the client's own pixel grid instead of being redrawn at
// screen resolution.
//
//   - walls: one bar on the tile edge the angle faces (0=W 1=N 2=E 3=S),
//     grey for plain walls, red for active ones (doors); WALL_L is two bars;
//   - square corner: one pixel; diagonal: the four pixels of the diagonal;
//   - shapes 1 (diagonal corner), 4-8 (wall decor) are never drawn: the
//     client does not draw them either;
//   - scenery with a mapscene sprite is drawn as that sprite, 1:1.
(() => {
  const WALL_GREY = "#eeeeee";
  const WALL_ACTIVE = "#ee0000";

  const sprites = new Map(); // mapscene index -> Image

  function sceneSprite(index) {
    let image = sprites.get(index);
    if (!image) {
      image = new Image();
      image.src = "/mapscene/" + index + ".png";
      sprites.set(index, image);
    }
    return image;
  }

  function tileOrigin(view, loc) {
    const x = (loc.x - view.originX) * view.tile;
    const y = (view.northZ - loc.z) * view.tile;
    return [x, y];
  }

  function drawSceneSprite(ctx, view, loc) {
    const image = sceneSprite(loc.mapscene);
    if (!image.complete || !image.naturalWidth) {
      return false; // still loading: the ground shows through meanwhile
    }
    const [left, top] = tileOrigin(view, loc);
    const width = loc.width || 1;
    const length = loc.length || 1;
    const offsetX = ((width * 4 - image.naturalWidth) / 2) | 0;
    const offsetY = ((length * 4 - image.naturalHeight) / 2) | 0;
    ctx.drawImage(
      image,
      left + offsetX + (loc.mapsceneX || 0),
      top + (1 - length) * 4 + offsetY + (loc.mapsceneY || 0),
      image.naturalWidth,
      image.naturalHeight
    );
    return true;
  }

  function isFlower(name) {
    return /flower/i.test(name || "");
  }

  function wallBar(ctx, left, top, angle) {
    if (angle === 0) ctx.fillRect(left, top, 1, 4);
    else if (angle === 1) ctx.fillRect(left, top, 4, 1);
    else if (angle === 2) ctx.fillRect(left + 3, top, 1, 4);
    else ctx.fillRect(left, top + 3, 4, 1);
  }

  function wallCorner(ctx, left, top, angle) {
    if (angle === 0) ctx.fillRect(left, top, 1, 1);
    else if (angle === 1) ctx.fillRect(left + 3, top, 1, 1);
    else if (angle === 2) ctx.fillRect(left + 3, top + 3, 1, 1);
    else ctx.fillRect(left, top + 3, 1, 1);
  }

  function wallDiagonal(ctx, left, top, angle) {
    // Client drawDetail: the four pixels of the 4x4 block diagonal
    const up = angle === 0 || angle === 2;
    for (let step = 0; step < 4; step++) {
      ctx.fillRect(left + step, top + (up ? 3 - step : step), 1, 1);
    }
  }

  function dot(ctx, sx, sy, radius, color) {
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.arc(sx, sy, radius, 0, Math.PI * 2);
    ctx.fill();
  }

  function drawScenery(ctx, view, locs, objs, flora, labels, buffer) {
    const labelled = new Set();
    let labelCount = 0;
    const tile = view.tile;

    for (const loc of locs || []) {
      const [left, top] = tileOrigin(view, loc);
      if (left < -40 || top < -40 || left > view.w + 40 || top > view.h + 40) continue;

      if (isFlower(loc.name)) {
        if (!buffer) dot(ctx, left + tile / 2, top + tile / 2, 2, "#e78ac3");
        continue;
      }

      const shape = loc.shape;
      const angle = (loc.angle || 0) & 3;
      if ((shape >= 4 && shape <= 8)) {
        continue; // the client draws no wall decor
      }

      if (loc.mapscene >= 0) {
        drawSceneSprite(ctx, view, loc);
        continue;
      }

      let painted = false;
      if (shape === 0 || shape === 2) {
        ctx.fillStyle = loc.active ? WALL_ACTIVE : WALL_GREY;
        wallBar(ctx, left, top, angle);
        if (shape === 2) wallBar(ctx, left, top, (angle + 1) & 3);
        painted = true;
      } else if (shape === 3) {
        // square corner only: the client draws nothing for a diagonal corner
        ctx.fillStyle = loc.active ? WALL_ACTIVE : WALL_GREY;
        wallCorner(ctx, left, top, angle);
        painted = true;
      } else if (shape === 9) {
        ctx.fillStyle = loc.active ? WALL_ACTIVE : WALL_GREY;
        wallDiagonal(ctx, left, top, angle);
        painted = true;
      }

      if (loc.act && labelCount < 14 && Math.abs(loc.x - view.px) + Math.abs(loc.z - view.pz) <= 6 && !labelled.has(loc.name)) {
        labelled.add(loc.name);
        labelCount += 1;
        const sx = view.cx + (loc.x - view.px) * tile;
        const sy = view.cy - (loc.z - view.pz) * tile;
        labels.push([sx + 5, sy - 4, "#8d9", "9px monospace", String(loc.name || ("scenery #" + loc.id)).slice(0, 22)]);
      }
      void painted;
    }

    if (buffer) return; // items and flora are overlay dots, drawn on the canvas

    for (const weed of flora || []) {
      const sx = view.cx + (weed.x - view.px) * tile;
      const sy = view.cy - (weed.z - view.pz) * tile;
      if (sx < -20 || sy < -20 || sx > view.w + 20 || sy > view.h + 20) continue;
      dot(ctx, sx, sy, 2, "#e78ac3");
    }

    for (const obj of objs || []) {
      const sx = view.cx + (obj.x - view.px) * tile;
      const sy = view.cy - (obj.z - view.pz) * tile;
      if (sx < -20 || sy < -20 || sx > view.w + 20 || sy > view.h + 20) continue;
      if (isFlower(obj.name)) {
        dot(ctx, sx, sy, 2, "#e78ac3");
        continue;
      }
      dot(ctx, sx, sy, 3, "#ee0000");
      labels.push([sx + 6, sy + 4, "#ccc", "10px monospace", String(obj.name || ("object #" + obj.id)).slice(0, 30)]);
    }
  }

  window.SpectateScenery = { drawScenery };
})();
