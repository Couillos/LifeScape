//! Tests of `tool_kit` (split out, rule 4) on the rev 274 data: shop
//! prices hammer 1, tinderbox 1, bronze pickaxe 1, iron pickaxe 140,
//! steel pickaxe 500, mithril pickaxe 1300, bronze axe 16, iron axe 56,
//! steel axe 200 (Bob); mithril axes and up are smithed only.

use super::{kit_keep, kit_purchase, kit_purchases, KitPurchase};

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| name.to_string()).collect()
}

fn items(purchases: &[KitPurchase]) -> Vec<&str> {
    purchases.iter().map(|purchase| purchase.item.as_str()).collect()
}

/// User 2026-11-14: every bot owns an axe, a pickaxe, a hammer and a
/// tinderbox. A fresh bot with 100 coins buys the bronze kit, cheap
/// tools first.
#[test]
fn a_fresh_bot_buys_the_whole_base_kit() {
    let fresh = kit_purchases(&[], &[], 100);
    assert_eq!(items(&fresh), vec!["hammer", "tinderbox", "bronze_pickaxe", "bronze_axe"]);
    assert!(fresh.iter().all(|purchase| purchase.base));
    assert_eq!(fresh[3].price, 16, "Bob's bronze axe");
}

/// A poor bot buys what half its wallet pays, cheapest tier: 2 coins
/// open the 1-coin tools, the bronze axe waits for 32 coins.
#[test]
fn a_poor_wallet_buys_the_cheap_tools_first() {
    assert!(kit_purchases(&[], &[], 0).is_empty(), "no coins: the bot earns first");
    assert!(kit_purchases(&[], &[], 1).is_empty(), "half of one coin pays nothing");
    assert_eq!(items(&kit_purchases(&[], &[], 2)), vec!["hammer", "tinderbox", "bronze_pickaxe"]);
    assert_eq!(items(&kit_purchases(&[], &[], 31)), vec!["hammer", "tinderbox", "bronze_pickaxe"]);
    assert_eq!(items(&kit_purchases(&[], &[], 32)), vec!["hammer", "tinderbox", "bronze_pickaxe", "bronze_axe"]);
}

/// Upgrades: the best shop tier the level opens within a quarter of
/// the wallet (a steel pickaxe at mining 41 with 4000 coins; Bob's iron
/// axe at woodcutting 1). Never above the shop: no mithril axe here.
#[test]
fn upgrades_take_the_best_tier_a_quarter_pays() {
    let kit = names(&["hammer", "tinderbox", "bronze_pickaxe", "bronze_axe"]);
    let upgrades = kit_purchases(&[("mining", 41), ("woodcutting", 1)], &kit, 4000);
    assert_eq!(upgrades, vec![
        KitPurchase { item: "steel_pickaxe".to_string(), price: 500, base: false },
        KitPurchase { item: "iron_axe".to_string(), price: 56, base: false },
    ]);
    let rich = kit_purchases(&[("mining", 41), ("woodcutting", 60)], &kit, 1_000_000);
    assert_eq!(items(&rich), vec!["rune_pickaxe", "steel_axe"], "mithril axes and up: the market only");
    assert!(kit_purchases(&[("mining", 41), ("woodcutting", 1)], &kit, 223).is_empty(), "a quarter of 223 pays no upgrade");
}

/// No loop: the best tier owned ends the purchases until the level
/// opens a better one.
#[test]
fn the_owned_best_tier_ends_the_purchases() {
    let top = names(&["hammer", "tinderbox", "rune_pickaxe", "steel_axe"]);
    assert_eq!(kit_purchase(&[("mining", 41), ("woodcutting", 60)], &top, 1_000_000), None);
    let steel = names(&["hammer", "tinderbox", "steel_pickaxe", "iron_axe"]);
    assert_eq!(kit_purchase(&[("mining", 20), ("woodcutting", 5)], &steel, 1_000_000), None, "mithril needs 21, steel axe 6");
}

/// Server truth: a pickaxe above the mining level does not mine
/// (`pickaxe_checker.rs2`), an axe above the woodcutting level still
/// chops (`woodcutting_axe_checker`: no level in 2004).
#[test]
fn only_a_pickaxe_above_the_level_is_no_tool() {
    let held = names(&["hammer", "tinderbox", "rune_pickaxe", "rune_axe"]);
    assert_eq!(
        kit_purchase(&[("mining", 1), ("woodcutting", 1)], &held, 100),
        Some(KitPurchase { item: "bronze_pickaxe".to_string(), price: 1, base: true })
    );
    assert_eq!(kit_purchases(&[("mining", 1), ("woodcutting", 1)], &held, 100).len(), 1, "the rune axe chops");
}

/// Keep: the best held member of each family (even above the level),
/// the best working one, one hammer and one tinderbox; lower tiers and
/// foreign tools are surplus.
#[test]
fn the_kit_keeps_the_best_and_the_working_tools() {
    let held = names(&["bronze_axe", "steel_axe", "bronze_pickaxe", "iron_pickaxe", "rune_pickaxe", "hammer", "tinderbox", "net"]);
    let keep = kit_keep(&[("mining", 10), ("woodcutting", 10)], &held);
    assert_eq!(keep, names(&["hammer", "iron_pickaxe", "rune_pickaxe", "steel_axe", "tinderbox"]));
}
