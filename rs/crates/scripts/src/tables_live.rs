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
    if let Some(entry) = lifescape_gamedata::objs_search(needle).into_iter().next() {
        return Some(entry.obj_id as u16);
    }
    // Live callers pass display names ("Shrimps"), never internal keys
    // ("shrimp"): exact display fallback, first dump row winning (the
    // cooked shrimp sorts before its certificate). Fixes the banked-food
    // gate (Usage unknown obj "Shrimps") for every caller at once.
    let wanted = needle.to_lowercase();
    lifescape_gamedata::all_objs()
        .iter()
        .find(|entry| entry.display.is_some_and(|display| display.to_lowercase() == wanted))
        .map(|entry| entry.obj_id as u16)
}

fn com_id(dotted: &str) -> Option<u16> {
    lifescape_gamedata::com_id(dotted)
}

fn bank_client_stands(x: i32, z: i32, level: i32) -> Option<Vec<(i32, i32)>> {
    lifescape_gamedata::bank_stands::client_stands(i64::from(x), i64::from(z), i64::from(level))
        .map(|stands| stands.iter().map(|(stand_x, stand_z)| (*stand_x as i32, *stand_z as i32)).collect())
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
    bank_client_stands,
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

    /// open_bank reads the client side of each booth from gamedata: the
    /// Varrock west east stand (3187,3436) is the bankers' side, never
    /// listed; a members booth is untabled (open_bank fails typed).
    #[test]
    fn live_resolves_bank_client_stands() {
        assert_eq!((LIVETABLES.bank_client_stands)(3186, 3436, 0), Some(vec![(3185, 3436)]));
        assert_eq!((LIVETABLES.bank_client_stands)(3256, 3419, 0), Some(vec![(3256, 3420), (3257, 3419)]));
        assert!((LIVETABLES.bank_client_stands)(2721, 3494, 0).is_none());
    }

    /// Live callers pass display names ("Shrimps"), never internal keys:
    /// the banked-food gate failed with Usage unknown obj "Shrimps".
    #[test]
    fn live_resolves_display_names() {
        assert_eq!((LIVETABLES.find_obj_by_name)("Shrimps"), Some(315), "cooked shrimps by display");
        assert_eq!((LIVETABLES.find_obj_by_name)("shrimp"), Some(315), "internal key still first");
    }
}
