//! Rune Mysteries, step for step from the server scripts
//! (`area_lumbridge/duke_horacio.rs2`, `area_wizard_tower/sedridor.rs2`,
//! `area_varrock/aubury.rs2`).
//!
//! The quest varp is server-only (`quest_runemysteries.varp`: `scope=perm`,
//! no `transmit`): the client never learns the stage, so the old
//! varp-gated solver read 0 forever. The stage is read off the quest
//! objects the pack holds instead, and every talk is safe at any stage:
//! - research notes held (stage 5) -> Sedridor takes them, quest complete;
//! - research package held (stage 3) -> Aubury takes it (stage 4), then
//!   hands his notes on the next talk (stage 5);
//! - neither -> the Duke ("Have you any quests for me?" -> "Sure, no
//!   problem.", air talisman, stage 1), Sedridor ("I'm looking for the
//!   head wizard." -> "Ok, here you are." -> "Yes, certainly.", package,
//!   stage 3; a lost package is re-issued), then Aubury when the round
//!   ended with no object (stage 4 after a crash: notes; stage 5 lost:
//!   duplicates).
//!
//! Answers never open a shop: Aubury's rune shop is declined ("No thank
//! you"), Sedridor's idle line is "Nothing thanks".

use lifescape_sdk::Bot;

use super::common as solver;
use crate::ScriptError;

/// Duke Horacio, Lumbridge castle level 1: quest offer then acceptance.
pub const DUKE_ANSWERS: [&str; 2] = ["have you any quests", "sure, no problem"];
/// Sedridor (`head_wizard`), tower basement: the talisman handover then
/// the package errand; idle otherwise.
pub const SEDRIDOR_ANSWERS: [&str; 4] = ["looking for the head wizard", "ok, here you are", "yes, certainly", "nothing thanks"];
/// Aubury, Varrock rune shop: the package, never the shop.
pub const AUBURY_ANSWERS: [&str; 2] = ["sent here with a package", "no thank you"];
/// Rounds before the solver gives up (a full quest takes three: Duke +
/// Sedridor, Aubury twice, Sedridor).
pub const ROUNDS: u32 = 4;
/// Dialogue tick bound per talk.
const TALK_TICKS: u64 = 400;

/// Pack slots the quest needs free (talisman, package, notes: one at a
/// time, the server `inv_add` has no full-pack branch).
pub const FREE_SLOTS: usize = 1;

/// Where the quest stands, read off the pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Aubury's notes held: back to Sedridor.
    Notes,
    /// Sedridor's package held: to Aubury.
    Package,
    /// Neither: start (or resume from a lost object) at the Duke.
    Start,
}

/// Stage from the counts of notes and package carried.
pub fn stage_of(notes: u32, package: u32) -> Stage {
    if notes > 0 {
        Stage::Notes
    } else if package > 0 {
        Stage::Package
    } else {
        Stage::Start
    }
}

/// Bank everything but the quest objects when the pack is full: the
/// server hands each object with a bare `inv_add`.
fn make_room(bot: &mut Bot, keep: &[u16]) -> Result<(), ScriptError> {
    if crate::kit::pack::free_slots(bot) >= FREE_SLOTS {
        return Ok(());
    }
    bot.intend("rune mysteries: banking to free a slot");
    let bank = crate::kit::bank::nearest_bank(bot)?;
    crate::kit::bank::bank_trip(bot, &bank, keep, 0)?;
    Ok(())
}

/// Quest objects a bank trip put away (live 2026-11-14, bct5rxpg: the
/// Duke's air talisman banked at 09:16, every later round read Start
/// and Sedridor never took it): withdrawn before the stage is read, the
/// stage being read off the pack only.
fn fetch_banked(bot: &mut Bot, ids: &[u16]) -> Result<(), ScriptError> {
    if ids.iter().any(|id| solver::count(bot, *id) > 0) {
        return Ok(());
    }
    // The bank view streams only while a booth is open (a fresh session
    // reads it empty): open the nearest one, read it, withdraw there.
    bot.intend("rune mysteries: checking the bank for the quest objects");
    let bank = crate::kit::bank::nearest_bank(bot)?;
    if !bot.open_bank(bank.x as i32, bank.z as i32, bank.loc_id as u16, 300)? {
        return Ok(());
    }
    bot.pump(3)?;
    let banked = bot.bank_items();
    let mut pulled = 0;
    for id in ids {
        if banked.get(id).copied().unwrap_or(0) > 0 && bot.bank_withdraw(*id, 1, 50).is_ok() {
            pulled += 1;
        }
    }
    bot.close_modal()?;
    bot.pump(2)?;
    bot.log(&format!("rune mysteries: {pulled} quest object(s) withdrawn from the bank"));
    Ok(())
}

pub(crate) fn solve(bot: &mut Bot) -> Result<(), ScriptError> {
    bot.intend("quest runemysteries: delivering research");
    let notes = solver::obj_id("research_notes")?;
    let package = solver::obj_id("research_package")?;
    let talisman = solver::obj_id("air_talisman")?;
    make_room(bot, &[notes, package, talisman])?;
    fetch_banked(bot, &[notes, package, talisman])?;
    for round in 0..ROUNDS {
        let stage = stage_of(solver::count(bot, notes), solver::count(bot, package));
        bot.log(&format!("rune mysteries round {round}: {stage:?}"));
        match stage {
            Stage::Notes => {
                solver::talk_like(bot, "head_wizard", &SEDRIDOR_ANSWERS, None, TALK_TICKS)?;
                return Ok(());
            }
            Stage::Package => {
                solver::talk_like(bot, "aubury", &AUBURY_ANSWERS, None, TALK_TICKS)?;
                solver::talk_like(bot, "aubury", &AUBURY_ANSWERS, None, TALK_TICKS)?;
            }
            Stage::Start => {
                if round == 0 {
                    solver::talk_like(bot, "duke_of_lumbridge", &DUKE_ANSWERS, None, TALK_TICKS)?;
                }
                solver::talk_like(bot, "head_wizard", &SEDRIDOR_ANSWERS, None, TALK_TICKS)?;
                if stage_of(solver::count(bot, notes), solver::count(bot, package)) == Stage::Start {
                    solver::talk_like(bot, "aubury", &AUBURY_ANSWERS, None, TALK_TICKS)?;
                }
            }
        }
    }
    Err(ScriptError::Failed(format!(
        "rune mysteries: no notes after {ROUNDS} rounds at {:?} (notes {}, package {}, talisman {})",
        bot.position(),
        solver::count(bot, notes),
        solver::count(bot, package),
        solver::count(bot, talisman)
    )))
}

#[cfg(test)]
mod tests {
    use super::{stage_of, Stage, AUBURY_ANSWERS, DUKE_ANSWERS, SEDRIDOR_ANSWERS};

    /// Server option texts (content scripts, verbatim): each answer list
    /// picks the quest line and never the shop.
    #[test]
    fn answers_match_server_options() {
        let picks = |keywords: &[&str], options: &[&str]| -> Option<String> {
            keywords.iter().find_map(|keyword| options.iter().find(|option| option.to_lowercase().contains(keyword)).map(|found| found.to_string()))
        };
        assert_eq!(picks(&DUKE_ANSWERS, &["Have you any quests for me?", "Where can I find money?"]).as_deref(), Some("Have you any quests for me?"));
        assert_eq!(
            picks(&DUKE_ANSWERS, &["I seek a shield that will protect me from the dragon's breath.", "Have you any quests for me?", "Where can I find money?"]).as_deref(),
            Some("Have you any quests for me?"),
            "dragon slayer variant"
        );
        assert_eq!(picks(&DUKE_ANSWERS, &["Sure, no problem.", "Not right now."]).as_deref(), Some("Sure, no problem."));
        assert_eq!(
            picks(&SEDRIDOR_ANSWERS, &["Nothing thanks, I'm just looking around.", "What are you doing down here?", "I'm looking for the head wizard."]).as_deref(),
            Some("I'm looking for the head wizard.")
        );
        assert_eq!(picks(&SEDRIDOR_ANSWERS, &["Ok, here you are.", "No, I'll only give it to the head wizard."]).as_deref(), Some("Ok, here you are."));
        assert_eq!(picks(&SEDRIDOR_ANSWERS, &["Yes, certainly.", "No, I'm busy."]).as_deref(), Some("Yes, certainly."));
        assert_eq!(
            picks(&SEDRIDOR_ANSWERS, &["Nothing thanks, I'm just looking around.", "Can you teleport me to the Rune Essence?"]).as_deref(),
            Some("Nothing thanks, I'm just looking around."),
            "quest done: idle line"
        );
        assert_eq!(
            picks(&AUBURY_ANSWERS, &["Yes please!", "Oh, it's a rune shop. No thank you, then.", "I have been sent here with a package for you."]).as_deref(),
            Some("I have been sent here with a package for you.")
        );
        assert_eq!(picks(&AUBURY_ANSWERS, &["Yes please!", "Oh, it's a rune shop. No thank you, then."]).as_deref(), Some("Oh, it's a rune shop. No thank you, then."), "never the shop");
    }

    /// Stage from the pack: notes win, then the package, else the start.
    #[test]
    fn stage_reads_the_pack() {
        assert_eq!(stage_of(1, 0), Stage::Notes);
        assert_eq!(stage_of(1, 1), Stage::Notes);
        assert_eq!(stage_of(0, 1), Stage::Package);
        assert_eq!(stage_of(0, 0), Stage::Start);
    }

    /// Quest objects resolve in the dumps (debugnames from the scripts).
    #[test]
    fn quest_objects_resolve() {
        for name in ["research_notes", "research_package", "air_talisman"] {
            super::solver::obj_id(name).unwrap_or_else(|_| panic!("{name} in dumps"));
        }
    }
}
