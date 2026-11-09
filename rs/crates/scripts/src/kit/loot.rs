//! Loot: ground-pickup policy after a kill. Takes everything but bones
//! (D16: tout compte — loot, produced and ground spawns all feed the
//! rates), coins first so a nearly-full pack never starves the coin
//! pickup. The value reference is the shop price today
//! (`drop_value`); the market reference price plugs in at L5 without
//! changing the policy. Provenance tagging lands with the task report
//! (L2); the take loop already names every item it touches.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::skill_common::{bury_bones, free_slots};
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
    crate::skill_common::obj_price(name)
}

/// Take ground drops nearest-first, coins first, bones never. Returns
/// the taken count. Dead sockets fail typed; refused takes only log.
pub fn loot_ground(bot: &mut Bot) -> Result<u32, ScriptError> {
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
        if free_slots(bot) == 0 {
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
