//! Tests of `tools.rs`: every production method names its gear
//! (audit 2026-11-13: smith, fire, jewellery, runecraft and leather legs
//! dispatched blind), and a missing tool blocks typed.

use super::{jewelry_mould, method_feasible, required_tool, rune_talisman};
use crate::expand::ReadyCtx;

/// (tools, consumables) of one method, as plain strings.
fn gear(method: &str) -> Option<(Vec<&'static str>, Vec<&'static str>)> {
    required_tool(method).map(|need| {
        let leak = |names: Vec<String>| names.into_iter().map(|name| &*Box::leak(name.into_boxed_str())).collect::<Vec<&'static str>>();
        (leak(need.tools), leak(need.consumables))
    })
}

#[test]
fn production_methods_name_their_gear() {
    assert_eq!(gear("smith:bronze_sword"), Some((vec!["hammer"], vec![])));
    assert_eq!(gear("fire:willow_logs"), Some((vec!["tinderbox"], vec![])));
    assert_eq!(gear("jewelry:gold_ring"), Some((vec!["ring_mould"], vec![])));
    assert_eq!(gear("jewelry:unstrung_gold_amulet"), Some((vec!["amulet_mould"], vec![])));
    assert_eq!(gear("runecraft:airrune"), Some((vec!["air_talisman"], vec![])));
    assert_eq!(gear("craft:leather_gloves"), Some((vec!["needle"], vec!["thread"])));
    assert_eq!(gear("farm:raw_trout"), Some((vec!["fly_fishing_rod"], vec!["feather"])));
    assert_eq!(gear("farm:gold_ore"), Some((vec!["bronze_pickaxe"], vec![])));
    assert_eq!(gear("farm:willow_logs"), Some((vec!["bronze_axe"], vec![])));
    assert_eq!(gear("farm:feather"), None, "a drop farm fights bare-handed");
    assert_eq!(jewelry_mould("nostringstar").as_deref(), Some("holy_symbol_mould"));
    assert_eq!(jewelry_mould("sapphire_necklace").as_deref(), Some("necklace_mould"));
    assert_eq!(rune_talisman("bodyrune").as_deref(), Some("body_talisman"));
    assert_eq!(rune_talisman("naturerune"), None, "members altar");
    // Every named gear is a real object.
    for method in ["smith:bronze_sword", "fire:logs", "jewelry:gold_ring", "jewelry:nostringstar", "runecraft:firerune", "craft:leather_gloves"] {
        let need = required_tool(method).expect("gear");
        assert!(!lifescape_gamedata::objs_by_name(need.first()).is_empty(), "{} exists", need.first());
    }
}

/// Audit 2026-11-13: the gear came from a hand list (gem cutting, smithing
/// at farm legs, pottery, stringing dispatched blind). Every production
/// and gather method now reads its way's tools from the server data.
#[test]
fn gear_derives_from_the_ways_for_every_family() {
    assert_eq!(gear("cut:sapphire"), Some((vec!["chisel"], vec![])), "gem cutting takes a chisel");
    assert_eq!(gear("gather:fishing/raw_lobster"), Some((vec!["lobster_pot"], vec![])));
    assert_eq!(gear("gather:fishing/raw_tuna"), Some((vec!["harpoon"], vec![])));
    assert_eq!(gear("gather:fishing/raw_pike"), Some((vec!["fishing_rod"], vec!["fishing_bait"])));
    assert_eq!(gear("farm:blankrune"), Some((vec!["bronze_pickaxe"], vec![])));
    assert_eq!(gear("gather:sheep/wool"), Some((vec!["shears"], vec![])));
    assert_eq!(gear("milk:bucket_milk"), Some((vec![], vec!["bucket_empty"])));
    assert_eq!(gear("smelt:bronze_bar"), None, "a furnace needs no carried tool");
    assert_eq!(gear("spin:ball_of_wool"), None);
    assert_eq!(gear("cook/raw_shrimp"), None);
    // Magic burns the runes of the spell its tranche casts.
    let mut wind = gear("fight:magic/cows").expect("wind strike runes");
    wind.1.sort();
    assert_eq!(wind, (vec![], vec!["airrune", "mindrune"]));
    let fire = gear("fight:magic/fire_strike/cows").expect("fire strike runes");
    assert!(fire.1.contains(&"firerune") && fire.1.contains(&"mindrune"), "{fire:?}");
}

/// A skeleton's canonical tool is met by any family member: a steel
/// pickaxe for `bronze_pickaxe`, iron arrows for bronze ones, a staff of
/// air for any number of air runes.
#[test]
fn the_family_meets_the_canonical_tool() {
    let count_of = |name: &str| match name {
        "steel_pickaxe" => 1,
        "iron_arrow" => 300,
        "staff_of_air" => 1,
        _ => 0,
    };
    assert_eq!(super::family_held("bronze_pickaxe", 1, &count_of), 1);
    assert_eq!(super::family_held("bronze_arrow", 500, &count_of), 300);
    assert_eq!(super::family_held("airrune", 900, &count_of), 900);
    assert_eq!(super::family_held("hammer", 1, &count_of), 0);
}

/// An elemental staff casts unlimited runes of its element.
#[test]
fn a_staff_stands_for_its_rune() {
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.tools = vec!["mindrune".to_string(), "staff_of_air".to_string()];
    assert!(method_feasible("fight:magic/wind_strike/cows", &ctx), "staff of air + mind runes");
    ctx.tools = vec!["mindrune".to_string()];
    assert!(!method_feasible("fight:magic/wind_strike/cows", &ctx), "air runes missing");
}

/// The gear gate counts only what the bot holds: no executor buys its own
/// gear (commit 44bde31, "aucun achat interne"), so a shop tool the wallet
/// could fund is still missing and becomes an obtain sub-task BEFORE the
/// launch. Live 2026-11-13: fiona_park, bdr53jee and nina_rhodes launched
/// `fishing raw_tuna` with 500+ coins and no harpoon (blocked at once),
/// t01 `fishing raw_shrimp` without its net.
#[test]
fn gear_feasibility_counts_held_gear_only() {
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.coins = 1000;
    assert!(!method_feasible("runecraft:airrune", &ctx), "talisman: market or goblins");
    ctx.tools = vec!["air_talisman".to_string()];
    assert!(method_feasible("runecraft:airrune", &ctx));
    assert!(!method_feasible("smith:bronze_sword", &ctx), "a fundable hammer is not a held hammer");
    assert!(!method_feasible("farm:raw_tuna", &ctx), "a fundable harpoon is not a held harpoon");
    assert!(!super::gear_ok("net", &ctx), "nets are merchant goods, still bought by the obtain leg");
    ctx.tools.push("harpoon".to_string());
    assert!(method_feasible("farm:raw_tuna", &ctx));
    // A produce quota of tuna without a harpoon blocks typed at resolve.
    let mut node = crate::model::Node::new("n1", crate::model::NodeKind::ObtainItems { item: "raw_tuna".to_string(), count: 100 }, Vec::new(), 100);
    node.alternatives = crate::obtain_legs::produce_alternatives("raw_tuna");
    ctx.tools.clear();
    ctx.levels = vec![("fishing".to_string(), 40)];
    crate::expand::resolve_ready(&mut node, &ctx);
    assert_eq!(node.status, crate::model::NodeStatus::Blocked { reason: "missing:harpoon".to_string() }, "method {:?}", node.method);
}
