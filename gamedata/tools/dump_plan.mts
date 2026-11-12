// Planning-data dump for LifeScape gamedata.
//
// Adds what the upstream dump does not carry and the planner needs:
//   1. `tradeable` on every row of gamedata/data/objs.json (raw cache flag:
//      default true, false for code-15 untradeables, dummy items and notes
//      inheriting their link; read in members mode so the F2P world override
//      does not clobber members items).
//   2. gamedata/data/drops.json: static NPC death drops scraped from the
//      COMPILED drop-table scripts (exact server truth, not regex on source):
//      every `obj_add(npc_coord, <const obj>, ...)` inside `[ai_queue3,*]`
//      handlers of the drop-tables dir, following same-file label jumps,
//      expanding category subjects (`_chicken`) to member NPCs and shared
//      procs (`~randomherb`) to their constant return sets.
//
// Regen (rev 274), from the engine dir so `data/pack` and `world.json`
// resolve, with engine deps installed (same requirement as gamedata-dump):
//   cd ../lostcity/engine && npx tsx ../../LifeScape/gamedata/tools/dump_plan.mts
//
// Read-only against LostCity: cache + compiled scripts + objs.json in,
// objs.json + drops.json out. Never hand-edit the outputs.

import fs from 'node:fs';
import path from 'node:path';

import Environment from '../../../lostcity/engine/src/util/Environment.js';
import ObjType from '../../../lostcity/engine/src/cache/config/ObjType.js';
import NpcType from '../../../lostcity/engine/src/cache/config/NpcType.js';
import ParamType from '../../../lostcity/engine/src/cache/config/ParamType.js';
import ScriptProvider from '../../../lostcity/engine/src/engine/script/ScriptProvider.js';
import { ScriptOpcode } from '../../../lostcity/engine/src/engine/script/ScriptOpcode.js';
import { handlerRates, handlerShops, indexBlocks, shopPaths } from './drop_rates.mts';
import CategoryType from '../../../lostcity/engine/src/cache/config/CategoryType.js';
import InvType from '../../../lostcity/engine/src/cache/config/InvType.js';

const AI_QUEUE3 = 119;

const lifescapeRoot = path.resolve(process.argv[2] ?? '../LifeScape');
const dataDir = path.join(lifescapeRoot, 'gamedata', 'data');

// Raw cache flags: the parse forces members items untradeable in F2P mode,
// so read in members mode. Only ObjType consults this flag.
Environment.node.members = true;
ObjType.load('data/pack');
NpcType.load('data/pack');
ScriptProvider.load('data/pack');

const scripts = (ScriptProvider as unknown as { scripts: Array<any> }).scripts;
const byId = new Map<number, any>();
const byName = new Map<string, any>();
for (const script of scripts) {
    if (!script) {
        continue;
    }
    byId.set(script.id, script);
    byName.set(script.name, script);
}

// ---- 1. tradeable enrichment of objs.json ----

const objsPath = path.join(dataDir, 'objs.json');
const objs = JSON.parse(fs.readFileSync(objsPath, 'utf8'));
if (!Array.isArray(objs) || objs.length !== ObjType.configs.length) {
    throw new Error(`objs.json has ${objs.length} rows, cache has ${ObjType.configs.length}: regenerate it with gamedata-dump first`);
}
let untradeable = 0;
for (let id = 0; id < objs.length; id++) {
    const row = objs[id];
    if (row === null || row === undefined) {
        continue;
    }
    const flag = ObjType.get(id).tradeable;
    row.tradeable = flag;
    if (!flag) {
        untradeable++;
    }
}
fs.writeFileSync(objsPath, JSON.stringify(objs));
console.log(`objs: ${objs.length} rows, ${untradeable} untradeable`);

// ---- 2. static NPC drops from compiled scripts ----

// Constant return sets of shared drop procs: `[PUSH obj, PUSH n, RETURN]`.
const procReturns = new Map<number, Set<number>>();
function returnsOf(proc: any): Set<number> {
    const cached = procReturns.get(proc.id);
    if (cached) {
        return cached;
    }
    const found = new Set<number>();
    procReturns.set(proc.id, found);
    const { opcodes, intOperands } = proc;
    for (let i = 2; i < opcodes.length; i++) {
        if (opcodes[i] !== ScriptOpcode.RETURN || opcodes[i - 1] !== ScriptOpcode.PUSH_CONSTANT_INT || opcodes[i - 2] !== ScriptOpcode.PUSH_CONSTANT_INT) {
            continue;
        }
        const obj = intOperands[i - 2];
        if (Number.isInteger(obj) && obj >= 0 && obj < ObjType.configs.length) {
            found.add(obj);
        }
    }
    return found;
}

const DROP_DIR = 'drop tables';
const pairs = new Map<number, Set<number>>();
const unknownSubjects: string[] = [];
let procHits = 0;
let constHits = 0;
const subjectNpcs = new Map<string, number[]>();
const otherHandlers = new Set<number>();

for (const script of scripts) {
    if (!script || typeof script.name !== 'string') {
        continue;
    }
    if (!script.name.startsWith('[ai_queue3,')) {
        continue;
    }
    const file: string = script.info?.sourceFilePath ?? '';
    const key = (script.info?.lookupKey ?? 0) >>> 0;
    if ((key & 0xff) !== AI_QUEUE3) {
        continue;
    }
    const kind = (key >> 8) & 0x3;
    const subject = key >>> 10;
    let npcIds: number[] = [];
    if (kind === 2) {
        npcIds = [subject];
    } else if (kind === 1) {
        for (let id = 0; id < NpcType.count; id++) {
            if ((NpcType.get(id) as any).category === subject) {
                npcIds.push(id);
            }
        }
    }
    if (!file.includes(DROP_DIR)) {
        // Quest/area death handlers own their drops: no default bones.
        for (const npc of npcIds) {
            otherHandlers.add(npc);
        }
        continue;
    }
    if (npcIds.length === 0) {
        unknownSubjects.push(script.name);
        continue;
    }
    subjectNpcs.set(script.name.slice('[ai_queue3,'.length, -1), npcIds);
    // Stub `[JUMP_WITH_PARAMS target, RETURN]` delegates to a same-file
    // label proc holding the table; inline bodies scan as-is.
    const bodies = [script];
    if (script.opcodes.length === 2 && script.opcodes[0] === ScriptOpcode.JUMP_WITH_PARAMS && script.opcodes[1] === ScriptOpcode.RETURN) {
        const target = byId.get(script.intOperands[0]);
        if (target && target.info?.sourceFilePath === file) {
            bodies.push(target);
        }
    }
    for (const body of bodies) {
        const { opcodes, intOperands } = body;
        for (let i = 4; i < opcodes.length; i++) {
            if (opcodes[i] !== ScriptOpcode.OBJ_ADD) {
                continue;
            }
            if (
                opcodes[i - 4] === ScriptOpcode.NPC_COORD &&
                opcodes[i - 3] === ScriptOpcode.PUSH_CONSTANT_INT &&
                opcodes[i - 2] === ScriptOpcode.PUSH_CONSTANT_INT &&
                opcodes[i - 1] === ScriptOpcode.PUSH_CONSTANT_INT
            ) {
                const obj = intOperands[i - 3];
                if (obj >= 0 && obj < ObjType.configs.length) {
                    for (const npc of npcIds) {
                        let set = pairs.get(npc);
                        if (!set) {
                            set = new Set();
                            pairs.set(npc, set);
                        }
                        set.add(obj);
                    }
                    constHits++;
                }
            } else if (
                opcodes[i - 3] === ScriptOpcode.NPC_COORD &&
                opcodes[i - 2] === ScriptOpcode.GOSUB_WITH_PARAMS &&
                opcodes[i - 1] === ScriptOpcode.PUSH_CONSTANT_INT
            ) {
                // `obj_add(npc_coord, ~sharedproc, duration)`: the proc
                // returns (obj, count) onto the stack.
                const proc = byId.get(intOperands[i - 2]);
                if (proc && typeof proc.name === 'string' && proc.name.startsWith('[proc,')) {
                    for (const obj of returnsOf(proc)) {
                        for (const npc of npcIds) {
                            let set = pairs.get(npc);
                            if (!set) {
                                set = new Set();
                                pairs.set(npc, set);
                            }
                            set.add(obj);
                        }
                    }
                    procHits++;
                }
            }
        }
    }
}

// ---- 3. drop rates from the RuneScript sources (F2P world truth) ----
//
// The bytecode pass above proves WHICH objs a table can add; the source
// pass (drop_rates.mts) says HOW OFTEN on an F2P world, plus the
// `npc_param(death_drop)` bones the bytecode pass cannot see. A pair the
// bytecode found but the source evaluates to 0 (members-gated branch)
// keeps rate 0: present, never farmed in F2P.

const contentScripts = path.resolve('../content/scripts');
const sourceFiles: Array<{ file: string; text: string }> = [];
const walk = (dir: string) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) {
            walk(full);
        } else if (entry.name.endsWith('.rs2')) {
            sourceFiles.push({ file: full, text: fs.readFileSync(full, 'utf8') });
        }
    }
};
walk(contentScripts);
const blocks = indexBlocks(sourceFiles);

ParamType.load('data/pack');
const deathDropParam = ParamType.getId('death_drop');
const deathDropDefault = deathDropParam >= 0 ? ParamType.get(deathDropParam).defaultInt : -1;
function deathDropOf(npcId: number): number {
    const value = (NpcType.get(npcId) as any).params?.get(deathDropParam);
    return typeof value === 'number' ? value : deathDropDefault;
}
const objIdByName = new Map<string, number>();
for (let id = 0; id < ObjType.configs.length; id++) {
    const name = (ObjType.get(id) as any).debugname;
    if (typeof name === 'string') {
        objIdByName.set(name, id);
    }
}

interface Rate {
    rate: number;
    qtyMin: number | null;
    qtyMax: number | null;
    exact: boolean;
}
const rates = new Map<number, Map<number, Rate>>();
function addRate(npc: number, obj: number, rate: Rate) {
    let table = rates.get(npc);
    if (!table) {
        table = new Map();
        rates.set(npc, table);
    }
    const known = table.get(obj);
    if (!known) {
        table.set(obj, { ...rate });
        return;
    }
    known.rate += rate.rate;
    known.exact = known.exact && rate.exact;
    known.qtyMin = known.qtyMin === null || rate.qtyMin === null ? null : Math.min(known.qtyMin, rate.qtyMin);
    known.qtyMax = known.qtyMax === null || rate.qtyMax === null ? null : Math.max(known.qtyMax, rate.qtyMax);
}

const handled = new Set<number>();
for (const [subject, npcIds] of subjectNpcs) {
    const emissions = handlerRates(blocks, subject);
    for (const npc of npcIds) {
        handled.add(npc);
        for (const row of emissions) {
            const objId = row.obj === '$death_drop' ? deathDropOf(npc) : objIdByName.get(row.obj) ?? -1;
            if (objId < 0) {
                continue;
            }
            addRate(npc, objId, { rate: row.rate, qtyMin: row.qtyMin, qtyMax: row.qtyMax, exact: row.exact });
        }
    }
}
// Attackable NPCs with no drop-table handler run `[ai_queue3,_]`
// (npc_default_death): their death_drop param, once per kill.
let defaultDeaths = 0;
for (let id = 0; id < NpcType.count; id++) {
    if (handled.has(id) || otherHandlers.has(id)) {
        continue;
    }
    const ops: Array<string | null> = (NpcType.get(id) as any).op ?? [];
    if (!ops.some(op => typeof op === 'string' && op.toLowerCase() === 'attack')) {
        continue;
    }
    const drop = deathDropOf(id);
    if (drop < 0) {
        continue;
    }
    addRate(id, drop, { rate: 1, qtyMin: 1, qtyMax: 1, exact: true });
    defaultDeaths++;
}

const rows: Array<Record<string, unknown>> = [];
const allNpcs = new Set<number>([...pairs.keys(), ...rates.keys()]);
for (const npcId of [...allNpcs].sort((a, b) => a - b)) {
    const npc = NpcType.get(npcId);
    const npcName = (npc as any).debugname ?? npc.name ?? `npc_${npcId}`;
    const objIds = new Set<number>([...(pairs.get(npcId) ?? []), ...(rates.get(npcId)?.keys() ?? [])]);
    for (const objId of [...objIds].sort((a, b) => a - b)) {
        const obj = ObjType.get(objId);
        // An evaluated table that never reaches a bytecode pair on an
        // F2P world (members branch) drops it at rate 0, exactly.
        const rate = rates.get(npcId)?.get(objId) ?? (handled.has(npcId) ? { rate: 0, qtyMin: null, qtyMax: null, exact: true } : undefined);
        rows.push({
            npc: npcName,
            npc_id: npcId,
            obj: (obj as any).debugname ?? obj.name ?? `obj_${objId}`,
            obj_id: objId,
            rate: rate ? Number(rate.rate.toFixed(6)) : null,
            qty_min: rate?.qtyMin ?? null,
            qty_max: rate?.qtyMax ?? null,
            exact: rate ? rate.exact : false,
        });
    }
}
fs.writeFileSync(path.join(dataDir, 'drops.json'), JSON.stringify(rows) + '\n');
const rated = rows.filter(row => row.rate !== null).length;
console.log(`drops: ${rows.length} pairs over ${allNpcs.size} npcs (${constHits} const, ${procHits} shared-proc hits); ${rated} rated, ${defaultDeaths} default death drops`);
if (unknownSubjects.length > 0) {
    console.log(`unresolved subjects (${unknownSubjects.length}): ${[...new Set(unknownSubjects)].slice(0, 10).join(', ')}`);
}

// ---- 4. which shops open on an F2P world (shop_access.json) ----
//
// A shop is reachable when an NPC's `[opnpcN,npc]` (or category) handler
// reaches `~openshop(inv, ...)` or `~openshop_activenpc` (its
// `owned_shop` param) on an F2P world. Closed-in-F2P keepers (Shantay,
// tea seller, fancy dress) gate every shop call behind map_members, so
// their handlers evaluate to nothing; Zaff's proc picks `staffshop`.
// `handlers` = 0 means the NPC has no trade handler of its own.

// varp -> owning quest dir (`content/scripts/quests/<dir>/configs/*.varp`):
// a shop gated on a members quest stays shut in F2P; an F2P quest gate
// becomes a requirement.
const varpQuest = new Map<string, string>();
const walkVarps = (dir: string) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) {
            walkVarps(full);
        } else if (entry.name.endsWith('.varp')) {
            const quest = /[\\/]quests[\\/](quest_[a-z0-9_]+)[\\/]/.exec(full)?.[1] ?? null;
            for (const found of fs.readFileSync(full, 'utf8').matchAll(/^\[([a-z0-9_]+)\]/gm)) {
                if (quest) {
                    varpQuest.set(found[1], quest);
                }
            }
        }
    }
};
walkVarps(contentScripts);

CategoryType.load('data/pack');
InvType.load('data/pack');
const ownedShopParam = ParamType.getId('owned_shop');
const access: Array<Record<string, unknown>> = [];
for (let id = 0; id < NpcType.count; id++) {
    const npc = NpcType.get(id) as any;
    const name: string | undefined = npc.debugname;
    if (!name) {
        continue;
    }
    const subjects = [name];
    if (typeof npc.category === 'number' && npc.category >= 0) {
        const category = (CategoryType.get(npc.category) as any)?.debugname;
        if (category) {
            subjects.push(`_${category}`);
        }
    }
    const owned = npc.params?.get(ownedShopParam);
    const ownedName = typeof owned === 'number' && owned >= 0 ? (InvType.get(owned) as any)?.debugname ?? null : null;
    // inv -> gate sets of every path that opens it (an ungated path
    // makes the shop open without condition).
    const opens = new Map<string, Set<string>[]>();
    const addOpen = (inv: string, gates: string[]) => {
        const paths = opens.get(inv) ?? [];
        paths.push(new Set(gates));
        opens.set(inv, paths);
    };
    let handlers = 0;
    // Per op, the NPC's own handler overrides its category handler (the
    // engine looks the specific subject up first).
    for (let op = 1; op <= 5; op++) {
        for (const subject of subjects) {
            const found = handlerShops(blocks, `opnpc${op}`, subject);
            if (found === null) {
                continue;
            }
            handlers++;
            for (const path_ of shopPaths(found)) {
                if (path_.inv === '$owned_shop') {
                    if (ownedName) {
                        addOpen(ownedName, path_.gates);
                    }
                } else {
                    addOpen(path_.inv, path_.gates);
                }
            }
            break;
        }
    }
    if (ownedName === null && opens.size === 0) {
        continue;
    }
    const f2pOpens = [...opens.entries()]
        .sort((left, right) => left[0].localeCompare(right[0]))
        .map(([inv, paths]) => {
            // Gates common to every path are the shop's requirement.
            const common = paths.reduce((acc, gates) => new Set([...acc].filter(gate => gates.has(gate))));
            return { inv, gates: [...common].sort().map(varp => ({ varp, quest: varpQuest.get(varp) ?? null })) };
        });
    access.push({ npc: name, npc_id: id, owned_shop: ownedName, handlers, f2p_opens: f2pOpens });
}
fs.writeFileSync(path.join(dataDir, 'shop_access.json'), JSON.stringify({ npcs: access }) + '\n');
const closed = access.filter(row => row.owned_shop !== null && (row.handlers as number) > 0 && !(row.f2p_opens as Array<{ inv: string }>).some(open => open.inv === row.owned_shop));
const gated = access.flatMap(row => (row.f2p_opens as Array<{ inv: string; gates: Array<{ varp: string; quest: string | null }> }>).filter(open => open.gates.length > 0).map(open => `${row.npc}(${open.inv}:${open.gates.map(gate => `${gate.varp}=${gate.quest}`).join('+')})`));
console.log(`shop_access: ${access.length} shop npcs; owned shop unreachable in F2P: ${closed.map(row => `${row.npc}(${row.owned_shop})`).join(', ')}`);
console.log(`shop_access: varp-gated: ${gated.join(', ')}`);

// ---- 5. dialogue sales (dialogue_buys.json) ----
//
// Objects an NPC sells through dialogue instead of a shop interface
// (Karim's kebab, bartenders' beer, Aggie's dyes): an
// `inv_del(inv, coins, N)` followed in the same block by
// `inv_add(inv, obj, n)`. The NPC is the file's `[opnpcN,npc]` subject.
// Members-gated branches are not distinguished here; the Rust side keeps
// only NPCs standing in F2P zones and F2P objects.

const buys: Array<Record<string, unknown>> = [];
for (const { file, text } of sourceFiles) {
    const subjects = [...new Set([...text.matchAll(/^\[opnpc[1-5],([a-z0-9_]+)\]/gm)].map(found => found[1]))];
    if (subjects.length === 0) {
        continue;
    }
    const clean = text.replace(/\/\/[^\n]*/g, '');
    for (const paid of clean.matchAll(/inv_del\(\s*inv\s*,\s*coins\s*,\s*(\d+)\s*\)/g)) {
        const after = clean.slice((paid.index ?? 0) + paid[0].length);
        const stop = after.search(/inv_del\(\s*inv\s*,\s*coins|^\[/m);
        const window = stop < 0 ? after.slice(0, 800) : after.slice(0, Math.min(stop, 800));
        const got = /inv_add\(\s*inv\s*,\s*([a-z0-9_]+)\s*,\s*(\d+)\s*\)/.exec(window);
        if (!got || got[1] === 'coins') {
            continue;
        }
        for (const npc of subjects) {
            const npcId = NpcType.getId(npc);
            buys.push({ npc, npc_id: npcId, obj: got[1], qty: Number(got[2]), cost: Number(paid[1]), file: path.relative(contentScripts, file) });
        }
    }
}
fs.writeFileSync(path.join(dataDir, 'dialogue_buys.json'), JSON.stringify({ buys }) + '\n');
console.log(`dialogue_buys: ${buys.length} rows: ${[...new Set(buys.map(row => row.obj))].join(', ')}`);

// ---- 6. script grants (script_grants.json) ----
//
// Every `inv_add(inv, obj, n)` outside drop tables and shops, with the
// block that runs it (`[opheldu,bowl_water]`, `[oploc1,cupboard]`,
// `[opnpc1,duke_horacio]`, `[proc,quest_imp_complete]`), the objects the
// same block deletes (`inv_del`: item-on-item inputs) and its file. The
// Rust index turns these into obtain ways (quest reward, item-on-item
// recipe, loc search, NPC gift, leftover after eating). Members gating
// is not evaluated here: the Rust side filters F2P objects, F2P quests
// and NPCs standing in F2P zones.

const grants: Array<Record<string, unknown>> = [];
const headerRe = /^\[([a-z0-9_]+),([a-z0-9_]+)\]/gm;
for (const { file, text } of sourceFiles) {
    const rel = path.relative(contentScripts, file);
    if (rel.startsWith('drop tables') || rel.startsWith('shop') || rel.startsWith('_test') || rel.startsWith('_unpack')) {
        continue;
    }
    const clean = text.replace(/\/\/[^\n]*/g, '');
    const heads = [...clean.matchAll(headerRe)];
    for (let index = 0; index < heads.length; index++) {
        const start = (heads[index].index ?? 0) + heads[index][0].length;
        const end = index + 1 < heads.length ? heads[index + 1].index ?? clean.length : clean.length;
        const body = clean.slice(start, end);
        const adds = [...body.matchAll(/inv_add\(\s*inv\s*,\s*([a-z0-9_]+)\s*,\s*(\d+)\s*\)/g)];
        if (adds.length === 0) {
            continue;
        }
        const dels = [...new Set([...body.matchAll(/inv_del\(\s*inv\s*,\s*([a-z0-9_]+)\s*,\s*(\d+)\s*\)/g)].filter(found => found[1] !== 'coins').map(found => `${found[1]}:${found[2]}`))];
        const cost = [...body.matchAll(/inv_del\(\s*inv\s*,\s*coins\s*,\s*(\d+)\s*\)/g)].map(found => Number(found[1]))[0] ?? 0;
        for (const add of adds) {
            if (add[1] === 'coins' || add[1] === 'null') {
                continue;
            }
            grants.push({ obj: add[1], qty: Number(add[2]), trigger: heads[index][1], subject: heads[index][2], consumes: dels, coins: cost, file: rel });
        }
    }
}
// File anchors: the player-facing handlers of each granting file, so the
// Rust side can tell a Taverley chest label (members loc) from a
// Lumbridge one.
const anchors: Record<string, string[]> = {};
for (const row of grants) {
    const rel = row.file as string;
    if (anchors[rel]) {
        continue;
    }
    const text = sourceFiles.find(entry => path.relative(contentScripts, entry.file) === rel)?.text ?? '';
    anchors[rel] = [...new Set([...text.matchAll(/^\[(op(?:npc|loc|held|obj)[a-z0-9]*),([a-z0-9_]+)\]/gm)].map(found => `${found[1]},${found[2]}`))];
}
// Quest gates per granting/selling file: quest directories of every
// `%varp` the file reads (Sir Prysin re-sells Silverlight only after
// Demon Slayer; the gate sits in his file, not in the sale label).
const fileQuests: Record<string, string[]> = {};
const questsOfFile = (rel: string) => {
    if (fileQuests[rel]) {
        return;
    }
    const text = sourceFiles.find(entry => path.relative(contentScripts, entry.file) === rel)?.text ?? '';
    const quests = new Set<string>();
    for (const found of text.matchAll(/%([a-z0-9_]+)/g)) {
        const quest = varpQuest.get(found[1]);
        if (quest) {
            quests.add(quest);
        }
    }
    fileQuests[rel] = [...quests].sort();
};
for (const row of [...grants, ...buys]) {
    questsOfFile(row.file as string);
}
fs.writeFileSync(path.join(dataDir, 'script_grants.json'), JSON.stringify({ grants, anchors, file_quests: fileQuests }) + '\n');
console.log(`script_grants: ${grants.length} rows over ${new Set(grants.map(row => row.obj)).size} objs`);

// ---- 7. config-driven productions (config_recipes.json) ----
//
//   spell:    `data=convertobj,FROM,TO,anim,seq,spell` (magic_spells.dbrow):
//             enchanting a strung amulet / ring with a spell.
//   mix:      dye structs with ingredient + secondary_ingredient -> product.
//   dye_cape: any cape + the colour's dye -> that colour's cape.
//   leftover: obj param `next_obj_stage` (eating a meat pie leaves half).

const configRecipes: Array<Record<string, unknown>> = [];
const configText = (relative: string) => fs.readFileSync(path.join(contentScripts, relative), 'utf8');
for (const found of configText('skill_magic/configs/magic_spells.dbrow').matchAll(/^data=convertobj,([a-z0-9_]+),([a-z0-9_]+),[^,\n]*,([a-z0-9_]+),([a-z0-9_]+)/gm)) {
    // The cast seq names the spellbook tier (`enchant_amulet_lvl3` ->
    // spells.json `enchant_lvl3`); rings use the generic seq.
    const tier = /lvl(\d)/.exec(found[3])?.[1] ?? null;
    configRecipes.push({ kind: 'spell', inputs: [found[1]], output: found[2], spell: tier ? `enchant_lvl${tier}` : found[4] });
}
const dyeStructs = configText('skill_crafting/configs/dye_cape/dye_cape.struct');
const capes: string[] = [];
const dyes = new Map<string, string>(); // colour -> dye obj
const capeOf = new Map<string, string>(); // colour -> cape obj
for (const block of dyeStructs.split(/^\[/m).slice(1)) {
    const param = (name: string) => new RegExp(`^param=${name},([a-z0-9_]+)`, 'm').exec(block)?.[1] ?? null;
    const colour = param('color');
    const product = param('product');
    const cape = param('cape');
    const ingredient = param('ingredient');
    const secondary = param('secondary_ingredient');
    if (cape === null && product !== null && colour !== null) {
        capes.push(product);
        capeOf.set(colour, product);
    }
    if (cape !== null && product !== null && colour !== null) {
        dyes.set(colour, product);
    }
    if (ingredient !== null && secondary !== null && product !== null) {
        configRecipes.push({ kind: 'mix', inputs: [ingredient, secondary], output: product });
    }
}
for (const [colour, dye] of dyes) {
    const cape = capeOf.get(colour);
    if (cape) {
        configRecipes.push({ kind: 'dye_cape', inputs: ['red_cape', dye], output: cape, any_cape: capes });
    }
}
const nextStage = ParamType.getId('next_obj_stage');
for (let id = 0; id < ObjType.configs.length; id++) {
    const obj = ObjType.get(id) as any;
    const next = obj.params?.get(nextStage);
    if (typeof next === 'number' && next >= 0 && obj.debugname) {
        const nextName = (ObjType.get(next) as any).debugname;
        if (nextName) {
            configRecipes.push({ kind: 'leftover', inputs: [obj.debugname], output: nextName });
        }
    }
}
fs.writeFileSync(path.join(dataDir, 'config_recipes.json'), JSON.stringify({ recipes: configRecipes }) + '\n');
console.log(`config_recipes: ${configRecipes.length} rows (${['spell', 'mix', 'dye_cape', 'leftover'].map(kind => `${kind} ${configRecipes.filter(row => row.kind === kind).length}`).join(', ')})`);
