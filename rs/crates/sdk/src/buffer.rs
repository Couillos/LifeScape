//! Big-endian packet reader and writer, mirroring the server Packet layout.

/// Read error: truncated or malformed frame body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadError(pub String);

impl std::fmt::Display for ReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "buffer read failed: {}", self.0)
    }
}

impl std::error::Error for ReadError {}

pub type ReadResult<T> = Result<T, ReadError>;

fn need(data_len: usize, pos: usize, length: usize, what: &str) -> Result<(), ReadError> {
    if pos + length > data_len {
        return Err(ReadError(format!("{what} needs {length} bytes at {pos} (len {data_len})")));
    }
    Ok(())
}

/// Big-endian cursor over a frame body.
#[derive(Debug, Clone)]
pub struct Reader {
    data: Vec<u8>,
    pub pos: usize,
}

impl Reader {
    pub fn new(data: &[u8]) -> Reader {
        Reader { data: data.to_vec(), pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn peek_u8(&self) -> ReadResult<u8> {
        need(self.data.len(), self.pos, 1, "peek_u8")?;
        Ok(self.data[self.pos])
    }

    pub fn raw(&mut self, length: usize) -> ReadResult<Vec<u8>> {
        need(self.data.len(), self.pos, length, "raw")?;
        let chunk = self.data[self.pos..self.pos + length].to_vec();
        self.pos += length;
        Ok(chunk)
    }

    pub fn u8(&mut self) -> ReadResult<u8> {
        need(self.data.len(), self.pos, 1, "u8")?;
        let value = self.data[self.pos];
        self.pos += 1;
        Ok(value)
    }

    pub fn u16(&mut self) -> ReadResult<u16> {
        need(self.data.len(), self.pos, 2, "u16")?;
        let value = u16::from_be_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        self.pos += 2;
        Ok(value)
    }

    pub fn i16(&mut self) -> ReadResult<i16> {
        need(self.data.len(), self.pos, 2, "i16")?;
        let value = i16::from_be_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        self.pos += 2;
        Ok(value)
    }

    pub fn u24(&mut self) -> ReadResult<u32> {
        need(self.data.len(), self.pos, 3, "u24")?;
        let value = u32::from_be_bytes([0, self.data[self.pos], self.data[self.pos + 1], self.data[self.pos + 2]]);
        self.pos += 3;
        Ok(value)
    }

    pub fn i32(&mut self) -> ReadResult<i32> {
        need(self.data.len(), self.pos, 4, "i32")?;
        let value = i32::from_be_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
        ]);
        self.pos += 4;
        Ok(value)
    }

    pub fn u32(&mut self) -> ReadResult<u32> {
        need(self.data.len(), self.pos, 4, "u32")?;
        let value = u32::from_be_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
        ]);
        self.pos += 4;
        Ok(value)
    }

    pub fn u64(&mut self) -> ReadResult<u64> {
        need(self.data.len(), self.pos, 8, "u64")?;
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&self.data[self.pos..self.pos + 8]);
        self.pos += 8;
        Ok(u64::from_be_bytes(bytes))
    }

    pub fn boolean(&mut self) -> ReadResult<bool> {
        Ok(self.u8()? == 1)
    }

    /// Latin-1 string up to `terminator` (default 10, like the server).
    pub fn jstr(&mut self, terminator: u8) -> ReadResult<String> {
        let end = self.data[self.pos..]
            .iter()
            .position(|byte| *byte == terminator)
            .ok_or_else(|| ReadError("jstr without terminator".to_string()))?;
        let text: String = self.data[self.pos..self.pos + end].iter().map(|byte| *byte as char).collect();
        self.pos += end + 1;
        Ok(text)
    }

    pub fn varint(&mut self) -> ReadResult<u32> {
        let mut value: u32 = 0;
        loop {
            let byte = self.u8()?;
            value = (value << 7) | u32::from(byte & 0x7F);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
    }

    pub fn smart(&mut self) -> ReadResult<i32> {
        if self.peek_u8()? < 0x80 {
            Ok(i32::from(self.u8()?) - 0x40)
        } else {
            Ok(i32::from(self.u16()?) - 0xC000)
        }
    }

    pub fn smarts(&mut self) -> ReadResult<u16> {
        if self.peek_u8()? < 0x80 {
            self.u8().map(u16::from)
        } else {
            Ok(self.u16()? - 0x8000)
        }
    }
}

/// Big-endian frame builder.
#[derive(Debug, Clone, Default)]
pub struct Writer {
    out: Vec<u8>,
}

impl Writer {
    pub fn new() -> Writer {
        Writer { out: Vec::new() }
    }

    pub fn raw(&mut self, data: &[u8]) {
        self.out.extend_from_slice(data);
    }

    pub fn u8(&mut self, value: u8) {
        self.out.push(value);
    }

    pub fn i8(&mut self, value: i8) {
        self.out.push(value as u8);
    }

    pub fn u16(&mut self, value: u16) {
        self.out.extend_from_slice(&value.to_be_bytes());
    }

    pub fn i16(&mut self, value: i16) {
        self.out.extend_from_slice(&value.to_be_bytes());
    }

    pub fn u24(&mut self, value: u32) {
        self.out.push((value >> 16) as u8);
        self.out.push((value >> 8) as u8);
        self.out.push(value as u8);
    }

    pub fn i32(&mut self, value: i32) {
        self.out.extend_from_slice(&value.to_be_bytes());
    }

    pub fn u32(&mut self, value: u32) {
        self.out.extend_from_slice(&value.to_be_bytes());
    }

    pub fn u64(&mut self, value: u64) {
        self.out.extend_from_slice(&value.to_be_bytes());
    }

    pub fn boolean(&mut self, value: bool) {
        self.u8(u8::from(value));
    }

    /// Latin-1 string plus `terminator` (default 10, like the server).
    pub fn jstr(&mut self, text: &str, terminator: u8) {
        for ch in text.chars() {
            self.out.push(if (ch as u32) < 256 { ch as u8 } else { b'?' });
        }
        self.out.push(terminator);
    }

    pub fn smart(&mut self, value: i32) {
        if (-64..64).contains(&value) {
            self.u8((value + 64) as u8);
        } else {
            self.u16((value + 0xC000) as u16);
        }
    }

    pub fn bytes(&self) -> Vec<u8> {
        self.out.clone()
    }
}

/// Bit cursor over a frame body (player/npc info masks).
#[derive(Debug, Clone)]
pub struct BitReader {
    data: Vec<u8>,
    bit: usize,
}

impl BitReader {
    pub fn new(data: &[u8]) -> BitReader {
        BitReader { data: data.to_vec(), bit: 0 }
    }

    pub fn bits(&mut self, count: usize) -> ReadResult<u32> {
        let mut value: u32 = 0;
        for _ in 0..count {
            let byte = *self.data.get(self.bit >> 3).ok_or_else(|| ReadError("bits past end".to_string()))?;
            value <<= 1;
            value |= u32::from((byte >> (7 - (self.bit & 7))) & 1);
            self.bit += 1;
        }
        Ok(value)
    }

    pub fn remaining_bits(&self) -> usize {
        self.data.len() * 8 - self.bit.min(self.data.len() * 8)
    }

    pub fn bit_pos(&self) -> usize {
        self.bit
    }

    /// Skip the padding bits up to the next byte boundary.
    pub fn align_byte(&mut self) -> ReadResult<()> {
        let pad = (8 - self.bit % 8) % 8;
        if pad > 0 {
            self.bits(pad)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{BitReader, Reader, Writer};

    #[test]
    fn writer_reader_round_trip() {
        let mut writer = Writer::new();
        writer.u8(0xFF);
        writer.u16(274);
        writer.i32(-7);
        writer.jstr("t01", 10);
        let bytes = writer.bytes();
        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.u8().unwrap(), 0xFF);
        assert_eq!(reader.u16().unwrap(), 274);
        assert_eq!(reader.i32().unwrap(), -7);
        assert_eq!(reader.jstr(10).unwrap(), "t01");
        assert_eq!(reader.remaining(), 0);
    }

    #[test]
    fn bit_reader_msb_first() {
        let mut bits = BitReader::new(&[0b1010_0000]);
        assert_eq!(bits.bits(1).unwrap(), 1);
        assert_eq!(bits.bits(3).unwrap(), 0b010);
        assert_eq!(bits.remaining_bits(), 4);
    }
}
