//! D53 gate: every test function carries exactly one `#[test]`.
//!
//! Regression net for the wave-0 find (moved tests lost their attribute,
//! orphaned attributes stacked on the next test): this integration test
//! scans the crate's own `src/` tree and fails on (a) a `#[test]` that
//! never reaches a `fn` (orphan or duplicate), (b) a private free `fn`
//! with no `#[test]` that nothing references (lost test: dynamic table
//! references count, so registry-dispatched solvers pass).

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn src_files(root: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(root).expect("src dir lists");
    let mut entries: Vec<_> = entries.map(|entry| entry.expect("dir entry").path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            src_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Code remainder of a line: strips `//` comments and string/char
/// literals so brace counting never trips on `"{...}"` format strings.
fn code_part(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    let mut in_string: Option<char> = None;
    while let Some(cell) = chars.next() {
        if let Some(quote) = in_string {
            if cell == '\\' {
                chars.next();
            } else if cell == quote {
                in_string = None;
            }
            continue;
        }
        if cell == '"' || cell == '\'' {
            in_string = Some(cell);
        } else if cell == '/' && chars.peek() == Some(&'/') {
            break;
        } else {
            out.push(cell);
        }
    }
    out
}

fn is_name_char(cell: char) -> bool {
    cell.is_alphanumeric() || cell == '_'
}

fn fn_name(line: &str) -> Option<(&str, bool)> {
    let trimmed = line.trim_start();
    let (rest, public) = match trimmed.strip_prefix("pub") {
        Some(after) if after.starts_with([' ', '(', '<']) => {
            let rest = after.trim_start_matches([' ', '\t']);
            let rest = if rest.starts_with('(') {
                let end = rest.find(')')? + 1;
                rest[end..].trim_start()
            } else {
                rest
            };
            (rest.strip_prefix("fn ")?, true)
        }
        _ => (trimmed.strip_prefix("fn ")?, false),
    };
    let end = rest.find(|cell: char| !is_name_char(cell))?;
    Some((&rest[..end], public))
}

/// True when a `#[test]` sits directly above the `fn` (blanks, doc
/// comments and other attributes like `#[ignore]` may sit between).
fn has_test_attr(lines: &[&str], index: usize) -> bool {
    let mut cursor = index;
    while cursor > 0 {
        cursor -= 1;
        let trimmed = lines[cursor].trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        if trimmed == "#[test]" {
            return true;
        }
        if trimmed.starts_with("#[") {
            continue;
        }
        return false;
    }
    false
}

/// Every `#[test]` must reach a `fn` before the next `#[test]` or EOF.
fn check_attached(file: &str, lines: &[&str], failures: &mut Vec<String>) {
    for (index, line) in lines.iter().enumerate() {
        if line.trim() != "#[test]" {
            continue;
        }
        let mut cursor = index + 1;
        let mut attached = false;
        while cursor < lines.len() {
            let trimmed = lines[cursor].trim();
            if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with("#[") {
                if trimmed == "#[test]" {
                    break;
                }
                cursor += 1;
                continue;
            }
            attached = fn_name(lines[cursor]).is_some();
            break;
        }
        if !attached {
            failures.push(format!("{file}:{}: orphaned or duplicated #[test]", index + 1));
        }
    }
}

/// Whole-word mentions of every identifier, so registry-dispatched
/// functions (stored as values, never called with parens) still count.
/// Char-boundary scan: byte slicing would panic on multi-byte text.
fn count_mentions(files: &[PathBuf]) -> HashMap<String, usize> {
    let mut mentions = HashMap::new();
    for path in files {
        let text = std::fs::read_to_string(path).expect("source reads");
        let mut word = String::new();
        let flush = |word: &mut String, mentions: &mut HashMap<String, usize>| {
            if !word.is_empty() {
                *mentions.entry(std::mem::take(word)).or_insert(0) += 1;
            }
        };
        for cell in text.chars() {
            if is_name_char(cell) {
                word.push(cell);
            } else {
                flush(&mut word, &mut mentions);
            }
        }
        flush(&mut word, &mut mentions);
    }
    mentions
}

/// A private free `fn` without `#[test]` must be referenced somewhere.
/// Trait/inherent method bodies are skipped (brace-tracked `impl`
/// regions): uncalled methods already trip the compiler's own lint.
fn check_private_fns(
    file: &str,
    lines: &[&str],
    mentions: &HashMap<String, usize>,
    failures: &mut Vec<String>,
) {
    let mut depth: i64 = 0;
    let mut impl_depths: Vec<i64> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let code = code_part(line);
        let head = code.trim_start();
        if head.starts_with("impl ") || head.starts_with("impl<") || head.starts_with("unsafe impl") {
            impl_depths.push(depth + code.matches('{').count() as i64);
        }
        if let Some((name, public)) = fn_name(line) {
            let inside_impl = impl_depths.iter().any(|open| depth >= *open);
            if !public && !inside_impl && !has_test_attr(lines, index) && mentions.get(name).copied().unwrap_or(0) <= 1
            {
                failures.push(format!("{file}:{}: fn `{name}` has no #[test] and no references", index + 1));
            }
        }
        depth += code.matches('{').count() as i64 - code.matches('}').count() as i64;
        while impl_depths.last().is_some_and(|open| depth < *open) {
            impl_depths.pop();
        }
    }
}

#[test]
fn every_test_fn_has_one_test_attribute() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    src_files(&root, &mut files);
    assert!(!files.is_empty(), "crate src/ scans");
    let mentions = count_mentions(&files);
    let mut failures = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path).expect("source reads");
        let lines: Vec<&str> = text.lines().collect();
        let file = path.strip_prefix(&root).unwrap_or(path).to_string_lossy().into_owned();
        check_attached(&file, &lines, &mut failures);
        check_private_fns(&file, &lines, &mentions, &mut failures);
    }
    assert!(failures.is_empty(), "lost or duplicated test attributes:\n{}", failures.join("\n"));
}
