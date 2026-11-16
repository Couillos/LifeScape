//! Swap choice of a bank visit: the best banked tool the level works
//! with replaces a lesser held one, never the other way round.

use super::{banked_upgrade, upgrade_pick, UPGRADE_FAMILIES};
use crate::kit::tools::{AXE_SPEC, FISH_SPEC, PICK_SPEC};
use crate::tables_live::attach_live_tables;
use lifescape_gamedata::gear::usable_tools;
use lifescape_sdk::Bot;

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| name.to_string()).collect()
}

/// Live 2026-11-16 (jim_ellis, woodcutting 83): bronze axe in the pack,
/// iron axe in the bank → the visit swaps; the reverse keeps the iron.
#[test]
fn bronze_held_iron_banked_swaps_and_never_back() {
    let usable = usable_tools("axe", &[("woodcutting", 83)]);
    let swap = upgrade_pick(&AXE_SPEC, &names(&["bronze_axe"]), &names(&["iron_axe"]), &usable);
    assert_eq!(swap.as_deref(), Some("iron_axe"), "a better banked axe comes out");
    let keep = upgrade_pick(&AXE_SPEC, &names(&["iron_axe"]), &names(&["bronze_axe"]), &usable);
    assert_eq!(keep, None, "a lesser banked axe never replaces the held one");
    let same = upgrade_pick(&AXE_SPEC, &names(&["iron_axe"]), &names(&["iron_axe"]), &usable);
    assert_eq!(same, None, "an equal banked axe is no upgrade");
    let best = upgrade_pick(&AXE_SPEC, &names(&["bronze_axe"]), &names(&["iron_axe", "steel_axe", "bronze_axe"]), &usable);
    assert_eq!(best.as_deref(), Some("steel_axe"), "the best banked one, not the first");
    assert_eq!(upgrade_pick(&AXE_SPEC, &names(&["bronze_axe"]), &[], &usable), None, "empty bank, nothing");
}

/// A banked tool above the skill level never comes out (user rule: the
/// best tier the level opens); a usable one below it still does.
#[test]
fn banked_tool_above_the_level_stays() {
    let low = usable_tools("axe", &[("woodcutting", 1)]);
    assert!(!low.contains(&"rune_axe".to_string()), "rune axe needs more than woodcutting 1: {low:?}");
    let swap = upgrade_pick(&AXE_SPEC, &names(&["bronze_axe"]), &names(&["rune_axe"]), &low);
    assert_eq!(swap, None, "rune axe above the level stays banked");
    let pick_low = usable_tools("pickaxe", &[("mining", 30)]);
    let swap = upgrade_pick(&PICK_SPEC, &names(&["bronze_pickaxe"]), &names(&["rune_pickaxe", "iron_pickaxe"]), &pick_low);
    assert_eq!(swap.as_deref(), Some("iron_pickaxe"), "mining 30: rune stays, iron comes out");
    let pick_high = usable_tools("pickaxe", &[("mining", 41)]);
    let swap = upgrade_pick(&PICK_SPEC, &names(&["bronze_pickaxe"]), &names(&["rune_pickaxe", "iron_pickaxe"]), &pick_high);
    assert_eq!(swap.as_deref(), Some("rune_pickaxe"), "mining 41 works the rune pickaxe");
}

/// Pickaxes: iron held + bronze banked keeps the iron; a held pickaxe
/// above the level (useless to the server) yields to a usable banked one.
#[test]
fn pickaxes_follow_the_same_choice() {
    let usable = usable_tools("pickaxe", &[("mining", 1)]);
    assert_eq!(upgrade_pick(&PICK_SPEC, &names(&["iron_pickaxe"]), &names(&["bronze_pickaxe"]), &usable), None);
    let swap = upgrade_pick(&PICK_SPEC, &names(&["bronze_pickaxe"]), &names(&["iron_pickaxe"]), &usable);
    assert_eq!(swap.as_deref(), Some("iron_pickaxe"), "iron works at mining 1");
    let swap = upgrade_pick(&PICK_SPEC, &names(&["rune_pickaxe"]), &names(&["bronze_pickaxe"]), &usable);
    assert_eq!(swap.as_deref(), Some("bronze_pickaxe"), "an unusable held pickaxe yields to a working one");
}

/// Fishing tools are per-tier (net, rod, harpoon, pot), never ranks of
/// one family: no bank visit swaps them.
#[test]
fn fishing_tools_never_swap() {
    let specs: Vec<&[&str]> = UPGRADE_FAMILIES.iter().map(|(spec, _, _)| spec.order).collect();
    assert!(specs.contains(&AXE_SPEC.order) && specs.contains(&PICK_SPEC.order), "axes and pickaxes upgrade");
    assert!(!specs.contains(&FISH_SPEC.order), "fishing tools never swap");
}

/// Live state: pack, worn, bank view and skill level read off the bot.
#[test]
fn upgrade_reads_pack_bank_and_level() {
    let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
    attach_live_tables(&mut bot);
    let bronze = crate::kit::pack::obj_id(&bot, "bronze_axe").expect("bronze_axe in tables");
    let iron = crate::kit::pack::obj_id(&bot, "iron_axe").expect("iron_axe in tables");
    let rune = crate::kit::pack::obj_id(&bot, "rune_axe").expect("rune_axe in tables");
    let pack = (bot.tables.com_id)("inventory:inv").expect("inventory:inv in dumps");
    let main = (bot.tables.com_id)("bank_main:inv").expect("bank component");
    bot.world.inv.insert(pack, [(0u8, (bronze, 1u32))].into_iter().collect());
    bot.world.inv.insert(main, [(4u8, (iron, 1u32))].into_iter().collect());
    bot.world.stats.insert(crate::kit::woodcut::WC_STAT, (83, 2_000_000));
    assert_eq!(banked_upgrade(&bot, &AXE_SPEC, "axe", crate::kit::woodcut::WC_STAT).as_deref(), Some("iron_axe"));
    bot.world.inv.insert(pack, [(0u8, (iron, 1u32))].into_iter().collect());
    bot.world.inv.insert(main, [(4u8, (bronze, 1u32))].into_iter().collect());
    assert_eq!(banked_upgrade(&bot, &AXE_SPEC, "axe", crate::kit::woodcut::WC_STAT), None, "iron held, bronze banked: nothing");
    bot.world.stats.insert(crate::kit::woodcut::WC_STAT, (1, 0));
    bot.world.inv.insert(pack, [(0u8, (bronze, 1u32))].into_iter().collect());
    bot.world.inv.insert(main, [(4u8, (rune, 1u32))].into_iter().collect());
    assert_eq!(banked_upgrade(&bot, &AXE_SPEC, "axe", crate::kit::woodcut::WC_STAT), None, "rune above woodcutting 1 stays");
    bot.world.inv.insert(pack, std::collections::HashMap::new());
    assert_eq!(banked_upgrade(&bot, &AXE_SPEC, "axe", crate::kit::woodcut::WC_STAT), None, "no held tool: the restore's job");
}
