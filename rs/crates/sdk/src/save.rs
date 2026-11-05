//! Offline player saves: the live client never receives quest varps or
//! the bank, but the engine save has them. Positions, stats, inventories
//! and varps are extracted; unknown or corrupt saves return None.

use std::collections::HashMap;

/// What a quest or bank check needs from the save file.
#[derive(Debug, Clone, Default)]
pub struct Save {
    pub x: u16,
    pub z: u16,
    pub level: u8,
    /// skill -> (level, xp)
    pub stats: HashMap<u8, (u8, i32)>,
    pub varps: HashMap<u16, i32>,
    /// obj_id -> count
    pub inv: HashMap<u16, u32>,
    pub worn: HashMap<u16, u32>,
    pub bank: HashMap<u16, u32>,
}

pub fn default_save_dir() -> String {
    if let Ok(dir) = std::env::var("LIFESCAPE_SAVE_DIR") {
        return dir;
    }
    let engine = std::env::var("LIFESCAPE_ENGINE_DIR").unwrap_or_else(|_| "../lostcity/engine".to_string());
    format!("{engine}/data/players/main")
}

pub fn account_path(account: &str, save_dir: Option<&str>) -> String {
    let dir = save_dir.map(str::to_string).unwrap_or_else(default_save_dir);
    format!("{dir}/{}.sav", account.to_lowercase())
}

pub fn load_account(account: &str, save_dir: Option<&str>) -> Option<Save> {
    parse_file(&account_path(account, save_dir))
}

struct Cursor<'data> {
    data: &'data [u8],
    pos: usize,
}

impl<'data> Cursor<'data> {
    fn take(&mut self, length: usize) -> Option<&'data [u8]> {
        let chunk = self.data.get(self.pos..self.pos + length)?;
        self.pos += length;
        Some(chunk)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|chunk| chunk[0])
    }

    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
    }

    fn i32(&mut self) -> Option<i32> {
        self.take(4).map(|chunk| i32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
    }

    fn varint(&mut self) -> Option<i32> {
        let mut value: i32 = 0;
        loop {
            let byte = self.u8()?;
            value = (value << 7) | i32::from(byte & 0x7F);
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
    }
}

fn inventory_category(size: u16) -> Option<&'static str> {
    match size {
        28 => Some("inv"),
        14 => Some("worn"),
        100.. => Some("bank"),
        _ => None,
    }
}

pub fn parse_file(path: &str) -> Option<Save> {
    parse(&std::fs::read(path).ok()?)
}

pub fn parse(data: &[u8]) -> Option<Save> {
    if data.len() < 24 {
        return None;
    }
    let mut reader = Cursor { data, pos: 0 };
    if reader.u16()? != 0x2004 {
        return None;
    }
    let version = reader.u16()?;
    if version > 7 {
        return None;
    }
    reader.pos = 4;
    let x = reader.u16()?;
    let z = reader.u16()?;
    let level = reader.u8()?;
    reader.take(7)?; // body
    reader.take(5)?; // colours
    reader.u8()?; // gender
    reader.u16()?; // run energy
    if version >= 2 {
        reader.i32()?; // playtime
    } else {
        reader.u16()?;
    }
    let mut stats = HashMap::new();
    for index in 0..21u8 {
        let xp = reader.i32()?;
        stats.insert(index, (reader.u8()?, xp));
    }
    let mut varps = HashMap::new();
    for _ in 0..reader.u16()? {
        if version >= 7 {
            varps.insert(reader.u16()?, reader.varint()?);
        } else {
            reader.i32()?; // no ids in older saves: cannot map them
        }
    }
    let mut inv = HashMap::new();
    let mut worn = HashMap::new();
    let mut bank = HashMap::new();
    for _ in 0..reader.u8()? {
        reader.u16()?; // inv type
        let size = if version >= 5 { reader.u16()? } else { 0 };
        let category = inventory_category(size);
        for _ in 0..size {
            let raw = reader.u16()?;
            if raw == 0 {
                continue;
            }
            let obj_id = raw - 1;
            let mut count = u32::from(reader.u8()?);
            if count == 255 {
                count = reader.i32()? as u32;
            }
            if category.is_some_and(|_| count > 0) {
                let bucket = match category {
                    Some("inv") => &mut inv,
                    Some("worn") => &mut worn,
                    _ => &mut bank,
                };
                { let total = bucket.entry(obj_id).or_insert(0u32); *total = total.saturating_add(count); }
            }
        }
    }
    Some(Save { x, z, level, stats, varps, inv, worn, bank })
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn corrupt_saves_return_none() {
        assert!(parse(&[]).is_none());
        assert!(parse(&[0u8; 24]).is_none());
    }
}
