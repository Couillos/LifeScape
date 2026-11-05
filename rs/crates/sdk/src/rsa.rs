//! Raw RSA with the game public key, no padding, stdlib only.
//!
//! Ports `lifescape/rsa.py`: the login block is encrypted as one big
//! integer (`ciphertext = plain^e mod n`), sent as a length byte followed
//! by the big-endian bytes. Big integers are little-endian `Vec<u32>`
//! limbs; division is binary long division, fast enough for login keys.

use std::cmp::Ordering;

/// Unsigned big integer, little-endian 32-bit limbs, no leading zero limbs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BigUint {
    limbs: Vec<u32>,
}

impl BigUint {
    fn zero() -> Self {
        Self { limbs: Vec::new() }
    }

    fn one() -> Self {
        Self { limbs: vec![1] }
    }

    fn is_zero(&self) -> bool {
        self.limbs.is_empty()
    }

    fn normalize(&mut self) {
        while self.limbs.last() == Some(&0) {
            self.limbs.pop();
        }
    }

    fn from_be_bytes(bytes: &[u8]) -> Self {
        let mut limbs = Vec::new();
        for chunk in bytes.rchunks(4) {
            let mut word = [0u8; 4];
            word[4 - chunk.len()..].copy_from_slice(chunk);
            limbs.push(u32::from_be_bytes(word));
        }
        let mut value = Self { limbs };
        value.normalize();
        value
    }

    fn to_be_bytes_sized(&self, size: usize) -> Vec<u8> {
        let mut out = vec![0u8; size];
        for (index, limb) in self.limbs.iter().enumerate() {
            let bytes = limb.to_be_bytes();
            let end = size - index * 4;
            let start = end.saturating_sub(4);
            let width = end - start;
            out[start..end].copy_from_slice(&bytes[4 - width..]);
        }
        out
    }

    fn bit_len(&self) -> usize {
        match self.limbs.last() {
            None => 0,
            Some(top) => (self.limbs.len() - 1) * 32 + (32 - top.leading_zeros() as usize),
        }
    }

    fn cmp_limbs(&self, other: &Self) -> Ordering {
        if self.limbs.len() != other.limbs.len() {
            return self.limbs.len().cmp(&other.limbs.len());
        }
        for (mine, theirs) in self.limbs.iter().rev().zip(other.limbs.iter().rev()) {
            if mine != theirs {
                return mine.cmp(theirs);
            }
        }
        Ordering::Equal
    }

    /// `self -= other`; caller guarantees `self >= other`.
    fn sub_assign(&mut self, other: &Self) {
        let mut borrow: i64 = 0;
        for index in 0..self.limbs.len() {
            let subtrahend = i64::from(*other.limbs.get(index).unwrap_or(&0)) + borrow;
            let remainder = i64::from(self.limbs[index]) - subtrahend;
            if remainder < 0 {
                self.limbs[index] = (remainder + (1i64 << 32)) as u32;
                borrow = 1;
            } else {
                self.limbs[index] = remainder as u32;
                borrow = 0;
            }
        }
        debug_assert_eq!(borrow, 0);
        self.normalize();
    }

    fn shl(&self, bits: usize) -> Self {
        if self.is_zero() {
            return Self::zero();
        }
        let word_shift = bits / 32;
        let bit_shift = (bits % 32) as u32;
        let mut limbs = vec![0u32; self.limbs.len() + word_shift + 1];
        if bit_shift == 0 {
            limbs[word_shift..word_shift + self.limbs.len()].copy_from_slice(&self.limbs);
        } else {
            let mut carry = 0u32;
            for (index, limb) in self.limbs.iter().enumerate() {
                let low = (limb << bit_shift) | carry;
                limbs[word_shift + index] = low;
                carry = limb >> (32 - bit_shift);
            }
            limbs[word_shift + self.limbs.len()] = carry;
        }
        let mut value = Self { limbs };
        value.normalize();
        value
    }

    fn shr1(&mut self) {
        let mut carry = 0u32;
        for limb in self.limbs.iter_mut().rev() {
            let low = (*limb & 1) << 31;
            *limb = (*limb >> 1) | carry;
            carry = low;
        }
        self.normalize();
    }

    fn set_bit(&mut self, bit: usize) {
        let word = bit / 32;
        if self.limbs.len() <= word {
            self.limbs.resize(word + 1, 0);
        }
        self.limbs[word] |= 1 << (bit % 32);
    }

    fn mul(&self, other: &Self) -> Self {
        if self.is_zero() || other.is_zero() {
            return Self::zero();
        }
        let mut limbs = vec![0u32; self.limbs.len() + other.limbs.len()];
        for (outer, mine) in self.limbs.iter().enumerate() {
            let mut carry = 0u64;
            for (inner, theirs) in other.limbs.iter().enumerate() {
                let slot = outer + inner;
                let total = u64::from(limbs[slot]) + u64::from(*mine) * u64::from(*theirs) + carry;
                limbs[slot] = total as u32;
                carry = total >> 32;
            }
            let mut slot = outer + other.limbs.len();
            while carry > 0 {
                let total = u64::from(limbs[slot]) + carry;
                limbs[slot] = total as u32;
                carry = total >> 32;
                slot += 1;
            }
        }
        let mut value = Self { limbs };
        value.normalize();
        value
    }

    /// Long division, binary shift-and-subtract.
    fn divmod(&self, divisor: &Self) -> (Self, Self) {
        assert!(!divisor.is_zero(), "division by zero");
        if self.cmp_limbs(divisor) == Ordering::Less {
            return (Self::zero(), self.clone());
        }
        let mut remainder = self.clone();
        let mut shifted = divisor.shl(self.bit_len() - divisor.bit_len());
        let mut quotient = Self::zero();
        for bit in (0..=self.bit_len() - divisor.bit_len()).rev() {
            if remainder.cmp_limbs(&shifted) != Ordering::Less {
                remainder.sub_assign(&shifted);
                quotient.set_bit(bit);
            }
            shifted.shr1();
        }
        (quotient, remainder)
    }

    fn powmod(&self, exponent: &Self, modulus: &Self) -> Self {
        if *modulus == Self::one() {
            return Self::zero();
        }
        let (_, mut base) = self.divmod(modulus);
        let mut result = Self::one();
        for word in &exponent.limbs {
            for bit in 0..32 {
                if (word >> bit) & 1 == 1 {
                    let product = result.mul(&base);
                    result = product.divmod(modulus).1;
                }
                let squared = base.mul(&base);
                base = squared.divmod(modulus).1;
            }
        }
        result
    }
}

/// RSA public (or modulus+exponent) key.
#[derive(Debug, Clone)]
pub struct RsaKey {
    modulus: BigUint,
    exponent: BigUint,
    /// Encrypted block size in bytes: `(modulus.bit_length() + 7) // 8`.
    pub size: usize,
}

impl RsaKey {
    pub fn new(modulus_be: &[u8], exponent_be: &[u8]) -> Self {
        let modulus = BigUint::from_be_bytes(modulus_be);
        let size = (modulus.bit_len() + 7) / 8;
        Self {
            modulus,
            exponent: BigUint::from_be_bytes(exponent_be),
            size,
        }
    }

    /// Raw `plain^e mod n`, with a leading zero byte when the top bit is
    /// set, exactly like the Python `encrypt`.
    pub fn encrypt(&self, plain: &[u8]) -> Result<Vec<u8>, String> {
        let raw = BigUint::from_be_bytes(plain);
        if raw.cmp_limbs(&self.modulus) != Ordering::Less {
            return Err("login block longer than key".to_string());
        }
        let cipher = raw.powmod(&self.exponent, &self.modulus);
        let mut out = cipher.to_be_bytes_sized(self.size);
        if !out.is_empty() && out[0] & 0x80 != 0 {
            let mut prefixed = Vec::with_capacity(out.len() + 1);
            prefixed.push(0);
            prefixed.append(&mut out);
            return Ok(prefixed);
        }
        Ok(out)
    }
}

/// Loads a PEM `PUBLIC KEY` (SPKI) file.
pub fn load_public_key(pem: &str) -> Result<RsaKey, String> {
    let der = pem_body(pem)?;
    let (modulus, exponent) = parse_spki(&der)?;
    Ok(RsaKey::new(&modulus, &exponent))
}

/// Loads modulus+exponent from a PEM private key (PKCS#8), mirroring
/// Python `load_private_numbers` (`numbers[1]`, `numbers[2]`).
pub fn load_private_numbers(pem: &str) -> Result<RsaKey, String> {
    let der = pem_body(pem)?;
    let (_, outer, _) = read_tlv(&der, 0)?;
    let mut pos = 0;
    let (_, _, next) = read_tlv(&outer, pos)?;
    pos = next;
    let (_, _, next) = read_tlv(&outer, pos)?;
    pos = next;
    let (_, inner, _) = read_tlv(&outer, pos)?;
    let (_, inner_seq, _) = read_tlv(&inner, 0)?;
    let mut numbers: Vec<Vec<u8>> = Vec::new();
    let mut int_pos = 0;
    while int_pos < inner_seq.len() {
        let (_, value, next) = read_tlv(&inner_seq, int_pos)?;
        numbers.push(value);
        int_pos = next;
    }
    if numbers.len() < 3 {
        return Err("private key has too few integers".to_string());
    }
    Ok(RsaKey::new(&numbers[1], &numbers[2]))
}

fn pem_body(pem: &str) -> Result<Vec<u8>, String> {
    let mut body = String::new();
    for line in pem.lines() {
        if !line.starts_with("-----") {
            body.push_str(line.trim());
        }
    }
    base64_decode(&body)
}

fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut buffer: u32 = 0;
    let mut collected = 0;
    for symbol in text.chars() {
        if symbol == '=' {
            break;
        }
        let digit = match symbol {
            'A'..='Z' => symbol as u32 - 'A' as u32,
            'a'..='z' => symbol as u32 - 'a' as u32 + 26,
            '0'..='9' => symbol as u32 - '0' as u32 + 52,
            '+' => 62,
            '/' => 63,
            _ => return Err(format!("invalid base64 symbol: {symbol}")),
        };
        buffer = (buffer << 6) | digit;
        collected += 1;
        if collected == 4 {
            out.push((buffer >> 16) as u8);
            out.push((buffer >> 8) as u8);
            out.push(buffer as u8);
            buffer = 0;
            collected = 0;
        }
    }
    if collected == 2 {
        out.push((buffer >> 4) as u8);
    } else if collected == 3 {
        out.push((buffer >> 10) as u8);
        out.push((buffer >> 2) as u8);
    } else if collected != 0 {
        return Err("truncated base64 group".to_string());
    }
    Ok(out)
}

/// Reads one DER tag-length-value at `pos`; returns tag, value bytes,
/// and the offset just past the value.
fn read_tlv(der: &[u8], pos: usize) -> Result<(u8, Vec<u8>, usize), String> {
    if pos + 2 > der.len() {
        return Err("DER value runs past the end".to_string());
    }
    let tag = der[pos];
    let mut length = usize::from(der[pos + 1]);
    let mut cursor = pos + 2;
    if length & 0x80 != 0 {
        let count = length & 0x7F;
        if cursor + count > der.len() {
            return Err("DER length runs past the end".to_string());
        }
        length = 0;
        for byte in &der[cursor..cursor + count] {
            length = (length << 8) | usize::from(*byte);
        }
        cursor += count;
    }
    if cursor + length > der.len() {
        return Err("DER value runs past the end".to_string());
    }
    Ok((tag, der[cursor..cursor + length].to_vec(), cursor + length))
}

fn parse_spki(der: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
    if der.first() != Some(&0x30) {
        return Err("not a DER sequence".to_string());
    }
    let (_, outer, _) = read_tlv(der, 0)?;
    let mut pos = 0;
    let (_, _, next) = read_tlv(&outer, pos)?;
    pos = next;
    let (bit_tag, bit_string, _) = read_tlv(&outer, pos)?;
    if bit_tag != 0x03 || bit_string.first() != Some(&0) {
        return Err("not a bit string key".to_string());
    }
    let (_, inner, _) = read_tlv(&bit_string[1..], 0)?;
    let mut int_pos = 0;
    let (_, modulus, next) = read_tlv(&inner, int_pos)?;
    int_pos = next;
    let (_, exponent, _) = read_tlv(&inner, int_pos)?;
    Ok((modulus, exponent))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bigint_mul_div_roundtrip() {
        let big_a = BigUint::from_be_bytes(&[0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0]);
        let big_b = BigUint::from_be_bytes(&[0x11, 0x22, 0x33, 0x44]);
        let product = big_a.mul(&big_b);
        let (quotient, remainder) = product.divmod(&big_b);
        assert_eq!(quotient, big_a);
        assert!(remainder.is_zero());
    }

    #[test]
    fn encrypt_matches_reference_pow() {
        // Python: pow(0x010203, 0x10001, 0x10001 * 0x101) == 0x010203.
        let modulus_value = 0x10001u64 * 0x101u64;
        let modulus_bytes = modulus_value.to_be_bytes();
        let key = RsaKey::new(&modulus_bytes[4..], &[0x01, 0x00, 0x01]);
        assert_eq!(key.size, 4);
        let cipher = key.encrypt(&[0x01, 0x02, 0x03]).expect("encrypt");
        assert_eq!(cipher, [0x00, 0x01, 0x02, 0x03]);
    }

    #[test]
    fn rejects_plaintext_over_modulus() {
        let key = RsaKey::new(&[0x01, 0x00, 0x01], &[0x01, 0x00, 0x01]);
        assert!(key.encrypt(&[0x02, 0x00, 0x02]).is_err());
    }

    #[test]
    fn spki_and_private_pem_parse() {
        // Table-driven DER built by hand: SPKI wrapping modulus 0x010001*0x101.
        let modulus = vec![0x01, 0x01, 0x01, 0x01];
        let exponent = vec![0x01, 0x00, 0x01];
        let inner = tlv(0x30, &[tlv(0x02, &modulus), tlv(0x02, &exponent)].concat());
        let alg = tlv(0x30, &[tlv(0x06, &[0x2A]), tlv(0x05, &[])].concat());
        let spki = tlv(0x30, &[alg, tlv(0x03, &[&[0u8][..], &inner[..]].concat())].concat());
        let pem = format!(
            "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
            base64_encode(&spki)
        );
        let key = load_public_key(&pem).expect("spki");
        assert_eq!(key.size, 4);
        assert!(key.encrypt(&[0x01]).is_ok());
    }

    fn tlv(tag: u8, value: &[u8]) -> Vec<u8> {
        let mut out = vec![tag, value.len() as u8];
        out.extend_from_slice(value);
        out
    }

    fn base64_encode(raw: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut text = String::new();
        for chunk in raw.chunks(3) {
            let mut buffer: u32 = 0;
            for byte in chunk {
                buffer = (buffer << 8) | u32::from(*byte);
            }
            buffer <<= (3 - chunk.len()) * 8;
            for slot in 0..chunk.len() + 1 {
                text.push(ALPHABET[((buffer >> (18 - slot * 6)) & 63) as usize] as char);
            }
            for _ in 0..3 - chunk.len() {
                text.push('=');
            }
        }
        text
    }
}
