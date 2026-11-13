//! Companion of `dump-npc-ranges`: the same server `.npc` configs
//! (`lostcity/content/scripts/**/*.npc`, read-only, `_unpack` included)
//! give every attackable NPC's combat block (hitpoints, attack,
//! strength, defence, `attackbonus`/`strengthbonus`, the three melee
//! defence bonuses, `attackrate`, `damagetype`). Written to
//! `gamedata/data/npc_combat.json` next to `npc_ranges.json`: the game
//! dump carries none of them, and the melee ladder needs them to rate a
//! spot (xp per tick, damage taken). Later files override earlier ones,
//! as the packer does (`_unpack` sorts before the area configs).

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::Path;

use lifescape_json::{object, Value};

/// Keys copied as integers (`key=value` lines and `param=key,value`).
const INT_KEYS: [&str; 10] = [
    "hitpoints",
    "attack",
    "strength",
    "defence",
    "attackbonus",
    "strengthbonus",
    "stabdefence",
    "slashdefence",
    "crushdefence",
    "attackrate",
];

/// One NPC's explicit combat fields (absent keys take engine defaults
/// on the reading side).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct CombatBlock {
    pub ints: BTreeMap<String, i64>,
    pub damagetype: Option<String>,
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

    #[test]
    fn later_files_override_earlier_ones() {
        let mut blocks = std::collections::BTreeMap::new();
        parse_combat("[cow]\nhitpoints=8\n", &mut blocks);
        parse_combat("[cow]\nhitpoints=9\n", &mut blocks);
        assert_eq!(blocks.get("cow").and_then(|block| block.ints.get("hitpoints")), Some(&9));
    }
}
