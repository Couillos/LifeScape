// Drop-rate evaluator for LostCity rev 274 RuneScript drop tables.
//
// Pure: script source text in, per-object probabilities out. Used by
// dump_plan.mts to put a rate on every (npc, obj) drop pair. The tables
// are `def_int $x = random(N)` draws followed by `if ($x < K)` chains;
// the evaluator walks blocks with an "alive" probability mass:
//   - a chain clause on a known draw gets (K - previous K) / N;
//   - `else` gets what the earlier clauses left;
//   - `return(obj, n)`, `@label` jumps and `return;` end the flow;
//   - `obj_add(npc_coord, X, n, ...)` emits X at the current mass;
//   - `~proc` operands (shared tables) expand to their own distribution.
// F2P world truth: `map_members = ^true` is false, `npc_findhero` holds.
// Anything else the evaluator cannot decide counts as true and marks the
// emission inexact (`exact: false`), never silently dropped.

export interface Emission {
    obj: string; // debug name, or '$death_drop' for npc_param(death_drop)
    rate: number; // expected drops of this obj per kill (probability)
    qtyMin: number | null;
    qtyMax: number | null;
    exact: boolean;
}

export interface Block {
    file: string;
    body: string;
}

/** Index of every `[kind,name]` block across sources: key `kind,name`. */
export function indexBlocks(files: Array<{ file: string; text: string }>): Map<string, Block> {
    const out = new Map<string, Block>();
    const header = /^\[([a-z0-9_]+),([a-z0-9_]+)\]/gm;
    for (const { file, text } of files) {
        const starts: Array<{ key: string; at: number }> = [];
        let found: RegExpExecArray | null;
        header.lastIndex = 0;
        while ((found = header.exec(text)) !== null) {
            starts.push({ key: `${found[1]},${found[2]}`, at: found.index + found[0].length });
        }
        for (let index = 0; index < starts.length; index++) {
            const from = starts[index].at;
            const nextHeader = index + 1 < starts.length ? text.lastIndexOf('[', starts[index + 1].at) : text.length;
            let body = text.slice(from, nextHeader);
            // Proc signatures `(args)(returns)` ride the header line.
            body = stripSignature(body);
            if (!out.has(starts[index].key)) {
                out.set(starts[index].key, { file, body });
            }
        }
    }
    return out;
}

function stripSignature(body: string): string {
    let rest = body;
    for (let pass = 0; pass < 2; pass++) {
        const trimmed = rest.replace(/^[ \t]*/, '');
        if (!trimmed.startsWith('(')) {
            break;
        }
        const close = matchParen(trimmed, 0);
        if (close < 0) {
            break;
        }
        rest = trimmed.slice(close + 1);
    }
    return rest;
}

function matchParen(text: string, open: number): number {
    let depth = 0;
    for (let index = open; index < text.length; index++) {
        const char = text[index];
        if (char === '(') {
            depth++;
        } else if (char === ')') {
            depth--;
            if (depth === 0) {
                return index;
            }
        }
    }
    return -1;
}

function matchBrace(text: string, open: number): number {
    let depth = 0;
    for (let index = open; index < text.length; index++) {
        const char = text[index];
        if (char === '{') {
            depth++;
        } else if (char === '}') {
            depth--;
            if (depth === 0) {
                return index;
            }
        }
    }
    return -1;
}

function stripComments(text: string): string {
    return text.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '');
}

/** Split on top-level commas (parens and quotes respected). */
function splitArgs(text: string): string[] {
    const out: string[] = [];
    let depth = 0;
    let start = 0;
    for (let index = 0; index < text.length; index++) {
        const char = text[index];
        if (char === '(') {
            depth++;
        } else if (char === ')') {
            depth--;
        } else if (char === ',' && depth === 0) {
            out.push(text.slice(start, index).trim());
            start = index + 1;
        }
    }
    const last = text.slice(start).trim();
    if (last.length > 0) {
        out.push(last);
    }
    return out;
}

interface Ctx {
    blocks: Map<string, Block>;
    vars: Map<string, number>; // random draw sizes
    emits: Emission[];
    depth: number;
    exact: boolean;
    procCache: Map<string, Emission[]>;
    /** Shop mode: emit `$inv:<inv>` / `$owned_shop` on shop-opening calls. */
    shops: boolean;
    /** Shop mode: labels/procs already walked for this handler (reachability needs one visit). */
    seen: Set<string>;
    /** Shop mode: quest/state varps (`%name`) guarding the current path. */
    gates: string[];
}

function parseQty(expr: string): [number | null, number | null] {
    const text = expr.trim();
    if (/^-?\d+$/.test(text)) {
        const value = Number(text);
        return [value, value];
    }
    const range = /^~random_range\(\s*(\d+)\s*,\s*(\d+)\s*\)$/.exec(text);
    if (range) {
        return [Number(range[1]), Number(range[2])];
    }
    return [null, null];
}

function emit(ctx: Ctx, objExpr: string, qtyExpr: string | null, mass: number) {
    if (mass <= 0) {
        return;
    }
    const obj = objExpr.trim();
    if (obj.startsWith('~')) {
        const proc = obj.slice(1).replace(/\(.*$/, '');
        for (const row of procDistribution(ctx, proc)) {
            ctx.emits.push({ ...row, rate: row.rate * mass, exact: row.exact && ctx.exact });
        }
        return;
    }
    let name = obj;
    if (/^npc_param\(\s*death_drop\s*\)$/.test(obj)) {
        name = '$death_drop';
    } else if (!/^[a-z0-9_]+$/.test(obj) || obj === 'null') {
        return; // variables and computed objs carry no static identity
    }
    const [qtyMin, qtyMax] = qtyExpr === null ? [1, 1] : parseQty(qtyExpr);
    ctx.emits.push({ obj: name, rate: mass, qtyMin, qtyMax, exact: ctx.exact });
}

/** Distribution of a `[proc,name]` returning (obj, count). */
export function procDistribution(ctx: Ctx, proc: string): Emission[] {
    const cached = ctx.procCache.get(proc);
    if (cached) {
        return cached;
    }
    ctx.procCache.set(proc, []); // cycle guard
    const block = ctx.blocks.get(`proc,${proc}`);
    if (!block || ctx.depth > 8) {
        return [];
    }
    const inner: Ctx = { ...ctx, vars: new Map(), emits: [], depth: ctx.depth + 1, exact: true };
    evalBlock(inner, stripComments(block.body), 1);
    ctx.procCache.set(proc, inner.emits);
    return inner.emits;
}

/** Probability of one condition clause, given the chain's lower bound. */
function clauseProb(ctx: Ctx, cond: string, chain: { variable: string | null; lower: number }): number {
    let text = cond.trim();
    while (text.startsWith('(') && matchParen(text, 0) === text.length - 1) {
        text = text.slice(1, -1).trim();
    }
    const conjuncts = splitTop(text, '&');
    if (conjuncts.length > 1) {
        const before = ctx.exact;
        let product = 1;
        for (const part of conjuncts) {
            product *= clauseProb(ctx, part, { variable: null, lower: 0 });
        }
        // A definitely-false conjunct (members gate) decides the clause:
        // the undecidable parts beside it cost no exactness.
        if (product === 0) {
            ctx.exact = before;
        }
        return product;
    }
    if (splitTop(text, '|').length > 1) {
        ctx.exact = false;
        return 1;
    }
    if (/^map_members\s*=\s*\^true$/.test(text)) {
        return 0;
    }
    if (/^map_members\s*=\s*\^false$/.test(text)) {
        return 1;
    }
    if (/^npc_findhero\s*=\s*\^false$/.test(text)) {
        return 0;
    }
    if (/^npc_findhero\s*=\s*\^true$/.test(text) || /^\$[a-z0-9_]+\s*!\s*null$/.test(text)) {
        return 1;
    }
    const compare = /^\$([a-z0-9_]+)\s*(<=|<|=|>=|>)\s*(\d+)$/.exec(text);
    if (compare && ctx.vars.has(compare[1])) {
        const size = ctx.vars.get(compare[1]) ?? 1;
        const bound = Number(compare[3]);
        const sameChain = chain.variable === compare[1];
        const lower = sameChain ? chain.lower : 0;
        let width = 0;
        if (compare[2] === '<') {
            width = Math.max(0, Math.min(bound, size) - lower);
            if (sameChain) {
                chain.lower = Math.max(chain.lower, Math.min(bound, size));
            }
        } else if (compare[2] === '<=') {
            width = Math.max(0, Math.min(bound + 1, size) - lower);
            if (sameChain) {
                chain.lower = Math.max(chain.lower, Math.min(bound + 1, size));
            }
        } else if (compare[2] === '=') {
            width = bound >= lower && bound < size ? 1 : 0;
        } else if (compare[2] === '>=') {
            width = Math.max(0, size - Math.max(bound, lower));
        } else {
            width = Math.max(0, size - Math.max(bound + 1, lower));
        }
        return width / size;
    }
    ctx.exact = false;
    return 1;
}

function splitTop(text: string, separator: string): string[] {
    const out: string[] = [];
    let depth = 0;
    let start = 0;
    for (let index = 0; index < text.length; index++) {
        const char = text[index];
        if (char === '(') {
            depth++;
        } else if (char === ')') {
            depth--;
        } else if (char === separator && depth === 0) {
            out.push(text.slice(start, index));
            start = index + 1;
        }
    }
    out.push(text.slice(start));
    return out.map(part => part.trim()).filter(part => part.length > 0);
}

/** Walk one block; returns the mass that falls off its end. */
export function evalBlock(ctx: Ctx, text: string, alive: number): number {
    let index = 0;
    let mass = alive;
    while (index < text.length && mass > 1e-12) {
        while (index < text.length && /\s|;/.test(text[index])) {
            index++;
        }
        if (index >= text.length) {
            break;
        }
        if (text.startsWith('if', index) && /[\s(]/.test(text[index + 2] ?? '')) {
            const result = evalChain(ctx, text, index, mass);
            mass = result.mass;
            index = result.next;
            continue;
        }
        if (text.startsWith('while', index) && /[\s(]/.test(text[index + 5] ?? '')) {
            // Loops (dbrow tables) are not modelled: skip, flag inexact.
            const open = text.indexOf('(', index);
            const close = open < 0 ? -1 : matchParen(text, open);
            const brace = close < 0 ? -1 : text.indexOf('{', close);
            const end = brace < 0 ? -1 : matchBrace(text, brace);
            ctx.exact = false;
            index = end < 0 ? text.length : end + 1;
            continue;
        }
        let end = index;
        let depth = 0;
        while (end < text.length) {
            const char = text[end];
            if (char === '(') {
                depth++;
            } else if (char === ')') {
                depth--;
            } else if ((char === ';' || char === '\n') && depth === 0) {
                break;
            }
            end++;
        }
        const statement = text.slice(index, end).trim();
        index = end + 1;
        if (statement.length === 0) {
            continue;
        }
        mass = evalStatement(ctx, statement, mass);
    }
    return mass;
}

function evalChain(ctx: Ctx, text: string, start: number, mass: number): { mass: number; next: number } {
    let index = start;
    const chain: { variable: string | null; lower: number } = { variable: null, lower: 0 };
    let used = 0;
    let outflow = 0;
    let first = true;
    while (true) {
        let cond: string | null = null;
        if (first) {
            const open = text.indexOf('(', index);
            const close = open < 0 ? -1 : matchParen(text, open);
            if (close < 0) {
                ctx.exact = false;
                return { mass, next: text.length };
            }
            cond = text.slice(open + 1, close);
            index = close + 1;
        } else {
            const rest = text.slice(index);
            const elseIf = /^\s*else\s+if\s*\(/.exec(rest);
            const elseOnly = /^\s*else\s*\{/.exec(rest);
            if (elseIf) {
                const open = index + elseIf[0].length - 1;
                const close = matchParen(text, open);
                if (close < 0) {
                    ctx.exact = false;
                    return { mass, next: text.length };
                }
                cond = text.slice(open + 1, close);
                index = close + 1;
            } else if (elseOnly) {
                index = index + elseOnly[0].length - 1;
                cond = null;
            } else {
                break;
            }
        }
        const brace = text.indexOf('{', index);
        const end = brace < 0 ? -1 : matchBrace(text, brace);
        if (end < 0) {
            // Brace-less or unbalanced construct: stop here, inexact.
            ctx.exact = false;
            return { mass, next: text.length };
        }
        const body = text.slice(brace + 1, end);
        index = end + 1;
        let share: number;
        if (ctx.shops && (cond === null || !cond.includes('map_members'))) {
            // Shop mode measures reachability: a dialogue option is the
            // player's choice, so every non-members-gated branch is open.
            // A `%varp` condition is a quest/state gate: shops reached
            // under it carry the varp; a gated branch that returns gates
            // the rest of the block too (`if (%q < done) { return; }`).
            const varps = cond === null ? [] : [...cond.matchAll(/%([a-z0-9_]+)/g)].map(found => found[1]);
            const saved = ctx.gates;
            ctx.gates = [...saved, ...varps];
            const out = evalBlock(ctx, body, mass);
            ctx.gates = out <= 1e-12 && varps.length > 0 ? [...saved, ...varps] : saved;
            outflow += out;
            first = false;
            if (cond === null) {
                break;
            }
            continue;
        }
        if (cond === null) {
            share = Math.max(0, 1 - used);
        } else {
            const variable = /^\s*\(?\s*\$([a-z0-9_]+)\s*(<=|<)/.exec(cond);
            if (variable && (chain.variable === null || chain.variable === variable[1]) && splitTop(cond, '&').length === 1) {
                chain.variable = variable[1];
            }
            share = Math.min(clauseProb(ctx, cond, chain), Math.max(0, 1 - used));
        }
        used += share;
        outflow += evalBlock(ctx, body, mass * share);
        first = false;
        if (cond === null) {
            break;
        }
    }
    const fallthrough = mass * Math.max(0, 1 - used);
    return { mass: outflow + fallthrough, next: index };
}

function evalStatement(ctx: Ctx, rawStatement: string, mass: number): number {
    // `case 1 : stmt` inside switch_int: every case is a player choice,
    // so each body runs (over-approximates reachability, never rates).
    const statement = rawStatement.replace(/^case\s+[^:]+:\s*/, '').replace(/^default\s*:\s*/, '');
    if (ctx.shops && statement.startsWith('~')) {
        const name = /^~([a-z0-9_]+)/.exec(statement)?.[1] ?? '';
        if (name === 'openshop') {
            const open = statement.indexOf('(');
            const args = open < 0 ? [] : splitArgs(statement.slice(open + 1, matchParen(statement, open)));
            if (args.length > 0) {
                ctx.emits.push({ obj: `$inv:${args[0]}|${[...new Set(ctx.gates)].join(',')}`, rate: mass, qtyMin: null, qtyMax: null, exact: ctx.exact });
            }
            return mass;
        }
        if (name === 'openshop_activenpc') {
            ctx.emits.push({ obj: `$owned_shop|${[...new Set(ctx.gates)].join(',')}`, rate: mass, qtyMin: null, qtyMax: null, exact: ctx.exact });
            return mass;
        }
        const block = ctx.blocks.get(`proc,${name}`);
        if (block && name.includes('shop') && ctx.depth < 8 && !ctx.seen.has(`proc,${name}`)) {
            ctx.seen.add(`proc,${name}`);
            const inner: Ctx = { ...ctx, vars: new Map(), depth: ctx.depth + 1 };
            evalBlock(inner, stripComments(block.body), mass);
            return mass;
        }
        return mass;
    }
    const draw = /^(?:def_int\s+)?\$([a-z0-9_]+)\s*=\s*(random|randominc)\(\s*(\d+)\s*\)$/.exec(statement);
    if (draw) {
        const size = Number(draw[3]) + (draw[2] === 'randominc' ? 1 : 0);
        ctx.vars.set(draw[1], size);
        return mass;
    }
    if (statement.startsWith('obj_add(')) {
        const close = matchParen(statement, statement.indexOf('('));
        const args = splitArgs(statement.slice(statement.indexOf('(') + 1, close));
        if (args.length >= 3 && args[1].startsWith('~')) {
            emit(ctx, args[1], null, mass);
        } else if (args.length >= 3) {
            emit(ctx, args[1], args[2], mass);
        }
        return mass;
    }
    if (statement.startsWith('return')) {
        const open = statement.indexOf('(');
        if (open >= 0) {
            const args = splitArgs(statement.slice(open + 1, matchParen(statement, open)));
            if (args.length === 1 && args[0].startsWith('~')) {
                emit(ctx, args[0], null, mass);
            } else if (args.length >= 2) {
                emit(ctx, args[0], args[1], mass);
            }
        }
        return 0;
    }
    const jump = /^@([a-z0-9_]+)/.exec(statement);
    if (jump) {
        const block = ctx.blocks.get(`label,${jump[1]}`);
        if (ctx.shops && ctx.seen.has(`label,${jump[1]}`)) {
            return 0;
        }
        if (ctx.shops) {
            ctx.seen.add(`label,${jump[1]}`);
        }
        if (block && ctx.depth < 8) {
            const inner: Ctx = { ...ctx, depth: ctx.depth + 1 };
            evalBlock(inner, stripComments(block.body), mass);
            ctx.exact = ctx.exact && inner.exact;
        } else {
            ctx.exact = false;
        }
        return 0;
    }
    return mass;
}

/** Emissions of one `[ai_queue3,subject]` handler, merged per obj. */
export function handlerRates(blocks: Map<string, Block>, subject: string): Emission[] {
    const block = blocks.get(`ai_queue3,${subject}`);
    if (!block) {
        return [];
    }
    const ctx: Ctx = { blocks, vars: new Map(), emits: [], depth: 0, exact: true, procCache: new Map(), shops: false, seen: new Set(), gates: [] };
    evalBlock(ctx, stripComments(block.body), 1);
    return merge(ctx.emits);
}

export function merge(emits: Emission[]): Emission[] {
    const byObj = new Map<string, Emission>();
    for (const row of emits) {
        const known = byObj.get(row.obj);
        if (!known) {
            byObj.set(row.obj, { ...row });
            continue;
        }
        known.rate += row.rate;
        known.exact = known.exact && row.exact;
        known.qtyMin = known.qtyMin === null || row.qtyMin === null ? null : Math.min(known.qtyMin, row.qtyMin);
        known.qtyMax = known.qtyMax === null || row.qtyMax === null ? null : Math.max(known.qtyMax, row.qtyMax);
    }
    return [...byObj.values()];
}

/**
 * Shops one `[opnpcN,subject]` handler opens on an F2P world: inv names
 * (`$owned_shop` = the NPC's owned_shop param). Empty when the handler
 * exists but every shop call sits behind a members gate.
 */
export function handlerShops(blocks: Map<string, Block>, trigger: string, subject: string): string[] | null {
    const block = blocks.get(`${trigger},${subject}`);
    if (!block) {
        return null;
    }
    const ctx: Ctx = { blocks, vars: new Map(), emits: [], depth: 0, exact: true, procCache: new Map(), shops: true, seen: new Set(), gates: [] };
    evalBlock(ctx, stripComments(block.body), 1);
    return merge(ctx.emits)
        .filter(row => row.rate > 0)
        .map(row => (row.obj.startsWith('$inv:') ? row.obj.slice(5) : row.obj));
}

/** One shop-opening path: inv (`$owned_shop` = param) plus its varp gates. */
export interface ShopPath {
    inv: string;
    gates: string[];
}

/** Parse `handlerShops` entries (`inv|gate,gate`). */
export function shopPaths(entries: string[]): ShopPath[] {
    return entries.map(entry => {
        const [inv, gates] = entry.split('|');
        return { inv, gates: gates ? gates.split(',').filter(gate => gate.length > 0) : [] };
    });
}
