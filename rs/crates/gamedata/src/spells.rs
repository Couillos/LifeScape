//! Combat spells: level, runes, XP, max hit, interface component.

/// One static combat spell (build-time data, mirrored into [`Spell`] per call).
pub(crate) struct StaticSpell {
    pub spell: &'static str,
    pub name: Option<&'static str>,
    pub com: Option<&'static str>,
    pub members: bool,
    pub level: i64,
    pub runes: &'static [(&'static str, i64)],
    pub xp: i64,
    pub maxhit: Option<i64>,
}

mod generated {
    include!(concat!(env!("OUT_DIR"), "/spells_generated.rs"));
}

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

fn spell_from_static(row: &StaticSpell) -> Spell {
    Spell {
        spell: row.spell.to_string(),
        name: row.name.map(str::to_string),
        com: row.com.map(str::to_string),
        members: row.members,
        level: row.level,
        runes: row.runes.iter().map(|(name, count)| ((*name).to_string(), *count)).collect(),
        xp: row.xp,
        maxhit: row.maxhit,
    }
}

/// Every combat spell.
pub fn all_spells() -> Vec<Spell> {
    generated::ALL.iter().map(spell_from_static).collect()
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
        // 57 spells; wind_strike sits on magic:wind_strike,
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
