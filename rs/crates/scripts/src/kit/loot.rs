//! Loot: ground-pickup policy after a kill. The kill's own bones are
//! picked up and buried on the spot (user rule: every combat run trains
//! prayer as it goes); the rest is taken except buryables (D16: tout
//! compte — loot, produced and ground spawns all feed the rates), coins
//! first so a nearly-full pack never starves the coin pickup. The value
//! reference is the shop price today (`drop_value`); the market
//! reference price plugs in at L5 without changing the policy.
//! Provenance tagging lands with the task report (L2); the take loop
//! already names every item it touches.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::{Bot, Error};

use crate::kit::pack::{free_slots, obj_label, pack_slots};
use crate::ScriptError;

/// Worst-case take: walk <= LOOT_RADIUS tiles, click, see the pack grow.
/// Measured on 6 live melee bots: 98.9% of successful takes land within
/// 20 ticks of the call (99.2% within 30; the click itself answers in
/// <= 5 ticks 99% of the time), while every miss burned the whole bound
/// (3483 misses at ~97 ticks each under the old 100). The walk keeps its
/// own budget inside `take`; only the wait for a pickup that never
/// comes is cut.
pub const TAKE_TICKS: u64 = 30;
/// Pickup radius around the killer spot.
pub const LOOT_RADIUS: i32 = 6;
/// Ticks the kill's bones get to show next to the death tile: the drop
/// lands with the death (obj_add right after npc_death), one NPC_INFO
/// late at worst.
pub const BONES_WAIT_TICKS: u64 = 5;
/// Ticks one Bury click gets to empty its slot: `[opheld1,_bones]` runs
/// `p_delay(0)` then deletes the bone on the next tick (bury_bone.rs2).
pub const BURY_TICKS: u64 = 4;
/// Bury clicks per bone before failing typed: the server drops an op
/// while the player is delayed (OpHeldHandler), so one retry is normal,
/// three silent clicks are not.
pub const BURY_CLICKS: u32 = 3;

/// Inventory op index (1-based) of "Bury" for an object, from the dump's
/// inventory ops (`iop1=Bury` on every `category=bones` obj in
/// bones.obj). None for anything that is not buried, notes included.
pub fn bury_op(obj_id: u16) -> Option<u8> {
    let entry = knowledge::obj_by_id(usize::from(obj_id))?;
    let index = entry.inv_ops.iter().position(|op| op.is_some_and(|name| name.eq_ignore_ascii_case("bury")))?;
    u8::try_from(index + 1).ok()
}

/// The kill's bones among ground stacks `((x, z), obj)`: a buryable obj
/// at most one tile from the death tile, the closest one. Pure so the
/// choice is testable; `buryable` is `bury_op(..).is_some()` live.
pub fn pick_kill_bones(
    ground: &[((i32, i32), u16)],
    death: (i32, i32),
    buryable: impl Fn(u16) -> bool,
) -> Option<((i32, i32), u16)> {
    ground
        .iter()
        .filter(|((tile_x, tile_z), _)| (tile_x - death.0).abs() <= 1 && (tile_z - death.1).abs() <= 1)
        .filter(|(_, oid)| buryable(*oid))
        .min_by_key(|((tile_x, tile_z), oid)| ((tile_x - death.0).abs() + (tile_z - death.1).abs(), *oid))
        .copied()
}

/// Pick up the bones of the kill that died at `death` and bury them
/// where the bot stands. Ok(true) once buried; Ok(false) when the kill
/// left no bones or the pickup was refused (logged). Dead sockets and a
/// bury the server never answers fail typed.
pub fn bury_kill_bones(bot: &mut Bot, death: (i32, i32)) -> Result<bool, ScriptError> {
    let deadline = bot.world.ticks + BONES_WAIT_TICKS;
    let found = loop {
        let ground: Vec<((i32, i32), u16)> = bot
            .world
            .ground_items(death.0, death.1, bot.world.level, 2, None)
            .into_keys()
            .map(|((tile_x, tile_z, _), oid)| ((tile_x, tile_z), oid))
            .collect();
        if let Some(found) = pick_kill_bones(&ground, death, |oid| bury_op(oid).is_some()) {
            break Some(found);
        }
        if bot.world.ticks >= deadline {
            break None;
        }
        bot.pump(1)?;
    };
    let Some(((bones_x, bones_z), oid)) = found else {
        return Ok(false);
    };
    let name = obj_label(bot, oid);
    if free_slots(bot) == 0 {
        bot.log(&format!("kill bones {name} at ({bones_x},{bones_z}) left: pack full at {:?}", bot.position()));
        return Ok(false);
    }
    match bot.take(Some(oid), None, TAKE_TICKS) {
        Ok(true) => {}
        Ok(false) => {
            bot.log(&format!("kill bones {name} at ({bones_x},{bones_z}) gave nothing at {:?}", bot.position()));
            return Ok(false);
        }
        Err(error)
            if matches!(error, lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)) =>
        {
            return Err(error.into())
        }
        Err(error) => {
            bot.log(&format!("kill bones {name} at ({bones_x},{bones_z}) failed at {:?}: {error}", bot.position()));
            return Ok(false);
        }
    }
    Ok(bury_bones(bot)? > 0)
}

/// Reference value of a drop name (shop price today, market reference
/// at L5). Unknown names are worthless, never priceless.
pub fn drop_value(name: &str) -> i64 {
    crate::kit::pack::obj_price(name)
}

/// Take ground drops nearest-first, coins first, buryables never (the
/// kill's own bones go through `bury_kill_bones`). Returns the taken
/// count. Dead sockets fail typed; refused takes only log.
pub fn loot_ground(bot: &mut Bot) -> Result<u32, ScriptError> {
    // A ranged run keeps one slot for its arrows (picked up before the
    // loot, then merged into the quiver): junk never starves them.
    let reserve = if bot.combat_style == "ranged" { 1 } else { 0 };
    let mut taken = 0u32;
    let mut items: Vec<((i32, i32, i32), u16)> = bot
        .world
        .ground_items(bot.world.x, bot.world.z, bot.world.level, LOOT_RADIUS, None)
        .into_iter()
        .map(|((tile, oid), _)| (tile, oid))
        // Refused this session (past a fence): never re-clicked per kill.
        .filter(|(tile, oid)| !bot.unreachable_objs.contains(&(*oid, tile.0, tile.1, tile.2)))
        .collect();
    items.sort_by_key(|(tile, _)| (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs());
    // Coins first: a nearly-full pack never starves the coin pickup.
    let coins = (bot.tables.find_obj_by_name)("coins");
    items.sort_by_key(|(_, oid)| if Some(*oid) == coins { 0 } else { 1 });
    for (_, oid) in items {
        if free_slots(bot) <= reserve {
            break;
        }
        let name = (bot.tables.obj_name)(oid).unwrap_or_default();
        if name.is_empty() || bury_op(oid).is_some() {
            continue;
        }
        // Dead socket, not a missed pickup: typed instead of skipping.
        match bot.take(Some(oid), None, TAKE_TICKS) {
            Ok(true) => taken += 1,
            Ok(false) => bot.log(&format!("loot {name} gave nothing at {:?}", bot.position())),
            Err(error)
                if matches!(
                    error,
                    lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)
                ) =>
            {
                return Err(error.into())
            }
            Err(error) => bot.log(&format!("loot {name} failed at {:?}: {error}", bot.position())),
        }
    }
    bury_bones(bot)?;
    Ok(taken)
}


/// Arrow recovery radius: shots land around the target, which a ranged
/// bot hits from up to ~7 tiles away.
pub const ARROW_RADIUS: i32 = 12;

/// Pick up every arrow lying around, nearest first (PRD3 lot 6, user
/// rule: arrows are picked up whenever the bot is out of combat). Arrows
/// stack, so a full pack still takes a type it already carries. Returns
/// the stacks taken.
pub fn recover_arrows(bot: &mut Bot) -> Result<u32, ScriptError> {
    // The killing shot drops its arrow after the projectile flight
    // (`inv_dropitem_delayed`, player_ranged.rs2): let it land first.
    bot.pump(5)?;
    let mut items: Vec<((i32, i32, i32), u16)> = bot
        .world
        .ground_items(bot.world.x, bot.world.z, bot.world.level, ARROW_RADIUS, None)
        .into_iter()
        .map(|((tile, oid), _)| (tile, oid))
        .filter(|(_, oid)| {
            lifescape_gamedata::obj_by_id(usize::from(*oid))
                .and_then(|obj| obj.name)
                .is_some_and(crate::activities::ranged_style::is_arrow)
        })
        .collect();
    items.sort_by_key(|(tile, _)| (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs());
    if !items.is_empty() {
        bot.log(&format!("arrows on the ground: {} stacks within {ARROW_RADIUS}", items.len()));
    }
    let mut taken = 0u32;
    for (_, oid) in items {
        let carried = pack_slots(bot).iter().any(|(_, (held, _))| *held == oid);
        if free_slots(bot) == 0 && !carried {
            continue;
        }
        match bot.take(Some(oid), None, TAKE_TICKS) {
            Ok(true) => taken += 1,
            Ok(false) => {}
            Err(error)
                if matches!(error, lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)) =>
            {
                return Err(error.into())
            }
            Err(error) => bot.log(&format!("arrow pickup failed at {:?}: {error}", bot.position())),
        }
    }
    if taken > 0 {
        bot.log(&format!("ok recovered {taken} arrow stacks"));
    }
    Ok(taken)
}

/// Wield every carried arrow stack the worn bow accepts: picked-up arrows
/// land in the pack, wielding merges them into the quiver and frees the
/// slot for the next pickup. Returns the stacks merged.
pub fn merge_arrows(bot: &mut Bot) -> Result<u32, ScriptError> {
    let mut merged = 0;
    for _ in 0..4 {
        let carried: Vec<u16> = pack_slots(bot)
            .iter()
            .map(|(_, (held, _))| *held)
            .filter(|id| {
                lifescape_gamedata::obj_by_id(usize::from(*id))
                    .and_then(|obj| obj.name)
                    .is_some_and(crate::activities::ranged_style::is_arrow)
            })
            .collect();
        let Some(first) = carried.first().copied() else { break };
        if !crate::kit::gear::wield_held(bot, first)? {
            break;
        }
        merged += 1;
    }
    Ok(merged)
}

/// Bury every buryable in the pack (frees slots, trains prayer), one
/// click per bone, each watched until its slot empties. Returns the
/// count buried. A bone still held after `BURY_CLICKS` clicks fails
/// typed instead of clicking forever.
pub fn bury_bones(bot: &mut Bot) -> Result<u32, ScriptError> {
    let mut buried = 0u32;
    let mut clicked: Option<(u8, u16)> = None;
    let mut clicks = 0u32;
    loop {
        let found = pack_slots(bot).into_iter().find_map(|(slot, (held, _))| bury_op(held).map(|op| (slot, held, op)));
        let Some((slot, held, op)) = found else {
            return Ok(buried);
        };
        if clicked == Some((slot, held)) {
            clicks += 1;
        } else {
            clicked = Some((slot, held));
            clicks = 1;
        }
        if clicks > BURY_CLICKS {
            return Err(Error::Unreachable(format!(
                "{} in slot {slot} still held after {BURY_CLICKS} bury clicks at {:?}",
                obj_label(bot, held),
                bot.position()
            ))
            .into());
        }
        let Some(pack_com) = bot.backpack().0 else {
            return Err(Error::Unreachable("backpack not streamed".to_string()).into());
        };
        bot.op_held(op, held, slot, pack_com)?;
        let deadline = bot.world.ticks + BURY_TICKS;
        while bot.world.ticks < deadline {
            bot.pump(1)?;
            if !pack_slots(bot).iter().any(|(at, (now_held, _))| *at == slot && *now_held == held) {
                buried += 1;
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{bury_op, drop_value, pick_kill_bones};
    use lifescape_gamedata as knowledge;

    fn obj(name: &str) -> u16 {
        let entry = knowledge::objs_by_name(name).into_iter().next().unwrap_or_else(|| panic!("{name} in dumps"));
        u16::try_from(entry.obj_id).expect("obj id fits u16")
    }

    /// Bury is inventory op 1 on every bones obj (bury_bone.rs2
    /// `[opheld1,_bones]`), absent on junk and on the banknote.
    #[test]
    fn bury_op_reads_the_dump() {
        assert_eq!(bury_op(obj("bones")), Some(1));
        assert_eq!(bury_op(obj("big_bones")), Some(1));
        assert_eq!(bury_op(obj("bones_burnt")), Some(1));
        assert_eq!(bury_op(obj("cow_hide")), None, "hides are loot, not buried");
        assert_eq!(bury_op(obj("cert_bones")), None, "banknotes are never buried");
    }

    /// The kill's bones: on or next to the death tile, closest first;
    /// bones two tiles off (another kill) and junk on the tile never win.
    #[test]
    fn kill_bones_pick_the_death_tile() {
        let buryable = |oid: u16| oid == 526 || oid == 532;
        let ground = vec![((3250, 3290), 1739), ((3252, 3290), 526), ((3251, 3291), 532), ((3250, 3290), 526)];
        assert_eq!(pick_kill_bones(&ground, (3250, 3290), buryable), Some(((3250, 3290), 526)), "on the tile wins");
        let off_tile = vec![((3250, 3290), 1739), ((3251, 3291), 532), ((3253, 3290), 526)];
        assert_eq!(pick_kill_bones(&off_tile, (3250, 3290), buryable), Some(((3251, 3291), 532)), "diagonal neighbour still the kill's");
        let elsewhere = vec![((3250, 3290), 1739), ((3252, 3290), 526)];
        assert_eq!(pick_kill_bones(&elsewhere, (3250, 3290), buryable), None, "two tiles off is another kill");
    }

    /// Reference value: known junk prices above zero, unknown never wins.
    #[test]
    fn drop_value_prices_junk_above_zero() {
        assert!(drop_value("coins") >= 0, "coins have a reference");
        assert!(drop_value("bronze_sword") > 0, "shop stock prices");
        assert_eq!(drop_value("no_such_item_xyz"), 0, "unknown is worthless, got: {}", drop_value("no_such_item_xyz"));
    }
}
