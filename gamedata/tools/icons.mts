// Icon sheets of the dashboard (2026-11-24): every item icon and every
// skill icon in two PNG sprite sheets plus one index, written into
// orchestration/static/icons/ (served by the daemon under /icons/).
//
// Sources (rev 274, read-only):
//   - items: the game client's own inventory-icon renderer
//     (`ObjType.getSprite` of Client-TS, the client LostCity ships),
//     run headless on the server cache: obj configs from
//     `engine/data/pack/client/config`, textures from `.../textures`,
//     models from `engine/data/pack/main_file_cache` (index 1). Same
//     pixels as the in-game backpack (32x32, black outline + shadow,
//     colour table at brightness 0.6, the client's brightest setting).
//   - skills: the client's stat-tab sprites `staticons` / `staticons2`
//     of the media archive (`.../client/media`), 25x25.
//   - tabs (2026-11-24, spectator Inventory / Worn / Skills panels): the
//     side-panel tabs `inventory`, `wornitems` and `stats` of the
//     interface cache (`.../client/interface`, ids from
//     `content/pack/interface.pack`), drawn by a port of the client's
//     `drawInterface` with its own Pix2D / Pix32 / PixFont on the stone
//     background `invback`: what never changes is baked into one panel
//     image per tab; what the client fills at run time (inv slots, the
//     `%1` texts, the hover layers) is exported as layout for the front
//     (`gametabs.js`), with the glyphs of the client fonts p11/p12/b12
//     (`.../client/title`). Everything lands in `tabs.png`.
//   - `items.stack` (stackable ids: the client prints their count even
//     at 1) and `items.wear` (id -> server wearpos, from
//     gamedata/data/equipment.json + objs.json: worn slot of the frames
//     of an older worker that sends no `slot`).
// No icon is drawn by hand: an item the renderer cannot draw (no model,
// dummy) has no slot and shows an empty cell in the front.
//
// Regen, from the LifeScape root, Client-TS and lostcity checked out
// next to it (bun runs the client's TypeScript as is, no install):
//   bun gamedata/tools/icons.mts [--client ../Client-TS] [--lostcity ../lostcity]
// Deterministic: the same cache gives the same bytes.

import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import crypto from 'node:crypto';

function arg(name: string, fallback: string): string {
    const at = process.argv.indexOf('--' + name);
    return at >= 0 && process.argv[at + 1] ? process.argv[at + 1] : fallback;
}

const root = path.resolve(import.meta.dirname, '..', '..');
const clientDir = path.resolve(root, arg('client', '../Client-TS'));
const lostcityDir = path.resolve(root, arg('lostcity', '../lostcity'));
const packDir = path.join(lostcityDir, 'engine', 'data', 'pack');
const outDir = path.join(root, 'orchestration', 'static', 'icons');

// Jpeg.ts makes a canvas at module load: a stub is enough, no JPEG is
// decoded here (media sprites are indexed, not JPEG).
(globalThis as any).document = { createElement: () => ({ getContext: () => null }) };

const src = (file: string) => path.join(clientDir, 'src', file);
const { default: JagFile } = await import(src('io/JagFile.ts'));
const { default: ObjType } = await import(src('config/ObjType.ts'));
const { default: Model } = await import(src('dash3d/Model.ts'));
const { default: Pix3D } = await import(src('dash3d/Pix3D.ts'));
const { default: Pix32 } = await import(src('graphics/Pix32.ts'));
const { default: Pix8 } = await import(src('graphics/Pix8.ts'));
const { default: Pix2D } = await import(src('graphics/Pix2D.ts'));
const { default: PixFont } = await import(src('graphics/PixFont.ts'));
const { default: IfType } = await import(src('config/IfType.ts'));

// ---- cache: index 1 of main_file_cache (models), 520-byte sectors ----
function readCacheFiles(index: number): Map<number, Uint8Array> {
    const dat = fs.readFileSync(path.join(packDir, 'main_file_cache.dat'));
    const idx = fs.readFileSync(path.join(packDir, `main_file_cache.idx${index}`));
    const files = new Map<number, Uint8Array>();
    for (let file = 0; file * 6 + 6 <= idx.length; file++) {
        const size = (idx[file * 6] << 16) | (idx[file * 6 + 1] << 8) | idx[file * 6 + 2];
        let sector = (idx[file * 6 + 3] << 16) | (idx[file * 6 + 4] << 8) | idx[file * 6 + 5];
        if (size <= 0 || sector <= 0) continue;
        const out = new Uint8Array(size);
        let done = 0;
        let part = 0;
        while (done < size) {
            const at = sector * 520;
            const owner = (dat[at] << 8) | dat[at + 1];
            const chunk = (dat[at + 2] << 8) | dat[at + 3];
            const next = (dat[at + 4] << 16) | (dat[at + 5] << 8) | dat[at + 6];
            const store = dat[at + 7];
            if (owner !== file || chunk !== part || store !== index + 1) throw new Error(`cache idx${index} file ${file}: bad sector ${sector}`);
            const take = Math.min(512, size - done);
            out.set(dat.subarray(at + 8, at + 8 + take), done);
            done += take;
            part += 1;
            sector = next;
        }
        files.set(file, out);
    }
    return files;
}

function gunzipModel(data: Uint8Array): Uint8Array {
    // Stored files end with a 2-byte version trailer (the client strips it).
    try {
        return new Uint8Array(zlib.gunzipSync(data.subarray(0, data.length - 2)));
    } catch {
        return new Uint8Array(zlib.gunzipSync(data));
    }
}

// ---- PNG writer (RGBA), deterministic. Filter "none" on every row:
// on this sheet (transparent cells, black outlines) it deflates smaller
// than the adaptive per-row filters (978 KB vs 1.31 MB, measured). ----
const CRC_TABLE = (() => {
    const table = new Uint32Array(256);
    for (let n = 0; n < 256; n++) {
        let c = n;
        for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
        table[n] = c >>> 0;
    }
    return table;
})();

function crc32(bytes: Uint8Array): number {
    let c = 0xffffffff;
    for (const byte of bytes) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
    return (c ^ 0xffffffff) >>> 0;
}

function chunk(type: string, data: Uint8Array): Buffer {
    const head = Buffer.alloc(8);
    head.writeUInt32BE(data.length, 0);
    head.write(type, 4, 'ascii');
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(Buffer.concat([head.subarray(4), data])), 0);
    return Buffer.concat([head, data, crc]);
}

function png(width: number, height: number, rgba: Uint8Array): Buffer {
    const stride = width * 4;
    const raw = Buffer.alloc((stride + 1) * height);
    for (let y = 0; y < height; y++) {
        raw[y * (stride + 1)] = 0;
        raw.set(rgba.subarray(y * stride, (y + 1) * stride), y * (stride + 1) + 1);
    }
    const ihdr = Buffer.alloc(13);
    ihdr.writeUInt32BE(width, 0);
    ihdr.writeUInt32BE(height, 4);
    ihdr[8] = 8; // bit depth
    ihdr[9] = 6; // RGBA
    return Buffer.concat([
        Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
        chunk('IHDR', ihdr),
        chunk('IDAT', zlib.deflateSync(raw, { level: 9 })),
        chunk('IEND', new Uint8Array(0)),
    ]);
}

// ---- sheet: cell 0 stays empty (the front's "no icon" position) ----
type Cell = Int32Array; // cell*cell client pixels, 0 = transparent

function sheet(cells: Cell[], size: number): { cols: number; rows: number; bytes: Buffer } {
    const count = cells.length + 1;
    const cols = Math.ceil(Math.sqrt(count));
    const rows = Math.ceil(count / cols);
    const width = cols * size;
    const rgba = new Uint8Array(width * rows * size * 4);
    cells.forEach((pixels, at) => {
        const slot = at + 1;
        const x0 = (slot % cols) * size;
        const y0 = Math.floor(slot / cols) * size;
        for (let y = 0; y < size; y++) {
            for (let x = 0; x < size; x++) {
                const rgb = pixels[x + y * size];
                if (rgb === 0) continue;
                const o = ((y0 + y) * width + x0 + x) * 4;
                rgba[o] = (rgb >> 16) & 0xff;
                rgba[o + 1] = (rgb >> 8) & 0xff;
                rgba[o + 2] = rgb & 0xff;
                rgba[o + 3] = 0xff;
            }
        }
    });
    return { cols, rows, bytes: png(width, rows * size, rgba) };
}

// Identical icons (same pixels) share one slot.
function addCell(cells: Cell[], seen: Map<string, number>, pixels: Cell): number {
    const key = crypto.createHash('sha1').update(new Uint8Array(pixels.buffer, pixels.byteOffset, pixels.byteLength)).digest('hex');
    const known = seen.get(key);
    if (known !== undefined) return known;
    cells.push(pixels);
    seen.set(key, cells.length);
    return cells.length;
}

const hash = (bytes: Buffer) => crypto.createHash('sha256').update(bytes).digest('hex').slice(0, 16);

// ---- items ----
const config = new JagFile(new Uint8Array(fs.readFileSync(path.join(packDir, 'client', 'config'))));
const textures = new JagFile(new Uint8Array(fs.readFileSync(path.join(packDir, 'client', 'textures'))));
const media = new JagFile(new Uint8Array(fs.readFileSync(path.join(packDir, 'client', 'media'))));

Pix3D.unpackTextures(textures);
// The client jitters the brightness by ±0.015 at random on every start:
// pinned to the middle (exactly 0.6, the brightest of the 4 settings:
// 0.9 / 0.8 default / 0.7 / 0.6) so the sheet is reproducible.
const random = Math.random;
Math.random = () => 0.5;
Pix3D.initColourTable(0.6);
Math.random = random;
Pix3D.initPool(20);
ObjType.init(config, true);

const models = readCacheFiles(1);
const modelCount = Math.max(...models.keys()) + 1;
Model.init(modelCount, { requestModel: () => {} });
for (let id = 0; id < modelCount; id++) {
    const data = models.get(id);
    Model.unpack(id, data ? gunzipModel(data) : null);
}

const itemCells: Cell[] = [];
const itemSeen = new Map<string, number>();
const itemSlot: number[] = new Array(ObjType.numDefinitions).fill(0);
let named = 0;
const missing: string[] = [];
for (let id = 0; id < ObjType.numDefinitions; id++) {
    const obj = ObjType.list(id);
    const hasName = obj.name !== null && obj.name !== 'null' && obj.name !== '';
    if (hasName) named += 1;
    let icon = null;
    try {
        icon = ObjType.getSprite(id, 1, 0);
    } catch {
        icon = null;
    }
    const drawn = icon && icon.data.some((rgb: number) => rgb > 1);
    if (!drawn) {
        if (hasName) missing.push(`${id} ${obj.name}`);
        continue;
    }
    itemSlot[id] = addCell(itemCells, itemSeen, Int32Array.from(icon.data));
}

// ---- stack sizes: a stack shows another obj's model (coins pile up,
// arrows bundle) once its count reaches a `countco` step, exactly like
// the backpack (`getSprite(id, count)` picks the last step <= count).
// The front only knows these steps (icons.js COUNT_STEPS): every step of
// the cache must be one of them, or the regen stops here.
const COUNT_STEPS = [2, 3, 4, 5, 25, 100, 250, 1000, 10000];
const itemCount: Record<string, Record<string, number>> = {};
for (let id = 0; id < ObjType.numDefinitions; id++) {
    const obj = ObjType.list(id);
    if (!obj.countobj || !obj.countco || !itemSlot[id]) continue;
    const steps: Record<string, number> = {};
    for (const step of COUNT_STEPS) {
        let variant = -1;
        for (let i = 0; i < 10; i++) {
            const co = obj.countco[i];
            if (co === 0) continue;
            if (!COUNT_STEPS.includes(co)) throw new Error(`obj ${id}: count step ${co} unknown to icons.js COUNT_STEPS`);
            if (step >= co) variant = obj.countobj[i];
        }
        const slot = variant === -1 ? 0 : itemSlot[variant] ?? 0;
        if (slot && slot !== itemSlot[id]) steps[String(step)] = slot;
    }
    if (Object.keys(steps).length) itemCount[String(id)] = steps;
}

// ---- skills: stat-tab sprites, at their offset in the 25x25 frame ----
const SKILL_SPRITES: [string, string, number][] = [
    ['attack', 'staticons', 0], ['strength', 'staticons', 1], ['defence', 'staticons', 2],
    ['ranged', 'staticons', 3], ['prayer', 'staticons', 4], ['magic', 'staticons', 5],
    ['hitpoints', 'staticons', 6], ['agility', 'staticons', 7], ['herblore', 'staticons', 8],
    ['thieving', 'staticons', 9], ['crafting', 'staticons', 10], ['fletching', 'staticons', 11],
    ['mining', 'staticons', 12], ['smithing', 'staticons', 13], ['fishing', 'staticons', 14],
    ['cooking', 'staticons', 15], ['firemaking', 'staticons', 16], ['woodcutting', 'staticons', 17],
    ['runecraft', 'staticons2', 0],
];
const SKILL_SIZE = 25;
const skillCells: Cell[] = [];
const skillSeen = new Map<string, number>();
const skillSlot: Record<string, number> = {};
for (const [skill, name, index] of SKILL_SPRITES) {
    const sprite = Pix32.depack(media, name, index);
    if (sprite.owi !== SKILL_SIZE || sprite.ohi !== SKILL_SIZE) throw new Error(`${name},${index}: ${sprite.owi}x${sprite.ohi}, expected 25x25`);
    const cell = new Int32Array(SKILL_SIZE * SKILL_SIZE);
    for (let y = 0; y < sprite.hi; y++) {
        for (let x = 0; x < sprite.wi; x++) {
            cell[x + sprite.xof + (y + sprite.yof) * SKILL_SIZE] = sprite.data[x + y * sprite.wi];
        }
    }
    skillSlot[skill] = addCell(skillCells, skillSeen, cell);
}

// ---- stackable ids and worn slots ----
const itemStack: number[] = [];
for (let id = 0; id < ObjType.numDefinitions; id++) if (ObjType.list(id).stackable) itemStack.push(id);
const objRows: { name: string; display: string }[] = JSON.parse(fs.readFileSync(path.join(root, 'gamedata', 'data', 'objs.json'), 'utf8'));
const wearRows: Record<string, { wearpos: number }> = JSON.parse(fs.readFileSync(path.join(root, 'gamedata', 'data', 'equipment.json'), 'utf8'));
const itemWear: Record<string, number> = {};
objRows.forEach((row, id) => {
    const wear = wearRows[row.name];
    if (!wear || wear.wearpos < 0) return;
    // objs.json is in obj id order: the client's name must agree.
    if (ObjType.list(id).name !== row.display) throw new Error(`objs.json row ${id} (${row.name}) is not the client's obj ${id} (${ObjType.list(id).name})`);
    itemWear[String(id)] = wear.wearpos;
});

// ---- tabs: inventory, wornitems, stats ----
const title = new JagFile(new Uint8Array(fs.readFileSync(path.join(packDir, 'client', 'title'))));
const fonts: Record<string, any> = {
    p11: PixFont.depack(title, 'p11_full', false),
    p12: PixFont.depack(title, 'p12_full', false),
    b12: PixFont.depack(title, 'b12_full', false),
    q8: PixFont.depack(title, 'q8_full', true),
};
const fontName = new Map<any, string>(Object.entries(fonts).map(([name, font]) => [font, name]));
IfType.init(new JagFile(new Uint8Array(fs.readFileSync(path.join(packDir, 'client', 'interface')))), media, [fonts.p11, fonts.p12, fonts.b12, fonts.q8]);
const interfaceIds = new Map<string, number>();
for (const line of fs.readFileSync(path.join(lostcityDir, 'content', 'pack', 'interface.pack'), 'utf8').split('\n')) {
    const at = line.indexOf('=');
    if (at > 0) interfaceIds.set(line.slice(at + 1).trim(), Number(line.slice(0, at)));
}
const invback = Pix8.depack(media, 'invback', 0);
const SIDE_W = invback.owi;
const SIDE_H = invback.ohi;
// The worn tab's bonus box (layer com_23 and its frame, y >= 200) shows
// texts the server writes when the tab opens: the snapshot carries no
// bonuses, so that part is left out (the panel stops above it).
const TAB_HEIGHT: Record<string, number> = { inventory: SIDE_H, wornitems: 200, stats: SIDE_H };
// Pixels never drawn (hover layers, slot sprites, glyphs): transparent.
const CLEAR = 0x1000000;

type Img = { w: number; h: number; px: Int32Array };
const tabImages = new Map<string, Img>();
function canvasOf(w: number, h: number, fill: number): Img {
    const px = new Int32Array(w * h).fill(fill);
    Pix2D.setPixels(px, w, h);
    return { w, h, px };
}

// One `%1` text of the client (`getIfVar` of its first script): a single
// op, decoded here, evaluated by the front on the snapshot.
const VAR_OPS: Record<number, [string, boolean]> = {
    1: ['stat_level', true], 2: ['stat_base_level', true], 3: ['stat_xp', true], 5: ['pushvar', true],
    6: ['stat_xp_remaining', true], 8: ['combat_level', false], 9: ['total_level', false],
};
const varpNames = new Map<number, string>();
for (const line of fs.readFileSync(path.join(lostcityDir, 'content', 'pack', 'varp.pack'), 'utf8').split('\n')) {
    const at = line.indexOf('=');
    if (at > 0) varpNames.set(Number(line.slice(0, at)), line.slice(at + 1).trim());
}
function textVar(com: any): { op: string; arg?: number; varp?: string } {
    const script = com.scripts && com.scripts[0];
    if (!script) throw new Error(`com ${com.id}: %1 without a script`);
    const known = VAR_OPS[script[0]];
    if (!known) throw new Error(`com ${com.id}: script op ${script[0]} unknown to gametabs.js`);
    const end = known[1] ? 2 : 1;
    if (script[end] !== 0) throw new Error(`com ${com.id}: more than one script op`);
    if (known[0] === 'pushvar') return { op: known[0], arg: script[1], varp: varpNames.get(script[1]) };
    return known[1] ? { op: known[0], arg: script[1] } : { op: known[0] };
}

type TabLayout = {
    texts: any[];
    hovers: { x: number; y: number; w: number; h: number; layer: number }[];
    hidden: { com: any; x: number; y: number }[];
    inv: any;
};

// Port of Client.drawInterface for what a side tab holds (layer, inv,
// rect, text, graphic), nothing hovered or active: static parts drawn
// with the client's own primitives, run-time parts recorded.
function drawStatic(com: any, x: number, y: number, out: TabLayout): void {
    if (com.type !== 0 || !com.children || !com.childX || !com.childY) return;
    const left = Pix2D.clipMinX, top = Pix2D.clipMinY, right = Pix2D.clipMaxX, bottom = Pix2D.clipMaxY;
    Pix2D.setClipping(x, y, x + com.width, y + com.height);
    for (let i = 0; i < com.children.length; i++) {
        const child = IfType.list[com.children[i]];
        const childX = com.childX[i] + x + child.x;
        const childY = com.childY[i] + y + child.y;
        if (child.clientCode > 0) throw new Error(`com ${child.id}: client code ${child.clientCode}`);
        if (child.overLayerId >= 0) out.hovers.push({ x: childX, y: childY, w: child.width, h: child.height, layer: child.overLayerId });
        if (child.type === 0) {
            if (child.hide) out.hidden.push({ com: child, x: childX, y: childY });
            else drawStatic(child, childX, childY, out);
        } else if (child.type === 2) {
            if (out.inv) throw new Error(`com ${child.id}: a second inv`);
            const off: number[][] = [];
            for (let slot = 0; slot < 20; slot++) off.push([child.invBackgroundX[slot], child.invBackgroundY[slot]]);
            out.inv = { com: child, x: childX, y: childY, cols: child.width, rows: child.height, mx: child.marginX, my: child.marginY, off };
        } else if (child.type === 3) {
            if (child.trans === 0) {
                if (child.fill) Pix2D.fillRect(childX, childY, child.width, child.height, child.colour);
                else Pix2D.drawRect(childX, childY, child.width, child.height, child.colour);
            } else if (child.fill) {
                Pix2D.fillRectTrans(childX, childY, child.width, child.height, child.colour, 256 - (child.trans & 0xff));
            } else {
                Pix2D.drawRect(childX, childY, child.width, child.height, child.colour);
                Pix2D.drawRectTrans(childX, childY, child.width, child.height, child.colour, 256 - (child.trans & 0xff));
            }
        } else if (child.type === 4) {
            const font = child.font;
            const text: string | null = child.text;
            if (!font || !text) continue;
            if (text.includes('\\n')) throw new Error(`com ${child.id}: multi-line text`);
            const baseX = child.centre ? childX + ((child.width / 2) | 0) : childX;
            const baseY = childY + font.height;
            if (text.includes('%')) {
                out.texts.push({ font: fontName.get(font), x: baseX, y: baseY, centre: child.centre, shadow: child.shadow, colour: child.colour, text, ...textVar(child) });
            } else if (child.centre) {
                font.centreStringTag(text, baseX, baseY, child.colour, child.shadow);
            } else {
                font.drawStringTag(text, baseX, baseY, child.colour, child.shadow);
            }
        } else if (child.type === 5) {
            child.graphic?.plotSprite(childX, childY);
        } else {
            throw new Error(`com ${child.id}: type ${child.type} unknown to the tab port`);
        }
    }
    Pix2D.setClipping(left, top, right, bottom);
}

const tabs: Record<string, any> = {};
for (const tab of ['inventory', 'wornitems', 'stats']) {
    const id = interfaceIds.get(tab);
    if (id === undefined || !IfType.list[id]) throw new Error(`interface ${tab} not in the cache`);
    const height = TAB_HEIGHT[tab];
    const panel = canvasOf(SIDE_W, SIDE_H, 0);
    invback.plotSprite(0, 0);
    const out: TabLayout = { texts: [], hovers: [], hidden: [], inv: null };
    drawStatic(IfType.list[id], 0, 0, out);
    tabImages.set(`${tab}`, { w: SIDE_W, h: height, px: panel.px.slice(0, SIDE_W * height) });
    const layers: Record<string, any> = {};
    for (const hidden of out.hidden) {
        const sub: TabLayout = { texts: [], hovers: [], hidden: [], inv: null };
        canvasOf(hidden.com.width, hidden.com.height, CLEAR);
        drawStatic(hidden.com, 0, 0, sub);
        if (sub.hidden.length || sub.inv || sub.hovers.length) throw new Error(`layer ${hidden.com.id}: nested run-time parts`);
        const img = Pix2D.pixels;
        tabImages.set(`${tab}:layer${hidden.com.id}`, { w: hidden.com.width, h: hidden.com.height, px: img });
        layers[String(hidden.com.id)] = { x: hidden.x, y: hidden.y, image: `${tab}:layer${hidden.com.id}`, texts: sub.texts };
    }
    for (const hover of out.hovers) if (!layers[String(hover.layer)]) throw new Error(`${tab}: hover layer ${hover.layer} is not a hidden layer of the tab`);
    let inv = null;
    if (out.inv) {
        const empty: (string | null)[] = [];
        for (let slot = 0; slot < 20; slot++) {
            const sprite = out.inv.com.invBackground[slot];
            if (!sprite) {
                empty.push(null);
                continue;
            }
            canvasOf(sprite.owi, sprite.ohi, CLEAR);
            sprite.plotSprite(0, 0);
            tabImages.set(`${tab}:slot${slot}`, { w: sprite.owi, h: sprite.ohi, px: Pix2D.pixels });
            empty.push(`${tab}:slot${slot}`);
        }
        inv = { x: out.inv.x, y: out.inv.y, cols: out.inv.cols, rows: out.inv.rows, mx: out.inv.mx, my: out.inv.my, off: out.inv.off, empty };
    }
    tabs[tab] = { w: SIDE_W, h: height, image: tab, inv, texts: out.texts, hovers: out.hovers.map(({ x, y, w, h, layer }) => ({ x, y, w, h, layer: String(layer) })), layers };
}

// Glyphs of the fonts the front draws with (printable ASCII; item names
// and numbers use nothing else): mask cells, white where the client plots.
const tabFonts: Record<string, any> = {};
for (const name of ['p11', 'p12', 'b12']) {
    const font = fonts[name];
    const chars: Record<string, any> = {};
    for (let c = 32; c < 127; c++) {
        const w = font.charMaskWidth[c], h = font.charMaskHeight[c];
        let image = null;
        if (c !== 32 && w > 0 && h > 0) {
            const px = new Int32Array(w * h).fill(CLEAR);
            for (let i = 0; i < w * h; i++) if (font.charMask[c][i] !== 0) px[i] = 0xffffff;
            image = `${name}:${c}`;
            tabImages.set(image, { w, h, px });
        }
        chars[String(c)] = [image, font.charOffsetX[c], font.charOffsetY[c], font.charAdvance[c]];
    }
    tabFonts[name] = { height: font.height, chars };
}

// Shelf packing, tallest first; ties by name (deterministic).
const TAB_SHEET_W = 3 * SIDE_W + 2;
const packed: Record<string, number[]> = {};
let shelfX = 0, shelfY = 0, shelfH = 0;
for (const [name, img] of [...tabImages.entries()].sort((a, b) => b[1].h - a[1].h || (a[0] < b[0] ? -1 : 1))) {
    if (shelfX + img.w > TAB_SHEET_W) {
        shelfY += shelfH;
        shelfX = 0;
        shelfH = 0;
    }
    packed[name] = [shelfX, shelfY, img.w, img.h];
    shelfX += img.w + 1;
    shelfH = Math.max(shelfH, img.h + 1);
}
const tabSheetH = shelfY + shelfH;
const tabRgba = new Uint8Array(TAB_SHEET_W * tabSheetH * 4);
for (const [name, img] of tabImages) {
    const [x0, y0] = packed[name];
    for (let y = 0; y < img.h; y++) {
        for (let x = 0; x < img.w; x++) {
            const rgb = img.px[x + y * img.w];
            if (rgb === CLEAR) continue;
            const o = ((y0 + y) * TAB_SHEET_W + x0 + x) * 4;
            tabRgba[o] = (rgb >> 16) & 0xff;
            tabRgba[o + 1] = (rgb >> 8) & 0xff;
            tabRgba[o + 2] = rgb & 0xff;
            tabRgba[o + 3] = 0xff;
        }
    }
}
const tabBytes = png(TAB_SHEET_W, tabSheetH, tabRgba);

// ---- write ----
const items = sheet(itemCells, 32);
const skills = sheet(skillCells, SKILL_SIZE);
fs.mkdirSync(outDir, { recursive: true });
fs.writeFileSync(path.join(outDir, 'items.png'), items.bytes);
fs.writeFileSync(path.join(outDir, 'skills.png'), skills.bytes);
fs.writeFileSync(path.join(outDir, 'tabs.png'), tabBytes);
const index = {
    source: 'rev 274 cache: ObjType.getSprite (Client-TS) for items, media staticons/staticons2 for skills, interface inventory/wornitems/stats + invback + fonts for tabs; gamedata/tools/icons.mts',
    items: { file: 'items.png', hash: hash(items.bytes), cell: 32, cols: items.cols, slot: itemSlot, count: itemCount, stack: itemStack, wear: itemWear },
    skills: { file: 'skills.png', hash: hash(skills.bytes), cell: SKILL_SIZE, cols: skills.cols, slot: skillSlot },
    tabs: { file: 'tabs.png', hash: hash(tabBytes), images: packed, fonts: tabFonts, tabs },
};
fs.writeFileSync(path.join(outDir, 'index.json'), JSON.stringify(index) + '\n');

const drawnItems = itemSlot.filter((slot) => slot > 0).length;
console.log(`items: ${ObjType.numDefinitions} obj ids, ${named} named, ${drawnItems} with an icon (${itemCells.length} distinct), ${missing.length} named without icon; sheet ${items.cols}x${items.rows} cells, ${items.bytes.length} bytes`);
console.log(`stacks: ${Object.keys(itemCount).length} items change icon with their count`);
console.log(`skills: ${Object.keys(skillSlot).length} icons; sheet ${skills.cols}x${skills.rows} cells, ${skills.bytes.length} bytes`);
console.log(`tabs: ${Object.keys(tabs).join(', ')}; ${tabImages.size} images, sheet ${TAB_SHEET_W}x${tabSheetH}, ${tabBytes.length} bytes`);
if (missing.length) console.log('named without icon: ' + missing.join(', '));
