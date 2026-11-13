//! Tests of `tools.rs`: every production method names its gear
//! (audit 2026-11-13: smith, fire, jewellery, runecraft and leather legs
//! dispatched blind), and a missing tool blocks typed.

use super::{jewelry_mould, method_feasible, required_tool, rune_talisman};
use crate::expand::ReadyCtx;

#[test]
fn production_methods_name_their_gear() {
    let tool = |method: &str| required_tool(method).map(|need| (need.tool, need.bait));
    assert_eq!(tool("smith:bronze_sword"), Some(("hammer", None)));
    assert_eq!(tool("fire:willow_logs"), Some(("tinderbox", None)));
    assert_eq!(tool("jewelry:gold_ring"), Some(("ring_mould", None)));
    assert_eq!(tool("jewelry:unstrung_gold_amulet"), Some(("amulet_mould", None)));
    assert_eq!(tool("runecraft:airrune"), Some(("air_talisman", None)));
    assert_eq!(tool("craft:leather_gloves"), Some(("needle", Some("thread"))));
    assert_eq!(tool("farm:raw_trout"), Some(("fly_fishing_rod", Some("feather"))));
    assert_eq!(tool("farm:gold_ore"), Some(("bronze_pickaxe", None)));
    assert_eq!(tool("farm:willow_logs"), Some(("bronze_axe", None)));
    assert_eq!(tool("farm:feather"), None, "a drop farm fights bare-handed");
    assert_eq!(jewelry_mould("nostringstar"), Some("holy_symbol_mould"));
    assert_eq!(jewelry_mould("sapphire_necklace"), Some("necklace_mould"));
    assert_eq!(rune_talisman("bodyrune"), Some("body_talisman"));
    assert_eq!(rune_talisman("naturerune"), None, "members altar");
    // Every named gear is a real object.
    for method in ["smith:bronze_sword", "fire:logs", "jewelry:gold_ring", "jewelry:nostringstar", "runecraft:firerune", "craft:leather_gloves"] {
        let need = required_tool(method).expect("gear");
        assert!(!lifescape_gamedata::objs_by_name(need.tool).is_empty(), "{} exists", need.tool);
    }
}

/// A talisman drops from goblins (a farmable drop): never a shop buy, so
/// without one in hand the runecraft leg is not workable; a hammer is
/// shop stock the wallet funds.
#[test]
fn gear_feasibility_follows_the_merchant_rule() {
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.coins = 100;
    assert!(!method_feasible("runecraft:airrune", &ctx), "talisman: market or goblins");
    ctx.tools = vec!["air_talisman".to_string()];
    assert!(method_feasible("runecraft:airrune", &ctx));
    assert!(method_feasible("smith:bronze_sword", &ctx), "the hammer is shop stock");
    ctx.coins = 0;
    assert!(!method_feasible("smith:bronze_sword", &ctx), "broke and no hammer");
}
