#![forbid(unsafe_code)]
//! Clean economy guards (AGENTS.md rule 13): what admin cheats can never
//! hand to another account, and which accounts never count in the public
//! holdings totals.
//!
//! Live 2026-11-11: `cheat giveother t03 coins 5000` (twice) and
//! `giveother t99 coins 1000` put 11 000 cheated coins in the economy,
//! 100 of them spent on t01's sardines before anyone noticed. The cheat
//! is refused at the call gate now; the exclusion list is persisted so a
//! staff or test account is excluded without a rebuild.

use std::collections::BTreeSet;

/// Always excluded: staff and the owner's private account.
const ALWAYS_EXCLUDED: [&str; 2] = ["admin", "bite"];

/// Cheats that move an object from nowhere into another player's hands.
const FORBIDDEN_CHEATS: [&str; 1] = ["giveother"];

/// Why `command` (with or without its `::`) is refused, or None when the
/// cheat may run. Only the verb is read, case-insensitive.
pub fn cheat_refusal(command: &str) -> Option<String> {
    let text = command.trim();
    let text = text.strip_prefix("::").unwrap_or(text);
    let verb = text.split_whitespace().next().unwrap_or("").to_lowercase();
    FORBIDDEN_CHEATS.contains(&verb.as_str()).then(|| {
        format!("forbidden: cheat '{verb}' hands a cheated object to another account (AGENTS.md rule 13: admin's cheats stay with admin)")
    })
}

/// File of extra excluded accounts, one name per line, `#` comments.
pub fn excluded_path() -> std::path::PathBuf {
    crate::paths::run_dir().join("economy-excluded.txt")
}

/// Accounts never counted in the holdings totals: the fixed staff pair
/// plus the persisted list (lowercase).
pub fn excluded_accounts() -> BTreeSet<String> {
    let text = std::fs::read_to_string(excluded_path()).unwrap_or_default();
    parse_excluded(&text)
}

fn parse_excluded(text: &str) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = ALWAYS_EXCLUDED.iter().map(|name| name.to_string()).collect();
    for line in text.lines() {
        let name = line.split('#').next().unwrap_or("").trim().to_lowercase();
        if !name.is_empty() {
            names.insert(name);
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::{cheat_refusal, parse_excluded};

    #[test]
    fn giveother_is_refused_in_every_spelling() {
        for command in ["giveother t03 coins 5000", "::giveother t99 coins 1000", "  GiveOther t01 net 1"] {
            let refusal = cheat_refusal(command).expect("refused");
            assert!(refusal.contains("rule 13"), "{refusal}");
        }
        // Admin's own tests keep their tools: nothing leaves admin.
        for command in ["give coins 100", "tele 0,50,50,10,10", "teleother t40", "setvarother t40 qp 0"] {
            assert!(cheat_refusal(command).is_none(), "{command} stays allowed");
        }
    }

    #[test]
    fn exclusion_list_keeps_staff_and_reads_the_file() {
        let names = parse_excluded("# test accounts\nProof_A\n\nproof_c  # dev deals\n");
        let listed: Vec<&str> = names.iter().map(String::as_str).collect();
        assert_eq!(listed, vec!["admin", "bite", "proof_a", "proof_c"]);
        assert!(parse_excluded("").contains("admin"), "staff always excluded");
    }
}
