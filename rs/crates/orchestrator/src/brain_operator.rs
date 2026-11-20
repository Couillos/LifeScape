#![forbid(unsafe_code)]
//! Operator ambition switches (user 2026-11-18: « créé l'ambition alcher
//! et donne-la à Ryan Mason »): `orchestrator ambition <compte> <kind>`
//! files the switch here; the brain applies it at the start of that
//! account's next pass, on the life it just loaded (the brain is the only
//! writer of a life file: no race with a pass in flight). Kinds: the
//! alchemist (`alcher`), and every trade the fleet draws (user
//! 2026-11-20: « trouve les rôles idéaux pour ces 11 comptes », the
//! retired market makers placed on the piles to consume).

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

/// A trade the fleet draws (catalogue kind with a ladder, never the
/// market maker): an operator may hand it out too.
fn drawable_trade(kind: &str) -> bool {
    lifescape_planner::ambition_catalog::specs().iter().any(|entry| entry.kind == kind) && !lifescape_planner::ambition::ladderless(kind) && kind != lifescape_planner::MARKET_MAKER_KIND
}

/// File a switch for `account` (typed refusal for an unknown kind).
pub fn request(account: &str, kind: &str) -> Result<(), String> {
    if account.is_empty() {
        return Err("ambition <compte> <kind>: no account".to_string());
    }
    if !OPERATOR_KINDS.contains(&kind) && !drawable_trade(kind) {
        return Err(format!("unknown operator ambition {kind:?} (known: {} or a drawn trade)", OPERATOR_KINDS.join(", ")));
    }
    pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(account.to_string(), kind.to_string());
    crate::logln!("brain {account}: operator ambition {kind} filed, applied at its next idle pass");
    Ok(())
}

/// Apply the switch filed for this life, if any, and lift the bans a
/// switch caused (ryan_mason 2026-11-18: its abandoned nodes read as a
/// stuck graph, magic banned 6000 ticks); true when the life changed
/// (the caller saves it).
pub(crate) fn apply_pending(life: &mut Life, live: &Value) -> bool {
    let lifted = lift_role_bans(life);
    let Some(kind) = pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&life.account) else { return lifted };
    if kind != ALCHER {
        if !drawable_trade(&kind) {
            return lifted;
        }
        // A trade: the open work stops, the kind is built like a redraw
        // (its ladder over the fleet's demand and surplus), the next goal
        // drawn at once.
        let abandoned = lifescape_planner::abandon_for_role(life, "role: operator ambition");
        life.goal_origin = None;
        crate::brain_ambition::assign_kind(life, live, &kind, &format!("operator:{kind}"));
        crate::brain::push_history(life, "goal", &format!("role:{kind} (operator)"));
        crate::logln!("brain {}: ambition -> {kind} (operator), {} nodes abandoned", life.account, abandoned.len());
        if let Some(label) = crate::brain_succession::draw_next(life, live) {
            crate::logln!("brain {}: {kind}'s first goal {label}", life.account);
        }
        lifescape_planner::normalize(life);
        return true;
    }
    let stamp = crate::brain_ambition::stamp_now();
    let switched = crate::brain_ambition::with_now(live, |now| lifescape_planner::become_alcher(life, now, stamp));
    match switched {
        Ok(abandoned) => {
            crate::brain::push_history(life, "goal", "role:alcher (operator)");
            crate::logln!("brain {}: ambition -> alcher (operator), {} nodes abandoned: {}", life.account, abandoned.len(), abandoned.join(","));
            // The next goal at once (alchemy first), like the market
            // maker's switch: never a stuck graph that bans its skills.
            match crate::brain_succession::draw_next(life, live) {
                Some(label) => crate::logln!("brain {}: alcher's first goal {label}", life.account),
                None => crate::logln!("brain {}: alcher drew no goal yet (see the reason above)", life.account),
            }
            lifescape_planner::normalize(life);
            true
        }
        Err(error) => {
            crate::logln!("brain {}: operator ambition alcher refused: {error:?}", life.account);
            lifted
        }
    }
}

/// Unban the skills a role switch banned (`planner::role_lifted_bans`).
fn lift_role_bans(life: &mut Life) -> bool {
    if !lifescape_planner::is_alcher(life) {
        return false;
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0);
    let ticks_since = |stamp: i64| crate::brain_stuck::seconds_to_ticks(now - stamp);
    let skills = lifescape_planner::role_lifted_bans(&life.history, &ticks_since);
    for skill in &skills {
        crate::brain::push_history(life, "goal", &lifescape_planner::unban_entry(skill));
        crate::logln!("brain {}: unban:{skill} (the alcher switch caused it)", life.account);
    }
    !skills.is_empty()
}

#[cfg(test)]
mod tests {
    /// User 2026-11-20: the retired makers are placed on trades by the
    /// operator; the market maker and unknown kinds stay refused.
    #[test]
    fn an_operator_hands_out_alcher_or_any_drawn_trade() {
        for kind in ["alcher", "cook", "fisher", "crafter", "firemaker", "priest", "runecrafter"] {
            assert!(super::request("t_operator", kind).is_ok(), "{kind}");
        }
        assert!(super::request("t_operator", lifescape_planner::MARKET_MAKER_KIND).is_err());
        assert!(super::request("t_operator", "pirate").is_err());
        assert!(super::request("", "cook").is_err());
    }
}
