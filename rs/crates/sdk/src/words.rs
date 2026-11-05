//! 4-bit chat packing and base-37 names, mirroring the server exactly.

const LOOKUP: &[&str] = &[
    " ", "e", "t", "a", "o", "i", "h", "n", "s", "r", "d", "l", "u", "m", "w", "c", "y", "f", "g", "p", "b", "v",
    "k", "x", "j", "q", "z", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", " ", "!", "?", ".", ",", ":",
    ";", "(", ")", "-", "&", "*", "\\", "'", "@", "#", "+", "=", "£", "$", "%", "\"", "[", "]",
];

fn lookup_index(ch: char) -> usize {
    let needle = ch.to_string();
    for (index, candidate) in LOOKUP.iter().enumerate() {
        if *candidate == needle {
            return index;
        }
    }
    0
}

/// Pack chat text the way the 274 client sends it (max 80 chars).
pub fn pack(text: &str) -> Vec<u8> {
    let lowered = text.to_lowercase();
    let clipped: String = lowered.chars().take(80).collect();
    let mut out = Vec::new();
    let mut carry: i32 = -1;
    for ch in clipped.chars() {
        let mut index = lookup_index(ch) as i32;
        if index > 12 {
            index += 195;
        }
        if carry == -1 {
            if index < 13 {
                carry = index;
            } else {
                out.push(index as u8);
            }
        } else if index < 13 {
            out.push(((carry << 4) + index) as u8);
            carry = -1;
        } else {
            out.push(((carry << 4) + (index >> 4)) as u8);
            carry = index & 0xF;
        }
    }
    if carry != -1 {
        out.push((carry << 4) as u8);
    }
    out
}

/// Unpack server chat bytes to display text.
pub fn unpack(packed: &[u8]) -> String {
    let mut chars: Vec<char> = Vec::new();
    let mut carry: i32 = -1;
    for byte in packed {
        for nibble in [u32::from(byte >> 4), u32::from(byte & 0xF)] {
            if carry != -1 {
                let index = (carry as u32) * 16 + nibble - 195;
                if let Some(glyph) = LOOKUP.get(index as usize) {
                    chars.extend(glyph.chars());
                }
                carry = -1;
            } else if nibble < 13 {
                if let Some(glyph) = LOOKUP.get(nibble as usize) {
                    chars.extend(glyph.chars());
                }
            } else {
                carry = nibble as i32;
            }
        }
    }
    sentence_case(&chars.iter().collect::<String>())
}

fn sentence_case(text: &str) -> String {
    let mut chars: Vec<char> = text.to_lowercase().chars().collect();
    let mut punctuation = true;
    for ch in chars.iter_mut() {
        if punctuation && ch.is_ascii_lowercase() {
            *ch = ch.to_ascii_uppercase();
            punctuation = false;
        }
        if *ch == '.' || *ch == '!' {
            punctuation = true;
        }
    }
    chars.iter().collect()
}

const CHARSET: &[u8] = b"_abcdefghijklmnopqrstuvwxyz0123456789";

/// Encode a player name the way appearance masks and friend lists do.
pub fn to_base37(name: &str) -> u64 {
    let mut value: u64 = 0;
    for ch in name.to_lowercase().chars().take(12) {
        if let Some(index) = CHARSET.iter().position(|byte| *byte as char == ch) {
            value = value * 37 + index as u64;
        }
    }
    value
}

/// Decode a base-37 name back to text.
pub fn base37_name(mut value: u64) -> String {
    let mut name = String::new();
    while value > 0 {
        let digit = (value % 37) as usize;
        value /= 37;
        name.insert(0, CHARSET[digit] as char);
    }
    name
}

#[cfg(test)]
mod tests {
    use super::{base37_name, pack, to_base37, unpack};

    #[test]
    fn chat_round_trip() {
        let packed = pack("hello lifescape");
        assert_eq!(unpack(&packed), "Hello lifescape");
    }

    #[test]
    fn hello_world_matches_reference() {
        // Bytes captured from lifescape.words.pack("Hello, world!").
        let packed = pack("Hello, world!");
        assert_eq!(packed, [97, 187, 78, 192, 209, 73, 186, 233]);
        assert_eq!(unpack(&packed), "Hello, world!");
    }

    #[test]
    fn base37_round_trip() {
        assert_eq!(base37_name(to_base37("olivia_king")), "olivia_king");
    }
}
