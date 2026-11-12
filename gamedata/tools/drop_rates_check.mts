// Manual check of drop_rates.mts against the live rev 274 sources.
// Run: node drop_rates_check.mts <content/scripts dir> <subject>...
import fs from 'node:fs';
import path from 'node:path';
import { indexBlocks, handlerRates } from './drop_rates.mts';

const root = process.argv[2];
const files: Array<{ file: string; text: string }> = [];
const walk = (dir: string) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) walk(full);
        else if (entry.name.endsWith('.rs2')) files.push({ file: full, text: fs.readFileSync(full, 'utf8') });
    }
};
walk(root);
const blocks = indexBlocks(files);
for (const subject of process.argv.slice(3)) {
    const rows = handlerRates(blocks, subject).sort((a, b) => b.rate - a.rate);
    console.log(`== ${subject}: total ${rows.reduce((s, r) => s + r.rate, 0).toFixed(3)}`);
    for (const row of rows) console.log(`  ${row.obj.padEnd(24)} ${(row.rate * 100).toFixed(2).padStart(6)}% qty ${row.qtyMin}-${row.qtyMax}${row.exact ? '' : ' ~'}`);
}
