//! Toll-aware site picks (user decision 2026-11-13): a toll-free site
//! first; a site across the Al Kharid fence only with the fare budgeted
//! (or Prince Ali Rescue done), else the fare is a coin need.

use super::{pick_site_paid, Fare, SitePick};

const LUMBRIDGE: (i64, i64, i64) = (3232, 3203, 0);
const AL_KHARID: (i64, i64, i64) = (3290, 3180, 0);

fn wallet(coins: i64) -> Fare {
    Fare { wallet: coins, free_pass: false }
}

/// sara_miller (live 2026-11-13): "produce gold_ore 100" from Lumbridge
/// was sent to the Al Kharid mine (3294,3287), across the fence, with 27
/// coins. Rimmington's gold rocks sit on the same side: they win, even
/// with the fare in the wallet.
#[test]
fn gold_ore_from_lumbridge_mines_rimmington_not_al_kharid() {
    for coins in [0, 27, 5000] {
        match pick_site_paid("farm:gold_ore", 40, Some(LUMBRIDGE), wallet(coins)) {
            SitePick::At((x, z, _)) => {
                assert!(x < 3268, "toll-free gold first, got ({x},{z}) with {coins} coins");
                assert!(!lifescape_gamedata::toll::crossing(LUMBRIDGE, (x, z, 0)));
            }
            other => panic!("Rimmington gold is open at CB40, got {other:?}"),
        }
    }
    assert!(matches!(pick_site_paid("gather:mining/gold_ore", 40, Some(LUMBRIDGE), wallet(27)), SitePick::At((x, _, _)) if x < 3268));
}

/// bh99tw2f (live): stuck on the Al Kharid side, "cows across the toll
/// fence" on a loop. The fence ends at z 3329 (`gamedata::toll`, region
/// from the map): the cows north of it are open ground, walked around
/// the fence's top for free, so even a broke Al Kharid bot hunts there
/// and never blocks on the fare.
#[test]
fn cows_from_al_kharid_walk_around_the_fence_top() {
    for coins in [0, 9, 1070] {
        match pick_site_paid("fight:melee/cows", 30, Some(AL_KHARID), wallet(coins)) {
            SitePick::At(site) => {
                assert!(!lifescape_gamedata::toll::crossing(AL_KHARID, site), "{coins} coins: {site:?} is toll-free");
                assert!(site.1 > 3329, "{coins} coins: the cows past the fence top, got {site:?}");
            }
            other => panic!("{coins} coins: a toll-free cow field exists, got {other:?}"),
        }
    }
    // Same side, no fare: a Lumbridge bot hunts the Lumbridge cows.
    assert!(matches!(pick_site_paid("fight:melee/cows", 30, Some(LUMBRIDGE), wallet(0)), SitePick::At(_)));
}

/// Al Kharid warriors stand only behind the toll: from Lumbridge, with
/// coins the fare is budgeted (the site is taken, tolled); broke, the
/// fare is the need; Prince Ali Rescue done, the guard waves the bot
/// through for free.
#[test]
fn warriors_from_lumbridge_need_the_fare() {
    match pick_site_paid("fight:melee/al_kharid_warriors", 40, Some(LUMBRIDGE), wallet(1070)) {
        SitePick::Tolled(site) => assert!(lifescape_gamedata::toll::behind(site.0, site.1, site.2), "the warriors sit behind the toll: {site:?}"),
        other => panic!("1070 coins pay the fare, got {other:?}"),
    }
    assert_eq!(pick_site_paid("fight:melee/al_kharid_warriors", 40, Some(LUMBRIDGE), wallet(9)), SitePick::Fare(lifescape_gamedata::toll::FARE));
    let free = Fare { wallet: 0, free_pass: true };
    assert!(matches!(pick_site_paid("fight:melee/al_kharid_warriors", 40, Some(LUMBRIDGE), free), SitePick::Tolled(_)));
}

/// Drop farms pick their spot's sites too: feathers hunt the chickens,
/// on the bot's side of the fence.
#[test]
fn drop_farms_pick_a_site_like_any_field_method() {
    assert!(matches!(pick_site_paid("farm:feather", 3, Some(LUMBRIDGE), wallet(0)), SitePick::At(_)));
    // Never a fishing table for meat: raw beef drops from cows.
    assert!(matches!(pick_site_paid("farm:raw_beef", 3, Some(LUMBRIDGE), wallet(0)), SitePick::At(_)));
}

/// End to end at resolve: the node records the site and the toll flag;
/// broke, it blocks on the fare (a `have_coins` dependency earns it).
#[test]
fn resolve_budgets_the_fare_or_blocks_on_it() {
    use crate::expand::{resolve_ready, ReadyCtx};
    use crate::model::{Node, NodeKind, NodeStatus};
    let climb = || {
        let mut node = Node::new("n4", NodeKind::ReachLevel { skill: "attack".to_string(), level: 20 }, Vec::new(), 20);
        node.alternatives.push(crate::model::Alternative { method: "fight:melee/al_kharid_warriors".to_string(), est_ticks: 0, est_coins: 0 });
        node
    };
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.combat = 40;
    ctx.position = Some(LUMBRIDGE);
    ctx.coins = 1070;
    let mut node = climb();
    resolve_ready(&mut node, &ctx);
    assert!(node.toll, "fare budgeted: {:?}", node.status);
    assert!(node.site.is_some_and(|(x, z, level)| lifescape_gamedata::toll::behind(x, z, level)));
    ctx.coins = 0;
    let mut node = climb();
    resolve_ready(&mut node, &ctx);
    assert_eq!(node.status, NodeStatus::Blocked { reason: format!("missing:coins:{}", lifescape_gamedata::toll::FARE) });
    assert!(!node.toll);
    // sara_miller's gold quota from Lumbridge: Rimmington, no toll.
    let mut gold = Node::new("n74", NodeKind::ObtainItems { item: "gold_ore".to_string(), count: 100 }, Vec::new(), 100);
    gold.alternatives = crate::obtain_legs::produce_alternatives("gold_ore");
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.combat = 40;
    ctx.position = Some(LUMBRIDGE);
    ctx.coins = 27;
    ctx.tools = vec!["bronze_pickaxe".to_string()];
    ctx.levels = vec![("mining".to_string(), 45)];
    resolve_ready(&mut gold, &ctx);
    assert_eq!(gold.method.as_deref(), Some("farm:gold_ore"), "{:?}", gold.status);
    assert!(!gold.toll && gold.site.is_some_and(|(x, _, _)| x < 3268), "site {:?}", gold.site);
}
