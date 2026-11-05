//! Socket, login handshake and framed opcodes for revision 274.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::buffer::Writer;
use crate::error::{Error, Result};
use crate::isaac::Isaac;
use crate::protocol::{client_op, server_op};

// ---------------------------------------------------------------------------
// Small std-only helpers: base64, DER, big-int modpow, crc32
// ---------------------------------------------------------------------------

fn base64_decode(text: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut quad: u32 = 0;
    let mut count = 0;
    for ch in text.chars() {
        if ch.is_ascii_whitespace() {
            continue;
        }
        if ch == '=' {
            break;
        }
        let digit = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
            .find(ch)
            .ok_or_else(|| Error::Login("bad base64 in key file".to_string()))? as u32;
        quad = (quad << 6) | digit;
        count += 1;
        if count == 4 {
            out.push((quad >> 16) as u8);
            out.push((quad >> 8) as u8);
            out.push(quad as u8);
            quad = 0;
            count = 0;
        }
    }
    if count == 2 {
        out.push((quad >> 4) as u8);
    } else if count == 3 {
        out.push((quad >> 10) as u8);
        out.push((quad >> 2) as u8);
    }
    Ok(out)
}

/// Read one DER TLV at `pos`: (tag, contents, next pos).
fn read_tlv(der: &[u8], pos: usize) -> Result<(u8, Vec<u8>, usize)> {
    if pos + 2 > der.len() {
        return Err(Error::Login("truncated key DER".to_string()));
    }
    let tag = der[pos];
    let mut length = usize::from(der[pos + 1]);
    let mut next = pos + 2;
    if length & 0x80 != 0 {
        let count = length & 0x7F;
        if count == 0 || count > 4 || next + count > der.len() {
            return Err(Error::Login("bad DER length".to_string()));
        }
        length = 0;
        for byte in &der[next..next + count] {
            length = (length << 8) | usize::from(*byte);
        }
        next += count;
    }
    if next + length > der.len() {
        return Err(Error::Login("truncated key value".to_string()));
    }
    Ok((tag, der[next..next + length].to_vec(), next + length))
}

/// Little-endian u32 limbs, just enough for a 512-bit RSA modpow.
#[derive(Debug, Clone)]
struct Big {
    limbs: Vec<u32>,
}

impl Big {
    fn zero() -> Big {
        Big { limbs: Vec::new() }
    }

    fn from_be(bytes: &[u8]) -> Big {
        let mut padded = bytes.to_vec();
        while padded.len() % 4 != 0 {
            padded.insert(0, 0);
        }
        let mut limbs: Vec<u32> = padded
            .chunks(4)
            .map(|chunk| u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();
        limbs.reverse();
        let mut big = Big { limbs };
        big.trim();
        big
    }

    fn trim(&mut self) {
        while self.limbs.last() == Some(&0) {
            self.limbs.pop();
        }
    }

    fn is_zero(&self) -> bool {
        self.limbs.is_empty()
    }

    fn bit_len(&self) -> usize {
        match self.limbs.last() {
            None => 0,
            Some(top) => (self.limbs.len() - 1) * 32 + (32 - top.leading_zeros() as usize),
        }
    }

    fn cmp(&self, other: &Big) -> std::cmp::Ordering {
        if self.limbs.len() != other.limbs.len() {
            return self.limbs.len().cmp(&other.limbs.len());
        }
        for (left, right) in self.limbs.iter().rev().zip(other.limbs.iter().rev()) {
            if left != right {
                return left.cmp(right);
            }
        }
        std::cmp::Ordering::Equal
    }

    fn shl_one(&mut self) {
        let mut carry = 0u32;
        for limb in self.limbs.iter_mut() {
            let next = *limb >> 31;
            *limb = (*limb << 1) | carry;
            carry = next;
        }
        if carry != 0 {
            self.limbs.push(carry);
        }
    }

    fn sub_from(&mut self, other: &Big) {
        // Assumes self >= other.
        let mut borrow: i64 = 0;
        for index in 0..self.limbs.len() {
            let sub = i64::from(*other.limbs.get(index).unwrap_or(&0)) + borrow;
            let mine = i64::from(self.limbs[index]) - sub;
            if mine < 0 {
                self.limbs[index] = (mine + (1 << 32)) as u32;
                borrow = 1;
            } else {
                self.limbs[index] = mine as u32;
                borrow = 0;
            }
        }
        self.trim();
    }

    fn mul(&self, other: &Big) -> Big {
        if self.is_zero() || other.is_zero() {
            return Big::zero();
        }
        let mut limbs = vec![0u32; self.limbs.len() + other.limbs.len()];
        for (i, left) in self.limbs.iter().enumerate() {
            let mut carry: u64 = 0;
            for (j, right) in other.limbs.iter().enumerate() {
                let at = i + j;
                let sum = u64::from(limbs[at]) + u64::from(*left) * u64::from(*right) + carry;
                limbs[at] = sum as u32;
                carry = sum >> 32;
            }
            let mut at = i + other.limbs.len();
            while carry != 0 {
                let sum = u64::from(limbs[at]) + carry;
                limbs[at] = sum as u32;
                carry = sum >> 32;
                at += 1;
            }
        }
        let mut out = Big { limbs };
        out.trim();
        out
    }

    /// Remainder by shift-subtract long division.
    fn rem(&self, modulus: &Big) -> Big {
        if modulus.is_zero() || self.cmp(modulus) == std::cmp::Ordering::Less {
            let mut out = Big { limbs: self.limbs.clone() };
            out.trim();
            return out;
        }
        let mut remainder = Big::zero();
        for bit in (0..self.bit_len()).rev() {
            remainder.shl_one();
            let limb = bit / 32;
            if limb < self.limbs.len() && (self.limbs[limb] >> (bit % 32)) & 1 == 1 {
                if remainder.limbs.is_empty() {
                    remainder.limbs.push(1);
                } else {
                    remainder.limbs[0] |= 1;
                }
            }
            if remainder.cmp(modulus) != std::cmp::Ordering::Less {
                remainder.sub_from(modulus);
            }
        }
        remainder
    }

    fn modpow(&self, exp: &Big, modulus: &Big) -> Big {
        let mut result = Big { limbs: vec![1] };
        let base = self.rem(modulus);
        for bit in (0..exp.bit_len()).rev() {
            result = result.mul(&result).rem(modulus);
            let limb = bit / 32;
            if limb < exp.limbs.len() && (exp.limbs[limb] >> (bit % 32)) & 1 == 1 {
                result = result.mul(&base).rem(modulus);
            }
        }
        result
    }

    fn to_be_sized(&self, size: usize) -> Vec<u8> {
        let mut out = vec![0u8; size];
        for (index, limb) in self.limbs.iter().enumerate() {
            let bytes = limb.to_be_bytes();
            let end = size.saturating_sub(index * 4);
            let start = end.saturating_sub(4);
            if start >= size {
                break;
            }
            let width = end - start;
            out[start..end].copy_from_slice(&bytes[4 - width..]);
        }
        out
    }
}

fn crc32(data: &[u8]) -> u32 {
    // Bit-by-bit: login CRCs run once per session, tables cost more code.
    let mut crc: u32 = 0xFFFF_FFFF;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            if crc & 1 == 1 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

// ---------------------------------------------------------------------------
// Packed cache reads (mirrors lifescape/cache.py)
// ---------------------------------------------------------------------------

const SECTOR_SIZE: usize = 520;

fn index_path(pack_dir: &str, archive: u32) -> String {
    format!("{pack_dir}/main_file_cache.idx{archive}")
}

fn dat_path(pack_dir: &str) -> String {
    format!("{pack_dir}/main_file_cache.dat")
}

fn read_at(path: &str, pos: u64, length: usize) -> Result<Vec<u8>> {
    use std::io::Seek;
    let mut handle = std::fs::File::open(path).map_err(|err| Error::Io(format!("cannot open {path}: {err}")))?;
    handle.seek(std::io::SeekFrom::Start(pos)).map_err(|err| Error::Io(format!("cannot seek {path}: {err}")))?;
    // Plain reads, like the Python side: a short tail at EOF is data, not
    // an error (the last sector of a file is usually partial).
    let mut out = Vec::new();
    let mut chunk = [0u8; 4096];
    while out.len() < length {
        let want = (length - out.len()).min(chunk.len());
        match handle.read(&mut chunk[..want]) {
            Ok(0) => break,
            Ok(got) => out.extend_from_slice(&chunk[..got]),
            Err(err) => return Err(Error::Io(format!("cannot read {path}: {err}"))),
        }
    }
    Ok(out)
}

/// Raw archive file bytes, exactly as stored (what the login CRC hashes).
pub fn cache_read_file(pack_dir: &str, archive: u32, file: u32) -> Option<Vec<u8>> {
    let index = read_at(&index_path(pack_dir, archive), u64::from(file) * 6, 6).ok()?;
    let size = (u32::from(index[0]) << 16) | (u32::from(index[1]) << 8) | u32::from(index[2]);
    let mut sector = (u32::from(index[3]) << 16) | (u32::from(index[4]) << 8) | u32::from(index[5]);
    if size > 2_000_000 || sector == 0 {
        return None;
    }
    let mut out = Vec::new();
    let mut part = 0u32;
    while (out.len() as u32) < size {
        let header = read_at(&dat_path(pack_dir), u64::from(sector) * SECTOR_SIZE as u64, SECTOR_SIZE).ok()?;
        if header.len() < 8 {
            return None;
        }
        let entry_file = (u16::from(header[0]) << 8) | u16::from(header[1]);
        let entry_part = (u16::from(header[2]) << 8) | u16::from(header[3]);
        let entry_next = (u32::from(header[4]) << 16) | (u32::from(header[5]) << 8) | u32::from(header[6]);
        let entry_archive = u32::from(header[7]).wrapping_sub(1);
        if u32::from(entry_file) != file || entry_part != part as u16 || entry_archive != archive {
            return None;
        }
        let want = ((size - out.len() as u32).min(512) as usize).min(header.len() - 8);
        out.extend_from_slice(&header[8..8 + want]);
        sector = entry_next;
        part += 1;
        if sector == 0 {
            break;
        }
    }
    Some(out)
}

/// 9 CRCs over archive 0 (what the login block carries).
pub fn login_crcs(pack_dir: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(36);
    for file in 0..9 {
        let crc = cache_read_file(pack_dir, 0, file).map_or(0, |blob| crc32(&blob));
        out.extend_from_slice(&crc.to_be_bytes());
    }
    out
}

// ---------------------------------------------------------------------------
// RSA (raw, no padding: cipher = plain^e mod n, like the Python side)
// ---------------------------------------------------------------------------

/// Game key: raw modular exponentiation, stdlib only.
#[derive(Debug, Clone)]
pub struct RsaKey {
    modulus: Big,
    exponent: Big,
    size: usize,
}

impl RsaKey {
    fn encrypt(&self, plain: &[u8]) -> Result<Vec<u8>> {
        let message = Big::from_be(plain);
        if message.cmp(&self.modulus) != std::cmp::Ordering::Less {
            return Err(Error::Login("login block longer than key".to_string()));
        }
        let cipher = message.modpow(&self.exponent, &self.modulus);
        let mut bytes = cipher.to_be_sized(self.size);
        if bytes.first().is_some_and(|first| first & 0x80 != 0) {
            let mut prefixed = vec![0u8];
            prefixed.append(&mut bytes);
            bytes = prefixed;
        }
        Ok(bytes)
    }
}

fn load_key_numbers(key_path: &str) -> Result<RsaKey> {
    let text = std::fs::read_to_string(key_path).map_err(|err| Error::Login(format!("cannot read key {key_path}: {err}")))?;
    let body: String = text.lines().filter(|line| !line.starts_with("-----")).collect();
    let der = base64_decode(&body)?;
    let (_, outer, _) = read_tlv(&der, 0)?;
    let mut pos = 0;
    let (_, _, next) = read_tlv(&outer, pos)?;
    pos = next;
    let (_, _, next) = read_tlv(&outer, pos)?;
    pos = next;
    let (_, inner, _) = read_tlv(&outer, pos)?;
    let (_, inner_seq, _) = read_tlv(&inner, 0)?;
    let mut numbers: Vec<Vec<u8>> = Vec::new();
    let mut at = 0;
    while at < inner_seq.len() {
        let (_, value, next) = read_tlv(&inner_seq, at)?;
        numbers.push(value);
        at = next;
    }
    if numbers.len() < 3 {
        return Err(Error::Login("key has no RSA numbers".to_string()));
    }
    let modulus = Big::from_be(&numbers[1]);
    let size = numbers[1].len();
    Ok(RsaKey { modulus, exponent: Big::from_be(&numbers[2]), size })
}

// ---------------------------------------------------------------------------
// Connection
// ---------------------------------------------------------------------------

/// One decoded server frame.
#[derive(Debug, Clone)]
pub struct Frame {
    pub opcode: u8,
    pub name: String,
    pub body: Vec<u8>,
}

/// Login receipt: staff flag and the ISAAC seeds in use.
#[derive(Debug, Clone)]
pub struct LoginInfo {
    pub staff: u8,
    pub flag: u8,
}

/// Socket, login handshake and framed opcodes for revision 274.
pub struct Connection {
    stream: Option<TcpStream>,
    outgoing: Option<Isaac>,
    incoming: Option<Isaac>,
    buffer: Vec<u8>,
    packet_index: u64,
    timeout: Duration,
}

impl Connection {
    pub fn new(timeout: Duration) -> Connection {
        Connection { stream: None, outgoing: None, incoming: None, buffer: Vec::new(), packet_index: 0, timeout }
    }

    pub fn is_open(&self) -> bool {
        self.stream.is_some()
    }

    pub fn connect(&mut self, host: &str, port: u16) -> Result<()> {
        let address = format!("{host}:{port}");
        let stream = TcpStream::connect(&address).map_err(|err| Error::Io(format!("cannot connect to {address}: {err}")))?;
        stream.set_read_timeout(Some(Duration::from_millis(500))).map_err(Error::from)?;
        self.stream = Some(stream);
        self.buffer.clear();
        Ok(())
    }

    pub fn close(&mut self) {
        self.stream = None;
        self.outgoing = None;
        self.incoming = None;
    }

    fn xorshift(state: &mut u64) -> u32 {
        // std-only randomness for the login seeds (not gameplay).
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        (x & 0x7FFF_FFFF) as u32
    }

    pub fn login(&mut self, username: &str, password: &str, pack_dir: &str, key_path: &str, revision: u16) -> Result<LoginInfo> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|span| span.as_nanos() as u64).unwrap_or(0x243F_6A88);
        let mut rng = nanos | 1;
        let seeds = [Self::xorshift(&mut rng) as i32, Self::xorshift(&mut rng) as i32, Self::xorshift(&mut rng) as i32, Self::xorshift(&mut rng) as i32];
        let uid = Self::xorshift(&mut rng) as i32;

        self.send_raw(&[14, 0])?;
        if self.read_exact(8)? != vec![0u8; 8] || self.read_exact(1)? != vec![0u8] {
            return Err(Error::Login("bad handshake reply".to_string()));
        }
        self.read_exact(8)?; // server seed, unused like the Python side

        let mut rsa_plain = Writer::new();
        rsa_plain.u8(10);
        for seed in seeds {
            rsa_plain.i32(seed);
        }
        rsa_plain.i32(uid);
        rsa_plain.jstr(username, 10);
        rsa_plain.jstr(password, 10);

        let key = load_key_numbers(key_path)?;
        let cipher = key.encrypt(&rsa_plain.bytes())?;

        let mut login = Writer::new();
        login.u8(0xFF);
        login.u16(revision);
        login.u8(0);
        login.raw(&login_crcs(pack_dir));
        login.u8(cipher.len() as u8);
        login.raw(&cipher);
        let payload = login.bytes();
        let mut framed = vec![16, payload.len() as u8];
        framed.extend_from_slice(&payload);
        self.send_raw(&framed)?;

        let response = self.read_exact(1)?[0];
        if response != 2 {
            let hint = match response {
                1 => "invalid credentials",
                3 => "invalid login packet",
                5 => "already logged in",
                6 => "client out of date",
                7 => "world full",
                16 => "rate limited",
                _ => "unknown",
            };
            return Err(Error::Login(format!("login refused: {response} ({hint})")));
        }
        let flags = self.read_exact(2)?;
        self.outgoing = Some(Isaac::new(seeds));
        self.incoming = Some(Isaac::new([seeds[0] + 50, seeds[1] + 50, seeds[2] + 50, seeds[3] + 50]));
        Ok(LoginInfo { staff: flags[0], flag: flags[1] })
    }

    /// Send one client op by name: the payload is framed per the opcode
    /// table and the opcode byte ISAAC-shifted, like the 274 client.
    pub fn send(&mut self, name: &str, payload: &[u8]) -> Result<()> {
        let (op_id, size) = client_op(name).ok_or_else(|| Error::Protocol(format!("unknown client op {name}")))?;
        self.send_by_id(op_id, size, payload)
    }

    /// Send by numeric op id (the [`client_op`](crate::protocol::client_op) table).
    pub fn send_by_id(&mut self, op_id: u8, size: i16, payload: &[u8]) -> Result<()> {
        let frame = match size {
            -1 => {
                let mut framed = vec![payload.len() as u8];
                framed.extend_from_slice(payload);
                framed
            }
            -2 => {
                let mut framed = (payload.len() as u16).to_be_bytes().to_vec();
                framed.extend_from_slice(payload);
                framed
            }
            fixed => {
                if payload.len() != fixed as usize {
                    return Err(Error::Protocol(format!("op {op_id} needs {fixed} bytes, got {}", payload.len())));
                }
                payload.to_vec()
            }
        };
        let shift = self.outgoing.as_mut().ok_or_else(|| Error::Login("not logged in".to_string()))?.next_int();
        let mut packet = vec![(u32::from(op_id).wrapping_add(shift as u32) & 0xFF) as u8];
        packet.extend_from_slice(&frame);
        self.send_raw(&packet)
    }

    /// Decode the next server frame (opcode ISAAC-shifted, then sized body).
    pub fn recv(&mut self) -> Result<Frame> {
        let raw = self.read_exact(1)?[0];
        let shift = self.incoming.as_mut().ok_or_else(|| Error::Login("not logged in".to_string()))?.next_int();
        let opcode = raw.wrapping_sub(shift as u8);
        self.packet_index += 1;
        let (name, size) = server_op(opcode).ok_or_else(|| Error::Protocol(format!("unknown server opcode {opcode}")))?;
        let length = match size {
            -1 => usize::from(self.read_exact(1)?[0]),
            -2 => {
                let bytes = self.read_exact(2)?;
                (usize::from(bytes[0]) << 8) | usize::from(bytes[1])
            }
            fixed => fixed as usize,
        };
        let body = if length > 0 { self.read_exact(length)? } else { Vec::new() };
        Ok(Frame { opcode, name: name.to_string(), body })
    }

    /// True when the server already sent more than we read. Non-blocking
    /// on purpose: a client that waits here falls behind the tick.
    pub fn available(&mut self) -> Result<bool> {
        if !self.buffer.is_empty() {
            return Ok(true);
        }
        let stream = self.stream.as_mut().ok_or_else(|| Error::Io("no socket".to_string()))?;
        stream.set_nonblocking(true).map_err(Error::from)?;
        let mut chunk = [0u8; 65536];
        let outcome = stream.read(&mut chunk);
        let _ = stream.set_nonblocking(false);
        match outcome {
            Ok(0) => Err(Error::Io("server closed the socket".to_string())),
            Ok(got) => {
                self.buffer.extend_from_slice(&chunk[..got]);
                Ok(true)
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => Ok(false),
            Err(err) => Err(Error::from(err)),
        }
    }

    fn send_raw(&mut self, data: &[u8]) -> Result<()> {
        let stream = self.stream.as_mut().ok_or_else(|| Error::Io("no socket".to_string()))?;
        stream.write_all(data).map_err(Error::from)
    }

    fn read_exact(&mut self, length: usize) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(length);
        let deadline = Instant::now() + self.timeout;
        while out.len() < length {
            if !self.buffer.is_empty() {
                let take = (length - out.len()).min(self.buffer.len());
                out.extend_from_slice(&self.buffer[..take]);
                self.buffer.drain(..take);
                continue;
            }
            let stream = self.stream.as_mut().ok_or_else(|| Error::Io("no socket".to_string()))?;
            let mut chunk = vec![0u8; length - out.len()];
            match stream.read(&mut chunk) {
                Ok(0) => return Err(Error::Io("server closed the socket".to_string())),
                Ok(got) => out.extend_from_slice(&chunk[..got]),
                Err(err) if err.kind() == std::io::ErrorKind::TimedOut || err.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() > deadline {
                        return Err(Error::Timeout(format!("read timed out after {}s", self.timeout.as_secs())));
                    }
                }
                Err(err) => return Err(Error::from(err)),
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::{Big, base64_decode, crc32};

    #[test]
    fn modpow_matches_u64() {
        let base = Big::from_be(&[0x11, 0x22]);
        let exp = Big::from_be(&[0x11]);
        let modulus = Big::from_be(&[0xFF, 0xFF, 0xFF]);
        let got = u64::from_be_bytes({ let mut b = [0u8; 8]; let raw = base.modpow(&exp, &modulus).to_be_sized(8); b.copy_from_slice(&raw); b });
        // pow(0x1122, 0x11, 0xFFFFFF) == 0x75C5F7 (reference). U128 also overflows here.
        assert_eq!(got, 0x75C5F7);
    }

    #[test]
    fn crc32_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn base64_pem_shape() {
        assert_eq!(base64_decode("TWFu").unwrap(), b"Man");
    }
}
