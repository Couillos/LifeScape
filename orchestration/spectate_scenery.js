// Scenery painter for the LifeScape spectator, drawn the way the game
// minimap draws it (Client.drawDetail, Client-TS branch 274):
//   - walls are one thin bar on the tile EDGE their angle faces
//     (0=W 1=N 2=E 3=S), grey for plain walls and red for active ones
//     (doors), never a filled block;
//   - a square corner is a single corner pixel, a diagonal wall is a
//     diagonal line;
//   - other scenery is a small marker, ground items are red dots with
//     their name, flowers are dots without a name.
(() => {
  const EDGE = 0.25; // the client uses 1 of its 4 px per tile
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

  function drawSceneSprite(ctx, view, loc, sx, sy) {
    const image = sceneSprite(loc.mapscene);
    if (!image.complete || !image.naturalWidth) {
      return false; // still loading: the ground shows through meanwhile
    }
    const scale = view.tile / 4; // the client minimap is 4 px per tile
    const width = loc.width || 1;
    const length = loc.length || 1;
    const offsetX = ((width * 4 - image.naturalWidth) / 2) | 0;
    const offsetY = ((length * 4 - image.naturalHeight) / 2) | 0;
    const left = sx - view.tile / 2 + offsetX * scale;
    const top = sy - view.tile / 2 + ((1 - length) * 4 + offsetY) * scale;
    ctx.drawImage(image, left, top, image.naturalWidth * scale, image.naturalHeight * scale);
    return true;
  }

  function isFlower(name) {
    return /flower/i.test(name || "");
  }

  function wallBar(ctx, left, top, size, angle) {
    const thick = Math.max(2, Math.round(size * EDGE));
    if (angle === 0) ctx.fillRect(left, top, thick, size);            // west edge
    else if (angle === 1) ctx.fillRect(left, top, size, thick);       // north edge
    else if (angle === 2) ctx.fillRect(left + size - thick, top, thick, size); // east
    else ctx.fillRect(left, top + size - thick, size, thick);         // south edge
  }

  function wallCorner(ctx, left, top, size, angle) {
    const thick = Math.max(2, Math.round(size * EDGE));
    const x = angle === 0 || angle === 3 ? left : left + size - thick;
    const y = angle === 0 || angle === 1 ? top : top + size - thick;
    ctx.fillRect(x, y, thick, thick);
  }

  function wallDiagonal(ctx, left, top, size, angle) {
    const thick = Math.max(2, Math.round(size * EDGE));
    ctx.save();
    ctx.translate(left, top);
    if (angle === 0 || angle === 2) {
      // bottom-left to top-right
      ctx.fillRect(0, size - thick, thick, thick);
      ctx.fillRect((size - thick) / 2, (size - thick) / 2, thick, thick);
      ctx.fillRect(size - thick, 0, thick, thick);
    } else {
      ctx.fillRect(0, 0, thick, thick);
      ctx.fillRect((size - thick) / 2, (size - thick) / 2, thick, thick);
      ctx.fillRect(size - thick, size - thick, thick, thick);
    }
    ctx.restore();
  }

  function dot(ctx, sx, sy, radius, color) {
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.arc(sx, sy, radius, 0, Math.PI * 2);
    ctx.fill();
  }

  function drawScenery(ctx, view, locs, objs, flora, labels) {
    const size = view.tile;
    const half = view.half || size / 2; // never NaN: NaN coords paint nothing
    const labelled = new Set();
    let labelCount = 0;

    for (const loc of locs || []) {
      const sx = view.cx + (loc.x - view.px) * size;
      const sy = view.cy - (loc.z - view.pz) * size;
      if (sx < -40 || sy < -40 || sx > view.w + 40 || sy > view.h + 40) continue;
      const left = sx - half;
      const top = sy - half;

      if (isFlower(loc.name)) {
        dot(ctx, sx, sy, size * 0.16, "#e78ac3"); // overlaid on the ground, no name
        continue;
      }

      const shape = loc.shape;
      const angle = (loc.angle || 0) & 3;

      // the client draws the loc's mapscene sprite instead of a wall or a
      // marker when it has one (trees, banks, castle floors...)
      if (loc.mapscene >= 0 && drawSceneSprite(ctx, view, loc, sx, sy)) {
        if (loc.act && labelCount < 14 && Math.abs(loc.x - view.px) + Math.abs(loc.z - view.pz) <= 6 && !labelled.has(loc.name)) {
          labelled.add(loc.name);
          labelCount += 1;
          labels.push([sx + 6, sy + 4, "#8d9", "9px monospace", String(loc.name || ("scenery #" + loc.id)).slice(0, 22)]);
        }
        continue;
      }

      if (shape === 0 || shape === 2) {
        ctx.fillStyle = loc.act ? WALL_ACTIVE : WALL_GREY;
        wallBar(ctx, left, top, size, angle);
        continue;
      }
      if (shape === 3) {
        ctx.fillStyle = loc.act ? WALL_ACTIVE : WALL_GREY;
        wallCorner(ctx, left, top, size, angle);
        continue;
      }
      if (shape === 9) {
        ctx.fillStyle = loc.act ? WALL_ACTIVE : WALL_GREY;
        wallDiagonal(ctx, left, top, size, angle);
        continue;
      }

      if (loc.block) {
        // building mass: the minimap draws these as a lit stone body
        const wide = angle === 1 || angle === 3;
        const sizeX = (wide ? (loc.length || 1) : (loc.width || 1)) * size;
        const sizeZ = (wide ? (loc.width || 1) : (loc.length || 1)) * size;
        ctx.fillStyle = "rgba(150,150,150,0.55)";
        ctx.fillRect(left, top - sizeZ + size, sizeX, sizeZ);
      } else if (loc.act) {
        ctx.fillStyle = "#c8a15a";
        ctx.fillRect(sx - size * 0.22, sy - size * 0.22, size * 0.44, size * 0.44);
      } else {
        ctx.fillStyle = "#2f6b3c";
        ctx.fillRect(sx - size * 0.18, sy - size * 0.18, size * 0.36, size * 0.36);
      }

      if (loc.act && labelCount < 14 && Math.abs(loc.x - view.px) + Math.abs(loc.z - view.pz) <= 6 && !labelled.has(loc.name)) {
        labelled.add(loc.name);
        labelCount += 1;
        labels.push([sx + 5, sy - 4, "#8d9", "9px monospace", String(loc.name || ("scenery #" + loc.id)).slice(0, 22)]);
      }
    }

    for (const weed of flora || []) {
      const sx = view.cx + (weed.x - view.px) * size;
      const sy = view.cy - (weed.z - view.pz) * size;
      if (sx < -20 || sy < -20 || sx > view.w + 20 || sy > view.h + 20) continue;
      dot(ctx, sx, sy, size * 0.16, "#e78ac3");
    }

    for (const obj of objs || []) {
      const sx = view.cx + (obj.x - view.px) * size;
      const sy = view.cy - (obj.z - view.pz) * size;
      if (sx < -20 || sy < -20 || sx > view.w + 20 || sy > view.h + 20) continue;
      if (isFlower(obj.name)) {
        dot(ctx, sx, sy, size * 0.16, "#e78ac3");
        continue;
      }
      dot(ctx, sx, sy, size * 0.2, "#ee0000");
      labels.push([sx + 6, sy + 4, "#ccc", "10px monospace", String(obj.name || ("object #" + obj.id)).slice(0, 30)]);
    }
  }

  window.SpectateScenery = { drawScenery };
})();
