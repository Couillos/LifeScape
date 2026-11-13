//! Args contract check (planner `mapping.rs` → script, docs/planner.md
//! "Contrat d'arguments"): each launch line parses in its script's OWN
//! parser, without side effect. The registry dispatch stays in `lib.rs`;
//! this only reads arguments, never the world.

#![forbid(unsafe_code)]

use crate::ScriptError;

/// Parse a brain order the way `script` will at launch. `Usage` here is
/// a contract break (the same launch would fail the same way live).
pub fn check_order(script: &str, args: &[String]) -> Result<(), ScriptError> {
    let Some(known) = crate::names().into_iter().find(|name| *name == script) else {
        return Err(ScriptError::Usage(format!("unknown script {script:?}")));
    };
    match known {
        "market" => crate::activities::market::parse_order(args).map(drop),
        "walkto" | "goto" | "debug_walk" => crate::parse_target(args).map(drop),
        "equip" => crate::activities::equip::parse_style(args).map(drop),
        // Quest solvers read the quest state, not arguments: the node and
        // budget ride along for the session report only.
        name if name.starts_with("quest_") => Ok(()),
        name => crate::kit::cycle::parse_order(name, args).map(drop),
    }
}

#[cfg(test)]
#[path = "contract_tests.rs"]
mod tests;
