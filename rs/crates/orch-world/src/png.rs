#![forbid(unsafe_code)]
//! Minimal PNG encoder, std only: 8-bit truecolor (RGB) and truecolor
//! with alpha (RGBA), one IDAT with fixed-Huffman deflate.
//!
//! Fixed Huffman codes keep the code small (no LZ77 table search beyond a
//! short hash chain, no dynamic code tables) while compressing the flat
//! map areas well; the spectator images stay a few megabytes. CRC32 (chunk
//! checks) and Adler-32 (deflate stream check) are computed over the same
//! bytes a decoder verifies.

/// CRC32 (IEEE) over chunk type + body, like every PNG chunk trailer.
pub(crate) fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (index, slot) in table.iter_mut().enumerate() {
        let mut value = index as u32;
        for _ in 0..8 {
            if value & 1 == 1 {
                value = (value >> 1) ^ 0xEDB8_8320;
            } else {
                value >>= 1;
            }
        }
        *slot = value;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc = table[((crc ^ u32::from(*byte)) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let mut low: u32 = 1;
    let mut high: u32 = 0;
    for byte in data {
        low = (low + u32::from(*byte)) % 65521;
        high = (high + low) % 65521;
    }
    (high << 16) | low
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    let mut checked = Vec::with_capacity(4 + body.len());
    checked.extend_from_slice(kind);
    checked.extend_from_slice(body);
    out.extend_from_slice(&crc32(&checked).to_be_bytes());
}

// Length code 257..285: base length + extra bits (RFC 1951 §3.2.5).
const LENGTH_BASE: [u32; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u32; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
// Distance code 0..29: base distance + extra bits.
const DIST_BASE: [u32; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u32; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
// Code-length alphabet order (RFC 1951 §3.2.7).
const CODE_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// Raw scanlines (filter byte 0 + pixel bytes per row) compressed with
/// dynamic-Huffman deflate blocks inside a zlib stream: flat map areas
/// are mostly zero bytes, which fixed Huffman codes still at 8 bits each
/// (8.2 MB for the 16 files); dynamic tables put them near 1 bit.
fn deflate_compress(raw: &[u8]) -> Vec<u8> {
    struct Bits {
        out: Vec<u8>,
        acc: u32,
        nbits: u32,
    }
    impl Bits {
        fn put(&mut self, bits: u32, count: u32) {
            self.acc |= bits << self.nbits;
            self.nbits += count;
            while self.nbits >= 8 {
                self.out.push(self.acc as u8);
                self.acc >>= 8;
                self.nbits -= 8;
            }
        }
        fn huff(&mut self, code: u32, len: u32) {
            for shift in (0..len).rev() {
                self.put((code >> shift) & 1, 1);
            }
        }
    }
    /// Optimal code lengths via a heap tree (may exceed `max`); unused
    /// symbols (frequency 0) stay 0.
    fn optimal_lengths(freqs: &[u32]) -> Vec<u8> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        let mut lengths = vec![0u8; freqs.len()];
        let active: Vec<usize> = (0..freqs.len()).filter(|&index| freqs[index] > 0).collect();
        if active.len() <= 1 {
            if let Some(&only) = active.first() {
                lengths[only] = 1;
            }
            return lengths;
        }
        let mut heap: BinaryHeap<Reverse<(u64, usize)>> =
            active.iter().map(|&index| Reverse((u64::from(freqs[index]), index))).collect();
        let mut parent = vec![usize::MAX; freqs.len()];
        let mut next = freqs.len();
        while heap.len() > 1 {
            let Reverse((first_freq, first)) = heap.pop().expect("heap pair");
            let Reverse((second_freq, second)) = heap.pop().expect("heap pair");
            parent.push(usize::MAX);
            parent[first] = next;
            parent[second] = next;
            heap.push(Reverse((first_freq + second_freq, next)));
            next += 1;
        }
        for &index in &active {
            let (mut depth, mut node) = (0u8, index);
            while parent[node] != usize::MAX {
                node = parent[node];
                depth = depth.saturating_add(1).min(32);
            }
            lengths[index] = depth;
        }
        lengths
    }
    /// Length-limited assignment (15 bits): fold longer codes down, then
    /// rebalance the counts so Kraft holds exactly (miniz-style), finally
    /// dealing the shortest codes to the most frequent symbols. `None`
    /// when the counts cannot rebalance (caller falls back to a fixed
    /// block, which cannot happen on real input).
    fn limited_lengths(freqs: &[u32], max: usize) -> Option<Vec<u8>> {
        let optimal = optimal_lengths(freqs);
        let mut bl_count = [0u32; 33];
        for &len in &optimal {
            if len > 0 {
                bl_count[(len as usize).min(32)] += 1;
            }
        }
        for len in max + 1..33 {
            bl_count[max] += bl_count[len];
            bl_count[len] = 0;
        }
        let mut total: u64 = (1..=max).map(|len| (bl_count[len] as u64) << (max - len)).sum();
        let cap = 1u64 << max;
        while total > cap {
            bl_count[max] = bl_count[max].saturating_sub(1);
            let mut len = max - 1;
            while len > 1 && bl_count[len] == 0 {
                len -= 1;
            }
            if bl_count[len] == 0 {
                return None;
            }
            bl_count[len] -= 1;
            bl_count[len + 1] += 2;
            total -= 1;
        }
        let mut order: Vec<usize> = (0..freqs.len()).filter(|&index| freqs[index] > 0).collect();
        order.sort_by_key(|&index| (std::cmp::Reverse(freqs[index]), index));
        let mut lengths = vec![0u8; freqs.len()];
        let mut dealt = 0;
        for len in 1..=max {
            for _ in 0..bl_count[len] {
                if dealt >= order.len() {
                    break;
                }
                lengths[order[dealt]] = len as u8;
                dealt += 1;
            }
        }
        (dealt == order.len()).then_some(lengths)
    }
    /// Canonical codes from lengths: (code value, bit length) per symbol,
    /// unused symbols map to (0, 0).
    fn canonical(lengths: &[u8]) -> Vec<(u32, u32)> {
        let mut bl_count = [0u32; 16];
        for &len in lengths {
            if len > 0 {
                bl_count[len as usize] += 1;
            }
        }
        let mut code = 0u32;
        let mut next_code = [0u32; 16];
        for len in 1..16 {
            code = (code + bl_count[len - 1]) << 1;
            next_code[len] = code;
        }
        lengths
            .iter()
            .map(|&len| {
                if len == 0 {
                    (0, 0)
                } else {
                    let assigned = next_code[len as usize];
                    next_code[len as usize] += 1;
                    (assigned, u32::from(len))
                }
            })
            .collect()
    }
    /// Plain stored block (no compression): fallback for degenerate
    /// alphabets, always valid.
    fn emit_stored(bits: &mut Bits, last: bool, block: &[u8]) {
        bits.put(u32::from(last), 1);
        bits.put(0, 2);
        while bits.nbits >= 8 {
            bits.out.push(bits.acc as u8);
            bits.acc >>= 8;
            bits.nbits -= 8;
        }
        bits.acc = 0;
        bits.nbits = 0;
        let len = block.len() as u16;
        bits.out.extend_from_slice(&len.to_le_bytes());
        bits.out.extend_from_slice(&(!len).to_le_bytes());
        bits.out.extend_from_slice(block);
    }
    /// One literal/match in a dynamic block: (litlen symbol, length extra
    /// value, distance symbol, distance extra value); literals carry the
    /// byte in `litlen` and ignore the rest.
    #[derive(Clone, Copy)]
    struct Symbol {
        litlen: u16,
        len_extra: u16,
        dist: u16,
        dist_extra: u16,
    }
    /// Encode buffered symbols with per-block dynamic tables; stored-block
    /// fallback on the raw slice when the alphabets degenerate (keeps the
    /// stream valid no matter what the tables do).
    fn emit_block(
        bits: &mut Bits,
        last: bool,
        raw: &[u8],
        block_start: usize,
        block_end: usize,
        symbols: &[Symbol],
        freq_litlen: &[u32; 286],
        freq_dist: &[u32; 30],
    ) {
        let tables = limited_lengths(freq_litlen, 15)
            .and_then(|litlen| limited_lengths(freq_dist, 15).map(|dist| (litlen, dist)));
        let Some((litlen_lengths, mut dist_lengths)) = tables else {
            emit_stored(bits, last, &raw[block_start..block_end]);
            return;
        };
        if freq_dist.iter().all(|&freq| freq == 0) {
            // No match at all: two dummy 1-bit distance codes, never read.
            dist_lengths = vec![0u8; 30];
            dist_lengths[0] = 1;
            dist_lengths[1] = 1;
        }
        let mut litlen_count = 286;
        while litlen_count > 257 && litlen_lengths[litlen_count - 1] == 0 {
            litlen_count -= 1;
        }
        let mut dist_count = 30;
        while dist_count > 1 && dist_lengths[dist_count - 1] == 0 {
            dist_count -= 1;
        }
        let mut joined = Vec::with_capacity(litlen_count + dist_count);
        joined.extend_from_slice(&litlen_lengths[..litlen_count]);
        joined.extend_from_slice(&dist_lengths[..dist_count]);
        // Run-length encode the joined lengths (RFC 1951 §3.2.7).
        let mut tokens: Vec<(u32, u32)> = Vec::new();
        let mut freq_cl = [0u32; 19];
        let mut index = 0;
        while index < joined.len() {
            let value = joined[index];
            let cap = if value == 0 { 138 } else { 6 };
            let mut run = 1;
            while run < cap && index + run < joined.len() && joined[index + run] == value {
                run += 1;
            }
            if value == 0 {
                if run >= 11 {
                    tokens.push((18, (run - 11) as u32));
                    freq_cl[18] += 1;
                } else if run >= 3 {
                    tokens.push((17, (run - 3) as u32));
                    freq_cl[17] += 1;
                } else {
                    for _ in 0..run {
                        tokens.push((0, 0));
                        freq_cl[0] += 1;
                    }
                }
            } else {
                tokens.push((u32::from(value), 0));
                freq_cl[value as usize] += 1;
                let mut rest = run - 1;
                while rest >= 3 {
                    let take = rest.min(6);
                    tokens.push((16, (take - 3) as u32));
                    freq_cl[16] += 1;
                    rest -= take;
                }
                for _ in 0..rest {
                    tokens.push((u32::from(value), 0));
                    freq_cl[value as usize] += 1;
                }
            }
            index += run;
        }
        let Some(cl_lengths) = limited_lengths(&freq_cl, 7) else {
            emit_stored(bits, last, &raw[block_start..block_end]);
            return;
        };
        let mut hclen = 19;
        while hclen > 4 && cl_lengths[CODE_ORDER[hclen - 1]] == 0 {
            hclen -= 1;
        }
        bits.put(u32::from(last), 1);
        bits.put(2, 2); // BTYPE 10: dynamic Huffman codes
        bits.put((litlen_count - 257) as u32, 5);
        bits.put((dist_count - 1) as u32, 5);
        bits.put((hclen - 4) as u32, 4);
        for order in CODE_ORDER.iter().take(hclen) {
            bits.put(u32::from(cl_lengths[*order]), 3);
        }
        let cl_codes = canonical(&cl_lengths);
        for (symbol, extra) in &tokens {
            let (code, len) = cl_codes[*symbol as usize];
            bits.huff(code, len);
            match symbol {
                16 => bits.put(*extra, 2),
                17 => bits.put(*extra, 3),
                18 => bits.put(*extra, 7),
                _ => {}
            }
        }
        let litlen_codes = canonical(&litlen_lengths);
        let dist_codes = canonical(&dist_lengths);
        for symbol in symbols {
            let (code, len) = litlen_codes[symbol.litlen as usize];
            bits.huff(code, len);
            if symbol.litlen >= 257 {
                bits.put(u32::from(symbol.len_extra), LENGTH_EXTRA[symbol.litlen as usize - 257]);
                let (code, len) = dist_codes[symbol.dist as usize];
                bits.huff(code, len);
                bits.put(u32::from(symbol.dist_extra), DIST_EXTRA[symbol.dist as usize]);
            }
        }
        let (code, len) = litlen_codes[256]; // end of block
        bits.huff(code, len);
    }
    let mut bits = Bits { out: Vec::with_capacity(raw.len() / 4), acc: 0, nbits: 0 };
    // Greedy LZ77 over a 32 KiB window: one hash table of last positions
    // plus a ring of previous links (overwrites only cut chains short,
    // never corrupt them: every candidate is byte-compared). Symbols are
    // buffered per 1 MiB block so each block gets its own tables; the
    // tables persist across blocks, so matches may reach back freely.
    const HASH_SIZE: usize = 1 << 15;
    const BLOCK_RAW: usize = 1 << 20;
    let mut head = vec![-1i32; HASH_SIZE];
    let mut prev = vec![-1i32; HASH_SIZE];
    let mut pos = 0usize;
    let mut block_start = 0usize;
    let mut symbols: Vec<Symbol> = Vec::new();
    let mut freq_litlen = [0u32; 286];
    let mut freq_dist = [0u32; 30];
    fn flush_block(
        bits: &mut Bits,
        last: bool,
        raw: &[u8],
        start: usize,
        end: usize,
        symbols: &mut Vec<Symbol>,
        freq_litlen: &mut [u32; 286],
        freq_dist: &mut [u32; 30],
    ) {
        freq_litlen[256] += 1; // end of block, always coded
        emit_block(bits, last, raw, start, end, symbols, freq_litlen, freq_dist);
        symbols.clear();
        *freq_litlen = [0u32; 286];
        *freq_dist = [0u32; 30];
    }
    while pos < raw.len() {
        if pos - block_start >= BLOCK_RAW {
            flush_block(&mut bits, false, raw, block_start, pos, &mut symbols, &mut freq_litlen, &mut freq_dist);
            block_start = pos;
        }
        let mut best_len = 0usize;
        let mut best_dist = 0usize;
        if pos + 3 <= raw.len() {
            let hash =
                (((raw[pos] as usize) << 10) ^ ((raw[pos + 1] as usize) << 5) ^ (raw[pos + 2] as usize))
                    & (HASH_SIZE - 1);
            let mut candidate = head[hash];
            let longest = (raw.len() - pos).min(258);
            let mut links = 0;
            while candidate >= 0 && links < 64 {
                let start = candidate as usize;
                if (pos as i32) - candidate > 32768 {
                    break;
                }
                if raw[start] == raw[pos] {
                    let mut len = 1usize;
                    while len < longest && raw[start + len] == raw[pos + len] {
                        len += 1;
                    }
                    if len > best_len {
                        best_len = len;
                        best_dist = pos - start;
                        if len == 258 {
                            break;
                        }
                    }
                }
                candidate = prev[start & (HASH_SIZE - 1)];
                links += 1;
            }
            prev[pos & (HASH_SIZE - 1)] = head[hash];
            head[hash] = pos as i32;
        }
        if best_len >= 3 {
            let index = LENGTH_BASE.partition_point(|&base| base <= best_len as u32) - 1;
            let dist = DIST_BASE.partition_point(|&base| base <= best_dist as u32) - 1;
            symbols.push(Symbol {
                litlen: (257 + index) as u16,
                len_extra: (best_len as u32 - LENGTH_BASE[index]) as u16,
                dist: dist as u16,
                dist_extra: (best_dist as u32 - DIST_BASE[dist]) as u16,
            });
            freq_litlen[257 + index] += 1;
            freq_dist[dist] += 1;
            // Feed the skipped positions to the table so later matches can
            // start inside this one (long flat runs match repeatedly).
            for skipped in pos + 1..pos + best_len {
                if skipped + 3 <= raw.len() {
                    let hash = (((raw[skipped] as usize) << 10)
                        ^ ((raw[skipped + 1] as usize) << 5)
                        ^ (raw[skipped + 2] as usize))
                        & (HASH_SIZE - 1);
                    prev[skipped & (HASH_SIZE - 1)] = head[hash];
                    head[hash] = skipped as i32;
                }
            }
            pos += best_len;
        } else {
            symbols.push(Symbol { litlen: u16::from(raw[pos]), len_extra: 0, dist: 0, dist_extra: 0 });
            freq_litlen[usize::from(raw[pos])] += 1;
            pos += 1;
        }
    }
    flush_block(&mut bits, true, raw, block_start, pos, &mut symbols, &mut freq_litlen, &mut freq_dist);
    if bits.nbits > 0 {
        bits.out.push(bits.acc as u8);
    }
    let mut out = vec![0x78, 0x9C];
    out.extend_from_slice(&bits.out);
    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

fn encode(width: u32, height: u32, colour_type: u8, rows: &[u8]) -> Vec<u8> {
    assert_eq!(rows.len() % (height as usize).max(1), 0);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, colour_type, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &header);
    chunk(&mut png, b"IDAT", &deflate_compress(rows));
    chunk(&mut png, b"IEND", &[]);
    png
}

/// RGBA PNG: `pixels` holds `width * height` pixels, 4 bytes each.
pub fn encode_rgba(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    assert_eq!(pixels.len(), width as usize * height as usize * 4);
    let stride = width as usize * 4;
    let mut rows = Vec::with_capacity(pixels.len() + height as usize);
    for row in pixels.chunks(stride) {
        rows.push(0);
        rows.extend_from_slice(row);
    }
    encode(width, height, 6, &rows)
}

/// RGB PNG: `pixels` holds `width * height` pixels, 3 bytes each.
pub fn encode_rgb(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    assert_eq!(pixels.len(), width as usize * height as usize * 3);
    let stride = width as usize * 3;
    let mut rows = Vec::with_capacity(pixels.len() + height as usize);
    for row in pixels.chunks(stride) {
        rows.push(0);
        rows.extend_from_slice(row);
    }
    encode(width, height, 2, &rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test-only inflate for the blocks `deflate_compress` emits (fixed
    /// and dynamic): proves the stream decodes byte-identical.
    fn inflate_stream(stream: &[u8]) -> Vec<u8> {
        assert_eq!(&stream[..2], &[0x78, 0x9C], "zlib header for default compression");
        struct Reader<'a> {
            data: &'a [u8],
            acc: u32,
            nbits: u32,
            pos: usize,
        }
        impl<'a> Reader<'a> {
            fn bit(&mut self) -> u32 {
                if self.nbits == 0 {
                    self.acc = u32::from(self.data[self.pos]);
                    self.pos += 1;
                    self.nbits = 8;
                }
                let bit = self.acc & 1;
                self.acc >>= 1;
                self.nbits -= 1;
                bit
            }
            fn bits(&mut self, count: u32) -> u32 {
                let mut value = 0u32;
                for index in 0..count {
                    value |= self.bit() << index;
                }
                value
            }
        }
        // (code, length, symbol) rows, MSB-first walk like the encoder.
        fn decode(reader: &mut Reader, table: &[(u32, u32, u32)]) -> u32 {
            let (mut code, mut len) = (0u32, 0u32);
            loop {
                code = (code << 1) | reader.bit();
                len += 1;
                if let Some((_, _, symbol)) = table.iter().find(|(c, l, _)| *c == code && *l == len) {
                    return *symbol;
                }
                assert!(len < 15, "dead end in huffman table");
            }
        }
        fn table(lengths: &[u8]) -> Vec<(u32, u32, u32)> {
            let mut bl_count = [0u32; 16];
            for &len in lengths {
                if len > 0 {
                    bl_count[len as usize] += 1;
                }
            }
            let mut code = 0u32;
            let mut next = [0u32; 16];
            for len in 1..16 {
                code = (code + bl_count[len - 1]) << 1;
                next[len] = code;
            }
            lengths
                .iter()
                .enumerate()
                .filter_map(|(symbol, &len)| {
                    if len == 0 {
                        None
                    } else {
                        let assigned = next[len as usize];
                        next[len as usize] += 1;
                        Some((assigned, u32::from(len), symbol as u32))
                    }
                })
                .collect()
        }
        let body_end = stream.len() - 4;
        let mut reader = Reader { data: &stream[2..body_end], acc: 0, nbits: 0, pos: 0 };
        let mut out = Vec::new();
        loop {
            let last = reader.bit();
            let btype = reader.bits(2);
            let (litlen_table, dist_table) = if btype == 1 {
                // RFC fixed tables, as canonical codes of their lengths.
                let mut litlen = vec![8u8; 288];
                litlen[144..256].fill(9);
                litlen[256..280].fill(7);
                (table(&litlen), table(&vec![5u8; 32]))
            } else if btype == 2 {
                let hlit = reader.bits(5) + 257;
                let hdist = reader.bits(5) + 1;
                let hclen = reader.bits(4) + 4;
                let mut cl_lengths = [0u8; 19];
                for index in 0..hclen {
                    cl_lengths[CODE_ORDER[index as usize]] = reader.bits(3) as u8;
                }
                let cl_table = table(&cl_lengths);
                let total = (hlit + hdist) as usize;
                let mut lengths = Vec::with_capacity(total);
                while lengths.len() < total {
                    match decode(&mut reader, &cl_table) {
                        symbol @ 0..=15 => lengths.push(symbol as u8),
                        16 => {
                            let repeat = 3 + reader.bits(2) as usize;
                            let value = *lengths.last().expect("16 needs a previous length");
                            lengths.extend(std::iter::repeat(value).take(repeat));
                        }
                        17 => {
                            let repeat = 3 + reader.bits(3) as usize;
                            lengths.extend(std::iter::repeat(0).take(repeat));
                        }
                        18 => {
                            let repeat = 11 + reader.bits(7) as usize;
                            lengths.extend(std::iter::repeat(0).take(repeat));
                        }
                        symbol => panic!("bad code-length symbol {symbol}"),
                    }
                }
                let (litlen_lengths, dist_lengths) = lengths.split_at(hlit as usize);
                assert_eq!(dist_lengths.len(), hdist as usize);
                (table(litlen_lengths), table(dist_lengths))
            } else {
                panic!("unsupported block type {btype}");
            };
            loop {
                let symbol = decode(&mut reader, &litlen_table);
                if symbol == 256 {
                    break;
                }
                if symbol < 256 {
                    out.push(symbol as u8);
                    continue;
                }
                let lit = (symbol - 257) as usize;
                let len = LENGTH_BASE[lit] + reader.bits(LENGTH_EXTRA[lit]);
                let dist_code = decode(&mut reader, &dist_table);
                let dist = DIST_BASE[dist_code as usize] + reader.bits(DIST_EXTRA[dist_code as usize]);
                let start = out.len() - dist as usize;
                for index in 0..len as usize {
                    out.push(out[start + index]);
                }
            }
            if last == 1 {
                break;
            }
        }
        assert_eq!(reader.pos, reader.data.len(), "no trailing bits");
        let stored = u32::from_be_bytes(stream[body_end..].try_into().unwrap());
        assert_eq!(stored, adler32(&out), "adler over the raw scanlines");
        out
    }

    /// Raw scanlines back out of the file: filter bytes plus pixel rows.
    fn idat_raw(png: &[u8]) -> Vec<u8> {
        let parts = chunks(png);
        let stream: Vec<u8> = parts
            .iter()
            .filter(|(kind, _)| kind == b"IDAT")
            .flat_map(|(_, body)| body.clone())
            .collect();
        inflate_stream(&stream)
    }

    fn unfilter(raw: &[u8], stride: usize) -> Vec<u8> {
        let rows = raw.len() / (stride + 1);
        let mut out = Vec::with_capacity(rows * stride);
        for row in raw.chunks(stride + 1) {
            assert_eq!(row[0], 0, "filter 0 on every row");
            out.extend_from_slice(&row[1..]);
        }
        out
    }

    /// Walk the chunk list, checking lengths and CRCs like a decoder would.
    fn chunks(png: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let mut out = Vec::new();
        let mut position = 8;
        loop {
            let length = u32::from_be_bytes(png[position..position + 4].try_into().unwrap()) as usize;
            let kind: [u8; 4] = png[position + 4..position + 8].try_into().unwrap();
            let body = png[position + 8..position + 8 + length].to_vec();
            let stored = u32::from_be_bytes(png[position + 8 + length..position + 12 + length].try_into().unwrap());
            let mut checked = Vec::with_capacity(4 + length);
            checked.extend_from_slice(&kind);
            checked.extend_from_slice(&body);
            assert_eq!(crc32(&checked), stored, "bad CRC for {kind:?}");
            position += 12 + length;
            let done = kind == *b"IEND";
            out.push((kind, body));
            if done {
                break;
            }
        }
        assert_eq!(position, png.len());
        out
    }

    #[test]
    fn rgba_header_round_trip() {
        let pixels = vec![
            255, 0, 0, 255, 0, 255, 0, 255,
            0, 0, 255, 255, 0, 0, 0, 0,
        ];
        let png = encode_rgba(2, 2, &pixels);
        let parts = chunks(&png);
        assert_eq!(parts[0].0, *b"IHDR");
        let header = &parts[0].1;
        assert_eq!(u32::from_be_bytes(header[0..4].try_into().unwrap()), 2);
        assert_eq!(u32::from_be_bytes(header[4..8].try_into().unwrap()), 2);
        assert_eq!((header[8], header[9]), (8, 6));
        assert_eq!(parts.last().unwrap().0, *b"IEND");
        // Compressed stream decodes back to the filter-0 scanlines.
        let raw = idat_raw(&png);
        assert_eq!(unfilter(&raw, 2 * 4), pixels);
    }

    #[test]
    fn rgb_header_round_trip() {
        let png = encode_rgb(1, 1, &[1, 2, 3]);
        let parts = chunks(&png);
        let header = &parts[0].1;
        assert_eq!(u32::from_be_bytes(header[0..4].try_into().unwrap()), 1);
        assert_eq!(u32::from_be_bytes(header[4..8].try_into().unwrap()), 1);
        assert_eq!((header[8], header[9]), (8, 2));
        assert_eq!(unfilter(&idat_raw(&png), 3), vec![1, 2, 3]);
    }

    #[test]
    fn dynamic_blocks_beat_stored_on_flat_rows() {
        // One black 132-px row block, like empty ocean: matches must fire.
        let raw = vec![0u8; 132 * 3];
        let compressed = deflate_compress(&raw);
        let stored = raw.len() + 5 + 6; // 5-byte block header + zlib wrap
        assert!(compressed.len() < stored / 4, "flat rows compress, len={}", compressed.len());
        assert_eq!(inflate_stream(&compressed), raw);
    }

    #[test]
    fn dynamic_round_trip_mixed_bytes() {
        // Gradient plus flat runs plus noise: literals, lengths and long
        // distances all decode back.
        let mut raw = Vec::new();
        for row in 0..64 {
            raw.push(0);
            for col in 0..192 {
                raw.push(((row * 3 + col) & 0xFF) as u8);
            }
        }
        raw.extend_from_slice(&[7u8; 300]); // run past one match-length cap
        let mut noise = 0x1234_5678u32;
        for _ in 0..4096 {
            noise = noise.wrapping_mul(1664525).wrapping_add(1013904223);
            raw.push((noise >> 16) as u8);
        }
        let repeat = raw[..2000].to_vec(); // 16 KiB-back repeat
        raw.extend_from_slice(&repeat);
        let compressed = deflate_compress(&raw);
        assert!(compressed.len() < raw.len(), "mixed input still shrinks");
        assert_eq!(inflate_stream(&compressed), raw);
    }

    #[test]
    fn dynamic_round_trip_many_blocks() {
        // Past the 1 MiB block split: the decoder must chain blocks.
        let mut raw = Vec::with_capacity(1_200_000);
        for index in 0..1_200_000 {
            raw.push(((index * 7 + (index >> 9)) & 0xFF) as u8);
        }
        let compressed = deflate_compress(&raw);
        assert!(compressed.len() < raw.len() / 2, "textured input shrinks a lot");
        assert_eq!(inflate_stream(&compressed), raw);
    }

    #[test]
    fn crc32_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}
