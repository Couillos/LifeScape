//! Live game tables: the one place that wires `lifescape-gamedata`
//! into the SDK. The SDK only sees `Tables` function pointers and stays
//! dependency-free; this crate (which already depends on both) owns the
//! adapter.

use lifescape_sdk::{Bot, Tables};

fn npc_size(npc_type: u16) -> u8 {
    lifescape_gamedata::npc_by_id(usize::from(npc_type)).map(|entry| entry.size).unwrap_or(1).max(1).min(255) as u8
}

fn npc_name(npc_type: u16) -> Option<String> {
    lifescape_gamedata::npc_by_id(usize::from(npc_type)).and_then(|entry| entry.name).map(str::to_string)
}

fn loc_name(loc_id: u16) -> Option<String> {
    lifescape_gamedata::loc_by_id(usize::from(loc_id)).and_then(|entry| entry.name).map(str::to_string)
}

fn obj_name(obj_id: u16) -> Option<String> {
    lifescape_gamedata::obj_display_name(usize::from(obj_id)).map(str::to_string)
}

fn obj_take_op(obj_id: u16) -> Option<u8> {
    lifescape_gamedata::obj_by_id(usize::from(obj_id))
        .and_then(|entry| entry.ops.iter().position(|name| name.as_deref().is_some_and(|label| label.to_lowercase() == "take")))
        .map(|index| index as u8 + 1)
}

fn obj_drop_op(obj_id: u16) -> Option<u8> {
    lifescape_gamedata::obj_by_id(usize::from(obj_id))
        .and_then(|entry| entry.inv_ops.iter().position(|name| name.as_deref().is_some_and(|label| label.to_lowercase() == "drop")))
        .map(|index| index as u8 + 1)
}

fn find_obj_by_name(needle: &str) -> Option<u16> {
    // Exact match first (the legacy obj_id semantic), contains as fallback.
    if let Some(entry) = lifescape_gamedata::objs_by_name(needle).into_iter().next() {
        return Some(entry.obj_id as u16);
    }
    lifescape_gamedata::objs_search(needle).into_iter().next().map(|entry| entry.obj_id as u16)
}

fn com_id(dotted: &str) -> Option<u16> {
    lifescape_gamedata::com_id(dotted)
}

/// Live tables backed by the generated dumps.
pub const LIVETABLES: Tables = Tables {
    npc_size,
    npc_name,
    loc_name,
    obj_name,
    obj_take_op,
    obj_drop_op,
    find_obj_by_name,
    com_id,
};

/// Attach the live dumps to a bot. Call once after `Bot::new`.
pub fn attach_live_tables(bot: &mut Bot) {
    bot.tables = LIVETABLES;
}

#[cfg(test)]
mod tests {
    use super::LIVETABLES;

    #[test]
    fn live_resolves_bank_com_and_coins() {
        assert!((LIVETABLES.com_id)("bank_main:inv").is_some());
        assert_eq!((LIVETABLES.find_obj_by_name)("coins"), Some(995));
    }
}
