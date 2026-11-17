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

/// Spell -> NPC param its target must carry (`spell_targets.json`,
/// written by `orchestrator dump-npc-ranges` from the server's `apnpct`
/// procs: crumble undead refuses anything but `undead`).
const TARGETS: &str = include_str!("../../../../gamedata/data/spell_targets.json");

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

impl Spell {
    /// A spell cast on an NPC from the combat spellbook that deals damage
    /// (`spellcom` + `maxhit` in `magic_spell_table`): strikes, bolts,
    /// blasts, crumble undead... Never the display `name` (the server
    /// gives crumble undead none: its row was dropped by that test).
    pub fn deals_damage(&self) -> bool {
        self.com.is_some() && self.maxhit.is_some() && self.level >= 1
    }

    /// A spell cast on an NPC that deals no damage: the stat curses and
    /// bind (`spellcom`, no `maxhit`).
    pub fn npc_debuff(&self) -> bool {
        self.com.is_some() && self.maxhit.is_none() && self.level >= 1
    }
}

/// NPC param the target of `spell` must carry (`None`: any attackable
/// NPC). Server truth: `spell_targets.json`.
pub fn target_param(spell: &str) -> Option<String> {
    static TABLE: std::sync::OnceLock<Vec<(String, String)>> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let Ok(doc) = lifescape_json::parse(TARGETS) else { return Vec::new() };
        doc.get("spells").as_obj().map(|rows| rows.iter().filter_map(|(name, param)| Some((name.clone(), param.as_str()?.to_string()))).collect()).unwrap_or_default()
    });
    table.iter().find(|(name, _)| name == spell).map(|(_, param)| param.clone())
}

/// True when `spell` affects the NPC `npc` (debugname): no target param,
/// or the NPC's config sets it (`npc_combat::has_param`).
pub fn affects(spell: &str, npc: &str) -> bool {
    target_param(spell).is_none_or(|param| crate::npc_combat::has_param(npc, &param))
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

/// Impact graphic (spotanim id) a landed debuff shows on its foe, to
/// every client (`player_magic.rs2` `spotanim_npc(spotanim_target)`,
/// `magic_combat_spells.dbrow`, ids from `content/pack/spotanim.pack`):
/// a foe showing it was lowered by someone and refuses the same curse
/// until it regenerates. `None` for any other spell.
pub fn impact_graphic(spell: &str) -> Option<u16> {
    match spell.trim_start_matches('^') {
        "confuse" => Some(104),
        "weaken" => Some(107),
        "curse" => Some(110),
        "bind" => Some(181),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{affects, all_spells, by_com, by_name, impact_graphic, target_param};

    /// The impact graphics match the server's packed ids and spell rows
    /// (read-only: `../lostcity/content`).
    #[test]
    fn debuff_impact_graphics_match_the_server() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../lostcity/content");
        let pack = std::fs::read_to_string(root.join("pack/spotanim.pack")).expect("spotanim.pack");
        let rows = std::fs::read_to_string(root.join("scripts/skill_combat/configs/magic/magic_combat_spells.dbrow")).expect("spell rows");
        for (spell, graphic) in [("confuse", "confuse_impact"), ("weaken", "weaken_impact"), ("curse", "curse_impact"), ("bind", "bind_impact")] {
            let id = pack.lines().find_map(|line| line.split_once('=').filter(|(_, name)| *name == graphic).and_then(|(id, _)| id.parse::<u16>().ok()));
            assert_eq!(impact_graphic(spell), id, "{spell}: {graphic}");
            assert!(rows.contains(&format!("data=spotanim_target,{graphic},")), "{graphic} is a spell impact");
        }
        assert_eq!(impact_graphic("wind_strike"), None);
    }

    /// Crumble undead is a damage spell like the strikes (its missing
    /// display name never hides it) and hits the undead only.
    #[test]
    fn crumble_undead_targets_the_undead_only() {
        let crumble = by_name("crumble_undead").into_iter().next().expect("crumble row");
        assert!(crumble.name.is_none(), "the server row has no name");
        assert!(crumble.deals_damage() && !crumble.members);
        assert_eq!((crumble.level, crumble.xp, crumble.maxhit), (39, 490, Some(8)));
        assert_eq!(target_param("crumble_undead").as_deref(), Some("undead"));
        assert_eq!(target_param("fire_bolt"), None);
        assert!(affects("crumble_undead", "skeleton_unarmed") && affects("crumble_undead", "ghost"));
        assert!(!affects("crumble_undead", "cow"));
        assert!(affects("fire_bolt", "cow"));
    }

    /// The four F2P NPC debuffs: confuse, weaken, curse, bind.
    #[test]
    fn debuffs_are_the_damageless_npc_spells() {
        let mut debuffs: Vec<String> = all_spells().into_iter().filter(|row| row.npc_debuff() && !row.members).map(|row| row.spell).collect();
        debuffs.sort();
        assert_eq!(debuffs, vec!["bind", "confuse", "curse", "weaken"]);
    }

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
