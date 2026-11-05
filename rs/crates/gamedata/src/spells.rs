//! Combat spells: level, runes, XP, max hit, interface component.
//! Mirrors `gamedata/spells.py`.

use crate::{flag, num, opt_num, opt_str, str_int_map, table};

/// One combat spell.
#[derive(Debug, Clone)]
pub struct Spell {
    pub spell: String,
    pub name: Option<String>,
    pub com: Option<String>,
    pub members: bool,
    pub level: i64,
    pub runes: Vec<(String, i64)>,
    pub xp: i64,
    pub maxhit: Option<i64>,
}

/// Every combat spell.
pub fn all_spells() -> Vec<Spell> {
    table("spells")
        .iter()
        .map(|row| Spell {
            spell: opt_str(row, "spell").unwrap_or_default(),
            name: opt_str(row, "name"),
            com: opt_str(row, "com"),
            members: flag(row, "members", false),
            level: num(row, "level", 0),
            runes: str_int_map(row, "runes"),
            xp: num(row, "xp", 0),
            maxhit: opt_num(row, "maxhit"),
        })
        .collect()
}

/// Spells with this debugname (case-insensitive, leading `^` stripped).
pub fn by_name(spell: &str) -> Vec<Spell> {
    let wanted = spell.to_lowercase().trim_start_matches('^').to_string();
    all_spells().into_iter().filter(|entry| entry.spell.to_lowercase() == wanted).collect()
}

/// Spells bound to this interface component (case-insensitive).
pub fn by_com(com: &str) -> Vec<Spell> {
    let wanted = com.to_lowercase();
    all_spells()
        .into_iter()
        .filter(|entry| entry.com.as_deref().unwrap_or("").to_lowercase() == wanted)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{all_spells, by_com, by_name};

    #[test]
    fn spell_vectors() {
        // Python: 57 spells; wind_strike sits on magic:wind_strike,
        // level 1, 55 xp, max hit 2.
        assert_eq!(all_spells().len(), 57);
        let wind = by_name("wind_strike");
        assert_eq!(wind.len(), 1);
        assert_eq!(wind[0].com.as_deref(), Some("magic:wind_strike"));
        assert_eq!(wind[0].level, 1);
        assert_eq!(wind[0].xp, 55);
        assert_eq!(wind[0].maxhit, Some(2));
        assert!(wind[0].runes.contains(&("airrune".to_string(), 1)));
        assert_eq!(by_name("^wind_strike").len(), 1);
        assert_eq!(by_com("magic:wind_strike").len(), 1);
        assert!(by_name("nope").is_empty());
    }
}
