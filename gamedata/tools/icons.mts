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

// ---- write ----
const items = sheet(itemCells, 32);
const skills = sheet(skillCells, SKILL_SIZE);
fs.mkdirSync(outDir, { recursive: true });
fs.writeFileSync(path.join(outDir, 'items.png'), items.bytes);
fs.writeFileSync(path.join(outDir, 'skills.png'), skills.bytes);
const index = {
    source: 'rev 274 cache: ObjType.getSprite (Client-TS) for items, media staticons/staticons2 for skills; gamedata/tools/icons.mts',
    items: { file: 'items.png', hash: hash(items.bytes), cell: 32, cols: items.cols, slot: itemSlot, count: itemCount },
    skills: { file: 'skills.png', hash: hash(skills.bytes), cell: SKILL_SIZE, cols: skills.cols, slot: skillSlot },
};
fs.writeFileSync(path.join(outDir, 'index.json'), JSON.stringify(index) + '\n');

const drawnItems = itemSlot.filter((slot) => slot > 0).length;
console.log(`items: ${ObjType.numDefinitions} obj ids, ${named} named, ${drawnItems} with an icon (${itemCells.length} distinct), ${missing.length} named without icon; sheet ${items.cols}x${items.rows} cells, ${items.bytes.length} bytes`);
console.log(`stacks: ${Object.keys(itemCount).length} items change icon with their count`);
console.log(`skills: ${Object.keys(skillSlot).length} icons; sheet ${skills.cols}x${skills.rows} cells, ${skills.bytes.length} bytes`);
if (missing.length) console.log('named without icon: ' + missing.join(', '));
