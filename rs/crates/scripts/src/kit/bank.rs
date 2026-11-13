//! Bank: bank trips, booth discovery, toll fence, transit heals. One
//! resilient trip per call, verified deposits, loud failures. Every wait
//! is tick-bounded; a dead socket aborts typed. Absorbed from the old
//! `skill_common` layer (D61).

#![forbid(unsafe_code)]


use lifescape_gamedata as knowledge;
use lifescape_sdk::waits::wait_hitpoints;
use lifescape_sdk::Bot;

use crate::kit::pack::{obj_id, pack_slots, pack_total};
use crate::kit::survival::{death_message_seen, hitpoints};
use crate::ScriptError;
/// Transit slicing: the bank walk runs in 1500-tick legs for the same
/// 15000-tick total, checking death and hurt between legs instead of
/// walking blind into an ambush.
const TRANSIT_SLICE_TICKS: u64 = 1500;
const TRANSIT_TOTAL_TICKS: u64 = 15000;
const TRANSIT_ATTEMPTS: u32 = 3;
/// Past 3 hurts the trip aborts loudly instead of walking into death.
const TRANSIT_MAX_HURTS: u32 = 3;
const TRANSIT_HEAL_TICKS: u64 = 200;

/// Nearest free (non-members, same level) bank booth by Manhattan distance.
/// Nearest bankhall, F2P and on this level. Toll-fence aware: banks on
/// the far side of a toll gate are last resort, never the default.
/// Needs the toll gate's fence line, served by the nav service.
///
/// Pocket exits (basements, upper floors) are the planner's job now:
/// `bank_trip` walks there via `travel_to`, which rides mapped
/// transitions itself, instead of a script-side ladder scan.
pub fn nearest_bank(bot: &mut Bot) -> Result<knowledge::banks::Bank, ScriptError> {
    let (here_x, here_z) = (bot.world.x, bot.world.z);
    // On Karamja (no F2P bank) every bank is across the boat: rank them
    // from the Port Sarim dock it lands on.
    let (anchor_x, anchor_z, _) = knowledge::access::mainland_anchor(i64::from(here_x), i64::from(here_z), i64::from(bot.world.level));
    let (here_x, here_z) = (anchor_x as i32, anchor_z as i32);
    // Same discipline as `toll_gate_x`: a down service leaves the pick
    // unfenced (logged) instead of aborting the run; the walk itself
    // still fails loud with `nav-down`.
    let gate_x = match bot.toll_fence(here_x, here_z, bot.world.level, 150) {
        Ok(gate) => gate.map(|(gate_x, _, _)| gate_x),
        Err(error) => {
            bot.log(&format!("toll fence unavailable ({error}), picking unfenced"));
            None
        }
    };
    let mut best: Option<(i64, knowledge::banks::Bank)> = None;
    let mut fallback: Option<(i64, knowledge::banks::Bank)> = None;
    // A bank inside an aggro square of a hunter that can kill this bot
    // (Draynor's, beside the young dark wizards, for combat 14 and
    // under) is never picked while a safe one exists on this level.
    let threats = crate::kit::danger::threats(bot);
    let safe_exists = knowledge::banks::all_banks().iter().any(|bank| {
        !bank.members && bank.level == i64::from(bot.world.level) && crate::kit::danger::covering(&threats, bank.x as i32, bank.z as i32).is_none()
    });
    for bank in knowledge::banks::all_banks() {
        if bank.members || bank.level != i64::from(bot.world.level) {
            continue;
        }
        if safe_exists && crate::kit::danger::covering(&threats, bank.x as i32, bank.z as i32).is_some() {
            continue;
        }
        let dist = (bank.x - i64::from(here_x)).abs() + (bank.z - i64::from(here_z)).abs();
        let across = gate_x.is_some_and(|line| (bank.x >= line as i64) != (here_x as i64 >= line as i64));
        let slot = if across { &mut fallback } else { &mut best };
        let weaker = match slot {
            Some((known, _)) => dist < *known,
            None => true,
        };
        if weaker {
            *slot = Some((dist, bank));
        }
    }
    best.or(fallback)
        .map(|(_, bank)| bank)
        .ok_or_else(|| ScriptError::Failed(format!("no reachable bank from {:?}", bot.position())))
}

/// X of the nearest toll fence, served by the nav service. `None` when no
/// fence stands nearby — or when the service is down (logged): callers
/// proceed unfenced and the next plan fails loud with `nav-down`, never a
/// local guess. The `Option` shape is frozen by callers that cannot carry a
/// `Result` (`activities/woodcut.rs`, `activities/mining.rs` bootstrap budgets).
pub fn toll_gate_x(bot: &mut Bot) -> Option<i32> {
    let (here_x, here_z) = (bot.world.x, bot.world.z);
    match bot.toll_fence(here_x, here_z, bot.world.level, 150) {
        Ok(gate) => gate.map(|(gate_x, _, _)| gate_x),
        Err(error) => {
            bot.log(&format!("toll fence unavailable ({error}), proceeding unfenced"));
            None
        }
    }
}

/// Cross the toll fence through the guard's dialog (10 coins). The side
/// flip proves it: works eastward and westward, unlike a fixed check.
pub fn cross_toll_gate(bot: &mut Bot) -> Result<bool, ScriptError> {
    let Some(line) = bot
        .toll_fence(bot.world.x, bot.world.z, bot.world.level, 150)?
        .map(|(gate_x, _, _)| gate_x)
    else {
        bot.log(&format!("no toll gate found near {:?}", bot.position()));
        return Ok(false);
    };
    let start_side = bot.world.x >= line;
    bot.intend(&format!("paying the toll gate at x={line}"));
    let target = bot.world.ticks + 1500;
    while bot.world.ticks < target && (bot.world.x >= line) == start_side {
        let gate = match bot.toll_fence(bot.world.x, bot.world.z, bot.world.level, 150)? {
            Some(gate) => gate,
            None => return Ok(false),
        };
        bot.approach(gate.0, gate.1, gate.2, 1, 300)?;
        if (bot.world.x >= line) != start_side {
            bot.log(&format!("walked through the toll gate at {:?}", bot.position()));
            return Ok(true);
        }
        let gate_id = gate_id(bot, gate)?;
        let open_op = bot
            .door_defs(std::slice::from_ref(&gate_id))?
            .into_iter()
            .next()
            .and_then(|def| def.open_op);
        let Some(op) = open_op else {
            return Ok(false);
        };
        bot.op_loc(op, gate.0, gate.1, gate_id)?;
        let talked = bot.world.ticks + 25;
        while bot.world.ticks < talked && bot.world.modal.is_none() {
            bot.pump(5)?;
        }
        if bot.world.modal.is_none() {
            bot.approach(gate.0, gate.1, gate.2, 1, 300)?;
            continue;
        }
        crate::quests::common::drain_dialog(bot, 60)?;
        if bot.world.modal.is_some() {
            let _ = bot.chat_choose("Yes, ok.");
            bot.pump(10)?;
            crate::quests::common::drain_dialog(bot, 30)?;
        }
        let moved = bot.world.ticks + 30;
        while bot.world.ticks < moved && (bot.world.x >= line) == start_side {
            bot.pump(2)?;
        }
        if (bot.world.x >= line) != start_side {
            bot.log(&format!("through the toll gate at {:?}", bot.position()));
            return Ok(true);
        }
    }
    bot.log(&format!("toll gate did not move us at {:?}", bot.position()));
    Ok((bot.world.x >= line) != start_side)
}

fn gate_id(bot: &mut Bot, gate: (i32, i32, i32)) -> Result<u16, ScriptError> {
    if let Some((id, _, _)) = bot.world.loc_at(gate.0, gate.1, gate.2) {
        return Ok(id);
    }
    bot.static_locs_near(gate.0, gate.1, gate.2, 1)?
        .into_iter()
        .find(|entry| (entry.x, entry.z, entry.level) == gate)
        .map(|entry| entry.loc)
        .ok_or_else(|| ScriptError::Failed(format!("toll gate gone at {gate:?}")))
}

/// Live bankbooth loc nearest to the booth spawn (loc id comes from the
/// live stream, never assumed from the dump). Static booths come from one
/// `static_locs_near` batch; without the service only the live stream
/// counts (logged), never a painted guess.
pub fn live_bank_booth(bot: &mut Bot, bank_x: i32, bank_z: i32) -> Option<(i32, i32, u16)> {
    live_bank_booths(bot, bank_x, bank_z).into_iter().next()
}

/// True when a loc id is a bankbooth (open, closed or tutorial): the dump
/// name carries `bankbooth`, read straight from the static row — no id set,
/// no scan. `live_bank_booths` used to call `loc_name` (String alloc +
/// lowercase per entry) for every live and static loc on every bank trip:
/// past ~100k painted entries that silent scan outlasted the server's
/// 100-tick silence reap (~6 s, no pump inside) and the session FIN-looped
/// at the booth. The direct row read keeps the same predicate.
fn is_bankbooth(loc_id: u16) -> bool {
    knowledge::loc_by_id(usize::from(loc_id)).and_then(|entry| entry.name).is_some_and(|name| {
        let label = name.to_lowercase();
        label.contains("bankbooth") || label.contains("bank booth")
    })
}

/// Every live bankbooth near the booth spawn, nearest first. The dump's
/// Manhattan pick flips between spawns of one bank (Draynor 3242/3245)
/// and the nearest spawn can sit behind the counter: the trip tries each
/// booth in turn instead of grinding the nearest from its blind side.
/// Static booths arrive in one service batch (`static_locs_near`, radius
/// 64: booths cluster at the bank); a down service leaves the live
/// stream only, logged, never a painted guess.
pub fn live_bank_booths(bot: &mut Bot, bank_x: i32, bank_z: i32) -> Vec<(i32, i32, u16)> {
    let mut booths: Vec<(i64, (i32, i32, u16))> = Vec::new();
    let mut near: Option<(i64, u16)> = None;
    let level = bot.world.level;
    let mut consider = |x: i32, z: i32, loc_level: i32, loc: u16| {
        if loc_level != level {
            return;
        }
        let dist = i64::from((x - bank_x).abs() + (z - bank_z).abs());
        if !is_bankbooth(loc) {
            if near.as_ref().is_none_or(|(known, _)| dist < *known) {
                near = Some((dist, loc));
            }
            return;
        }
        booths.push((dist, (x, z, loc)));
    };
    for ((x, z, loc_level), slot) in &bot.world.locs {
        if let Some((loc, _, _)) = slot {
            consider(*x, *z, *loc_level, *loc);
        }
    }
    let mut remote_count = 0;
    match bot.static_locs_near(bank_x, bank_z, level, 64) {
        Ok(rows) => {
            remote_count = rows.len();
            for row in rows {
                consider(row.x, row.z, row.level, row.loc);
            }
        }
        Err(error) => {
            bot.log(&format!("bank search: static locs unavailable ({error}), live only"));
        }
    }
    booths.sort();
    booths.dedup();
    if booths.is_empty() {
        let near = near.map(|(dist, loc)| (dist, (bot.tables.loc_name)(loc).unwrap_or_default(), loc));
        bot.log(&format!("bank search: live={} remote={remote_count} near={near:?}", bot.world.locs.len()));
    }
    booths.into_iter().map(|(_, booth)| booth).collect()
}

pub use crate::kit::withdraw::bank_withdraw_items;

/// The deposit is verified, never assumed: leftovers are a loud error,
/// not a silent carry to the spot.
pub fn require_deposit_cleared(bot: &Bot, deposited: bool) -> Result<(), ScriptError> {
    if deposited {
        return Ok(());
    }
    Err(ScriptError::Failed(format!("deposit did not clear at {:?}", bot.position())))
}

/// One more transit injury: logs `hurt en route` with the hp drop, waits
/// bounded for a heal, and past the bound fails loudly instead of walking
/// into death.
fn register_transit_hurt(bot: &mut Bot, hurts: &mut u32, hp_entry: u8) -> Result<(), ScriptError> {
    *hurts += 1;
    bot.log(&format!("hurt en route at {:?} (hp {hp_entry}->{}, injury {hurts})", bot.position(), hitpoints(bot)));
    if *hurts > TRANSIT_MAX_HURTS {
        return Err(ScriptError::Failed(format!("too hurt en route, aborting trip at {:?}", bot.position())));
    }
    wait_healed(bot, TRANSIT_HEAL_TICKS)?;
    Ok(())
}

/// Bank a load, re-discovering the bank when the trip leaves produce
/// behind (teleport mid-run, pocket start, unreachable booth). One retry
/// from the fresh bank, then loud: the load is stuck, not silently unbanked.
/// An unreachable first trip retries once instead of propagating at once.
pub fn bank_trip_resilient(
    bot: &mut Bot,
    bank: &mut knowledge::banks::Bank,
    keep: &[u16],
    withdraw_coins: u32,
    product_ids: &[u16],
) -> Result<u32, ScriptError> {
    let first_trip = bank_trip(bot, bank, keep, withdraw_coins);
    let mut bank_coins = match first_trip {
        Ok(coins) => coins,
        Err(ScriptError::Failed(text)) if text.contains("bank unreachable") => {
            bot.log(&format!("bank unreachable from {:?}, re-discovering", bot.position()));
            *bank = nearest_bank(bot)?;
            bank_trip(bot, bank, keep, withdraw_coins)?
        }
        Err(other) => return Err(other),
    };
    if product_ids.iter().any(|id| pack_total(bot, *id) > 0) {
        bot.log(&format!("bank unreachable from {:?}, re-discovering", bot.position()));
        *bank = nearest_bank(bot)?;
        bank_coins = bank_trip(bot, bank, keep, withdraw_coins)?;
        if product_ids.iter().any(|id| pack_total(bot, *id) > 0) {
            return Err(ScriptError::Failed(format!("bank unreachable, load stuck at {:?}", bot.position())));
        }
    }
    Ok(bank_coins)
}

/// True when the trip has anything to do: the pack holds non-`keep` to
/// deposit, or coins were requested. Both false is the only true no-op.
fn bank_work_pending(bot: &Bot, effective_keep: &[u16], withdraw_coins: u32) -> bool {
    if withdraw_coins > 0 {
        return true;
    }
    pack_slots(bot).iter().any(|(_, (held, _))| !effective_keep.contains(held))
}

/// Death in transit: the pack dropped to the kept scraps and the respawn
/// sits across the map, so blindly resuming re-treks the whole gauntlet
/// for nothing (and dies again). Resume only next to the bank with work
/// left; an empty pack with nothing requested cost nothing (Ok); anything
/// else fails loud, load lost, instead of pacing round trips.
fn death_resume(
    bot: &Bot,
    bank_x: i32,
    bank_z: i32,
    bank_level: i32,
    keep: &[u16],
    withdraw_coins: u32,
) -> Result<(), ScriptError> {
    let near = bot.world.level == bank_level
        && (bot.world.x - bank_x).abs() + (bot.world.z - bank_z).abs() <= 40;
    if bank_work_pending(bot, keep, withdraw_coins) {
        if near {
            bot.log("died en route, resuming trip from respawn");
            return Ok(());
        }
    } else if pack_slots(bot).is_empty() {
        return Ok(());
    }
    let kept: Vec<u16> = pack_slots(bot).iter().map(|(_, (held, _))| *held).collect();
    Err(ScriptError::Failed(format!(
        "died en route at {:?}, load lost (kept {kept:?}), aborting trip to ({bank_x},{bank_z})",
        bot.position()
    )))
}

/// Walk to a bank, open the booth, deposit everything but `keep`, withdraw
/// `withdraw_coins`. Returns the banked coin total for upgrade budgeting.
///
/// Contract: `Ok(0)` is the true no-op only (pack already holds just `keep`
/// and `withdraw_coins` is 0). When the booth is not reached (walk failed,
/// death en route, no live booth, bank did not open) and there is anything
/// to do, this returns `Err(Usage("bank unreachable ..."))` so callers never
/// count an undeposited load as banked.
pub fn bank_trip(
    bot: &mut Bot,
    bank: &knowledge::banks::Bank,
    keep: &[u16],
    withdraw_coins: u32,
) -> Result<u32, ScriptError> {
    let side_com = (bot.tables.com_id)("bank_side:inv")
        .ok_or_else(|| ScriptError::Usage("bank_side:inv missing from component table".to_string()))?;
    bot.log(&format!("bank trip to ({},{}) from {:?}", bank.x, bank.z, bot.position()));
    bot.intend(&format!("walking to the bank at ({},{})", bank.x, bank.z));
    // The genie lamp is unbankable: depositing it always fails, so keep it.
    // A later randomevent check rubs it for XP instead.
    let mut keep: Vec<u16> = keep.to_vec();
    if let Some(lamp) = (bot.tables.find_obj_by_name)("macro_genilamp") {
        if !keep.contains(&lamp) {
            keep.push(lamp);
        }
    }
    // A paid run (`toll`) keeps the fare in the pack: the guard takes it
    // from the inventory, never from the bank (`kit::toll`).
    let mut withdraw_coins = withdraw_coins;
    if bot.allow_paid {
        if let Ok(coins) = obj_id(bot, "coins") {
            if !keep.contains(&coins) {
                keep.push(coins);
            }
            withdraw_coins = withdraw_coins.max(crate::kit::toll::reserve_short(bot, pack_total(bot, coins)));
        }
    }
    let bank_x = bank.x as i32;
    let bank_z = bank.z as i32;
    let bank_level = bank.level as i32;
    // True no-op: nothing to deposit and nothing to withdraw. Walking
    // anyway would send every clean pack on a pointless (and sometimes
    // deadly) trek before the caller even starts its real business.
    if !bank_work_pending(bot, &keep, withdraw_coins) {
        return Ok(0);
    }
    let mut reached = false;
    let mut seen_death_msgs = bot.world.messages.len();
    let mut hurts = 0u32;
    let slices = TRANSIT_TOTAL_TICKS.div_ceil(TRANSIT_SLICE_TICKS);
    'transit: for _ in 0..TRANSIT_ATTEMPTS {
        for _ in 0..slices {
            if bot.world.level == bank_level && (bot.world.x - bank_x).abs() + (bot.world.z - bank_z).abs() <= 6 {
                reached = true;
                break 'transit;
            }
            let hp_entry = hitpoints(bot);
            match bot.approach(bank_x, bank_z, bank_level, 6, TRANSIT_SLICE_TICKS) {
                Err(lifescape_sdk::Error::Interrupted(_)) => {
                    return Err(ScriptError::Sdk(lifescape_sdk::Error::Interrupted(format!(
                        "bank trip stopped at {:?}",
                        bot.position()
                    ))));
                }
                // Dead socket, not a detour: reconnect instead of billing
                // frozen ticks to the transit budget and failing "unreachable".
                Err(error @ (lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_))) => {
                    return Err(ScriptError::Sdk(error));
                }
                Err(other) => {
                    if death_message_seen(bot, seen_death_msgs) {
                        bot.log(&format!(
                            "died en route, lost {} slots: {:?}",
                            pack_slots(bot).len(),
                            pack_slots(bot).iter().map(|(_, (held, _))| held).collect::<Vec<_>>()
                        ));
                        seen_death_msgs = bot.world.messages.len();
                        death_resume(bot, bank_x, bank_z, bank_level, &keep, withdraw_coins)?;
                        continue;
                    }
                    seen_death_msgs = bot.world.messages.len();
                    bot.log(&format!("bank trip: approach failed ({other}), waiting a tick"));
                    wait_hitpoints(bot, hitpoints(bot).saturating_add(1), 200)?;
                    break;
                }
                Ok(()) => {
                    if death_message_seen(bot, seen_death_msgs) {
                        bot.log(&format!(
                            "died en route, lost {} slots: {:?}",
                            pack_slots(bot).len(),
                            pack_slots(bot).iter().map(|(_, (held, _))| held).collect::<Vec<_>>()
                        ));
                        seen_death_msgs = bot.world.messages.len();
                        death_resume(bot, bank_x, bank_z, bank_level, &keep, withdraw_coins)?;
                        continue;
                    }
                    seen_death_msgs = bot.world.messages.len();
                    if hitpoints(bot) < hp_entry {
                        register_transit_hurt(bot, &mut hurts, hp_entry)?;
                    }
                }
            }
        }
    }
    if !reached {
        bot.log(&format!("bank trip: could not reach ({bank_x},{bank_z})"));
        if bank_work_pending(bot, &keep, withdraw_coins) {
            return Err(ScriptError::Failed(format!(
                "bank unreachable: could not reach ({bank_x},{bank_z}) from {:?}, load stuck",
                bot.position()
            )));
        }
        return Ok(0);
    }
    if death_message_seen(bot, seen_death_msgs) {
        bot.log(&format!(
            "died en route, lost {} slots: {:?}",
            pack_slots(bot).len(),
            pack_slots(bot).iter().map(|(_, (held, _))| held).collect::<Vec<_>>()
        ));
        death_resume(bot, bank_x, bank_z, bank_level, &keep, withdraw_coins)?;
    }
    // Safe tile: heal low HP before the booth/next trek, never walk hurt.
    heal_at_bank(bot)?;
    let booths = live_bank_booths(bot, bank_x, bank_z);
    if booths.is_empty() {
        bot.log(&format!("bank trip: no live bankbooth near ({bank_x},{bank_z})"));
        if bank_work_pending(bot, &keep, withdraw_coins) {
            return Err(ScriptError::Failed(format!(
                "bank unreachable: no live bankbooth near ({bank_x},{bank_z}) at {:?}, load stuck",
                bot.position()
            )));
        }
        return Ok(0);
    }
    // Nearest booth first, every refusal falls through to the next: the
    // dump's Manhattan pick can name a spawn behind the counter while its
    // neighbour faces us (Draynor 3242/3245). Only all-refused is loud.
    let mut opened = false;
    for (booth_x, booth_z, booth_loc) in booths {
        if bot.open_bank(booth_x, booth_z, booth_loc, 50)? {
            opened = true;
            break;
        }
        bot.log(&format!("bank trip: booth ({booth_x},{booth_z}) refused from here, trying next"));
    }
    if !opened {
        bot.log(&format!("bank trip: bank did not open at {:?}", bot.position()));
        if bank_work_pending(bot, &keep, withdraw_coins) {
            return Err(ScriptError::Failed(format!(
                "bank unreachable: bank did not open at {:?}, load stuck",
                bot.position()
            )));
        }
        return Ok(0);
    }
    let deposited = bot.bank_deposit_all(side_com, &keep, 25)?;
    require_deposit_cleared(bot, deposited)?;
    if withdraw_coins > 0 {
        if let Ok(coins) = obj_id(bot, "coins") {
            let _ = bot.bank_withdraw(coins, withdraw_coins, 50);
            bot.pump(5)?;
        }
    }
    let coins_now = obj_id(bot, "coins").ok();
    let bank_coins = coins_now.and_then(|coins| bot.bank_items().get(&coins).copied()).unwrap_or(0);
    bot.close_modal()?;
    bot.pump(5)?;
    Ok(bank_coins)
}

/// Pump until hitpoints reach `target` (heal wait after a flee).
pub fn wait_healed(bot: &mut Bot, timeout: u64) -> Result<(), ScriptError> {
    wait_hitpoints(bot, hitpoints(bot).saturating_add(1), timeout)?;
    Ok(())
}

/// Bank heal: the bank tile is safe and regen is server-timed, so a low
/// pack heals here instead of walking hurt into the next gauntlet (the
/// t2494 14->8 then t3851 death never healed). Threshold 12 HP on a 14-15
/// max survives two goblin hits (observed max 6: 14->8); the wait is
/// bounded and logged so the live ticks/HP stay measurable.
/// Live measure: hurt 11->8 (t5177) then heal 9->12 in 300 ticks
/// (t5221->t5521), i.e. 100 ticks/HP, matching content health_regen 100
/// (stat.rs2, login.rs2). Bound 617 = 5 HP x 100 + 117 pump margin.
pub const HEAL_TARGET_HP: u8 = 12;
const HEAL_MAX_TICKS: u64 = 617;

/// Heal target for a bot whose full health is `max_hp`.
pub fn heal_target(max_hp: u8) -> u8 {
    HEAL_TARGET_HP.min(max_hp)
}

/// Heal on the spot when below target. Called at the bank (safe), never
/// mid-route. Logs the ticks actually spent for the regen measure.
/// Best-effort: a dead socket or stop still aborts, anything else just
/// logs (the booth open below surfaces the real failure).
pub fn heal_at_bank(bot: &mut Bot) -> Result<(), ScriptError> {
    let hp = hitpoints(bot);
    // Never above full health: a fresh 10-HP account waited the whole
    // 617-tick bound at every bank trip for a 12 it can never reach.
    let target = heal_target(crate::kit::survival::max_hitpoints(bot));
    if hp >= target {
        return Ok(());
    }
    let start = bot.world.ticks;
    bot.log(&format!("heal at bank: hp {hp}->{target} at {:?}", bot.position()));
    match wait_hitpoints(bot, target, HEAL_MAX_TICKS) {
        Ok(_) => {}
        Err(lifescape_sdk::Error::Interrupted(reason)) => {
            return Err(ScriptError::Sdk(lifescape_sdk::Error::Interrupted(reason)))
        }
        Err(error) => {
            bot.log(&format!("heal wait interrupted ({error}), continuing to booth"));
        }
    }
    bot.log(&format!(
        "healed to {} in {} ticks (from {hp})",
        hitpoints(bot),
        bot.world.ticks - start
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{bank_trip, bank_trip_resilient, is_bankbooth, live_bank_booth, live_bank_booths, nearest_bank, register_transit_hurt, require_deposit_cleared};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    /// Standing at Draynor, a low-level bot never banks beside the young
    /// dark wizards: a safe bank is picked instead (bots died on every
    /// Draynor deposit, cook and fishing runs alike).
    #[test]
    fn low_level_bot_never_picks_the_draynor_bank() {
        let mut bot = lifescape_sdk::Bot::new("127.0.0.1", 43594, ".", ".");
        crate::tables_live::attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3092;
        bot.world.z = 3245;
        let bank = super::nearest_bank(&mut bot).expect("a bank");
        assert!((bank.x - 3092).abs() + (bank.z - 3243).abs() > 20, "picked the Draynor bank at ({},{})", bank.x, bank.z);
        let threats = crate::kit::danger::threats(&bot);
        assert!(crate::kit::danger::covering(&threats, bank.x as i32, bank.z as i32).is_none());
    }

    /// Fresh accounts have 10 HP: the target never exceeds full health.
    #[test]
    fn heal_target_never_exceeds_full_health() {
        assert_eq!(super::heal_target(10), 10, "a 10-HP bot heals to 10, not 12");
        assert_eq!(super::heal_target(25), super::HEAL_TARGET_HP);
    }

    /// Mapless worker against a real nav service (repo dumps): the bot
    /// carries no `nav_world`/`nav_tables`, only the plan handle. Each
    /// test binds the service so the socket outlives the bot.
    fn served_bot(socket_tag: &str) -> (Bot, std::sync::Arc<lifescape_nav::service::Service>) {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let (root, data) = crate::test_roots();
        let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
        let gametypes = std::fs::read_to_string(data.join("gametypes.json")).expect("gametypes");
        let overrides = std::fs::read_to_string(data.join("nav_overrides.json")).expect("overrides");
        let defs = lifescape_nav::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
        let world = lifescape_nav::static_world::StaticWorld::open(&zip, defs).expect("static world");
        let zones = lifescape_nav::f2p::load_zones(&root.join("lostcity/content/maps/free2play.csv")).expect("f2p");
        world.set_f2p(zones);
        let tables = std::sync::Arc::new(lifescape_nav::tables::Tables::load(&data).expect("tables"));
        let version = format!("test-bank-{socket_tag}");
        let service = lifescape_nav::service::Service::start(
            lifescape_nav::service::Snapshot::new(world, tables, version.clone()),
            &lifescape_nav::service::test_socket(socket_tag),
            1,
        )
        .expect("service starts");
        bot.plan_client = Some(lifescape_nav::service::PlanClient::new(service.socket_path(), socket_tag));
        bot.plan_data_version = Some(version);
        bot.last_keepalive = Some(std::time::Instant::now());
        (bot, service)
    }


    /// Death in transit: a fake death line during the trip logs and the
    /// trip stays valid (Ok, no booth here so no network), continuing
    /// from the respawn position instead of erroring.
    #[test]
    fn bank_trip_survives_death_message() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3000;
        bot.world.z = 3000;
        bot.world.messages.push("Oh dear you are dead!".to_string());
        let bank = lifescape_gamedata::banks::Bank {
            x: 3000,
            z: 3000,
            level: 0,
            loc_id: 0,
            name: "test".to_string(),
            members: false,
        };
        let outcome = bank_trip(&mut bot, &bank, &[], 0);
        assert!(outcome.is_ok(), "death in transit stays valid, got: {outcome:?}");
    }

    /// True no-op never walks: far from the bank with an empty pack and no
    /// withdrawal, the trip is Ok(0) with no network (any walk attempt
    /// fails loudly offline, so Ok proves nothing was attempted).
    #[test]
    fn bank_trip_noop_never_walks() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3226;
        bot.world.z = 3223;
        let bank = lifescape_gamedata::banks::Bank {
            x: 3091,
            z: 3242,
            level: 0,
            loc_id: 0,
            name: "test".to_string(),
            members: false,
        };
        assert_eq!(bank_trip(&mut bot, &bank, &[], 0).expect("no-op is Ok(0)"), 0);
        assert_eq!((bot.world.x, bot.world.z), (3226, 3223), "never moved");
    }

    /// Death far from the bank aborts loud (load lost) instead of
    /// re-trekking the gauntlet from respawn for scraps; next to the bank
    /// with work left it resumes; an empty pack costs nothing. The helper
    /// is pure (no network), unlike the trip around it.
    #[test]
    fn bank_trip_death_far_aborts_load_lost() {
        use super::death_resume;
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3222;
        bot.world.z = 3218;
        let logs = crate::kit::pack::obj_id(&bot, "logs").expect("logs in tables");
        let com = (bot.tables.com_id)("inventory:inv").expect("inventory:inv in dumps");
        bot.world.inv.insert(com, [(0u8, (logs, 5u32))].into_iter().collect());
        let err = death_resume(&bot, 3091, 3242, 0, &[], 0).expect_err("far death aborts");
        assert!(err.to_string().contains("load lost"), "loud cause, got: {err}");
        bot.world.x = 3091;
        bot.world.z = 3242;
        death_resume(&bot, 3091, 3242, 0, &[], 0).expect("near death with work resumes");
        bot.world.inv.remove(&com);
        bot.world.x = 3222;
        bot.world.z = 3218;
        death_resume(&bot, 3091, 3242, 0, &[], 0).expect("empty pack costs nothing");
    }

    /// Fourth transit injury aborts loudly instead of walking into death
    /// (the abort fires before any heal pump, so no server is needed).
    #[test]
    fn fourth_transit_hurt_aborts_loud() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        let mut hurts = 3u32;
        let err = register_transit_hurt(&mut bot, &mut hurts, 10).expect_err("4th hurt aborts");
        assert!(err.to_string().contains("too hurt en route, aborting trip"), "loud abort, got: {err}");
        assert_eq!(hurts, 4, "injury counted before aborting");
    }

    /// Uncleared deposit is a loud error naming the cause; a cleared one
    /// passes silently.
    #[test]
    fn uncleared_deposit_fails_loud() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        require_deposit_cleared(&bot, true).expect("cleared passes");
        let err = require_deposit_cleared(&bot, false).expect_err("leftovers are loud");
        assert!(err.to_string().contains("deposit did not clear"), "loud cause, got: {err}");
    }

    /// Unreached booth with a deposit pending fails loudly, never Ok(0):
    /// the live hole (could not reach + ok load banked on an empty bank).
    #[test]
    fn unreachable_with_deposit_fails_loud() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3000;
        bot.world.z = 3000;
        let coins = crate::kit::pack::obj_id(&bot, "coins").expect("coins in dumps");
        let pack_com = (bot.tables.com_id)("inventory:inv").expect("pack com");
        bot.world.inv.insert(pack_com, [(0u8, (coins, 5u32))].into_iter().collect());
        let bank = lifescape_gamedata::banks::Bank {
            x: 3000,
            z: 3000,
            level: 0,
            loc_id: 0,
            name: "test".to_string(),
            members: false,
        };
        let err = bank_trip(&mut bot, &bank, &[], 0).expect_err("pending deposit without booth");
        assert!(err.to_string().contains("bank unreachable"), "loud cause, got: {err}");
        assert_eq!(crate::kit::pack::pack_total(&bot, coins), 5, "nothing deposited");
    }

    /// Far bank with a deposit pending and no socket: the transit approach
    /// fails typed (reconnect), never "bank unreachable".
    #[test]
    fn bank_trip_far_offline_aborts_typed() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3226;
        bot.world.z = 3223;
        let coins = crate::kit::pack::obj_id(&bot, "coins").expect("coins in dumps");
        let pack_com = (bot.tables.com_id)("inventory:inv").expect("pack com");
        bot.world.inv.insert(pack_com, [(0u8, (coins, 5u32))].into_iter().collect());
        let bank = lifescape_gamedata::banks::Bank {
            x: 3091,
            z: 3242,
            level: 0,
            loc_id: 0,
            name: "test".to_string(),
            members: false,
        };
        let err = bank_trip(&mut bot, &bank, &[], 0).expect_err("dead socket aborts");
        assert!(crate::kit::survival::is_walk_abort(&err), "typed for the reconnect arm, got: {err}");
        assert!(!err.to_string().contains("bank unreachable"), "no masking, got: {err}");
    }

    /// True no-op (pack already keep-only, nothing to withdraw) stays Ok(0)
    /// without a booth and without network.
    #[test]
    fn noop_without_booth_stays_ok() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3000;
        bot.world.z = 3000;
        let coins = crate::kit::pack::obj_id(&bot, "coins").expect("coins in dumps");
        let bank = lifescape_gamedata::banks::Bank {
            x: 3000,
            z: 3000,
            level: 0,
            loc_id: 0,
            name: "test".to_string(),
            members: false,
        };
        assert_eq!(bank_trip(&mut bot, &bank, &[], 0).expect("empty pack is a no-op"), 0);
        let pack_com = (bot.tables.com_id)("inventory:inv").expect("pack com");
        bot.world.inv.insert(pack_com, [(0u8, (coins, 5u32))].into_iter().collect());
        assert_eq!(bank_trip(&mut bot, &bank, &[coins], 0).expect("pack == keep is a no-op"), 0);
    }

    /// Resilient rediscovers once, then fails loud: the first trip finds
    /// no booth at the booth-free tile (served batch confirms), the
    /// rediscovered real bank is a dead-socket walk away, so the typed
    /// walk abort surfaces instead of masking as "bank unreachable".
    /// (3226,3223) is booth-free in the dumps (probed), keeping the
    /// first failure on the booth lookup.
    #[test]
    fn resilient_rediscovers_then_fails_loud() {
        let (mut bot, _service) = served_bot("bank-resilient");
        bot.world.level = 0;
        bot.world.x = 3226;
        bot.world.z = 3223;
        let coins = crate::kit::pack::obj_id(&bot, "coins").expect("coins in dumps");
        let pack_com = (bot.tables.com_id)("inventory:inv").expect("pack com");
        bot.world.inv.insert(pack_com, [(0u8, (coins, 5u32))].into_iter().collect());
        let mut bank = lifescape_gamedata::banks::Bank {
            x: 3226,
            z: 3223,
            level: 0,
            loc_id: 2213,
            name: "bankbooth".to_string(),
            members: false,
        };
        let err = bank_trip_resilient(&mut bot, &mut bank, &[], 0, &[coins]).expect_err("load never deposited");
        assert!(crate::kit::survival::is_walk_abort(&err), "dead socket surfaces typed, got: {err}");
        assert!((bank.x, bank.z) != (3226, 3223), "rediscovery ran before failing loud, got ({},{})", bank.x, bank.z);
        assert_eq!(crate::kit::pack::pack_total(&bot, coins), 5, "load still in the pack");
    }

    /// Booths resolve from the live stream first, static template second
    /// via one service batch: the painted `static_locs` map is never
    /// read. The remote half only adds booths, so a live booth at the
    /// spawn stays nearest whatever the dumps hold nearby.
    #[test]
    fn booth_found_in_live_stream() {
        let (mut bot, _service) = served_bot("bank-booth-live");
        bot.world.level = 0;
        bot.world.locs.insert((3015, 3354, 0), Some((2213, 0, 0)));
        let found = live_bank_booth(&mut bot, 3015, 3354).expect("Falador booth resolves");
        assert_eq!(found, (3015, 3354, 2213));
        bot.world.locs.insert((3200, 3200, 0), Some((2213, 0, 0)));
        let near = live_bank_booth(&mut bot, 3015, 3354).expect("nearest of two booths");
        assert_eq!(near, (3015, 3354, 2213));
    }

    /// No service, no scenery: only the live stream counts, never a
    /// painted guess. The down batch is logged, the live booth still
    /// resolves.
    #[test]
    fn booth_live_only_without_service() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        assert!(live_bank_booth(&mut bot, 3015, 3354).is_none(), "no booth without scenery");
        bot.world.locs.insert((3015, 3354, 0), Some((2213, 0, 0)));
        let found = live_bank_booth(&mut bot, 3015, 3354).expect("live booth resolves");
        assert_eq!(found, (3015, 3354, 2213));
    }

    /// Draynor has three booth spawns on one wall: all three come back
    /// nearest-first so the trip can fall through past a counter-blocked
    /// pick instead of grinding it (the trip #2 hole).
    #[test]
    fn booths_list_every_spawn_nearest_first() {
        let (mut bot, _service) = served_bot("bank-booths-near");
        bot.world.level = 0;
        for booth_z in [3242, 3243, 3245] {
            bot.world.locs.insert((3091, booth_z, 0), Some((2213, 0, 0)));
        }
        let booths = live_bank_booths(&mut bot, 3091, 3245);
        assert_eq!(booths[0], (3091, 3245, 2213), "spawn booth first, got: {booths:?}");
        for spawn in [(3091, 3245, 2213), (3091, 3243, 2213), (3091, 3242, 2213)] {
            assert!(booths.contains(&spawn), "spawn listed, got: {booths:?}");
        }
        assert_eq!(live_bank_booth(&mut bot, 3091, 3245), Some((3091, 3245, 2213)), "wrapper stays the nearest");
    }

    /// The id-set fast path matches the old per-entry name predicate:
    /// booth 2213 resolves, and a nearer non-booth never does (no name
    /// lookup per entry, so long sessions stop outlasting the reap).
    /// Unserved, so only the live stream counts: deterministic.
    #[test]
    fn booth_scan_ignores_non_booth_without_name_lookups() {
        assert!(is_bankbooth(2213), "booth id resolves");
        let plain = (0u16..).find(|id| !is_bankbooth(*id)).expect("a non-booth id exists");
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.locs.insert((3000, 3000, 0), Some((plain, 0, 0)));
        bot.world.locs.insert((3010, 3000, 0), Some((2213, 0, 0)));
        let booths = live_bank_booths(&mut bot, 3000, 3000);
        assert_eq!(booths, vec![(3010, 3000, 2213)], "only the booth resolves, got: {booths:?}");
    }

    /// Picking is pure: standing on a booth spawn returns that booth,
    /// and the bot never moves (pocket exits ride inside `travel_to` now,
    /// not in a script-side ladder scan). The fence line comes from the
    /// service; the underfoot bank can never sit across it.
    #[test]
    fn nearest_bank_picks_closest_without_moving() {
        let (mut bot, _service) = served_bot("bank-near");
        let want = lifescape_gamedata::banks::all_banks()
            .into_iter()
            .find(|bank| !bank.members && bank.level == 0)
            .expect("a free bank");
        bot.world.level = 0;
        bot.world.x = want.x as i32;
        bot.world.z = want.z as i32;
        let before = (bot.world.x, bot.world.z);
        let got = nearest_bank(&mut bot).expect("a bank resolves");
        assert_eq!((got.x, got.z, got.level), (want.x, want.z, want.level), "booth underfoot wins");
        assert_eq!((bot.world.x, bot.world.z), before, "picking never walks");
        bot.world.x = 2900;
        bot.world.z = 3200;
        let far = nearest_bank(&mut bot).expect("a bank resolves");
        assert!(!far.members && far.level == 0, "free surface bank, got: {far:?}");
    }

    /// No service handle (old daemon, unknown op): the bank pick stays
    /// unfenced (logged) instead of aborting the run — the walk itself
    /// still fails loud with `nav-down`. Same discipline as `toll_gate_x`.
    #[test]
    fn nearest_bank_picks_unfenced_without_service() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3091;
        bot.world.z = 3242;
        // Fence fallback only: the danger rule (Draynor wizards vs an
        // unknown level) is `low_level_bot_never_picks_the_draynor_bank`.
        bot.allow_wild = true;
        let got = nearest_bank(&mut bot).expect("unfenced pick resolves");
        assert_eq!((got.x, got.z, got.level), (3091, 3242, 0), "draynor wins unfenced, got: {got:?}");
    }
}
