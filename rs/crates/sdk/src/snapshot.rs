//! Shared snapshot reads: one source for the live Bot and future tooling.
//!
//! The same plain data (inventories, chat texts, stats, positions) answers
//! read-only calls everywhere, so live reads and heartbeat snapshots never
//! drift apart.

use std::collections::HashMap;

/// Total carried per object id, across every open inventory.
/// Saturating: banked billion-stacks plus pack counts must never panic.
pub fn aggregate(inv: &HashMap<u16, HashMap<u8, (u16, u32)>>) -> HashMap<u16, u32> {
    let mut items = HashMap::new();
    for slots in inv.values() {
        for (held, count) in slots.values() {
            let total = items.entry(*held).or_insert(0u32);
            *total = total.saturating_add(*count);
        }
    }
    items
}

/// Backpack com: the canonical `inventory:inv` container when known and
/// streamed, else the widest container holding items (shop and bank side
/// panels also stream 28-ish slots but stay empty once closed, so an exact
/// capacity match is not a reliable signal). `None` before it streams.
pub fn backpack_com(
    inv: &HashMap<u16, HashMap<u8, (u16, u32)>>,
    inv_cap: &HashMap<u16, u8>,
    canonical: Option<u16>,
) -> Option<u16> {
    if let Some(com) = canonical {
        if inv.contains_key(&com) {
            return Some(com);
        }
    }
    let widest = inv
        .iter()
        .filter(|(_, slots)| !slots.is_empty())
        .max_by_key(|(com, _)| (inv_cap.get(*com).copied().unwrap_or(0), std::cmp::Reverse(**com)));
    if let Some((com, _)) = widest {
        return Some(*com);
    }
    inv.keys().min_by_key(|com| inv.get(*com).map(HashMap::len).unwrap_or(usize::MAX)).copied()
}

/// Backpack com plus its slots, or (None, {}) before it streams.
pub fn backpack_view(
    inv: &HashMap<u16, HashMap<u8, (u16, u32)>>,
    inv_cap: &HashMap<u16, u8>,
    canonical: Option<u16>,
) -> (Option<u16>, Vec<(u8, u16, u32)>) {
    let Some(com) = backpack_com(inv, inv_cap, canonical) else {
        return (None, Vec::new());
    };
    let mut slots: Vec<(u8, u16, u32)> =
        inv.get(&com).map(|entries| entries.iter().map(|(slot, (held, count))| (*slot, *held, *count)).collect()).unwrap_or_default();
    slots.sort();
    (Some(com), slots)
}

/// Bank contents: only the `bank_main:inv` com counts.
pub fn bank_items(inv: &HashMap<u16, HashMap<u8, (u16, u32)>>, bank_com: Option<u16>) -> HashMap<u16, u32> {
    match bank_com {
        Some(com) => aggregate(&inv.get(&com).map(|slots| HashMap::from([(com, slots.clone())])).unwrap_or_default()),
        None => HashMap::new(),
    }
}

/// Choice interfaces (comps.json): (interface, its `com_0` header).
const CHOICE_PAGES: [(u16, u16); 4] = [(2459, 2460), (2469, 2470), (2480, 2481), (2492, 2493)];

/// Pure: true when header `com` belongs to a multi2/3/4/5 page that is
/// not the open chat modal. The server writes a choice's IF_SETTEXTs as
/// its script runs and the IF_OPENCHAT only at the cycle's end
/// (`NetworkPlayer.encodeOut`): until the open arrives the page may be
/// half written (live 2026-11-21, the customs officer's "Why?" read alone).
fn choice_page_not_open(com: u16, chat_modal: Option<u16>) -> bool {
    CHOICE_PAGES.iter().any(|(page, header)| *header == com && chat_modal != Some(*page))
}

/// (com, text) option rows: under the latest "Select an Option" header, or
/// the fixed multi2/3/4/5 option coms when that choice interface is open.
/// Custom headers (Terrova's "Do you want to go to the mainland?", the
/// tutorial recaps) never say "Select an Option", so the header rule alone
/// blinds the bot to them. A multi2/3/4/5 header counts only while its
/// page is the open chat modal (`choice_page_not_open`).
pub fn chat_options(texts: &HashMap<u16, String>, chat_modal: Option<u16>) -> Vec<(u16, String)> {
    let mut items: Vec<(u16, &String)> = texts.iter().map(|(com, text)| (*com, text)).collect();
    items.sort();
    let mut headers: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, (com, text))| text.to_lowercase().contains("select an option") && !choice_page_not_open(*com, chat_modal))
        .map(|(index, _)| index)
        .collect();
    let mut out: Vec<(u16, String)> = Vec::new();
    if let Some(header) = headers.pop() {
        let base = items[header].0;
        out.extend(
            items[header + 1..header + 6.min(items.len() - header)]
                .iter()
                .filter(|(com, _)| *com > base && *com <= base + 5)
                .map(|(com, text)| (*com, (*text).clone())),
        );
    }
    // Fixed option coms (comps.json): multi2 2461-2462, multi3 2471-2473,
    // multi4 2482-2485, multi5 2494-2498. Only while that choice interface
    // is the open chat modal, so answered choices never leak into later
    // dialogs as stale options.
    let fixed: &[u16] = match chat_modal {
        Some(2459) => &[2461, 2462],
        Some(2469) => &[2471, 2472, 2473],
        Some(2480) => &[2482, 2483, 2484, 2485],
        Some(2492) => &[2494, 2495, 2496, 2497, 2498],
        _ => &[],
    };
    for com in fixed {
        if let Some(text) = texts.get(com) {
            if !text.trim().is_empty() && !out.iter().any(|(known, _)| known == com) {
                out.push((*com, text.clone()));
            }
        }
    }
    out.sort();
    out
}

fn xp_table() -> [u32; 99] {
    // Server Player.ts levelExperience, in wire units (classic table).
    let mut table = [0u32; 99];
    let mut acc: u32 = 0;
    for (index, slot) in table.iter_mut().enumerate() {
        let level = index as f64 + 1.0;
        let delta = (level + 2f64.powf(level / 7.0) * 300.0) as u32;
        acc += delta;
        *slot = acc / 4;
    }
    table
}

/// Server getLevelByExp: max level for an xp drop (base level).
pub fn level_for_xp(exp: i32) -> u8 {
    let table = xp_table();
    for index in (0..99).rev() {
        if exp >= table[index] as i32 {
            return ((index + 2).min(99)) as u8;
        }
    }
    1
}

/// Server getCombatLevel on base levels (never current HP).
pub fn combat_level(stats: &HashMap<u8, (u8, i32)>) -> Option<u8> {
    let base = |index: u8| -> Option<u8> { stats.get(&index).map(|(_, xp)| level_for_xp(*xp)) };
    let (attack, defence, strength, hitpoints, ranged, prayer, magic) =
        (base(0)?, base(1)?, base(2)?, base(3)?, base(4)?, base(5)?, base(6)?);
    let base_part = 0.25 * f64::from(defence + hitpoints + prayer / 2);
    let melee = 0.325 * f64::from(attack + strength);
    let ranged_part = 0.325 * f64::from(ranged / 2 + ranged);
    let magic_part = 0.325 * f64::from(magic / 2 + magic);
    Some((base_part + melee.max(ranged_part).max(magic_part)) as u8)
}

#[cfg(test)]
mod tests {
    use super::{chat_options, combat_level, level_for_xp};
    use std::collections::HashMap;

    #[test]
    fn xp_thresholds() {
        assert_eq!(level_for_xp(0), 1);
        assert!(level_for_xp(13_034_431) >= 99);
    }

    #[test]
    fn options_under_header() {
        let texts = HashMap::from([(100, "Select an Option".to_string()), (101, "Yes".to_string()), (110, "No".to_string())]);
        let options = chat_options(&texts, None);
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].1, "Yes");
    }

    #[test]
    fn options_on_fixed_multi2_coms() {
        let texts = HashMap::from([(2460, "Do you want to go to the mainland?".to_string()), (2461, "Yes.".to_string()), (2462, "No.".to_string())]);
        assert!(chat_options(&texts, None).is_empty(), "custom header alone is not options");
        let options = chat_options(&texts, Some(2459));
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].1, "Yes.");
    }

    /// Live 2026-11-21 (tom_king, rae_stone, mila_wade: `custom
    /// boat_karamja_to_port_sarim asked Why?` 4 times in 6 h): the server
    /// writes a choice's IF_SETTEXTs while the script runs and its
    /// IF_OPENCHAT only at the cycle's end (`NetworkPlayer.encodeOut`),
    /// so a pump can stop between them: the header and "Why?" arrived,
    /// the two other answers not yet, the customs page (npcchat1, 4882)
    /// still open. A multi2/3/4/5 page offers nothing until it is the
    /// open chat modal (Client.ts draws only the open interface).
    #[test]
    fn a_choice_page_not_yet_open_offers_nothing() {
        let texts = HashMap::from([
            (2470, "Select an Option".to_string()),
            (2471, "Why?".to_string()),
            (4883, "Customs officer".to_string()),
            (4884, "You need to be searched before you can board.".to_string()),
        ]);
        assert!(chat_options(&texts, Some(4882)).is_empty(), "multi3 still arriving under the customs page");
        assert!(chat_options(&texts, None).is_empty(), "no chat modal open: no multi3 page either");
        let mut full = texts.clone();
        full.insert(2472, "Search away, I have nothing to hide.".to_string());
        full.insert(2473, "You're not putting your hands on my things!".to_string());
        let options: Vec<String> = chat_options(&full, Some(2469)).into_iter().map(|(_, text)| text).collect();
        assert_eq!(options, vec!["Why?", "Search away, I have nothing to hide.", "You're not putting your hands on my things!"], "multi3 open: its three answers");
    }

    #[test]
    fn combat_needs_full_set() {
        assert_eq!(combat_level(&HashMap::new()), None);
    }

    #[test]
    fn maxed_stacks_saturate_instead_of_panicking() {
        use super::aggregate;
        let mut inv = std::collections::HashMap::new();
        inv.insert(1u16, [(0u8, (995u16, u32::MAX)), (1u8, (995u16, u32::MAX))].into_iter().collect());
        inv.insert(2u16, [(0u8, (995u16, 7u32))].into_iter().collect());
        assert_eq!(aggregate(&inv).get(&995), Some(&u32::MAX));
    }
}

