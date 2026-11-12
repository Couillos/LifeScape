//! Player look (identikit): the kits and colours the server accepts on
//! `IDK_SAVEDESIGN`, plus a deterministic per-account design draw.
//!
//! Server truth (rev 274):
//! - `engine/src/network/game/client/handler/IdkSaveDesignHandler.ts`:
//!   gender 0 (man) or 1 (woman); body part `slot` must hold a kit whose
//!   `type` is `slot` (+7 for a woman) and that is not `disable`; the one
//!   exception is the woman jaw, which must be -1 (no woman_jaw kit
//!   exists). Colour `slot` must index `Player.DESIGN_BODY_COLORS[slot]`.
//! - Kit ids are the `content/pack/idk.pack` order; types and `disable`
//!   come from `content/scripts/player/configs/{man,woman}.idk`.
//! - `engine/src/engine/entity/Player.ts` `DESIGN_BODY_COLORS` row
//!   lengths (12, 16, 16, 6, 8) equal the client `ClientPlayer.recol1d`
//!   lengths (Client-TS `src/dash3d/ClientPlayer.ts`).
//!
//! The server default (`Player.ts` `body`/`colors`) is the same man for
//! every new account: the draw below replaces it at the tutorial
//! design screen. The table test cross-checks these constants against
//! the server content so a content change fails loudly.

/// Body part slots in packet order (`IdkSaveDesign` idkit[0..7]).
pub const BODY_PARTS: [&str; 7] = ["hair", "jaw", "torso", "arms", "hands", "legs", "feet"];

/// Colour slots in packet order (`IdkSaveDesign` color[0..5]).
pub const COLOUR_PARTS: [&str; 5] = ["hair", "torso", "legs", "feet", "skin"];

/// Number of choices per colour slot (`Player.DESIGN_BODY_COLORS` row
/// lengths); a colour index must stay strictly below its count.
pub const COLOUR_COUNTS: [u8; 5] = [12, 16, 16, 6, 8];

/// Enabled man kits per body part (`man.idk`, ids from `idk.pack`).
pub const MAN_KITS: [&[u8]; 7] = [
    &[0, 1, 2, 3, 4, 5, 6, 7, 8],         // man_hair (9 demon disabled)
    &[10, 11, 12, 13, 14, 15, 16, 17],    // man_jaw
    &[18, 19, 20, 21, 22, 23, 24, 25],    // man_torso
    &[26, 27, 28, 29, 30, 31],            // man_arms (32 demon disabled)
    &[33, 34],                            // man_hands (35 demon disabled)
    &[36, 37, 38, 39, 40],                // man_legs (41 demon disabled)
    &[42, 43],                            // man_feet (44 demon disabled)
];

/// Enabled woman kits per body part (`woman.idk`); the jaw has none and
/// is sent as -1 (server exception for type 8).
pub const WOMAN_KITS: [&[u8]; 7] = [
    &[45, 46, 47, 48, 49, 50, 51, 52, 53, 54], // woman_hair (55 demon disabled)
    &[],                                       // woman_jaw: none, sent as -1
    &[56, 57, 58, 59, 60],                     // woman_torso
    &[61, 62, 63, 64, 65],                     // woman_arms (66 demon disabled)
    &[67, 68],                                 // woman_hands (69 demon disabled)
    &[70, 71, 72, 73, 74, 75, 76, 77],         // woman_legs (78 demon disabled)
    &[79, 80],                                 // woman_feet (81 demon disabled)
];

/// Wire gender for a man (Client-TS sends `idkDesignGender ? 0 : 1`).
pub const GENDER_MAN: u8 = 0;
/// Wire gender for a woman.
pub const GENDER_WOMAN: u8 = 1;

/// One full look: what `IDK_SAVEDESIGN` carries. A `None` kit is the
/// absent part (only the woman jaw), sent as 255 / read as -1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlayerDesign {
    pub gender: u8,
    pub kits: [Option<u8>; 7],
    pub colours: [u8; 5],
}

/// Allowed kits per body part for a wire gender, None for gender > 1.
pub fn kits_for_gender(gender: u8) -> Option<&'static [&'static [u8]; 7]> {
    match gender {
        GENDER_MAN => Some(&MAN_KITS),
        GENDER_WOMAN => Some(&WOMAN_KITS),
        _ => None,
    }
}

/// Mirror of the server handler checks: true when the server would
/// apply this design instead of silently dropping it.
pub fn is_valid(design: &PlayerDesign) -> bool {
    let Some(allowed) = kits_for_gender(design.gender) else {
        return false;
    };
    let kits_ok = design.kits.iter().zip(allowed.iter()).all(|(kit, choices)| match kit {
        Some(id) => choices.contains(id),
        None => choices.is_empty(),
    });
    let colours_ok = design.colours.iter().zip(COLOUR_COUNTS.iter()).all(|(colour, count)| colour < count);
    kits_ok && colours_ok
}

/// Stable 64-bit seed for an account name (FNV-1a over the lowercased,
/// trimmed name: the server treats names case-insensitively).
pub fn account_seed(account: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in account.trim().to_ascii_lowercase().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// splitmix64 step: one well-mixed word per call.
fn next_word(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut word = *state;
    word = (word ^ (word >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    word = (word ^ (word >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    word ^ (word >> 31)
}

fn pick_index(state: &mut u64, count: usize) -> usize {
    (next_word(state) % count as u64) as usize
}

/// Deterministic valid design for a seed: gender, one enabled kit per
/// body part for that gender, one colour index per colour slot.
pub fn design_for_seed(seed: u64) -> PlayerDesign {
    let mut state = seed;
    let gender = if next_word(&mut state) & 1 == 0 { GENDER_MAN } else { GENDER_WOMAN };
    let allowed = if gender == GENDER_MAN { &MAN_KITS } else { &WOMAN_KITS };
    let mut kits = [None; 7];
    for (slot, choices) in allowed.iter().enumerate() {
        if !choices.is_empty() {
            kits[slot] = Some(choices[pick_index(&mut state, choices.len())]);
        }
    }
    let mut colours = [0u8; 5];
    for (slot, count) in COLOUR_COUNTS.iter().enumerate() {
        colours[slot] = pick_index(&mut state, usize::from(*count)) as u8;
    }
    PlayerDesign { gender, kits, colours }
}

/// The look an account always gets: same name, same design.
pub fn design_for_account(account: &str) -> PlayerDesign {
    design_for_seed(account_seed(account))
}

#[cfg(test)]
#[path = "player_design_tests.rs"]
mod tests;
