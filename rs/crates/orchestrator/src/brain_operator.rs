#![forbid(unsafe_code)]
//! Operator ambition switches (user 2026-11-18: « créé l'ambition alcher
//! et donne-la à Ryan Mason »): `orchestrator ambition <compte> <kind>`
//! files the switch here; the brain applies it at the start of that
//! account's next pass, on the life it just loaded (the brain is the only
//! writer of a life file: no race with a pass in flight). Kinds: the
//! alchemist (`alcher`) only.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use lifescape_json::Value;
use lifescape_planner::{Life, ALCHER};

fn pending() -> &'static Mutex<HashMap<String, String>> {
    static TABLE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Kinds an operator may hand out by this command.
pub const OPERATOR_KINDS: &[&str] = &[ALCHER];

/// File a switch for `account` (typed refusal for an unknown kind).
pub fn request(account: &str, kind: &str) -> Result<(), String> {
    if account.is_empty() {
        return Err("ambition <compte> <kind>: no account".to_string());
    }
    if !OPERATOR_KINDS.contains(&kind) {
        return Err(format!("unknown operator ambition {kind:?} (known: {})", OPERATOR_KINDS.join(", ")));
    }
    pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(account.to_string(), kind.to_string());
    crate::logln!("brain {account}: operator ambition {kind} filed, applied at its next idle pass");
    Ok(())
}

/// Apply the switch filed for this life, if any; true when the life
/// changed (the caller saves it).
pub(crate) fn apply_pending(life: &mut Life, live: &Value) -> bool {
    let Some(kind) = pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&life.account) else { return false };
    if kind != ALCHER {
        return false;
    }
    let stamp = crate::brain_ambition::stamp_now();
    let switched = crate::brain_ambition::with_now(live, |now| lifescape_planner::become_alcher(life, now, stamp));
    match switched {
        Ok(abandoned) => {
            crate::brain::push_history(life, "goal", "role:alcher (operator)");
            crate::logln!("brain {}: ambition -> alcher (operator), {} nodes abandoned: {}", life.account, abandoned.len(), abandoned.join(","));
            true
        }
        Err(error) => {
            crate::logln!("brain {}: operator ambition alcher refused: {error:?}", life.account);
            false
        }
    }
}
