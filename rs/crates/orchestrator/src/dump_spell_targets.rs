//! Companion of `dump-npc-ranges`: the server's combat spell scripts
//! (`lostcity/content/scripts/**/*.rs2`, read-only) name the NPC param a
//! spell's target must carry. `[apnpct,magic:crumble_undead]` runs
//! `~pvm_crumble_undead`, whose `npc_param(undead) = ^false` refuses
//! anything else ("This spell only affects skeletons, zombies, ghosts
//! and shades."). Written to `gamedata/data/spell_targets.json`
//! (spell -> param); the NPC side (`param=undead,^true`) rides in
//! `npc_combat.json`. The game dump carries neither.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lifescape_json::{object, Value};

/// Header and body lines of every `[trigger,subject]` block of a script.
fn blocks(text: &str) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('[') {
            if let Some((header, tail)) = rest.split_once(']') {
                out.push((header.to_string(), vec![tail.trim().to_string()]));
                continue;
            }
        }
        if let Some((_, body)) = out.last_mut() {
            body.push(trimmed.to_string());
        }
    }
    out
}

/// First `~proc` a block body calls.
fn first_call(body: &[String]) -> Option<String> {
    body.iter().find_map(|line| {
        let (_, after) = line.split_once('~')?;
        let name: String = after.chars().take_while(|letter| letter.is_ascii_alphanumeric() || *letter == '_').collect();
        (!name.is_empty()).then_some(name)
    })
}

/// Param a proc refuses targets without: `npc_param(<param>) = ^false`.
fn refused_param(body: &[String]) -> Option<String> {
    body.iter().find_map(|line| {
        let compact: String = line.chars().filter(|letter| !letter.is_whitespace()).collect();
        let (_, after) = compact.split_once("npc_param(")?;
        let (param, rest) = after.split_once(')')?;
        rest.starts_with("=^false").then(|| param.to_string())
    })
}

/// Spell -> target param from the scripts' text (all files at once: a
/// trigger and its proc may sit in different files).
pub fn parse_spell_targets(texts: &[String]) -> BTreeMap<String, String> {
    let all: Vec<(String, Vec<String>)> = texts.iter().flat_map(|text| blocks(text)).collect();
    let mut out = BTreeMap::new();
    for (header, body) in &all {
        let Some(spell) = header.strip_prefix("apnpct,magic:") else { continue };
        let Some(proc_name) = first_call(body) else { continue };
        let wanted = format!("proc,{proc_name}");
        let param = all.iter().filter(|(other, _)| *other == wanted).find_map(|(_, proc_body)| refused_param(proc_body));
        if let Some(param) = param {
            out.insert(spell.to_string(), param);
        }
    }
    out
}

fn script_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_dir() {
            script_files(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "rs2") {
            out.push(path);
        }
    }
    Ok(())
}

/// Write the spell -> target param table from the server scripts.
pub fn dump_spell_targets(scripts: &Path, output: &Path) -> Result<usize, String> {
    let mut files = Vec::new();
    script_files(scripts, &mut files)?;
    files.sort();
    let mut texts = Vec::new();
    for path in &files {
        texts.push(std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?);
    }
    let table = parse_spell_targets(&texts);
    let count = table.len();
    let rows: BTreeMap<String, Value> = table.into_iter().map(|(spell, param)| (spell, lifescape_json::str_value(&param))).collect();
    let doc = object(vec![
        ("source", lifescape_json::str_value("lostcity/content/scripts/**/*.rs2 apnpct procs (orchestrator dump-npc-ranges)")),
        ("spells", Value::Obj(rows)),
    ]);
    std::fs::write(output, lifescape_json::stringify_pretty(&doc) + "\n").map_err(|error| format!("{}: {error}", output.display()))?;
    println!("dump-spell-targets: {count} spells -> {}", output.display());
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::parse_spell_targets;

    /// The server's crumble undead shape: the trigger calls the proc in
    /// another file, the proc refuses a target without the param; a
    /// default spell names none.
    #[test]
    fn reads_the_param_a_spell_proc_refuses_without() {
        let triggers = "[apnpct,magic:crumble_undead] ~pvm_crumble_undead;\n[apnpct,magic:confuse]\nif (npc_type = ungadulu_good) {\n   ~legends_spell_usage(^confuse);\n}\n".to_string();
        let procs = "[proc,pvm_crumble_undead]\nif (npc_param(undead) = ^false) {\n    mes(\"This spell only affects skeletons, zombies, ghosts and shades.\");\n    return;\n}\n[proc,legends_spell_usage](int $spell)\nmes(\"x\");\n".to_string();
        let table = parse_spell_targets(&[triggers, procs]);
        assert_eq!(table.get("crumble_undead").map(String::as_str), Some("undead"));
        assert!(!table.contains_key("confuse"), "{table:?}");
    }
}
