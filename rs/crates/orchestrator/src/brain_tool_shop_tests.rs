//! Tests of `brain_tool_shop` (split out, rule 4): the merchant tool
//! rule on the rev 274 data (Nurmof: iron 140, steel 500, mithril 1300,
//! adamant 3200, rune 32000 coins).

use super::{merchant_tool, tool_purchase, ToolPurchase};

fn owned(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

fn bought(item: &str, price: i64) -> Option<ToolPurchase> {
    Some(ToolPurchase { item: item.to_string(), price })
}

/// Live 2026-11-14: mining 41 bots with a bronze pickaxe bid on rune and
/// mithril pickaxes nobody sells. The shop sells the best tier the
/// wallet share pays, never more.
#[test]
fn the_best_tier_the_wallet_share_pays() {
    let levels = [("mining", 41)];
    let bronze = owned(&["bronze_pickaxe"]);
    assert_eq!(tool_purchase("pickaxe", &levels, &bronze, 100_000), bought("rune_pickaxe", 32_000));
    assert_eq!(tool_purchase("pickaxe", &levels, &bronze, 3_199), bought("mithril_pickaxe", 1_300));
    assert_eq!(tool_purchase("pickaxe", &levels, &bronze, 500), bought("steel_pickaxe", 500));
    assert_eq!(tool_purchase("pickaxe", &levels, &bronze, 499), bought("iron_pickaxe", 140));
    assert_eq!(tool_purchase("pickaxe", &levels, &bronze, 139), None, "nothing affordable: the bot earns first");
}

/// The level gate is the server's (`pickaxe_checker.rs2`): iron mines
/// from level 1, steel from 6, mithril from 21.
#[test]
fn the_level_opens_the_tier() {
    let bronze = owned(&["bronze_pickaxe"]);
    assert_eq!(tool_purchase("pickaxe", &[("mining", 1)], &bronze, 100_000), bought("iron_pickaxe", 140));
    assert_eq!(tool_purchase("pickaxe", &[("mining", 20)], &bronze, 100_000), bought("steel_pickaxe", 500));
}

/// No loop: the owned tier ends the purchases until the level opens a
/// better one; a bot without the family buys nothing here (the graph
/// obtains a first tool).
#[test]
fn only_an_upgrade_of_an_owned_family() {
    assert_eq!(tool_purchase("pickaxe", &[("mining", 41)], &owned(&["rune_pickaxe", "bronze_pickaxe"]), 100_000), None);
    assert_eq!(tool_purchase("pickaxe", &[("mining", 20)], &owned(&["steel_pickaxe"]), 100_000), None, "mithril needs 21");
    assert_eq!(tool_purchase("pickaxe", &[("mining", 41)], &owned(&["coins", "bronze_axe"]), 100_000), None);
    // A rune pickaxe above the level does not count: steel is the
    // upgrade the server will use.
    assert_eq!(tool_purchase("pickaxe", &[("mining", 6)], &owned(&["rune_pickaxe", "iron_pickaxe"]), 100_000), bought("steel_pickaxe", 500));
}

/// Woodcutting axes are smithable (D-A): no shop step, the wishlist
/// keeps its market bid.
#[test]
fn smithable_axes_stay_on_the_market() {
    assert_eq!(tool_purchase("axe", &[("woodcutting", 41)], &owned(&["bronze_axe"]), 100_000), None);
    assert!(merchant_tool("rune_pickaxe") && merchant_tool("iron_pickaxe"));
    assert!(!merchant_tool("rune_axe") && !merchant_tool("steel_axe"), "axes: player trade");
    assert!(!merchant_tool("net"), "not a gathering tool of the wishlist");
}
