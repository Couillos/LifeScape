//! Build-time table codegen: `comps`, `objs`, `npcs` and `locs`, plus every
//! remaining dump (`equipment`, `foods`, `shops`, `quests`, `recipes`,
//! `rocks`, `spells`, `trees`, `varps`, `spawns_obj`).
//!
//! Parses the dumps with the workspace JSON reader and emits one generated
//! module per table (`*_generated.rs` in `OUT_DIR`) holding the typed row
//! slice (`ALL`) plus the lowercase name-sorted `(name, id)` index that the
//! `*_by_name` lookups search with `binary_search`/`partition_point`.
//! Rebuilding after a dump edit regenerates the tables.

#[path = "../json/src/lib.rs"]
#[allow(dead_code)]
mod workspace_json;

#[path = "codegen/equipment.rs"]
mod equipment_gen;
#[path = "codegen/foods.rs"]
mod foods_gen;
#[path = "codegen/quests.rs"]
mod quests_gen;
#[path = "codegen/recipes.rs"]
mod recipes_gen;
#[path = "codegen/rocks.rs"]
mod rocks_gen;
#[path = "codegen/shops.rs"]
mod shops_gen;
#[path = "codegen/spawns_obj.rs"]
mod spawns_obj_gen;
#[path = "codegen/spells.rs"]
mod spells_gen;
#[path = "codegen/trees.rs"]
mod trees_gen;
#[path = "codegen/varps.rs"]
mod varps_gen;

use std::env;
use std::fs;
use std::path::PathBuf;

/// Workspace-relative location of one dump, from this crate's manifest.
fn dump_path_from_manifest(manifest_dir: &str, dump_name: &str) -> PathBuf {
    PathBuf::from(manifest_dir).join(format!("../../../gamedata/data/{dump_name}.json"))
}

/// Read and parse one dump, panicking with the file name on failure.
fn parse_dump(manifest_dir: &str, dump_name: &str) -> workspace_json::Value {
    let dump_path = dump_path_from_manifest(manifest_dir, dump_name);
    let dump_text =
        fs::read_to_string(&dump_path).unwrap_or_else(|err| panic!("{dump_name} dump must be readable: {err}"));
    workspace_json::parse(&dump_text)
        .unwrap_or_else(|err| panic!("{dump_name} dump must be valid JSON: {err}"))
}

/// Dump rows as a slice; every G2 dump is a JSON array indexed by id.
fn dump_rows<'dump>(parsed_dump: &'dump workspace_json::Value, dump_name: &str) -> &'dump [workspace_json::Value] {
    match parsed_dump {
        workspace_json::Value::Arr(rows) => rows,
        _ => panic!("{dump_name} dump must be a JSON array of rows"),
    }
}

/// `(lowercase name, id)` pairs sorted by name, dump order kept among
/// equals: every same-name row stays in the index, so `*_by_name` returns
/// all matches in dump order exactly like the old linear scan. (Scalar
/// `com_id` keeps the G1 first-row-wins rule instead.)
fn sorted_name_index(dump_rows: &[workspace_json::Value], dump_name: &str) -> Vec<(String, usize)> {
    let mut collected: Vec<(String, usize)> = Vec::new();
    for (row_id, dump_row) in dump_rows.iter().enumerate() {
        if dump_row.is_null() {
            continue;
        }
        let row_name = match dump_row.get("name").as_str() {
            Some(name) => name.to_lowercase(),
            None => panic!("{dump_name} row {row_id} without a string name: {dump_row:?}"),
        };
        collected.push((row_name, row_id));
    }
    collected.sort_by(|left_entry, right_entry| left_entry.0.cmp(&right_entry.0));
    collected
}

/// Quote a dump string for a Rust literal (escapes `"`, `\` and controls).
fn escape_for_rust_literal(raw_name: &str) -> String {
    let mut escaped = String::with_capacity(raw_name.len());
    for letter in raw_name.chars() {
        match letter {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ if (letter as u32) < 0x20 => escaped.push_str(&format!("\\u{{{:x}}}", letter as u32)),
            _ => escaped.push(letter),
        }
    }
    escaped
}

/// `Some("…")` / `None` for an optional dump string.
fn render_opt_str(dump_value: &workspace_json::Value) -> String {
    match dump_value.as_str() {
        Some(text) => format!("Some(\"{}\")", escape_for_rust_literal(text)),
        None => String::from("None"),
    }
}

/// `[None, Some("Take"), …]` for a dump ops array (missing reads as empty,
/// non-string entries as `None`, like the old runtime rows).
fn render_ops_array(dump_row: &workspace_json::Value, key: &str) -> String {
    let mut rendered = String::from("&[");
    match dump_row.get(key) {
        workspace_json::Value::Arr(items) => {
            for (position, item) in items.iter().enumerate() {
                if position > 0 {
                    rendered.push_str(", ");
                }
                match item {
                    workspace_json::Value::Null => rendered.push_str("None"),
                    workspace_json::Value::Str(text) => {
                        rendered.push_str(&format!("Some(\"{}\")", escape_for_rust_literal(text)));
                    }
                    _ => rendered.push_str("None"),
                }
            }
        }
        _ => {}
    }
    rendered.push(']');
    rendered
}

/// `Some(true)` / `None` for a nullable dump flag.
fn render_opt_flag(dump_row: &workspace_json::Value, key: &str) -> String {
    match dump_row.get(key).as_bool() {
        Some(value) => format!("Some({value})"),
        None => String::from("None"),
    }
}

/// `&["a", "b"]` for a dump string list (non-strings skipped,
/// like the old `str_list` runtime helper).
fn render_str_array(dump_row: &workspace_json::Value, key: &str) -> String {
    let mut rendered = String::from("&[");
    let mut first = true;
    if let workspace_json::Value::Arr(items) = dump_row.get(key) {
        for item in items {
            let Some(text) = item.as_str() else {
                continue;
            };
            if !first {
                rendered.push_str(", ");
            }
            first = false;
            rendered.push_str(&format!("\"{}\"", escape_for_rust_literal(text)));
        }
    }
    rendered.push(']');
    rendered
}

/// `&[("a", 1)]` for a dump string->int map, keys sorted (like the old
/// `str_int_map` runtime helper, whose objects already iterate sorted).
fn render_str_int_map(dump_row: &workspace_json::Value, key: &str) -> String {
    let mut rendered = String::from("&[");
    let mut first = true;
    if let workspace_json::Value::Obj(fields) = dump_row.get(key) {
        for (name, value) in fields {
            let Some(number) = value.as_i64() else {
                continue;
            };
            if !first {
                rendered.push_str(", ");
            }
            first = false;
            rendered.push_str(&format!("(\"{}\", {number})", escape_for_rust_literal(name)));
        }
    }
    rendered.push(']');
    rendered
}

fn int_or(dump_row: &workspace_json::Value, key: &str, default: i64) -> i64 {
    dump_row.get(key).as_i64().unwrap_or(default)
}

fn flag_or(dump_row: &workspace_json::Value, key: &str, default: bool) -> bool {
    dump_row.get(key).as_bool().unwrap_or(default)
}

/// `(lowercase name, id)` index shared by the three `*_by_name` lookups.
fn render_name_index(sorted_entries: &[(String, usize)], static_name: &str) -> String {
    let mut rendered = format!("    pub static {static_name}: &[(&str, usize)] = &[\n");
    for (entry_name, entry_id) in sorted_entries {
        rendered.push_str(&format!("        (\"{}\", {entry_id}usize),\n", escape_for_rust_literal(entry_name)));
    }
    rendered.push_str("    ];\n");
    rendered
}

fn render_objs_module(dump_rows: &[workspace_json::Value]) -> String {
    let mut generated = String::from(
        "// Generated by rs/crates/gamedata/build.rs from gamedata/data/objs.json - do not edit.\n\
         pub mod objs {\n    \
         pub static ALL: &[crate::Obj] = &[\n",
    );
    for (obj_id, dump_row) in dump_rows.iter().enumerate() {
        if dump_row.is_null() {
            generated.push_str(&format!(
                "        crate::Obj {{ obj_id: {obj_id}usize, name: None, display: None, desc: None, \
                 ops: &[], inv_ops: &[], price: 0, alch_high: 0, alch_low: 0, stackable: false, \
                 members: None, respawn_ticks: 100 }},\n"
            ));
            continue;
        }
        let ops = render_ops_array(dump_row, "ops");
        let inv_ops = render_ops_array(dump_row, "iops");
        generated.push_str(&format!(
            "        crate::Obj {{ obj_id: {obj_id}usize, name: {}, display: {}, desc: {}, ops: {ops}, \
             inv_ops: {inv_ops}, price: {}, alch_high: {}, alch_low: {}, stackable: {}, members: {}, \
             respawn_ticks: {} }},\n",
            render_opt_str(dump_row.get("name")),
            render_opt_str(dump_row.get("display")),
            render_opt_str(dump_row.get("desc")),
            int_or(dump_row, "price", 0),
            int_or(dump_row, "alch_high", 0),
            int_or(dump_row, "alch_low", 0),
            flag_or(dump_row, "stackable", false),
            render_opt_flag(dump_row, "members"),
            int_or(dump_row, "respawn_ticks", 100),
        ));
    }
    generated.push_str("    ];\n");
    generated.push_str(&render_name_index(&sorted_name_index(dump_rows, "objs"), "BY_NAME"));
    generated.push_str("}\n");
    generated
}

/// Complete `x/z/level` npc spawns of one row, in dump order.
fn render_npc_spawns(dump_row: &workspace_json::Value) -> String {
    let mut rendered = String::from("&[");
    let mut first = true;
    if let workspace_json::Value::Arr(items) = dump_row.get("spawns") {
        for item in items {
            let (Some(x), Some(z), Some(level)) =
                (item.get("x").as_i64(), item.get("z").as_i64(), item.get("level").as_i64())
            else {
                continue;
            };
            if !first {
                rendered.push_str(", ");
            }
            first = false;
            let members = item.get("members").as_bool().unwrap_or(false);
            rendered.push_str(&format!("crate::NpcSpawn {{ x: {x}, z: {z}, level: {level}, members: {members} }}"));
        }
    }
    rendered.push(']');
    rendered
}

fn render_npcs_module(dump_rows: &[workspace_json::Value]) -> String {
    let mut generated = String::from(
        "// Generated by rs/crates/gamedata/build.rs from gamedata/data/npcs.json - do not edit.\n\
         pub mod npcs {\n    \
         pub static ALL: &[crate::Npc] = &[\n",
    );
    for (npc_id, dump_row) in dump_rows.iter().enumerate() {
        if dump_row.is_null() {
            generated.push_str(&format!(
                "        crate::Npc {{ npc_id: {npc_id}usize, name: None, size: 1, ops: &[], members: None, \
                 hunts_players: false, huntrange: 0, vislevel: 0, spawns: &[] }},\n"
            ));
            continue;
        }
        let ops = render_ops_array(dump_row, "ops");
        let spawns = render_npc_spawns(dump_row);
        generated.push_str(&format!(
            "        crate::Npc {{ npc_id: {npc_id}usize, name: {}, size: {}, ops: {ops}, members: {}, \
             hunts_players: {}, huntrange: {}, vislevel: {}, spawns: {spawns} }},\n",
            render_opt_str(dump_row.get("name")),
            int_or(dump_row, "size", 1),
            render_opt_flag(dump_row, "members"),
            flag_or(dump_row, "hunts_players", false),
            int_or(dump_row, "huntrange", 0),
            int_or(dump_row, "vislevel", 0),
        ));
    }
    generated.push_str("    ];\n");
    generated.push_str(&render_name_index(&sorted_name_index(dump_rows, "npcs"), "BY_NAME"));
    generated.push_str("}\n");
    generated
}

/// Complete `x/z/level` loc spawns of one row, in dump order.
fn render_loc_spawns(dump_row: &workspace_json::Value) -> String {
    let mut rendered = String::from("&[");
    let mut first = true;
    if let workspace_json::Value::Arr(items) = dump_row.get("spawns") {
        for item in items {
            let (Some(x), Some(z), Some(level)) =
                (item.get("x").as_i64(), item.get("z").as_i64(), item.get("level").as_i64())
            else {
                continue;
            };
            if !first {
                rendered.push_str(", ");
            }
            first = false;
            let members = match item.get("members").as_bool() {
                Some(value) => format!("Some({value})"),
                None => String::from("None"),
            };
            rendered.push_str(&format!("crate::LocSpawn {{ x: {x}, z: {z}, level: {level}, members: {members} }}"));
        }
    }
    rendered.push(']');
    rendered
}

fn render_locs_module(dump_rows: &[workspace_json::Value]) -> String {
    let mut generated = String::from(
        "// Generated by rs/crates/gamedata/build.rs from gamedata/data/locs.json - do not edit.\n\
         pub mod locs {\n    \
         pub static ALL: &[crate::Loc] = &[\n",
    );
    for (loc_id, dump_row) in dump_rows.iter().enumerate() {
        if dump_row.is_null() {
            generated.push_str(&format!(
                "        crate::Loc {{ loc_id: {loc_id}usize, name: None, width: 1, length: 1, \
                 blockwalk: false, blockrange: false, active: 0, ops: &[], spawns: &[] }},\n"
            ));
            continue;
        }
        let ops = render_ops_array(dump_row, "ops");
        let spawns = render_loc_spawns(dump_row);
        generated.push_str(&format!(
            "        crate::Loc {{ loc_id: {loc_id}usize, name: {}, width: {}, length: {}, \
             blockwalk: {}, blockrange: {}, active: {}, ops: {ops}, spawns: {spawns} }},\n",
            render_opt_str(dump_row.get("name")),
            int_or(dump_row, "width", 1),
            int_or(dump_row, "length", 1),
            flag_or(dump_row, "blockwalk", false),
            flag_or(dump_row, "blockrange", false),
            int_or(dump_row, "active", 0),
        ));
    }
    generated.push_str("    ];\n");
    generated.push_str(&render_name_index(&sorted_name_index(dump_rows, "locs"), "BY_NAME"));
    generated.push_str("}\n");
    generated
}

/// `(lowercase name, id)` pairs sorted by name, first table row winning any
/// same-name duplicate so the lookup matches the old linear scan.
fn sorted_comp_entries(parsed_dump: &workspace_json::Value) -> Vec<(String, u16)> {
    let dump_rows = match parsed_dump {
        workspace_json::Value::Arr(rows) => rows,
        _ => panic!("comps dump must be a JSON array of {{id, name}} rows"),
    };
    let mut collected: Vec<(String, u16)> = Vec::with_capacity(dump_rows.len());
    for dump_row in dump_rows {
        let comp_name = match dump_row.get("name").as_str() {
            Some(name) => name.to_lowercase(),
            None => panic!("comps dump row without a string name: {dump_row:?}"),
        };
        let comp_id = match dump_row.get("id").as_i64() {
            Some(number) if (0..=u16::MAX as i64).contains(&number) => number as u16,
            _ => panic!("comps dump row without a u16 id: {dump_row:?}"),
        };
        collected.push((comp_name, comp_id));
    }
    collected.sort_by(|left_entry, right_entry| left_entry.0.cmp(&right_entry.0));
    collected.dedup_by(|later_entry, first_entry| later_entry.0 == first_entry.0);
    collected
}

fn render_generated_module(sorted_entries: &[(String, u16)]) -> String {
    let mut generated = String::from(
        "// Generated by rs/crates/gamedata/build.rs from gamedata/data/comps.json - do not edit.\n\
         pub mod comps {\n    \
         pub static BY_NAME: &[(&str, u16)] = &[\n",
    );
    for (comp_name, comp_id) in sorted_entries {
        generated.push_str(&format!("        (\"{}\", {comp_id}u16),\n", escape_for_rust_literal(comp_name)));
    }
    generated.push_str("    ];\n}\n");
    generated
}

fn emit_generated(file_name: &str, contents: &str) {
    let output_dir = env::var("OUT_DIR").expect("cargo always sets OUT_DIR for build scripts");
    fs::write(PathBuf::from(output_dir).join(file_name), contents)
        .unwrap_or_else(|err| panic!("generated {file_name} must be writable: {err}"));
}

fn main() {
    let manifest_dir =
        env::var("CARGO_MANIFEST_DIR").expect("cargo always sets CARGO_MANIFEST_DIR for build scripts");
    for dump_name in [
        "comps",
        "objs",
        "npcs",
        "locs",
        "equipment",
        "foods",
        "shops",
        "quests",
        "recipes",
        "rocks",
        "spells",
        "trees",
        "varps",
        "spawns_obj",
    ] {
        println!("cargo:rerun-if-changed={}", dump_path_from_manifest(&manifest_dir, dump_name).display());
    }
    let parsed_comps = parse_dump(&manifest_dir, "comps");
    emit_generated("comps_generated.rs", &render_generated_module(&sorted_comp_entries(&parsed_comps)));
    let parsed_objs = parse_dump(&manifest_dir, "objs");
    emit_generated("objs_generated.rs", &render_objs_module(dump_rows(&parsed_objs, "objs")));
    let parsed_npcs = parse_dump(&manifest_dir, "npcs");
    emit_generated("npcs_generated.rs", &render_npcs_module(dump_rows(&parsed_npcs, "npcs")));
    let parsed_locs = parse_dump(&manifest_dir, "locs");
    emit_generated("locs_generated.rs", &render_locs_module(dump_rows(&parsed_locs, "locs")));
    equipment_gen::generate(&manifest_dir);
    foods_gen::generate(&manifest_dir);
    shops_gen::generate(&manifest_dir);
    quests_gen::generate(&manifest_dir);
    recipes_gen::generate(&manifest_dir);
    rocks_gen::generate(&manifest_dir);
    spells_gen::generate(&manifest_dir);
    trees_gen::generate(&manifest_dir);
    varps_gen::generate(&manifest_dir);
    spawns_obj_gen::generate(&manifest_dir);
}
