//! Stranded on Karamja: the customs officer takes the 30-coin fare back
//! from the pack (`customs_officer.rs2`) and the island has no F2P bank,
//! so a bot that landed without it loops NoRoute forever (james_mason,
//! leo_king, tom_king 2026-11-15: a fishing refill withdrew the last 30
//! banked coins, the way out took them). The island's own job pays it:
//! Luthas hands 30 coins for a crate of 10 bananas (`luthas.rs2`,
//! `banana_crate.rs2`; `banana_tree.rs2` needs one free slot per pick).
//! Every run starts here (`lib::run`); off the island or with the fare
//! carried it is a no-op.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::pack::{free_slots, is_worn, obj_id, pack_slots, pack_total};
use crate::quests::common as solver;
use crate::ScriptError;

/// Bananas Luthas wants in his crate before he pays.
const CRATE_BANANAS: i32 = 10;
/// Coins the customs officer takes for the way back.
const FARE_BACK: u32 = knowledge::services::BOAT_FARE_GP as u32;
/// Tick bound for the whole job: 10 picks and packs plus two talks.
const JOB_TICKS: u64 = 2400;

/// True when the bot stands behind the boat without the fare back.
pub fn stranded(island: bool, carried: u32) -> bool {
    island && carried < FARE_BACK
}

/// Earn the fare back with Luthas when stranded on Karamja.
pub fn earn_fare_if_stranded(bot: &mut Bot) -> Result<(), ScriptError> {
    crate::kit::survival::wait_pack_streamed(bot)?;
    let (x, z, level) = (bot.world.x as i64, bot.world.z as i64, bot.world.level as i64);
    let coins = obj_id(bot, "coins")?;
    if !stranded(knowledge::access::behind_boat(x, z, level), pack_total(bot, coins)) {
        return Ok(());
    }
    bot.log(&format!("stranded on Karamja with {} coins at {:?}: filling Luthas' banana crate for the fare", pack_total(bot, coins), bot.position()));
    bot.intend("earning the boat fare: Luthas' banana crate");
    let banana = obj_id(bot, "banana")?;
    let deadline = bot.world.ticks + JOB_TICKS;
    // Hired (or reminded of the task when already hired): the crate only
    // takes bananas from an employee.
    solver::talk_like(bot, "luthas", &["employment", "what did i have"], None, 400)?;
    let seen = bot.world.messages.len();
    while !crate_full(bot, seen) && bot.world.ticks < deadline {
        if pack_total(bot, banana) == 0 {
            if free_slots(bot) == 0 {
                drop_cheapest(bot, banana, coins)?;
            }
            let before = pack_total(bot, banana);
            let inner = bot.world.ticks + 300;
            while pack_total(bot, banana) == before && bot.world.ticks < inner.min(deadline) {
                let _ = solver::operate_loc(bot, &["bananatree"], 1, None);
            }
            continue;
        }
        solver::use_item_on_loc(bot, "banana", &["bananacrate"])?;
    }
    if !crate_full(bot, seen) {
        return Err(ScriptError::Failed(format!(
            "stranded on Karamja: crate not full ({} bananas packed) after {JOB_TICKS} ticks at {:?}",
            packed_since(bot, seen),
            bot.position()
        )));
    }
    // The payment needs a slot unless coins already stack in the pack
    // (tom_king 2026-11-15: a full pack, "Luthas hands you 30 coins.",
    // none carried afterwards).
    if pack_total(bot, coins) == 0 && free_slots(bot) == 0 {
        drop_cheapest(bot, banana, coins)?;
    }
    solver::talk_like(bot, "luthas", &["thank"], None, 400)?;
    // The coins land a few ticks after the dialog (tom_king read 0 right
    // after "Luthas hands you 30 coins.").
    let landed = bot.world.ticks + 20;
    while pack_total(bot, coins) < FARE_BACK && bot.world.ticks < landed {
        bot.pump(2)?;
    }
    let carried = pack_total(bot, coins);
    if carried < FARE_BACK {
        return Err(ScriptError::Failed(format!("stranded on Karamja: Luthas paid, {carried} coins carried at {:?}", bot.position())));
    }
    bot.log(&format!("fare earned: {carried} coins carried, sailing back is open"));
    Ok(())
}

/// A run that does not sail (no `boat` order) starting on Karamja goes
/// home first: its paths are unpaid, so every bank and site on the
/// mainland reads `Disconnected` from the island (leo_king 2026-11-15:
/// fare earned, 192 NoRoute to the Draynor bank from a melee run). The
/// customs crossing alone is opened, for this walk only.
pub fn leave_island_unless_sailing(bot: &mut Bot, args: &[String]) -> Result<(), ScriptError> {
    let (x, z, level) = (bot.world.x as i64, bot.world.z as i64, bot.world.level as i64);
    if !knowledge::access::behind_boat(x, z, level) || args.iter().any(|arg| arg == "boat") {
        return Ok(());
    }
    let (dock_x, dock_z, dock_level) = knowledge::access::KARAMJA_DOCK;
    bot.log(&format!("on Karamja without a boat order at {:?}: sailing back to Port Sarim first", bot.position()));
    bot.intend("sailing back from Karamja");
    let was_paid = bot.allow_paid;
    bot.allow_paid = true;
    let walked = bot.travel_to(lifescape_sdk::plan::Tile { x: dock_x as i32, z: dock_z as i32, level: dock_level as i32 });
    bot.allow_paid = was_paid;
    walked?;
    Ok(())
}

/// The crate is full: the server said so (`banana_crate.rs2`: "The crate
/// is already full."), or ten bananas went in since the job began. The
/// `crate_bananas` varp never reaches the client (leo_king 2026-11-15:
/// read 0 while the server refused a full crate).
fn crate_full(bot: &Bot, seen: usize) -> bool {
    solver::varp(bot, "crate_bananas") >= CRATE_BANANAS
        || bot.world.messages.iter().skip(seen).any(|line| line.to_lowercase().contains("crate is already full"))
        || packed_since(bot, seen) >= CRATE_BANANAS as usize
}

/// Bananas packed since message index `seen` ("You pack a banana into the
/// crate." is a mesbox: chat text, counted from the game messages too).
fn packed_since(bot: &Bot, seen: usize) -> usize {
    bot.world.messages.iter().skip(seen).filter(|line| line.to_lowercase().contains("you pack a banana")).count()
}

/// Drop one unit of the cheapest carried object (never coins, bananas or
/// worn gear) so a banana fits: the pick refuses a full pack.
fn drop_cheapest(bot: &mut Bot, banana: u16, coins: u16) -> Result<(), ScriptError> {
    let cheapest = pack_slots(bot)
        .iter()
        .map(|(_, (held, _))| *held)
        .filter(|held| *held != banana && *held != coins && !is_worn(bot, *held))
        .min_by_key(|held| knowledge::obj_by_id(usize::from(*held)).map(|entry| entry.price).unwrap_or(0));
    let Some(victim) = cheapest else {
        return Err(ScriptError::Failed(format!("stranded on Karamja: full pack with nothing to drop at {:?}", bot.position())));
    };
    bot.log(&format!("dropping one {} to free a slot for a banana", crate::kit::pack::obj_label(bot, victim)));
    bot.drop(victim, 750)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::stranded;

    #[test]
    fn only_the_island_without_the_fare_is_stranded() {
        assert!(stranded(true, 0));
        assert!(stranded(true, 29));
        assert!(!stranded(true, 30), "the fare back is carried");
        assert!(!stranded(false, 0), "the mainland banks");
        assert!(lifescape_gamedata::access::behind_boat(2948, 3146, 0), "the Musa Point dock is the island");
        assert!(!lifescape_gamedata::access::behind_boat(3029, 3217, 0), "the Port Sarim dock is the mainland");
    }
}
