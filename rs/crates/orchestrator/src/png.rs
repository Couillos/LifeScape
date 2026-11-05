#![forbid(unsafe_code)]
//! Minimal PNG encoder, std only: 8-bit truecolor (RGB) and truecolor
//! with alpha (RGBA), one IDAT with uncompressed deflate blocks.
//!
//! Stored blocks keep the code small (no LZ77/Huffman tables) at the cost
//! of larger files; the spectator images are small enough that this does
//! not matter. CRC32 (chunk checks) and Adler-32 (deflate stream check)
//! are computed over the same bytes a decoder verifies.

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

/// Raw scanlines (filter byte 0 + pixel bytes per row) wrapped in stored
/// deflate blocks, then in a zlib stream: the only compression PNG allows
/// without any code tables on our side.
fn deflate_stored(raw: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65535);
    // At least one block even for empty input, like zlib for zero bytes.
    let first = blocks.next().unwrap_or(&[]);
    let mut rest = vec![first];
    rest.extend(blocks);
    let total = rest.len();
    for (index, block) in rest.iter().enumerate() {
        let last = index + 1 == total;
        out.push(u8::from(last));
        let length = block.len() as u16;
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&(!length).to_le_bytes());
        out.extend_from_slice(block);
    }
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
    chunk(&mut png, b"IDAT", &deflate_stored(rows));
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
        // Stored deflate: filter 0 + 8 bytes per row inside one block.
        let idat: Vec<&Vec<u8>> = parts.iter().filter(|(kind, _)| kind == b"IDAT").map(|(_, body)| body).collect();
        let stream: Vec<u8> = idat.into_iter().flatten().copied().collect();
        assert_eq!(&stream[..2], &[0x78, 0x01]);
        assert_eq!(stream[2], 0x01); // single final stored block
        let length = u16::from_le_bytes([stream[3], stream[4]]) as usize;
        assert_eq!(length, 2 * (1 + 2 * 4));
        assert_eq!(&stream[5..7], &(!length as u16).to_le_bytes());
        assert_eq!(stream[7], 0); // filter byte, first row
    }

    #[test]
    fn rgb_header_round_trip() {
        let png = encode_rgb(1, 1, &[1, 2, 3]);
        let parts = chunks(&png);
        let header = &parts[0].1;
        assert_eq!(u32::from_be_bytes(header[0..4].try_into().unwrap()), 1);
        assert_eq!(u32::from_be_bytes(header[4..8].try_into().unwrap()), 1);
        assert_eq!((header[8], header[9]), (8, 2));
    }

    #[test]
    fn crc32_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}
