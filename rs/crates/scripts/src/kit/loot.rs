//! Loot: ground-pickup policy after a kill. Takes everything but bones
//! (D16: tout compte — loot, produced and ground spawns all feed the
//! rates), coins first so a nearly-full pack never starves the coin
//! pickup. The value reference is the shop price today
//! (`drop_value`); the market reference price plugs in at L5 without
//! changing the policy. Provenance tagging lands with the task report
//! (L2); the take loop already names every item it touches.

#![forbid(unsafe_code)]

use lifescape_sdk::{Bot, Error};

use crate::kit::pack::{free_slots, obj_label, pack_slots};
use crate::ScriptError;

/// Worst-case take: the drop lands on the death tick (goblin.rs2 obj_add
/// right after npc_death), walk <= 6 tiles + click. 600 burned 12 s on every
/// vanished/unreachable item; failures now log so the bound stays honest.
pub const TAKE_TICKS: u64 = 100;
/// Pickup radius around the killer spot.
pub const LOOT_RADIUS: i32 = 6;

/// Reference value of a drop name (shop price today, market reference
/// at L5). Unknown names are worthless, never priceless.
pub fn drop_value(name: &str) -> i64 {
    crate::kit::pack::obj_price(name)
}

/// Take ground drops nearest-first, coins first, bones never. Returns
/// the taken count. Dead sockets fail typed; refused takes only log.
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
        if name.is_empty() || name.to_lowercase().contains("bones") {
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

/// Bury every bone in the pack (frees slots, trains prayer).
pub fn bury_bones(bot: &mut Bot) -> Result<(), ScriptError> {
    loop {
        let found = pack_slots(bot).into_iter().find(|(_, (held, _))| {
            obj_label(bot, *held).to_lowercase().contains("bones")
        });
        let Some((slot, (held, _))) = found else {
            return Ok(());
        };
        let Some(pack_com) = bot.backpack().0 else {
            return Err(Error::Unreachable("backpack not streamed".to_string()).into());
        };
        bot.op_held(1, held, slot, pack_com)?;
        bot.pump(2)?;
    }
}

#[cfg(test)]
mod tests {
    use super::drop_value;

    /// Reference value: known junk prices above zero, unknown never wins.
    #[test]
    fn drop_value_prices_junk_above_zero() {
        assert!(drop_value("coins") >= 0, "coins have a reference");
        assert!(drop_value("bronze_sword") > 0, "shop stock prices");
        assert_eq!(drop_value("no_such_item_xyz"), 0, "unknown is worthless, got: {}", drop_value("no_such_item_xyz"));
    }
}
