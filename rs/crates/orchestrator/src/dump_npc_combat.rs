//! Companion of `dump-npc-ranges`: the same server `.npc` configs
//! (`lostcity/content/scripts/**/*.npc`, read-only, `_unpack` included)
//! give every attackable NPC's combat block (hitpoints, attack,
//! strength, defence, magic, `attackbonus`/`strengthbonus`, the three
//! melee defence bonuses, `rangedefence`/`magicdefence` (the ranged and
//! magic defence rolls of `npc_defence_roll_specific`), `attackrate`,
//! `damagetype`). Written to
//! `gamedata/data/npc_combat.json` next to `npc_ranges.json`: the game
//! dump carries none of them, and the melee ladder needs them to rate a
//! spot (xp per tick, damage taken). Later files override earlier ones,
//! as the packer does (`_unpack` sorts before the area configs). The
//! boolean target params a spell checks (`param=undead,^true`: crumble
//! undead refuses anything else, `crumble_undead.rs2`) ride as `true`.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::Path;

use lifescape_json::{object, Value};

/// Keys copied as integers (`key=value` lines and `param=key,value`).
const INT_KEYS: [&str; 13] = [
    "hitpoints",
    "attack",
    "strength",
    "defence",
    "magic",
    "attackbonus",
    "strengthbonus",
    "stabdefence",
    "slashdefence",
    "crushdefence",
    "rangedefence",
    "magicdefence",
    "attackrate",
];

/// Boolean NPC params a spell's target check reads (`npc_param(undead)`
/// in `crumble_undead.rs2`), copied when the config sets them `^true`.
pub const TARGET_PARAMS: [&str; 1] = ["undead"];

/// One NPC's explicit combat fields (absent keys take engine defaults
/// on the reading side).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct CombatBlock {
    pub ints: BTreeMap<String, i64>,
    pub damagetype: Option<String>,
    /// Target params set `^true` (`TARGET_PARAMS`).
    pub flags: Vec<String>,
}

/// `[name]` blocks with `key=value` and `param=key,value` lines.
pub fn parse_combat(text: &str, into: &mut BTreeMap<String, CombatBlock>) {
    let mut current: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            current = Some(name.to_string());
            continue;
        }
        let Some(name) = current.as_ref() else { continue };
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = match key.trim() {
            "param" => match value.split_once(',') {
                Some((param, raw)) => (param.trim(), raw.trim()),
                None => continue,
            },
            other => (other, value.trim()),
        };
        if key == "damagetype" {
            let style = value.trim_start_matches('^').trim_end_matches("_style").to_string();
            into.entry(name.clone()).or_default().damagetype = Some(style);
            continue;
        }
        if TARGET_PARAMS.contains(&key) {
            let flags = &mut into.entry(name.clone()).or_default().flags;
            flags.retain(|flag| flag != key);
            if value == "^true" {
                flags.push(key.to_string());
            }
            continue;
        }
        if !INT_KEYS.contains(&key) {
            continue;
        }
        let Ok(number) = value.parse::<i64>() else { continue };
        into.entry(name.clone()).or_default().ints.insert(key.to_string(), number);
    }
}

/// Write the combat table of every NPC with a `hitpoints` line.
pub fn dump_npc_combat(files: &[std::path::PathBuf], output: &Path) -> Result<usize, String> {
    let mut blocks: BTreeMap<String, CombatBlock> = BTreeMap::new();
    for path in files {
        let text = std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
        parse_combat(&text, &mut blocks);
    }
    let rows: BTreeMap<String, Value> = blocks
        .into_iter()
        .filter(|(_, block)| block.ints.contains_key("hitpoints"))
        .map(|(name, block)| {
            let mut fields: Vec<(&str, Value)> = Vec::new();
            for key in INT_KEYS {
                if let Some(number) = block.ints.get(key) {
                    fields.push((key, Value::Int(*number)));
                }
            }
            if let Some(style) = block.damagetype.as_deref() {
                fields.push(("damagetype", lifescape_json::str_value(style)));
            }
            for flag in TARGET_PARAMS.iter().filter(|param| block.flags.iter().any(|flag| flag == *param)) {
                fields.push((flag, Value::Bool(true)));
            }
            (name, object(fields))
        })
        .collect();
    let count = rows.len();
    let doc = object(vec![
        ("source", lifescape_json::str_value("lostcity/content/scripts/**/*.npc (orchestrator dump-npc-ranges)")),
        ("npcs", Value::Obj(rows)),
    ]);
    std::fs::write(output, lifescape_json::stringify_pretty(&doc) + "\n").map_err(|error| format!("{}: {error}", output.display()))?;
    println!("dump-npc-combat: {count} NPCs -> {}", output.display());
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::parse_combat;

    #[test]
    fn reads_stats_params_and_damage_type() {
        let mut blocks = std::collections::BTreeMap::new();
        parse_combat(
            "[guard1]\nvislevel=21\nhitpoints=22\nattack=19\nparam=attackbonus,4\nparam=damagetype,^stab_style\nparam=attack_anim,x\n[hans]\nname=Hans\n",
            &mut blocks,
        );
        let guard = blocks.get("guard1").expect("guard block");
        assert_eq!(guard.ints.get("hitpoints"), Some(&22));
        assert_eq!(guard.ints.get("attackbonus"), Some(&4));
        assert_eq!(guard.damagetype.as_deref(), Some("stab"));
        assert!(!guard.ints.contains_key("vislevel"), "only combat keys");
        assert!(blocks.get("hans").is_none_or(|block| !block.ints.contains_key("hitpoints")), "no hitpoints, no row");
    }

    /// The ranged and magic defence rolls (`npc_defence_roll_specific`):
    /// the magic level and both defence bonuses ride with the melee block.
    #[test]
    fn reads_the_ranged_and_magic_defences() {
        let mut blocks = std::collections::BTreeMap::new();
        parse_combat("[cow]\nhitpoints=8\nmagic=0\nparam=magicdefence,-21\nparam=rangedefence,-21\n", &mut blocks);
        let cow = blocks.get("cow").expect("cow block");
        assert_eq!(cow.ints.get("magic"), Some(&0));
        assert_eq!(cow.ints.get("magicdefence"), Some(&-21));
        assert_eq!(cow.ints.get("rangedefence"), Some(&-21));
    }

    /// `param=undead,^true` (skeletons, zombies, ghosts) rides as a flag;
    /// a cow has none.
    #[test]
    fn reads_the_undead_target_param() {
        let mut blocks = std::collections::BTreeMap::new();
        parse_combat("[skeleton_unarmed]\nhitpoints=17\nparam=undead,^true\n[cow]\nhitpoints=8\n", &mut blocks);
        assert_eq!(blocks.get("skeleton_unarmed").map(|block| block.flags.clone()), Some(vec!["undead".to_string()]));
        assert!(blocks.get("cow").is_some_and(|block| block.flags.is_empty()));
    }

    #[test]
    fn later_files_override_earlier_ones() {
        let mut blocks = std::collections::BTreeMap::new();
        parse_combat("[cow]\nhitpoints=8\n", &mut blocks);
        parse_combat("[cow]\nhitpoints=9\n", &mut blocks);
        assert_eq!(blocks.get("cow").and_then(|block| block.ints.get("hitpoints")), Some(&9));
    }
}
