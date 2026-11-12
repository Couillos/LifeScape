//! Player design (identikit) packet: `IDK_SAVEDESIGN`.
//!
//! Byte layout copied from Client-TS (branch 274) `Client.ts`
//! `CC_ACCEPT_DESIGN`: `p1(idkDesignGender ? 0 : 1)`, seven `p1` kits
//! (an absent kit is -1, written as 255), five `p1` colour indices;
//! fixed 13-byte payload (`ClientProt.IDK_SAVEDESIGN = 125`). The real
//! client sends it right before the accept `IF_BUTTON`, while the
//! server still has `allowdesign(true)` (tutorial.rs2 `player_kit`).
//! Which kits/colours the server accepts is game data
//! (`lifescape_gamedata::player_design`).

use crate::buffer::Writer;
use crate::client::Bot;
use crate::error::Result;

/// Body part slots on the wire.
pub const DESIGN_KIT_SLOTS: usize = 7;
/// Colour slots on the wire.
pub const DESIGN_COLOUR_SLOTS: usize = 5;
/// Absent kit marker (`-1` as one unsigned byte; server reads 255 as -1).
const NO_KIT: u8 = 255;

/// `IDK_SAVEDESIGN` payload: gender, 7 kits (None -> 255), 5 colours.
pub fn idk_savedesign_bytes(gender: u8, kits: &[Option<u8>; DESIGN_KIT_SLOTS], colours: &[u8; DESIGN_COLOUR_SLOTS]) -> Vec<u8> {
    let mut body = Writer::new();
    body.u8(gender);
    for kit in kits {
        body.u8(kit.unwrap_or(NO_KIT));
    }
    for colour in colours {
        body.u8(*colour);
    }
    body.bytes()
}

impl Bot {
    /// Send a full look. The server drops it silently when invalid or
    /// when the design screen is not open; callers then press accept.
    pub fn save_player_design(&mut self, gender: u8, kits: &[Option<u8>; DESIGN_KIT_SLOTS], colours: &[u8; DESIGN_COLOUR_SLOTS]) -> Result<()> {
        self.send_named("IDK_SAVEDESIGN", &idk_savedesign_bytes(gender, kits, colours))
    }
}

#[cfg(test)]
mod tests {
    use super::idk_savedesign_bytes;
    use crate::protocol::client_op;

    #[test]
    fn payload_matches_the_client_layout() {
        // Woman (wire 1), no jaw (-1 -> 255), colours in slot order.
        let bytes = idk_savedesign_bytes(1, &[Some(48), None, Some(57), Some(63), Some(68), Some(73), Some(80)], &[11, 0, 15, 5, 7]);
        assert_eq!(bytes, vec![1, 48, 255, 57, 63, 68, 73, 80, 11, 0, 15, 5, 7]);
        // Server default man (Player.ts body/colors).
        let default_man = idk_savedesign_bytes(0, &[Some(0), Some(10), Some(18), Some(26), Some(33), Some(36), Some(42)], &[0; 5]);
        assert_eq!(default_man, vec![0, 0, 10, 18, 26, 33, 36, 42, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn payload_size_matches_the_opcode_table() {
        let (op_id, size) = client_op("IDK_SAVEDESIGN").expect("IDK_SAVEDESIGN");
        assert_eq!(op_id, 125, "ClientProt.IDK_SAVEDESIGN");
        let bytes = idk_savedesign_bytes(0, &[Some(0); 7], &[0; 5]);
        assert_eq!(bytes.len() as i16, size, "fixed 13-byte payload");
    }
}
