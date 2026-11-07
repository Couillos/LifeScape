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

/// Raw scanlines (filter byte 0 + pixel bytes per row) compressed with
/// one final fixed-Huffman deflate block inside a zlib stream.
fn deflate_fixed(raw: &[u8]) -> Vec<u8> {
    // Length code 257..285: base length + extra bits (RFC 1951 §3.2.5).
    const LENGTH_BASE: [u32; 29] = [
        3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
        131, 163, 195, 227, 258,
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
        0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12,
        13, 13,
    ];
    /// Fixed literal/length code: (code value, bit length). Code bits go
    /// out most-significant first (RFC 1951 §3.1.1); only the byte packing
    /// itself is LSB-first. (Plain data bits — BFINAL/BTYPE and the length
    /// /distance extra bits — stay LSB-first.)
    fn litlen_code(symbol: u32) -> (u32, u32) {
        if symbol < 144 {
            (0x30 + symbol, 8)
        } else if symbol < 256 {
            (0x190 + symbol - 144, 9)
        } else if symbol < 280 {
            (symbol - 256, 7)
        } else {
            (0xC0 + symbol - 280, 8)
        }
    }
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
    let mut bits = Bits { out: Vec::with_capacity(raw.len() / 4), acc: 0, nbits: 0 };
    bits.put(1, 1); // BFINAL: single block holds the whole image
    bits.put(1, 2); // BTYPE 01: fixed Huffman codes
    // Greedy LZ77 over a 32 KiB window: one hash table of last positions
    // plus a ring of previous links (overwrites only cut chains short,
    // never corrupt them: every candidate is byte-compared).
    const HASH_SIZE: usize = 1 << 15;
    let mut head = vec![-1i32; HASH_SIZE];
    let mut prev = vec![-1i32; HASH_SIZE];
    let mut pos = 0usize;
    while pos < raw.len() {
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
            let (code, len) = litlen_code(257 + index as u32);
            bits.huff(code, len);
            bits.put(best_len as u32 - LENGTH_BASE[index], LENGTH_EXTRA[index]);
            let dist = DIST_BASE.partition_point(|&base| base <= best_dist as u32) - 1;
            bits.huff(dist as u32, 5);
            bits.put(best_dist as u32 - DIST_BASE[dist], DIST_EXTRA[dist]);
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
            let (code, len) = litlen_code(u32::from(raw[pos]));
            bits.huff(code, len);
            pos += 1;
        }
    }
    let (code, len) = litlen_code(256); // end of block
    bits.huff(code, len);
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
    chunk(&mut png, b"IDAT", &deflate_fixed(rows));
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

    /// Test-only inflate for the fixed-Huffman blocks `deflate_fixed`
    /// emits (nothing else): proves the stream decodes byte-identical.
    fn inflate_fixed(stream: &[u8]) -> Vec<u8> {
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
        let body_end = stream.len() - 4;
        let mut reader = Reader { data: &stream[2..body_end], acc: 0, nbits: 0, pos: 0 };
        let mut out = Vec::new();
        loop {
            let last = reader.bit();
            assert_eq!(reader.bits(2), 1, "only fixed-Huffman blocks");
            loop {
                // Fixed tables decoded the RFC way: each code bit arrives
                // most-significant first, so the running value stays a code
                // prefix (prefix-free: no 8-bit code starts with a 7-bit
                // one). 7-bit codes are 256..279, 8-bit 0..143 and
                // 280..287, 9-bit 144..255.
                let (mut code, mut len) = (0u32, 0u32);
                let symbol = loop {
                    code = (code << 1) | reader.bit();
                    len += 1;
                    if len == 7 && code <= 0x17 {
                        break 256 + code;
                    }
                    if len == 8 {
                        if (0x30..=0xBF).contains(&code) {
                            break code - 0x30;
                        }
                        if (0xC0..=0xC7).contains(&code) {
                            break 280 + code - 0xC0;
                        }
                    }
                    if len == 9 {
                        assert!((0x190..=0x1FF).contains(&code), "bad fixed code {code:#x}");
                        break 144 + code - 0x190;
                    }
                };
                if symbol == 256 {
                    break;
                }
                if symbol < 256 {
                    out.push(symbol as u8);
                    continue;
                }
                const LENGTH_BASE: [u32; 29] = [
                    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83,
                    99, 115, 131, 163, 195, 227, 258,
                ];
                const LENGTH_EXTRA: [u32; 29] = [
                    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5,
                    5, 0,
                ];
                const DIST_BASE: [u32; 30] = [
                    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769,
                    1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
                ];
                const DIST_EXTRA: [u32; 30] = [
                    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11,
                    11, 12, 12, 13, 13,
                ];
                let lit = (symbol - 257) as usize;
                let len = LENGTH_BASE[lit] + reader.bits(LENGTH_EXTRA[lit]);
                let mut code = 0u32; // fixed distance code: 5-bit value, MSB first
                for _ in 0..5 {
                    code = (code << 1) | reader.bit();
                }
                assert!(code < 30, "bad fixed distance {code}");
                let dist = DIST_BASE[code as usize] + reader.bits(DIST_EXTRA[code as usize]);
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
        inflate_fixed(&stream)
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
    fn fixed_blocks_beat_stored_on_flat_rows() {
        // One black 132-px row block, like empty ocean: matches must fire.
        let raw = vec![0u8; 132 * 3];
        let compressed = deflate_fixed(&raw);
        let stored = raw.len() + 5 + 6; // 5-byte block header + zlib wrap
        assert!(compressed.len() < stored / 4, "flat rows compress, len={}", compressed.len());
        assert_eq!(inflate_fixed(&compressed), raw);
    }

    #[test]
    fn fixed_round_trip_mixed_bytes() {
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
        let compressed = deflate_fixed(&raw);
        assert!(compressed.len() < raw.len(), "mixed input still shrinks");
        assert_eq!(inflate_fixed(&compressed), raw);
    }

    #[test]
    fn crc32_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}
