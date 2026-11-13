//! Site gates in the site pick (audit 2026-11-13 item A5): guild doors,
//! keys, the Karamja boat, dungeons and the Wilderness.

use super::{min_combat_for_method, pick_site_gated, Fare, Pass, SitePick};

const FALADOR: (i64, i64, i64) = (2964, 3378, 0);

fn broke() -> Fare {
    Fare { wallet: 0, free_pass: false }
}

fn crafter(level: u8, held: &[&str]) -> Pass {
    Pass { levels: vec![("crafting".to_string(), level)], held: held.iter().map(|item| item.to_string()).collect() }
}

/// Gold from Falador: the Crafting Guild rocks only with crafting 40
/// and a brown apron; otherwise the open rocks (Rimmington), never the
/// guild interior.
#[test]
fn guild_rocks_need_the_level_and_the_uniform() {
    let inside = |pick: SitePick| match pick {
        SitePick::At((x, z, level)) => !lifescape_gamedata::access::gates_at(x, z, level).is_empty(),
        other => panic!("gold is workable, got {other:?}"),
    };
    assert!(!inside(pick_site_gated("gather:mining/gold_ore", 60, Some(FALADOR), broke(), &Pass::default())), "no gate passed");
    assert!(!inside(pick_site_gated("gather:mining/gold_ore", 60, Some(FALADOR), broke(), &crafter(40, &[]))), "no apron");
    assert!(inside(pick_site_gated("gather:mining/gold_ore", 60, Some(FALADOR), broke(), &crafter(40, &["brown_apron"]))), "guild open");
}

/// Karamja water has no nav edge (`access::BOAT_ROUTED`): lobster, tuna
/// and swordfish stay unworkable at every level; a gated-only method
/// names its gate as the need.
#[test]
fn the_boat_and_dungeons_never_open() {
    for fish in ["raw_lobster", "raw_tuna", "raw_swordfish"] {
        let method = format!("gather:fishing/{fish}");
        assert_eq!(pick_site_gated(&method, 126, Some(FALADOR), Fare { wallet: 1_000, free_pass: true }, &Pass::default()), SitePick::Needs(lifescape_gamedata::danger::NEVER), "{fish}");
        assert_eq!(min_combat_for_method(&method), lifescape_gamedata::danger::NEVER);
    }
}

/// Stations are sites too (args contract 2026-11-13): a smelter, smith,
/// cook, spinner, potter, jeweller and runecrafter are sent to a picked
/// station; the Cooks' Guild range only with cooking 32 and a chef's hat.
#[test]
fn stations_are_picked_sites() {
    for method in ["smelt:bronze_bar", "smith:bronze_sword", "cook/raw_shrimp", "spin:ball_of_wool", "pottery:pot_empty", "jewelry:gold_ring", "runecraft:airrune"] {
        match pick_site_gated(method, 3, Some(FALADOR), broke(), &Pass::default()) {
            SitePick::At(site) => assert!(lifescape_gamedata::access::gates_at(site.0, site.1, site.2).is_empty(), "{method} at {site:?}"),
            other => panic!("{method}: a station is picked, got {other:?}"),
        }
    }
    let cooks_guild = (3144, 3450, 1);
    let cook = Pass { levels: vec![("cooking".to_string(), 32)], held: vec!["chefs_hat".to_string()] };
    match pick_site_gated("cook/raw_trout", 3, Some(cooks_guild), broke(), &cook) {
        SitePick::At(site) => assert!(!lifescape_gamedata::access::gates_at(site.0, site.1, site.2).is_empty(), "the guild range: {site:?}"),
        other => panic!("ranges exist, got {other:?}"),
    }
}
