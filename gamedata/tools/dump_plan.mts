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
import ScriptProvider from '../../../lostcity/engine/src/engine/script/ScriptProvider.js';
import { ScriptOpcode } from '../../../lostcity/engine/src/engine/script/ScriptOpcode.js';

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

for (const script of scripts) {
    if (!script || typeof script.name !== 'string') {
        continue;
    }
    if (!script.name.startsWith('[ai_queue3,')) {
        continue;
    }
    const file: string = script.info?.sourceFilePath ?? '';
    if (!file.includes(DROP_DIR)) {
        continue;
    }
    const key = script.info.lookupKey >>> 0;
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
    if (npcIds.length === 0) {
        unknownSubjects.push(script.name);
        continue;
    }
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

const rows: Array<{ npc: string; npc_id: number; obj: string; obj_id: number }> = [];
for (const [npcId, objs_] of [...pairs.entries()].sort((a, b) => a[0] - b[0])) {
    const npc = NpcType.get(npcId);
    const npcName = (npc as any).debugname ?? npc.name ?? `npc_${npcId}`;
    for (const objId of [...objs_].sort((a, b) => a - b)) {
        const obj = ObjType.get(objId);
        rows.push({ npc: npcName, npc_id: npcId, obj: (obj as any).debugname ?? obj.name ?? `obj_${objId}`, obj_id: objId });
    }
}
fs.writeFileSync(path.join(dataDir, 'drops.json'), JSON.stringify(rows) + '\n');
console.log(`drops: ${rows.length} pairs over ${pairs.size} npcs (${constHits} const, ${procHits} shared-proc hits)`);
if (unknownSubjects.length > 0) {
    console.log(`unresolved subjects (${unknownSubjects.length}): ${[...new Set(unknownSubjects)].slice(0, 10).join(', ')}`);
}
