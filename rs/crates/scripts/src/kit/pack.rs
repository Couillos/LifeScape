//! Pack: backpack reads and object-table lookups. State reads
//! only, never a walk or a send; unknown ids fail loudly, never
//! guessed. Absorbed from the old `skill_common` layer (D61).

#![forbid(unsafe_code)]

use std::collections::HashSet;

use lifescape_gamedata as knowledge;
use lifescape_sdk::{Bot, Error};

use crate::ScriptError;
pub const BACKPACK_SIZE: usize = 28;

/// Resolve an object id through the injected tables. Unknown names fail,
// never guess.
pub fn obj_id(bot: &Bot, name: &str) -> Result<u16, ScriptError> {
    (bot.tables.find_obj_by_name)(name)
        .ok_or_else(|| ScriptError::Usage(format!("unknown obj {name:?} at {:?}", bot.position())))
}

/// Shop price from the dump (0 when the dump carries none).
pub fn obj_price(name: &str) -> i64 {
    knowledge::objs_by_name(name).first().map(|entry| entry.price).unwrap_or(0)
}

/// Cheapest NPC-shop purchase price at base stock, the server formula
/// (`shop.rs2` `calc_shop_value`: cost × max(100, sell multiplier) /
/// 1000). `None` when no keeper stocks it: nothing to buy, never a
/// guessed price. Understocked shops charge more; the buy itself still
/// checks the coins the trade asks.
pub fn shop_price(name: &str) -> Option<i64> {
    let base = obj_price(name);
    knowledge::shops::all_shops()
        .into_iter()
        .filter(|shop| shop.members != Some(true) && shop.stocks.iter().any(|stock| stock.obj == name))
        .flat_map(|shop| shop.keepers.into_iter().map(|keeper| keeper.sell_multiplier.unwrap_or(1000)))
        .map(|multiplier| (base * multiplier.max(100) / 1000).max(1))
        .min()
}

/// Display name for logs, empty when the tables do not know the id.
pub fn obj_label(bot: &Bot, obj: u16) -> String {
    (bot.tables.obj_name)(obj).unwrap_or_default()
}

/// (slot, (obj, count)) for every backpack slot.
pub fn pack_slots(bot: &Bot) -> Vec<(u8, (u16, u32))> {
    bot.backpack().1
}

/// Total carried count of one object id.
pub fn pack_total(bot: &Bot, obj: u16) -> u32 {
    pack_slots(bot).iter().filter(|(_, (held, _))| *held == obj).map(|(_, (_, count))| *count).sum()
}

/// First backpack slot holding the object, or a loud error.
pub fn pack_slot(bot: &Bot, obj: u16) -> Result<u8, ScriptError> {
    pack_slots(bot)
        .iter()
        .find(|(_, (held, _))| *held == obj)
        .map(|(slot, _)| *slot)
        .ok_or_else(|| Error::Unreachable(format!("no obj {obj} in pack at {:?}", bot.position())).into())
}

/// Free backpack slots (28-cap, corpses of full loads).
pub fn free_slots(bot: &Bot) -> usize {
    BACKPACK_SIZE.saturating_sub(pack_slots(bot).len())
}

/// Distinct object ids currently carried.
pub fn pack_ids(bot: &Bot) -> HashSet<u16> {
    pack_slots(bot).iter().map(|(_, (held, _))| *held).collect()
}

#[cfg(test)]
mod tests {
    /// Purchase price from the dump (Gerrant sells at 100%, general
    /// stores at 130%); unstocked items have no price at all.
    #[test]
    fn shop_price_follows_the_server_formula() {
        assert_eq!(super::shop_price("net"), Some(super::obj_price("net")), "Gerrant sells nets at base");
        assert!(super::shop_price("fishing_rod").is_some_and(|price| price >= super::obj_price("fishing_rod")));
        assert_eq!(super::shop_price("dragon_platebody"), None, "no keeper stocks it");
    }
}
