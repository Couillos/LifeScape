//! Scripts the orchestrator may launch, mirroring orchestration/registry.py.
//!
//! Compiled in: no dynamic reload (Python re-imports the module when the
//! file changes; here adding a script means editing this table).

/// (script name, path under the project root).
pub const SCRIPTS: &[(&str, &str)] = &[
    ("fishing", "scripts/fishing/run.py"),
    ("mining", "scripts/mining/run.py"),
    ("woodcut", "scripts/woodcut/run.py"),
    ("melee", "scripts/melee/run.py"),
    ("tutorial", "scripts/tutorial/run.py"),
    ("quest_black_knights_fortress", "scripts/quests/black_knights_fortress.py"),
    ("quest_cooks_assistant", "scripts/quests/cooks_assistant.py"),
    ("quest_demon_slayer", "scripts/quests/demon_slayer.py"),
    ("quest_dorics_quest", "scripts/quests/dorics_quest.py"),
    ("quest_dragon_slayer", "scripts/quests/dragon_slayer.py"),
    ("quest_ernest_the_chicken", "scripts/quests/ernest_the_chicken.py"),
    ("quest_goblin_diplomacy", "scripts/quests/goblin_diplomacy.py"),
    ("quest_imp_catcher", "scripts/quests/imp_catcher.py"),
    ("quest_knights_sword", "scripts/quests/knights_sword.py"),
    ("quest_pirates_treasure", "scripts/quests/pirates_treasure.py"),
    ("quest_prince_ali_rescue", "scripts/quests/prince_ali_rescue.py"),
    ("quest_restless_ghost", "scripts/quests/restless_ghost.py"),
    ("quest_romeo_and_juliet", "scripts/quests/romeo_and_juliet.py"),
    ("quest_rune_mysteries", "scripts/quests/rune_mysteries.py"),
    ("quest_sheep_shearer", "scripts/quests/sheep_shearer.py"),
    ("quest_shield_of_arrav", "scripts/quests/shield_of_arrav.py"),
    ("quest_vampire_slayer", "scripts/quests/vampire_slayer.py"),
    ("quest_witchs_potion", "scripts/quests/witchs_potion.py"),
    ("rescue_maze", "scripts/rescue_maze.py"),
    ("rescue_mime", "scripts/rescue_mime.py"),
    ("rescue_cube", "scripts/rescue_cube.py"),
    ("rescue_down", "scripts/rescue_down.py"),
    ("rescue_fallback", "scripts/rescue_fallback.py"),
    ("check_transitions", "scripts/dev/check_transitions.py"),
    ("check_doors", "scripts/dev/check_doors.py"),
    ("check_routes", "scripts/dev/check_routes.py"),
    ("goto", "scripts/dev/goto.py"),
    ("walkto", "scripts/dev/walkto.py"),
    ("walktour", "scripts/dev/walktour.py"),
    ("bank_deposit", "scripts/dev/bank_deposit.py"),
    ("drop", "scripts/dev/drop.py"),
    ("debug_walk", "scripts/dev/debug_walk.py"),
    ("trade_swap", "scripts/dev/trade_swap.py"),
    ("quest_reset", "scripts/dev/quest_reset.py"),
    ("reset_quests", "scripts/dev/reset_quests.py"),
    ("give", "scripts/dev/give.py"),
];

/// Random events map to their rescue in the scripts crate
/// (`lifescape_scripts::randomevent::rescue_for`); the worker poses them
/// directly, so no daemon-side table is kept here.

pub fn script_names_sorted() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = SCRIPTS.iter().map(|(name, _)| *name).collect();
    names.sort_unstable();
    names
}

pub fn script_path(name: &str) -> Result<String, String> {
    match SCRIPTS.iter().find(|(entry, _)| *entry == name) {
        Some((_, rel)) => Ok(format!("{}/{rel}", crate::paths::root().display())),
        None => Err(format!(
            "unknown script '{}' (known: {})",
            name,
            script_names_sorted().join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_and_unknown() {
        assert!(script_path("fishing").unwrap().ends_with("scripts/fishing/run.py"));
        let error = script_path("nope").unwrap_err();
        assert!(error.starts_with("unknown script 'nope' (known: "));
    }
}
