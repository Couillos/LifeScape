//! `orchestrator dump-npc-ranges` : reads the server's NPC configs
//! (`lostcity/content/scripts/**/*.npc`, read-only, `_unpack` included:
//! the packer loads them too, e.g. the young dark wizards) and writes
//! `gamedata/data/npc_ranges.json` with the explicit `wanderrange` and
//! `maxrange` of every NPC that sets them. The game dump lacks both, and
//! the danger map needs them: Draynor jail guards wander 11 tiles, not
//! the 3 the old margin assumed (bots died 10 tiles off their spawn).

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lifescape_json::{object, Value};

use crate::paths;

fn content_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("LIFESCAPE_ENGINE_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("../content");
        }
    }
    paths::root().join("../lostcity/content")
}

fn npc_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_dir() {
            npc_files(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "npc") {
            out.push(path);
        }
    }
    Ok(())
}

/// `[name]` blocks with `key=value` lines: (name -> (wanderrange, maxrange)).
pub fn parse_ranges(text: &str, into: &mut BTreeMap<String, (Option<i64>, Option<i64>)>) {
    let mut current: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            current = Some(name.to_string());
            continue;
        }
        let Some(name) = current.as_ref() else { continue };
        let Some((key, value)) = line.split_once('=') else { continue };
        let Ok(number) = value.trim().parse::<i64>() else { continue };
        let slot = into.entry(name.clone()).or_insert((None, None));
        match key.trim() {
            "wanderrange" => slot.0 = Some(number),
            "maxrange" => slot.1 = Some(number),
            _ => {}
        }
    }
}

pub fn dump_npc_ranges(output: &Path) -> Result<usize, String> {
    let mut files = Vec::new();
    npc_files(&content_dir().join("scripts"), &mut files)?;
    files.sort();
    let mut ranges: BTreeMap<String, (Option<i64>, Option<i64>)> = BTreeMap::new();
    for path in &files {
        let text = std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
        parse_ranges(&text, &mut ranges);
    }
    let rows: BTreeMap<String, Value> = ranges
        .into_iter()
        .filter(|(_, (wander, max))| wander.is_some() || max.is_some())
        .map(|(name, (wander, max))| {
            let mut fields = Vec::new();
            if let Some(wander) = wander {
                fields.push(("wanderrange", Value::Int(wander)));
            }
            if let Some(max) = max {
                fields.push(("maxrange", Value::Int(max)));
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
    println!("dump-npc-ranges: {count} NPCs from {} files -> {}", files.len(), output.display());
    // Same configs, second table: the combat block the melee ladder reads.
    crate::dump_npc_combat::dump_npc_combat(&files, &output.with_file_name("npc_combat.json"))?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::parse_ranges;

    #[test]
    fn reads_wander_and_max_per_block() {
        let mut ranges = std::collections::BTreeMap::new();
        parse_ranges("[jailguard]\nvislevel=26\nwanderrange=11\nmaxrange=12\n[cow]\nname=Cow\n", &mut ranges);
        assert_eq!(ranges.get("jailguard"), Some(&(Some(11), Some(12))));
        assert_eq!(ranges.get("cow"), None, "no range lines, no entry");
    }
}
