//! Table lookups the SDK needs but must not own.
//!
//! The SDK never owns game tables: names and dumps live in the
//! scripts crate and gamedata. The `Tables` struct carries it:
//! plain function pointers, no
//! dependency, no lifetime. The default (`NOTABLES`) behaves like an
//! unknown table entry: size 1, no names, no component ids.

/// Function pointers resolving game-table facts. Set by the caller
/// (scripts crate) when the dumps are available; `NOTABLES` otherwise.
#[derive(Clone, Copy)]
pub struct Tables {
    /// NPC footprint side (1 for unknown).
    pub npc_size: fn(npc_type: u16) -> u8,
    /// NPC type name, if known.
    pub npc_name: fn(npc_type: u16) -> Option<String>,
    /// Loc type name, if known.
    pub loc_name: fn(loc_id: u16) -> Option<String>,
    /// Object display name, if known.
    pub obj_name: fn(obj_id: u16) -> Option<String>,
    /// Ground-op number (1-based) for `Take`, if any.
    pub obj_take_op: fn(obj_id: u16) -> Option<u8>,
    /// Inv-op number (1-based) for `Drop`, if any.
    pub obj_drop_op: fn(obj_id: u16) -> Option<u8>,
    /// First object id whose name contains the needle, if any.
    pub find_obj_by_name: fn(needle: &str) -> Option<u16>,
    /// Component id by dotted name, if known.
    pub com_id: fn(dotted: &str) -> Option<u16>,
    /// Client-side stands of the bank booth at (x, z, level): the stands
    /// planned in from outside the bank (gamedata `bank_stands`), never
    /// the bankers' sealed side. None when the booth is not tabled.
    pub bank_client_stands: fn(x: i32, z: i32, level: i32) -> Option<Vec<(i32, i32)>>,
}

fn unknown_size(_npc_type: u16) -> u8 {
    1
}

fn unknown_npc_name(_npc_type: u16) -> Option<String> {
    None
}

fn unknown_loc_name(_loc_id: u16) -> Option<String> {
    None
}

fn unknown_obj_name(_obj_id: u16) -> Option<String> {
    None
}

fn unknown_take_op(_obj_id: u16) -> Option<u8> {
    None
}

fn unknown_drop_op(_obj_id: u16) -> Option<u8> {
    None
}

fn unknown_find_obj(_needle: &str) -> Option<u16> {
    None
}

fn unknown_com_id(_dotted: &str) -> Option<u16> {
    None
}

fn unknown_bank_stands(_x: i32, _z: i32, _level: i32) -> Option<Vec<(i32, i32)>> {
    None
}

/// Default tables: every lookup misses, NPC size falls back to 1.
pub const NOTABLES: Tables = Tables {
    npc_size: unknown_size,
    npc_name: unknown_npc_name,
    loc_name: unknown_loc_name,
    obj_name: unknown_obj_name,
    obj_take_op: unknown_take_op,
    obj_drop_op: unknown_drop_op,
    find_obj_by_name: unknown_find_obj,
    com_id: unknown_com_id,
    bank_client_stands: unknown_bank_stands,
};

#[cfg(test)]
mod tests {
    use super::NOTABLES;

    #[test]
    fn notables_miss_size_one() {
        assert_eq!((NOTABLES.npc_size)(u16::MAX), 1);
        assert!((NOTABLES.npc_name)(0).is_none());
        assert!((NOTABLES.loc_name)(0).is_none());
        assert!((NOTABLES.obj_name)(0).is_none());
        assert!((NOTABLES.obj_take_op)(0).is_none());
        assert!((NOTABLES.obj_drop_op)(0).is_none());
        assert!((NOTABLES.find_obj_by_name)("coins").is_none());
        assert!((NOTABLES.com_id)("bank_main:inv").is_none());
        assert!((NOTABLES.bank_client_stands)(3186, 3436, 0).is_none());
    }
}
