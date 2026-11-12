//! Tutorial island design screen (`player_kit`): send the account's own
//! look, then press accept, like the real client (`IDK_SAVEDESIGN` then
//! `IF_BUTTON`). The look is drawn from the account name
//! (`lifescape_gamedata::player_design`): stable across re-runs, distinct
//! between bots, always within the kits/colours the server accepts.

use lifescape_gamedata::player_design::{self, PlayerDesign};
use lifescape_sdk::Bot;

use crate::tutorial::{check, wait_modal_closed};
use crate::ScriptError;

/// `player_kit:accept` (server `[if_button,player_kit:accept]`).
const COM_DESIGN_ACCEPT: u16 = 3651;

pub(crate) fn is_chat_modal(modal: &Option<lifescape_sdk::world::Modal>) -> bool {
    modal.as_ref().is_some_and(|open| open.layer == "chat")
}

fn describe(design: &PlayerDesign) -> String {
    let gender = if design.gender == player_design::GENDER_MAN { "man" } else { "woman" };
    let kits: Vec<String> = design.kits.iter().map(|kit| kit.map_or("-1".to_string(), |id| id.to_string())).collect();
    format!("{gender} kits=[{}] colours={:?}", kits.join(","), design.colours)
}

pub(crate) fn design(bot: &mut Bot) -> Result<(), ScriptError> {
    let target = bot.world.ticks + 50;
    while bot.world.ticks < target && bot.world.modal.is_none() {
        bot.pump(5)?;
    }
    if bot.world.modal.is_none() {
        bot.log("ok no design screen, already validated");
        return Ok(());
    }
    if is_chat_modal(&bot.world.modal) {
        // Resume with a leftover dialog (interrupted run, post-relogin):
        // the accept button lives on the `player_kit` main interface
        // (server `[if_button,player_kit:accept]`), never in a chat box —
        // firing it there answers "I can't reach that!" and the rejection
        // watch kills the run. Close it, the stage loop re-talks.
        bot.log("ok leftover dialog closed, stage loop re-drives");
        bot.close_modal()?;
        return Ok(());
    }
    let look = player_design::design_for_account(&bot.username);
    bot.log(&format!("design {}", describe(&look)));
    bot.save_player_design(look.gender, &look.kits, &look.colours)?;
    bot.if_button(COM_DESIGN_ACCEPT)?;
    check(bot.position(), wait_modal_closed(bot, 50)?, "design accepted")
}

#[cfg(test)]
mod tests {
    use super::{describe, is_chat_modal};
    use lifescape_gamedata::player_design::design_for_account;
    use lifescape_sdk::world::Modal;

    /// Defaut 2 (btjk4cpk, mine, dialogue mineur resté ouvert) : le préambule
    /// tirait l'accept du design dans la boîte de chat — "I can't reach
    /// that!" frais + 20 ticks sans bouger (FAIL verify), ou 50 ticks
    /// d'attente modale (FAIL design accepted). Seul le chat ferme.
    #[test]
    fn leftover_chat_modal_never_takes_design_accept() {
        let chat = Some(Modal { layer: "chat".to_string(), interface_id: 4887 });
        assert!(is_chat_modal(&chat), "resume leftover closes, never presses accept");
        let main = Some(Modal { layer: "main".to_string(), interface_id: 1 });
        assert!(!is_chat_modal(&main), "design screen keeps its accept press");
        assert!(!is_chat_modal(&None), "no modal stays a clean start");
    }

    #[test]
    fn log_line_names_the_whole_look() {
        let line = describe(&design_for_account("t01"));
        assert!(line.starts_with("man ") || line.starts_with("woman "), "{line}");
        assert!(line.contains("kits=[") && line.contains("colours=["), "{line}");
    }
}
