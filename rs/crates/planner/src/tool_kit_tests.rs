//! Tests of `tool_kit` (split out, rule 4) on the rev 274 data: shop
//! prices hammer 1, tinderbox 1, bronze pickaxe 1, iron pickaxe 140,
//! steel pickaxe 500, mithril pickaxe 1300, bronze axe 16, iron axe 56,
//! steel axe 200 (Bob); mithril axes and up are smithed only.

use super::{kit_keep, kit_purchase, kit_purchases, KitPurchase};
use crate::tool_upgrade::ToolSource;

fn shop(item: &str, price: i64) -> KitPurchase {
    KitPurchase { item: item.to_string(), price, base: false, source: ToolSource::Shop, financed: false, saved_ticks: 0 }
}

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
    assert_eq!(upgrades, vec![shop("steel_pickaxe", 500), shop("iron_axe", 56)]);
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
        Some(KitPurchase { base: true, ..shop("bronze_pickaxe", 1) })
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

// ---- Best tool on offer, funded when it pays (user 2026-11-17) ----

use super::{kit_purchases_in, kit_tool, step_coin_share, waiting_kit_step, KitMarket};
use crate::tool_upgrade::{planned_work, ticks_saved, ToolOffer};

/// The book and shops of the live case (2026-11-17): no rune or adamant
/// axe on sale (bids only), mithril asks from 312, steel asks from 120
/// (Bob: 200), iron and bronze at Bob's; pickaxes at Nurmof's.
fn jim_book(item: &str) -> Option<ToolOffer> {
    let ask = match item {
        "mithril_axe" => Some(312),
        "steel_axe" => Some(120),
        _ => None,
    };
    crate::tool_upgrade::cheapest(super::kit_price(item), ask, None)
}

fn jim_kit() -> Vec<String> {
    names(&["hammer", "tinderbox", "iron_pickaxe", "bronze_axe", "iron_axe"])
}

const JIM_LEVELS: [(&str, i64); 4] = [("woodcutting", 86), ("firemaking", 60), ("mining", 1), ("smithing", 1)];

fn jim_purchases(work_life: &crate::model::Life, coins: i64) -> Vec<KitPurchase> {
    let axe_work = planned_work(work_life, "bronze_axe", &crate::tool_upgrade::tests::jim_levels);
    let saved = |from: &str, to: &str| ticks_saved(&axe_work, from, to);
    kit_purchases_in(&JIM_LEVELS, &jim_kit(), coins, &KitMarket { offer: &jim_book, saved: &saved, coin_rate: 0.1 })
}

/// jim_ellis: rune axe nobody sells, mithril on the book, steel at Bob's.
/// The target is mithril (best ON OFFER), not rune; 24 coins: financed
/// (a money step earns the 312, then the step buys). With the coins in
/// hand it buys at once.
#[test]
fn jim_targets_the_best_axe_on_offer_and_funds_it() {
    let life = crate::tool_upgrade::tests::jim_life();
    let poor = jim_purchases(&life, 24);
    assert_eq!(poor.len(), 1, "{poor:?}");
    let axe = &poor[0];
    assert_eq!((axe.item.as_str(), axe.price, axe.source.clone(), axe.base, axe.financed), ("mithril_axe", 312, ToolSource::Ask, false, true));
    assert!(axe.saved_ticks > 3120, "4 291 yews save more than 312 coins at 0.1 coin a tick: {}", axe.saved_ticks);
    let rich = jim_purchases(&life, 400);
    assert_eq!((rich[0].item.as_str(), rich[0].financed), ("mithril_axe", false), "the free wallet holds the price: bought at once");
    // An adamant ask appears: the next tier up becomes the target.
    let with_adamant = |item: &str| if item == "adamant_axe" { crate::tool_upgrade::cheapest(None, Some(900), None) } else { jim_book(item) };
    let axe_work = planned_work(&life, "bronze_axe", &crate::tool_upgrade::tests::jim_levels);
    let saved = |from: &str, to: &str| ticks_saved(&axe_work, from, to);
    let climbed = kit_purchases_in(&JIM_LEVELS, &jim_kit(), 24, &KitMarket { offer: &with_adamant, saved: &saved, coin_rate: 0.1 });
    assert_eq!(climbed[0].item, "adamant_axe");
}

/// Gain under the cost: no funding. 50 yews left save less than the
/// price of any axe above a quarter of 24 coins: nothing is bought.
#[test]
fn an_upgrade_that_does_not_pay_is_never_funded() {
    let mut life = crate::tool_upgrade::tests::jim_life();
    life.nodes[0].progress_current = 20375;
    assert!(jim_purchases(&life, 24).is_empty(), "{:?}", jim_purchases(&life, 24));
    // No planned axe work at all: the shop rule alone (a quarter of the wallet).
    let idle = crate::tool_upgrade::tests::life_with(Vec::new());
    assert!(jim_purchases(&idle, 24).is_empty());
    assert_eq!(jim_purchases(&idle, 2000)[0].item, "mithril_axe", "312 within a quarter of 2000: bought as before");
}

/// Pickaxes alike (Nurmof: iron 140 ... adamant 3200, rune 32 000):
/// 2 000 coal at mining 41 with a bronze pickaxe fund the adamant one
/// (its ticks saved beat 32 000 ticks), never the rune one (320 000).
#[test]
fn the_pickaxe_climbs_to_the_tier_that_pays() {
    let levels = [("mining", 41), ("woodcutting", 1)];
    let kit = names(&["hammer", "tinderbox", "bronze_pickaxe", "bronze_axe"]);
    let work = vec![crate::tool_upgrade::PlannedWork { method: "gather:mining/coal".to_string(), level: 41, units: 2000.0 }];
    let saved = |from: &str, to: &str| ticks_saved(&work, from, to);
    let shops = |item: &str| super::kit_price(item).map(|price| ToolOffer { price, source: ToolSource::Shop });
    let purchases = kit_purchases_in(&levels, &kit, 24, &KitMarket { offer: &shops, saved: &saved, coin_rate: 0.1 });
    assert_eq!(purchases.len(), 1, "{purchases:?}");
    assert_eq!((purchases[0].item.as_str(), purchases[0].financed, purchases[0].source.clone()), ("adamant_pickaxe", true, ToolSource::Shop));
    let short = vec![crate::tool_upgrade::PlannedWork { units: 20.0, ..work[0].clone() }];
    let saved_short = |from: &str, to: &str| ticks_saved(&short, from, to);
    assert!(kit_purchases_in(&levels, &kit, 24, &KitMarket { offer: &shops, saved: &saved_short, coin_rate: 0.1 }).is_empty());
}

/// A kit tool's step spends the free wallet (its funded price is the
/// coin target); other gear keeps the gear plan's half.
#[test]
fn kit_tool_steps_spend_the_free_wallet() {
    assert!(kit_tool("mithril_axe") && kit_tool("rune_pickaxe") && kit_tool("hammer") && kit_tool("tinderbox"));
    assert!(!kit_tool("bronze_battleaxe") && !kit_tool("bronze_platebody"));
    assert_eq!(step_coin_share("mithril_axe"), None);
    assert_eq!(step_coin_share("iron_scimitar"), Some(crate::gear_ladder::GEAR_SHARE_PCT));
}

/// The funded step end to end in the planner: the mithril axe obtain
/// (24 coins, ask 312, the whole free wallet) blocks on 312 coins; the
/// block files a `have_coins` 312 the obtain waits on, inside the gear
/// step (launched before the goal's yews); while it waits the step is
/// the waiting kit step the brain re-checks.
#[test]
fn a_financed_axe_earns_its_price_then_buys() {
    use crate::expand::{resolve_ready, ReadyCtx};
    use crate::model::{NodeKind, NodeStatus};
    let mut life = crate::tool_upgrade::tests::jim_life();
    life.nodes[0].status = NodeStatus::Ready;
    let ids = crate::lifecycle::insert_gear_step(&mut life, Some("mithril_axe"), "melee");
    let obtain = life.nodes.iter().position(|node| node.id == ids[0]).expect("obtain");
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.coin_share_pct = step_coin_share("mithril_axe");
    ctx.coins = 24;
    ctx.price = 312;
    ctx.best_ask = Some(312);
    resolve_ready(&mut life.nodes[obtain], &ctx);
    assert_eq!(life.nodes[obtain].status, NodeStatus::Blocked { reason: "missing:coins:312".to_string() }, "the price, never twice it");
    crate::lifecycle::record_block_from(&mut life, obtain, &["coins:312".to_string()], &|_| 1);
    crate::lifecycle::normalize(&mut life);
    let earn = life.nodes.iter().find(|node| matches!(node.kind, NodeKind::HaveCoins { amount: 312 })).expect("have_coins 312").id.clone();
    assert!(life.nodes[obtain].deps.contains(&earn), "the obtain waits on its coins");
    assert!(crate::gear_ladder::gear_subtree(&life).contains(&earn), "the money step belongs to the gear step");
    let next = crate::gear_ladder::next_ready(&life).map(|at| life.nodes[at].id.clone());
    assert_eq!(next, Some(earn), "the money step runs before the goal's yews");
    assert_eq!(waiting_kit_step(&life), Some((obtain, "mithril_axe".to_string())));
    // Coins earned: the obtain resolves to the ask.
    ctx.coins = 330;
    let mut ready = life.nodes[obtain].clone();
    ready.status = NodeStatus::Ready;
    resolve_ready(&mut ready, &ctx);
    assert_eq!(ready.method.as_deref(), Some("market:buy"));
}
