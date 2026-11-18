#![forbid(unsafe_code)]
//! The alchemist (operator-only ambition, user 2026-11-18: « son but est
//! d'acheter des nature runes et les objets en vente au marché et les
//! alcher ; il va alcher ce qui rapporte le plus ; c'est sa mission
//! principale »). No ladder, held for good (`ambition::pinned`), never
//! drawn by the fleet (`ambition::ladderless`). The brain draws its
//! alchemy goal first, at a zero time cost (`brain_alch`); with nothing
//! to alch the planner hands it money, which funds the purchases.

use crate::ambition::{new_ambition, AmbitionError, BotNow, Stamp, ALCHER};
use crate::model::Life;

/// History cause of the nodes the switch abandons.
pub const ALCHER_ROLE_CAUSE: &str = "role:alcher";

/// True for a life whose ambition is the alchemist.
pub fn is_alcher(life: &Life) -> bool {
    life.ambition.as_ref().is_some_and(|ambition| ambition.kind == ALCHER)
}

/// Make `life` the alchemist: its open nodes abandoned (the operator's
/// role switch, `lifecycle::abandon_for_role`), the ambition replaced,
/// the goal left for the brain's next draw. Returns the abandoned ids.
pub fn become_alcher(life: &mut Life, now: &BotNow, stamp: Stamp) -> Result<Vec<String>, AmbitionError> {
    let ambition = new_ambition(ALCHER, "operator:alcher", now, life.seed, &[], stamp)?;
    let abandoned = crate::lifecycle::abandon_for_role(life, ALCHER_ROLE_CAUSE);
    life.ambition = Some(ambition);
    life.goal_origin = None;
    crate::lifecycle::normalize(life);
    Ok(abandoned)
}

/// Active bans whose cause is the role switch itself (its abandoned
/// nodes read as a stuck graph): lifted at once (the brain writes
/// `unban:<skill>` for each), sorted by skill.
pub fn role_lifted_bans(history: &[crate::model::HistoryEntry], ticks_since: &dyn Fn(i64) -> u64) -> Vec<String> {
    crate::bans::ban_states(history, ticks_since)
        .into_iter()
        .filter_map(|(skill, state)| match state {
            crate::bans::BanState::Active { cause, .. } if cause.contains(ALCHER_ROLE_CAUSE) => Some(skill),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
#[path = "alcher_tests.rs"]
mod tests;
