//! Tutorial skilling steps: item-on-scenery use, chop, fire, fish, cook.
//!
//! Split out of `tutorial.rs` (1000-line rule). The stage driver lives
//! there; these handlers only act and wait on observable inventory.

use lifescape_sdk::Bot;

use crate::quests::common as kit;
use crate::tutorial::{check, find_target_loc, op_loc_near, ordered_stands, talk_step, wait_inv};
use crate::tutorial_detect::{has, obj};
use crate::ScriptError;

/// Ticks granted to a use the server already consumed before its product
/// shows. The tutorial cook is one server tick (`inv_del` then `inv_add`,
/// tut_cooking.rs2 `tut_attempt_cook_item`), so this only covers the two
/// inventory updates landing in different frames.
const USE_RESOLVE_TICKS: u64 = 10;

/// What one item-on-scenery use has become.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum UseOutcome {
    /// The wanted product is there.
    Done,
    /// The item left the pack, the wanted product is not there: the use
    /// was consumed. Settle on the product, never click again (the item
    /// is gone, `inv_slot` would fail typed and kill the run).
    Resolving,
    /// Nothing consumed yet: this stand did not answer, try the next.
    Pending,
}

pub(crate) fn use_outcome(done: bool, held_at_click: u32, held_now: u32) -> UseOutcome {
    if done {
        UseOutcome::Done
    } else if held_now < held_at_click {
        UseOutcome::Resolving
    } else {
        UseOutcome::Pending
    }
}

fn wait_done(bot: &mut Bot, done: &impl Fn(&Bot) -> bool, timeout: u64) -> Result<bool, ScriptError> {
    let target = bot.world.ticks + timeout;
    while bot.world.ticks < target {
        if done(bot) {
            return Ok(true);
        }
        bot.pump(2)?;
    }
    Ok(done(bot))
}

pub(crate) fn use_held_on_loc_near(bot: &mut Bot, loc_names: &[&str], tool: u16, done: impl Fn(&Bot) -> bool, timeout: u64) -> Result<bool, ScriptError> {
    let (x, z, level, loc) = find_target_loc(bot, loc_names, 150)?;
    let cands = ordered_stands(x, z, bot.world.x, bot.world.z);
    let target = bot.world.ticks + timeout;
    let mut held_at_click: Option<u32> = None;
    for (cx, cz) in cands {
        if bot.world.ticks > target {
            break;
        }
        if bot.walk_to(cx, cz, level, 1250).is_err() {
            continue;
        }
        // The previous click can resolve after its 20-tick window, during
        // this walk (sara_miller: fresh fire, cook ~25 ticks after the
        // click): settle it before asking `inv_slot` for an eaten item.
        if let Some(before) = held_at_click {
            match use_outcome(done(bot), before, kit::count(bot, tool)) {
                UseOutcome::Done => return Ok(true),
                UseOutcome::Resolving => return wait_done(bot, &done, USE_RESOLVE_TICKS),
                UseOutcome::Pending => {}
            }
        }
        let held = kit::count(bot, tool);
        let (use_com, use_slot) = kit::inv_slot(bot, tool)?;
        bot.op_loc_u(x, z, loc, tool, u16::from(use_slot), use_com)?;
        held_at_click = Some(held);
        let inner = bot.world.ticks + 20;
        while bot.world.ticks < inner {
            bot.pump(2)?;
            match use_outcome(done(bot), held, kit::count(bot, tool)) {
                UseOutcome::Done => return Ok(true),
                UseOutcome::Resolving => return wait_done(bot, &done, USE_RESOLVE_TICKS),
                UseOutcome::Pending => {}
            }
        }
    }
    Ok(false)
}

pub(crate) fn chop(bot: &mut Bot) -> Result<(), ScriptError> {
    let axe = obj("bronze_axe")?;
    if !bot.inventory().contains_key(&axe) {
        // Defaut 1 offline cause: the box already asks for a tree while
        // the axe never landed (login lag, or a mid-quest resume that
        // skipped the grant). Re-talk before failing loud (t03/t05).
        let _ = talk_step(bot, "survival", (3103, 3095), &[], "survival re-grant");
        let _ = wait_inv(bot, "bronze_axe", 1, 60);
    }
    check(bot.position(), bot.inventory().contains_key(&axe), "axe held")?;
    let logs = obj("newbielogs")?;
    while bot.inventory().get(&logs).copied().unwrap_or(0) < 2 {
        if bot.world.modal.is_some() {
            bot.chat_continue()?;
            bot.pump(5)?;
        }
        let had = bot.inventory().get(&logs).copied().unwrap_or(0);
        check(bot.position(), op_loc_near(bot, &["newbietree", "newbietree2", "newbiedeadtree2"], 1, |candidate| candidate.inventory().get(&logs).copied().unwrap_or(0) > had, 120)?, "logs chopped")?;
    }
    Ok(())
}

pub(crate) fn light_fire(bot: &mut Bot) -> Result<(), ScriptError> {
    let logs = obj("newbielogs")?;
    let tinderbox = obj("tinderbox")?;
    if bot.inventory().get(&logs).copied().unwrap_or(0) < 1 {
        chop(bot)?;
    }
    for _ in 0..3 {
        if bot.inventory().get(&logs).copied().unwrap_or(0) < 1 {
            chop(bot)?;
        }
        let here = (bot.world.x, bot.world.z);
        let (com, slot) = kit::inv_slot(bot, logs)?;
        let (use_com, use_slot) = kit::inv_slot(bot, tinderbox)?;
        bot.op_held_u(logs, slot, com, tinderbox, use_slot, use_com)?;
        let target = bot.world.ticks + 50;
        while bot.world.ticks < target {
            bot.pump(5)?;
            if let Ok((fire_x, fire_z, _, _)) = kit::find_loc(bot, &["fire"], 150) {
                if (fire_x - here.0).abs() + (fire_z - here.1).abs() <= 1 {
                    bot.log("ok fire lit");
                    return Ok(());
                }
            }
        }
        let _ = bot.walk_to(here.0 + 2, here.1, bot.world.level, 1500);
    }
    check(bot.position(), false, "fire lighting")
}

pub(crate) fn catch_shrimp(bot: &mut Bot, timeout: u64) -> Result<bool, ScriptError> {
    let raw = obj("newbieraw_shrimp")?;
    let had = bot.inventory().get(&raw).copied().unwrap_or(0);
    let target = bot.world.ticks + timeout;
    // One spot can go stale (ghost slot answers nothing): rotate through
    // all visible spots instead of hammering the nearest forever.
    let mut dud: Vec<u16> = Vec::new();
    while bot.world.ticks < target {
        bot.pump(5)?;
        let mut spots: Vec<u16> = bot.find_npcs("newbiefishing").into_iter().filter(|slot| !dud.contains(slot)).collect();
        if spots.is_empty() {
            dud.clear();
            spots = bot.find_npcs("newbiefishing");
        }
        let Some(slot) = spots.into_iter().next() else { continue };
        let _ = bot.approach_npc(slot, 30);
        bot.op_npc(1, slot)?;
        let inner = bot.world.ticks + 60;
        while bot.world.ticks < inner {
            bot.pump(5)?;
            if bot.inventory().get(&raw).copied().unwrap_or(0) > had {
                return Ok(true);
            }
        }
        dud.push(slot);
    }
    Ok(false)
}

/// Cook `raw_name` on a fire until one more `want` is held. The server
/// (tut_cooking.rs2 `tut_attempt_cook_item`) always burns the first
/// shrimp, then asks to "First catch some shrimp, then use them on a
/// fire": every round re-fishes when the raw one is gone.
pub(crate) fn cook_on_fire(bot: &mut Bot, raw_name: &str, want: &str) -> Result<(), ScriptError> {
    let raw = obj(raw_name)?;
    let want_id = obj(want)?;
    // A product already held (old random burn) must not fake the cook.
    let had = kit::count(bot, want_id);
    // Tutorial fires burn out after 150 ticks: short cook rounds with a
    // relight between them, never one long hammering of a dead fire.
    for _ in 0..3 {
        if kit::count(bot, want_id) > had {
            return Ok(());
        }
        if !has(bot, raw_name, 1) {
            check(bot.position(), catch_shrimp(bot, 300)?, "shrimp caught for cooking")?;
        }
        check(bot.position(), wait_inv(bot, raw_name, 1, 90)?, &format!("has {raw_name}"))?;
        if kit::find_loc(bot, &["fire"], 15).is_err() {
            bot.log("no fire nearby, lighting");
            light_fire(bot)?;
        }
        if let Ok((fire_x, fire_z, _, _)) = kit::find_loc(bot, &["fire"], 150) {
            bot.log(&format!("cooking on fire at {fire_x},{fire_z}"));
        }
        if use_held_on_loc_near(bot, &["fire"], raw, |candidate| kit::count(candidate, want_id) > had, 100)? {
            return Ok(());
        }
        bot.log(&format!("{want} missed this fire, relighting"));
    }
    check(bot.position(), kit::count(bot, want_id) > had, &format!("{want} cooked"))
}

#[cfg(test)]
mod tests {
    use super::{use_outcome, UseOutcome};

    /// sara_miller (16:40:43): `op_loc_u fire + Raw shrimps`, 20 ticks
    /// without the burnt fish, a 1-tile walk to the next stand, then
    /// `inv_slot` failed "item 2514 is not in the backpack": the cook had
    /// resolved during the walk (task report: Raw shrimps -1, Burnt fish
    /// +1) and the loop reclicked without looking at the product. A
    /// consumed item is settled on its product, never reclicked.
    #[test]
    fn consumed_item_without_product_waits_instead_of_reclicking() {
        assert_eq!(use_outcome(false, 1, 0), UseOutcome::Resolving, "shrimp eaten, burnt fish pending");
        assert_eq!(use_outcome(true, 1, 0), UseOutcome::Done, "burnt fish arrived");
        assert_eq!(use_outcome(false, 1, 1), UseOutcome::Pending, "stand did not answer, next stand");
        assert_eq!(use_outcome(false, 2, 1), UseOutcome::Resolving, "one of a stack consumed");
    }
}
