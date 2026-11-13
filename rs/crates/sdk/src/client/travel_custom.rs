//! Live executor of hand-added plan edges (`Segment::Custom`), the
//! Karamja boat first (server: `sailors.rs2`, `customs_officer.rs2`,
//! `interface_boat/scripts/sail.rs2`, `gangplank.rs2`):
//! - "npc_fare": talk to one of the edge's NPCs, answer its dialogue
//!   with the edge's choices (any page order), the server takes the fare
//!   from the PACK and `~set_sail` shows the journey interface for a few
//!   ticks, then telejumps onto the ship deck (`expect`);
//! - "loc": click the loc op (the disembark gangplank), the server
//!   teleports then telejumps onto the dock (`expect`).
//! Every wait is bounded in ticks; a refusal is a typed error, never a
//! blind retry loop.

use lifescape_nav::plan::{CustomEdgeRef, Tile};

use crate::client::Bot;
use crate::error::{Error, Result};

/// Coins object id (pack fare check).
const COINS_OBJ: u16 = 995;
/// Ticks for the NPC to come into view once on the stand.
const NPC_SIGHT_WAIT: u64 = 10;
/// Ticks for the dialogue to open after the click (walk to the NPC).
const DIALOG_OPEN_WAIT: u64 = 25;
/// Ticks for one whole fare dialogue plus the sail (`~set_sail`:
/// `p_delay(7)` behind the journey interface).
const FARE_WAIT: u64 = 120;
/// Ticks for a loc teleport (gangplank: teleport, `p_delay(0)`, jump).
const LOC_JUMP_WAIT: u64 = 20;
/// Talk attempts per fare edge: an officer that confiscated rum ends the
/// dialogue without sailing; the second talk sails.
const FARE_TALKS: u32 = 2;

/// Al-Kharid toll (`border_gate.rs2`: `inv_total(inv, coins) < 10`
/// refuses with a chatplayer line, no message, the gate stays shut).
pub(crate) const TOLL_FARE: u32 = 10;

/// Paid paths only with the toll in the pack: a script's `allow_paid`
/// on an empty wallet routed through the gate, the guard said "Oh dear,
/// I don't actually seem to have enough money" and the door segment
/// failed twice (live 2026-11-13, bct5rxpg-13908/13942, ground_pickup
/// with no coins). Without the fare the planner walks around the fence
/// (golden `peage_detour`) or reports `PaidOnly coins:10`.
pub(crate) fn paid_allowed(allow_paid: bool, coins: u32) -> bool {
    allow_paid && coins >= TOLL_FARE
}

/// The dialogue option to pick among `options`: the first edge choice
/// (in edge order) some option contains, case-insensitive. `None` when no
/// option matches (the dialogue asks something the edge does not know).
pub(crate) fn fare_pick(options: &[(u16, String)], choices: &[String]) -> Option<String> {
    choices.iter().find_map(|choice| {
        let wanted = choice.to_lowercase();
        options.iter().find(|(_, text)| text.to_lowercase().contains(&wanted)).map(|(_, text)| text.clone())
    })
}

/// Ticks a closed dialogue may stay closed before the talk counts as
/// ended without a sail (the chat page closes before the journey
/// interface opens).
const CLOSED_GRACE: u64 = 6;

/// True when a game message since `seen` says the fare was taken
/// (`mes("You pay the 30 coins...")`, customs "You pay 30 coins and
/// board the ship."): the sail is underway, keep waiting.
fn fare_paid(bot: &Bot, seen: usize) -> bool {
    bot.world.messages.get(seen..).unwrap_or(&[]).iter().any(|line| line.to_lowercase().contains("you pay"))
}

fn at(bot: &Bot, tile: Tile) -> bool {
    (bot.world.x, bot.world.z, bot.world.level) == (tile.x, tile.z, tile.level)
}

fn last_game_message(bot: &Bot) -> String {
    bot.world.messages.last().cloned().unwrap_or_else(|| "no game message".to_string())
}

impl Bot {
    /// Ride one hand-added edge from its stand to `expect`.
    pub(crate) fn exec_custom(&mut self, edge: &CustomEdgeRef, stand: Tile, expect: Tile, deadline: u64) -> Result<()> {
        if !at(self, stand) && !at(self, expect) {
            self.exec_walk(&[stand], deadline).map_err(|err| {
                if crate::error::is_interrupted(&err) || matches!(err, Error::Io(_) | Error::Login(_)) {
                    err
                } else {
                    Error::StepRefused(format!("custom {}: off stand ({},{},{}) at {:?}: {err}", edge.name, stand.x, stand.z, stand.level, self.position()))
                }
            })?;
        }
        if at(self, expect) {
            return Ok(());
        }
        match edge.kind.as_str() {
            "loc" => self.exec_custom_loc(edge, expect, deadline),
            "npc_fare" => self.exec_custom_fare(edge, expect, deadline),
            other => Err(Error::Travel(format!("custom edge {} has no live executor (kind {other:?})", edge.name))),
        }
    }

    /// Gangplank-style loc: one click, then the jump onto `expect`.
    fn exec_custom_loc(&mut self, edge: &CustomEdgeRef, expect: Tile, deadline: u64) -> Result<()> {
        let Some(loc_tile) = edge.at else {
            return Err(Error::Travel(format!("custom edge {} (loc) carries no loc tile", edge.name)));
        };
        // The arrival mesbox of a sail ("The ship arrives at ...") waits on
        // its pause button: continue it so the click is not eaten.
        if self.world.modal.as_ref().is_some_and(|modal| modal.layer != "main") && self.chat_options().is_empty() {
            self.chat_continue()?;
            self.pump(1)?;
        }
        let start = (self.world.x, self.world.z, self.world.level);
        self.op_loc(edge.op, loc_tile.x, loc_tile.z, edge.loc)?;
        let watched = self.world.ticks + LOC_JUMP_WAIT;
        while self.world.ticks < watched {
            self.pump(1)?;
            if at(self, expect) {
                return Ok(());
            }
            if self.world.level != start.2 || (self.world.x - start.0).abs().max((self.world.z - start.1).abs()) > 3 {
                return Err(Error::RideWrongDestination(format!(
                    "custom {} landed at {:?}, want ({},{},{}); last game: {}",
                    edge.name, self.position(), expect.x, expect.z, expect.level, last_game_message(self),
                )));
            }
            if self.world.ticks > deadline {
                return Err(Error::Timeout(format!("custom {} timed out at {:?}", edge.name, self.position())));
            }
        }
        Err(Error::RideDidNotMove(format!(
            "custom {} (loc {} at ({},{},{})) never moved us from {:?}; last game: {}",
            edge.name, edge.loc, loc_tile.x, loc_tile.z, loc_tile.level, self.position(), last_game_message(self),
        )))
    }

    /// NPC fare: talk, answer, pay from the pack, sail onto `expect`.
    fn exec_custom_fare(&mut self, edge: &CustomEdgeRef, expect: Tile, deadline: u64) -> Result<()> {
        if !self.allow_paid {
            return Err(Error::Travel(format!("custom {}: {} coins fare with paid paths off at {:?}", edge.name, edge.fare, self.position())));
        }
        let carried = self.pack_count(COINS_OBJ);
        if carried < edge.fare {
            return Err(Error::Travel(format!("custom {}: fare {} coins, pack carries {carried} at {:?}", edge.name, edge.fare, self.position())));
        }
        let names: Vec<&str> = edge.npcs.iter().map(String::as_str).collect();
        let mut last_error = None;
        for _ in 0..FARE_TALKS {
            match self.talk_fare_once(edge, &names, expect, deadline) {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(err @ (Error::Io(_) | Error::Login(_) | Error::Interrupted(_) | Error::Timeout(_) | Error::NeedsDialogueChoice(_) | Error::RideWrongDestination(_))) => return Err(err),
                Err(err) => last_error = Some(err),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            Error::RideDidNotMove(format!(
                "custom {}: {} talks to {:?} never sailed from {:?}; last game: {}",
                edge.name, FARE_TALKS, edge.npcs, self.position(), last_game_message(self),
            ))
        }))
    }

    /// One talk: Ok(true) landed on `expect`, Ok(false) the dialogue
    /// closed without a sail (confiscated rum, wandered NPC).
    fn talk_fare_once(&mut self, edge: &CustomEdgeRef, names: &[&str], expect: Tile, deadline: u64) -> Result<bool> {
        let sight = self.world.ticks + NPC_SIGHT_WAIT;
        let slot = loop {
            let mut seen = self.npcs_named(names);
            seen.sort_by_key(|(_, x, z)| (x - self.world.x).abs() + (z - self.world.z).abs());
            if let Some((slot, _, _)) = seen.first() {
                break *slot;
            }
            if self.world.ticks >= sight {
                return Err(Error::RideDidNotMove(format!("custom {}: no {:?} in sight at {:?}", edge.name, edge.npcs, self.position())));
            }
            self.pump(1)?;
        };
        // A dialogue left open is not this NPC's answer: the click would
        // close it and the real one opens only on arrival.
        if self.world.modal.is_some() {
            self.close_modal()?;
            self.pump(1)?;
        }
        let seen_messages = self.world.messages.len();
        let start_level = self.world.level;
        self.talk_to(slot)?;
        let opened_by = self.world.ticks + DIALOG_OPEN_WAIT;
        let watched = self.world.ticks + FARE_WAIT;
        let mut opened = false;
        let mut closed_since: Option<u64> = None;
        let mut last_continue = 0u64;
        let mut last_pick: Option<(String, u64)> = None;
        while self.world.ticks < watched {
            self.pump(1)?;
            if at(self, expect) {
                self.log(&format!("custom {}: sailed to ({},{},{}) for {} coins", edge.name, expect.x, expect.z, expect.level, edge.fare));
                return Ok(true);
            }
            if self.world.level != start_level {
                return Err(Error::RideWrongDestination(format!(
                    "custom {} landed at {:?}, want ({},{},{}); last game: {}",
                    edge.name, self.position(), expect.x, expect.z, expect.level, last_game_message(self),
                )));
            }
            if self.world.ticks > deadline {
                return Err(Error::Timeout(format!("custom {} timed out at {:?}", edge.name, self.position())));
            }
            let Some(modal) = self.world.modal.clone() else {
                if fare_paid(self, seen_messages) {
                    continue;
                }
                let since = *closed_since.get_or_insert(self.world.ticks);
                if (opened && self.world.ticks >= since + CLOSED_GRACE) || (!opened && self.world.ticks >= opened_by) {
                    // Closed without a sail (rum confiscated, fare short)
                    // or never opened (NPC wandered out of reach).
                    return Ok(false);
                }
                continue;
            };
            opened = true;
            closed_since = None;
            // The journey interface (main layer) runs the sail: wait.
            if modal.layer == "main" {
                continue;
            }
            let options = self.chat_options();
            if options.is_empty() {
                if self.world.ticks >= last_continue + 2 {
                    self.chat_continue()?;
                    last_continue = self.world.ticks;
                }
                continue;
            }
            let Some(pick) = fare_pick(&options, &edge.choices) else {
                let prompt = options.iter().map(|(_, text)| text.clone()).collect::<Vec<_>>().join(" / ");
                let _ = self.close_modal();
                return Err(Error::NeedsDialogueChoice(format!("custom {} asked {prompt} at {:?}", edge.name, self.position())));
            };
            // One click per page: the same option still shown a tick
            // later is the page not yet replaced.
            if last_pick.as_ref().is_some_and(|(text, tick)| *text == pick && self.world.ticks < tick + 3) {
                continue;
            }
            self.chat_choose(&pick)?;
            last_pick = Some((pick, self.world.ticks));
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::{fare_pick, paid_allowed};

    /// Live 2026-11-13 (bct5rxpg-13908): allow_paid with an empty wallet
    /// planned through the toll the guard then refused. Paid paths need
    /// the 10-coin toll in the pack; the script switch still gates them.
    #[test]
    fn paid_paths_need_the_toll_in_the_pack() {
        assert!(!paid_allowed(true, 0), "broke: walk around");
        assert!(!paid_allowed(true, 9), "one short: walk around");
        assert!(paid_allowed(true, 10), "the toll in hand: pay");
        assert!(!paid_allowed(false, 500), "the script never allowed paying");
    }

    fn options(texts: &[&str]) -> Vec<(u16, String)> {
        texts.iter().enumerate().map(|(index, text)| (index as u16, text.to_string())).collect()
    }

    /// The sailor and the customs officer pages (server dialogue text):
    /// each page answers with its own edge choice, never the refusal.
    #[test]
    fn fare_dialogues_pick_the_paying_answers() {
        let sailor = vec!["Yes please.".to_string()];
        assert_eq!(fare_pick(&options(&["Yes please.", "No, thank you."]), &sailor).as_deref(), Some("Yes please."));
        assert_eq!(fare_pick(&options(&["I'd rather go to Crandor Isle.", "Yes please.", "No, thank you."]), &sailor).as_deref(), Some("Yes please."));
        let customs = vec!["Can I journey on this ship?".to_string(), "Search away, I have nothing to hide.".to_string(), "Ok.".to_string()];
        assert_eq!(fare_pick(&options(&["Can I journey on this ship?", "Does Karamja have unusual customs then?"]), &customs).as_deref(), Some("Can I journey on this ship?"));
        assert_eq!(
            fare_pick(&options(&["Why?", "Search away, I have nothing to hide.", "You're not putting your hands on my things!"]), &customs).as_deref(),
            Some("Search away, I have nothing to hide.")
        );
        assert_eq!(fare_pick(&options(&["Ok.", "Oh, I'll not bother then."]), &customs).as_deref(), Some("Ok."));
        assert_eq!(fare_pick(&options(&["Tell me a story."]), &customs), None, "an unknown page is never guessed");
    }
}
