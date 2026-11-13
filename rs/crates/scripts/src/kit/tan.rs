//! Tanner brick: turn carried hides into leather at an NPC tanner.
//! Grid verified against the 274 server scripts
//! (`content/scripts/areas/area_alkharid/scripts/tanner.rs2` labels +
//! `content/scripts/interfaces/tanner.if` buttons):
//!
//! - soft (cow_hide → leather, 1 gp): tan 1 = com_96, 5 = com_95, all = com_93
//! - hard (cow_hide → hard_leather, 3 gp): tan 1 = com_101, 5 = com_100, all = com_98
//! - dragon rows are members-only, never pressed here (F2P script).
//!
//! Flow: Trade (op 3) opens the tanner modal straight away
//! (`tanner.rs2` opnpc3, no dialog); when the grid does not open,
//! fall back to talk (op 1) → answer "Yes please." → modal.
//! An out-of-view Ellis resyncs (pump between tries), the first talk
//! retries once, then the run fails loud.
//! press the 1/5/all bucket (the server tans min(pressed, hides,
//! coins/cost)) → leather lands in the pack.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::pack::{obj_id, pack_total};
use crate::skill_walk::walk_checked;
use crate::{ScriptError, ScriptResult};

/// Tanning NPC family (Al-Kharid Ellis + Crafting Guild share it).
pub const TANNERS: [&str; 1] = ["tanner"];
/// Al-Kharid tanner stand (services dump, `tan_soft`).
pub const TANNER_X: i32 = 3276;
pub const TANNER_Z: i32 = 3193;
/// F2P costs per hide, from `tanner.constant` (soft 1, hard 3).
pub const TAN_SOFT_COST: u32 = 1;
pub const TAN_HARD_COST: u32 = 3;
/// Bucket buttons, soft then hard (tan 1 / 5 / all).
pub const TAN_SOFT_COMS: [&str; 3] = ["tanner:com_96", "tanner:com_95", "tanner:com_93"];
pub const TAN_HARD_COMS: [&str; 3] = ["tanner:com_101", "tanner:com_100", "tanner:com_98"];
/// Tick budgets: NPC approach, chat/modal waits, tan landing.
pub const TANNER_WALK_TICKS: u64 = 15000;
pub const TANNER_APPROACH_TICKS: u64 = 3000;
pub const TANNER_UI_TICKS: u64 = 200;
/// Trade op (`tanner.rs2` opnpc3): opens the grid with no dialog.
pub const TANNER_TRADE_OP: u8 = 3;
/// Resync when Ellis is out of view: rounds x pump ticks before the
/// run fails loud (stale stream, not a missing NPC, is the usual miss).
pub const TANNER_RESYNC_ROUNDS: u32 = 3;
pub const TANNER_RESYNC_PUMP: u32 = 10;

/// Hide, leather, cost and bucket coms for a sewn `product` (`None`
/// when the recipe needs no tanning or is members-only).
pub fn tan_spec(product: &str) -> Option<(&'static str, &'static str, u32, [&'static str; 3])> {
    let recipe = crate::activities::leatherwork::leather_recipe(product)?;
    if recipe.inputs.iter().any(|(name, _)| name == "hard_leather") {
        Some(("cow_hide", "hard_leather", TAN_HARD_COST, TAN_HARD_COMS))
    } else if recipe.inputs.iter().any(|(name, _)| name == "leather") {
        Some(("cow_hide", "leather", TAN_SOFT_COST, TAN_SOFT_COMS))
    } else {
        None
    }
}

/// Walk to the Al-Kharid tanner from wherever the bank left us.
pub fn walk_to_tanner(bot: &mut Bot) -> Result<(), ScriptError> {
    if !walk_checked(bot, TANNER_X, TANNER_Z, TANNER_WALK_TICKS)? {
        return Err(ScriptError::Failed(format!("no walk to the tanner ({TANNER_X},{TANNER_Z}) from {:?}", bot.position())));
    }
    Ok(())
}

/// Tan every affordable carried hide: talk, answer, press the bucket,
/// wait the leather. Returns hides tanned.
pub fn tan_carried(bot: &mut Bot, hide_name: &str, leather_name: &str, cost: u32, coms: [&str; 3]) -> Result<u32, ScriptError> {
    let hide = obj_id(bot, hide_name)?;
    let leather = obj_id(bot, leather_name)?;
    let coins = obj_id(bot, "coins")?;
    let have = pack_total(bot, hide);
    if have == 0 {
        return Ok(0);
    }
    let afford = pack_total(bot, coins) / cost.max(1);
    let count = have.min(afford);
    if count == 0 {
        return Err(ScriptError::Blocked { missing: vec![format!("coins:{}", have.saturating_mul(cost.max(1)))] });
    }
    let slot = tanner_slot(bot)?;
    bot.approach_npc(slot, TANNER_APPROACH_TICKS)?;
    if !open_trade_direct(bot, slot)? {
        talk_and_open(bot, slot)?;
    }
    let bucket = if count == 1 { coms[0] } else if count <= 5 { coms[1] } else { coms[2] };
    let com = (bot.tables.com_id)(bucket)
        .ok_or_else(|| ScriptError::Usage(format!("{bucket} missing from component table at {:?}", bot.position())))?;
    let before = pack_total(bot, leather);
    bot.if_button(com)?;
    let target = bot.world.ticks + TANNER_UI_TICKS;
    while bot.world.ticks < target {
        bot.pump(2)?;
        if pack_total(bot, leather) > before {
            break;
        }
    }
    if bot.world.modal.is_some() {
        bot.close_modal()?;
        bot.pump(2)?;
    }
    Ok(pack_total(bot, leather).saturating_sub(before))
}

/// Tanner slot with resync: Ellis wanders out of the NPC stream live
/// (minutes invisible); pump between tries so a stale view does not
/// fail the run on the first miss. Bounded, loud when truly gone.
fn tanner_slot(bot: &mut Bot) -> Result<u16, ScriptError> {
    for _ in 0..TANNER_RESYNC_ROUNDS {
        if let Some(slot) = bot.find_npc(TANNERS[0]) {
            return Ok(slot);
        }
        bot.pump(TANNER_RESYNC_PUMP)?;
    }
    Err(ScriptError::Failed(format!("no tanner in view at {:?} (walk to ({TANNER_X},{TANNER_Z}))", bot.position())))
}

/// Trade direct (op 3): `tanner.rs2` opnpc3 opens the grid with no
/// dialog, so no chat lines to answer and no pause to miss. True when
/// the grid opened within the wait.
fn open_trade_direct(bot: &mut Bot, slot: u16) -> Result<bool, ScriptError> {
    bot.op_npc(TANNER_TRADE_OP, slot)?;
    let tanner = (bot.tables.com_id)("tanner")
        .ok_or_else(|| ScriptError::Usage(format!("tanner missing from component table at {:?}", bot.position())))?;
    let target = bot.world.ticks + TANNER_UI_TICKS;
    while bot.world.ticks < target {
        bot.pump(2)?;
        if bot.world.modal.as_ref().is_some_and(|modal| modal.interface_id == tanner) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Talk fallback with one retry: the first click can catch Ellis
/// mid-wander (no offer streams in); re-talk once, then fail loud.
fn talk_and_open(bot: &mut Bot, slot: u16) -> Result<(), ScriptError> {
    bot.talk_to(slot)?;
    if answer_yes_please(bot).is_err() {
        bot.talk_to(slot)?;
        answer_yes_please(bot)?;
    }
    open_tanner(bot)
}

/// Answer the tanner's "tan them for you?" choice with "Yes please.".
fn answer_yes_please(bot: &mut Bot) -> Result<(), ScriptError> {
    let target = bot.world.ticks + TANNER_UI_TICKS;
    while bot.world.ticks < target {
        bot.pump(2)?;
        if bot.chat_options().iter().any(|(_, text)| text.eq_ignore_ascii_case("Yes please.")) {
            bot.chat_choose("Yes please.")?;
            return Ok(());
        }
    }
    Err(ScriptError::Failed(format!("tanner never offered tanning at {:?}", bot.position())))
}

/// Wait until the tanner grid modal is open.
fn open_tanner(bot: &mut Bot) -> Result<(), ScriptError> {
    let tanner = (bot.tables.com_id)("tanner")
        .ok_or_else(|| ScriptError::Usage(format!("tanner missing from component table at {:?}", bot.position())))?;
    let target = bot.world.ticks + TANNER_UI_TICKS;
    while bot.world.ticks < target {
        bot.pump(2)?;
        if bot.world.modal.as_ref().is_some_and(|modal| modal.interface_id == tanner) {
            return Ok(());
        }
    }
    Err(ScriptError::Failed(format!("tanner grid never opened at {:?}", bot.position())))
}

/// Tan step for the leatherwork run: the product, or 0 hides tanned.
pub fn tan_run(bot: &mut Bot, product: &str) -> ScriptResult {
    let Some((hide_name, leather_name, cost, coms)) = tan_spec(product) else {
        return Err(ScriptError::Usage(format!("no F2P tanning makes {product:?} at {:?}", bot.position())));
    };
    Ok(tan_carried(bot, hide_name, leather_name, cost, coms)? as i32)
}

#[cfg(test)]
mod tests {
    use super::{
        open_trade_direct, tan_spec, tanner_slot, TAN_HARD_COMS, TAN_HARD_COST, TAN_SOFT_COMS, TAN_SOFT_COST,
        TANNER_TRADE_OP, TANNER_X, TANNER_Z, TANNERS,
    };
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Gloves tan soft at 1 gp, the hard body tans hard at 3 gp, the
    /// members coif tans nothing (never pressed here).
    #[test]
    fn tan_specs_come_from_the_recipes() {
        assert_eq!(
            tan_spec("leather_gloves"),
            Some(("cow_hide", "leather", TAN_SOFT_COST, TAN_SOFT_COMS)),
            "gloves tan soft"
        );
        assert_eq!(TAN_SOFT_COST, 1, "soft costs 1 gp (tanner.constant)");
        assert_eq!(
            tan_spec("hardleather_body"),
            Some(("cow_hide", "hard_leather", TAN_HARD_COST, TAN_HARD_COMS)),
            "hard body tans hard"
        );
        assert_eq!(TAN_HARD_COST, 3, "hard costs 3 gp (tanner.constant)");
        assert!(tan_spec("coif").is_none(), "members coif never tans here");
        assert!(tan_spec("no_such_product").is_none(), "unknown never tans");
    }

    /// Every bucket button and the tanner interface resolve through the
    /// live component tables (the exact coms `tanner.rs2` presses).
    #[test]
    fn tanner_buttons_resolve() {
        let bot = offline_bot();
        assert!(!lifescape_gamedata::npcs_by_name(TANNERS[0]).is_empty(), "a tanner NPC exists");
        assert!((bot.tables.com_id)("tanner").is_some(), "tanner modal resolves");
        for com in TAN_SOFT_COMS.into_iter().chain(TAN_HARD_COMS) {
            assert!((bot.tables.com_id)(com).is_some(), "{com} resolves");
        }
        assert_eq!((TANNER_X, TANNER_Z), (3276, 3193), "Ellis stand (services dump)");
    }

    /// Trade direct is op 3 (`tanner.rs2` opnpc3 opens the grid, no
    /// dialog): the click reaches the send path offline (dead socket
    /// reads `Login`, never a bad op name), from an adjacent tile so
    /// no walk runs before the send.
    #[test]
    fn tanner_trade_opens_direct() {
        assert_eq!(TANNER_TRADE_OP, 3, "Trade direct, not Talk (tanner.rs2 opnpc3)");
        let mut bot = offline_bot();
        bot.world.level = 0;
        bot.world.x = 3275;
        bot.world.z = 3193;
        bot.world.npcs.insert(
            7,
            lifescape_sdk::world::NpcInfo { npc_type: 804, x: 3276, z: 3193, level: 0, name: None, anim: None, face_entity: None, hit_tick: None },
        );
        bot.approach_npc(7, 60).expect("adjacent tile needs no walk");
        let err = bot.op_npc(TANNER_TRADE_OP, 7).expect_err("dead socket fails loud");
        assert!(format!("{err}").contains("not logged in"), "Trade send attempted, got: {err}");
    }

    /// An out-of-view Ellis resyncs instead of failing on the first
    /// miss: the slot hunt pumps (dead socket reads `Login`, proving
    /// the resync ran), and a present tanner resolves with no pump.
    #[test]
    fn tanner_slot_resyncs_before_failing() {
        let mut bot = offline_bot();
        let err = tanner_slot(&mut bot).expect_err("no tanner offline");
        assert!(
            matches!(err, crate::ScriptError::Sdk(lifescape_sdk::Error::Login(_))),
            "resync pumped before failing, got: {err}"
        );
        bot.world.npcs.insert(
            7,
            lifescape_sdk::world::NpcInfo { npc_type: 804, x: 3276, z: 3193, level: 0, name: None, anim: None, face_entity: None, hit_tick: None },
        );
        assert!(tanner_slot(&mut bot).is_ok(), "present tanner resolves");
    }

    /// Direct-then-talk order: `open_trade_direct` sends the Trade op
    /// first (dead socket fails at the send, never at the tables —
    /// the "tanner" com resolves), so a live Ellis opens the grid
    /// with no dialog and the talk fallback only runs on a shut grid.
    #[test]
    fn tanner_direct_falls_back_to_talk() {
        let mut bot = offline_bot();
        bot.world.level = 0;
        bot.world.x = 3275;
        bot.world.z = 3193;
        bot.world.npcs.insert(
            7,
            lifescape_sdk::world::NpcInfo { npc_type: 804, x: 3276, z: 3193, level: 0, name: None, anim: None, face_entity: None, hit_tick: None },
        );
        let err = open_trade_direct(&mut bot, 7).expect_err("dead socket fails at the Trade send");
        assert!(format!("{err}").contains("not logged in"), "Trade tried first, got: {err}");
    }
}
