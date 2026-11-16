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

/// Staff test accounts (AGENTS.md rules 11 and 13): they cheat, and
/// what they cheat moves only between them.
pub const STAFF: [&str; 2] = ["admin", "admin2"];

/// Always excluded: staff and the owner's private account.
const ALWAYS_EXCLUDED: [&str; 3] = ["admin", "admin2", "bite"];

/// True for a staff test account (case-insensitive).
pub fn is_staff(account: &str) -> bool {
    STAFF.iter().any(|name| name.eq_ignore_ascii_case(account))
}

/// Why `command` (with or without its `::`) is refused, or None when the
/// cheat may run. A cheat aimed at another player (`giveother`,
/// `teleother`, `setvarother`…: the verb ends in `other`, its first
/// argument names the target) runs only on a staff target: admin and
/// admin2 cheat between themselves, never on the fleet.
pub fn cheat_refusal(command: &str) -> Option<String> {
    let text = command.trim();
    let text = text.strip_prefix("::").unwrap_or(text);
    let mut words = text.split_whitespace();
    let verb = words.next().unwrap_or("").to_lowercase();
    if !verb.ends_with("other") {
        return None;
    }
    let target = words.next().unwrap_or("");
    (!is_staff(target)).then(|| {
        format!("forbidden: cheat '{verb}' aimed at {target:?}: staff cheats only between admin and admin2 (AGENTS.md rule 13)")
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
    fn cheats_on_others_target_staff_only() {
        for command in ["giveother t03 coins 5000", "::giveother t99 coins 1000", "  GiveOther t01 net 1", "teleother t40", "setvarother t40 qp 0"] {
            let refusal = cheat_refusal(command).expect("refused");
            assert!(refusal.contains("rule 13"), "{refusal}");
        }
        for command in ["give coins 100", "tele 0,50,50,10,10", "giveother admin2 coins 500", "::teleother ADMIN", "setvarother admin2 qp 0"] {
            assert!(cheat_refusal(command).is_none(), "{command} stays allowed");
        }
    }

    #[test]
    fn exclusion_list_keeps_staff_and_reads_the_file() {
        let names = parse_excluded("# test accounts\nProof_A\n\nproof_c  # dev deals\n");
        let listed: Vec<&str> = names.iter().map(String::as_str).collect();
        assert_eq!(listed, vec!["admin", "admin2", "bite", "proof_a", "proof_c"]);
        assert!(parse_excluded("").contains("admin"), "staff always excluded");
    }
}
