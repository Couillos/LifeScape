use super::{kept_gear, plan_gear, spare_gear_units, unworn_upgrade};
use lifescape_gamedata::gear::{Style, SHIELD, WEAPON};

/// Expected price as the brain gives it for untraded objects: the high
/// alchemy value (the natural floor).
fn alch_price(item: &str) -> i64 {
    lifescape_gamedata::objs_by_name(item).into_iter().next().map(|obj| obj.alch_high).unwrap_or(1).max(1)
}

fn names(wants: &[super::GearWant]) -> Vec<&str> {
    wants.iter().map(|want| want.item.as_str()).collect()
}

const FIGHTER: [(&str, i64); 4] = [("attack", 50), ("defence", 50), ("strength", 50), ("hitpoints", 50)];

/// Live 2026-11-14: level 50 fighters in a bronze med helm and an iron
/// dagger bid on a BRONZE scimitar only. With the coins, a fighter wants
/// the best piece of every slot its levels open, all at once.
#[test]
fn a_rich_fighter_wants_the_best_piece_of_every_slot() {
    let owned = vec!["bronze_med_helm".to_string(), "iron_dagger".to_string()];
    let wants = plan_gear(Style::Melee, &FIGHTER, &[], &owned, 1_000_000, &alch_price);
    let items = names(&wants);
    eprintln!("{wants:?}");
    assert!(wants[0].item.starts_with("rune_"), "attack 50: a rune weapon (scimitar, longsword or battleaxe by score), {items:?}");
    assert!(items.iter().any(|item| item.starts_with("rune_") && (item.ends_with("platelegs") || item.ends_with("plateskirt"))), "{items:?}");
    assert!(items.contains(&"rune_full_helm"), "{items:?}");
    assert!(items.contains(&"rune_kiteshield"), "{items:?}");
    assert!(!items.contains(&"rune_platebody"), "the rune platebody needs Dragon Slayer: {items:?}");
    assert!(items.len() >= 6, "body, legs, helm, shield, weapon and more: {items:?}");
    assert_eq!(wants[0].wearpos, WEAPON, "the weapon leads");
}

/// The budget picks the tier: the dearest slot steps down until the
/// expected prices fit, every ceiling covers its expected price and the
/// ceilings share the budget.
#[test]
fn a_thin_wallet_steps_the_dearest_slots_down() {
    let owned = vec!["bronze_med_helm".to_string(), "iron_dagger".to_string()];
    let budget = 3000;
    let wants = plan_gear(Style::Melee, &FIGHTER, &[], &owned, budget, &alch_price);
    let items = names(&wants);
    eprintln!("{wants:?}");
    assert!(!items.iter().any(|item| item.starts_with("rune_")), "rune is out of reach at 3000: {items:?}");
    let total: i64 = wants.iter().map(|want| want.expected).sum();
    assert!(total <= budget, "expected prices fit the budget: {total}");
    assert!(wants.iter().all(|want| want.ceiling >= want.expected));
    let ceilings: i64 = wants.iter().map(|want| want.ceiling).sum();
    assert!(ceilings <= budget, "the ceilings share the budget: {ceilings}");
    assert!(wants.iter().any(|want| want.wearpos == WEAPON), "a weapon upgrade still fits: {items:?}");
    assert!(plan_gear(Style::Melee, &FIGHTER, &[], &owned, 0, &alch_price).is_empty(), "no coins, no bids");
}

/// Owning the best piece of a slot ends that slot's want; a better piece
/// the bot cannot wear yet is never wanted.
#[test]
fn owned_or_unwearable_pieces_are_not_wanted() {
    let low = [("attack", 1), ("defence", 1), ("strength", 1)];
    let wants = plan_gear(Style::Melee, &low, &[], &["bronze_scimitar".to_string()], 100_000, &alch_price);
    let items = names(&wants);
    assert!(!items.iter().any(|item| item.ends_with("scimitar")), "level 1 owns the only scimitar it may wield: {items:?}");
    assert!(!items.iter().any(|item| item.starts_with("rune_") || item.starts_with("mithril_")), "{items:?}");
}

/// Ranged wants a bow and ranged armour, never a shield.
#[test]
fn a_ranger_wants_a_bow_and_no_shield() {
    let levels = [("ranged", 40), ("defence", 40), ("attack", 1)];
    let wants = plan_gear(Style::Ranged, &levels, &[], &[], 1_000_000, &alch_price);
    let items = names(&wants);
    eprintln!("{wants:?}");
    assert!(wants.iter().any(|want| want.wearpos == WEAPON && want.item.contains("bow")), "{items:?}");
    assert!(!wants.iter().any(|want| want.wearpos == SHIELD), "{items:?}");
}

/// Replaced pieces become surplus; the best wearable piece of each slot
/// and better ones not wearable yet stay.
#[test]
fn replaced_pieces_are_surplus() {
    let levels = [("attack", 20), ("defence", 20), ("strength", 20)];
    let owned: Vec<String> = ["bronze_scimitar", "mithril_scimitar", "rune_scimitar", "bronze_platebody", "iron_platebody"].iter().map(|name| name.to_string()).collect();
    let keep = kept_gear(&levels, &[], &owned);
    assert!(keep.contains(&"mithril_scimitar".to_string()) && keep.contains(&"rune_scimitar".to_string()), "{keep:?}");
    assert!(keep.contains(&"iron_platebody".to_string()), "{keep:?}");
    assert!(!keep.contains(&"bronze_scimitar".to_string()) && !keep.contains(&"bronze_platebody".to_string()), "{keep:?}");
    let worn = vec!["bronze_scimitar".to_string()];
    assert_eq!(unworn_upgrade(Style::Melee, &levels, &[], &owned, &worn).as_deref(), Some("mithril_scimitar"));
}

/// Live 2026-11-14 (leo_king, pia_brown) : une hache de bûcheron portée
/// n'est pas l'arme du bot ; la masse possédée est une montée à porter.
/// ryan_park : en distance, l'arc banqué bat la dague portée.
#[test]
fn a_worn_woodcutting_axe_is_no_weapon() {
    let levels = [("attack", 54), ("defence", 43), ("strength", 54), ("ranged", 1)];
    let owned: Vec<String> = ["steel_axe", "iron_mace", "iron_dagger"].iter().map(|name| name.to_string()).collect();
    let worn = vec!["steel_axe".to_string()];
    assert_eq!(unworn_upgrade(Style::Melee, &levels, &[], &owned, &worn).as_deref(), Some("iron_mace"));
    let owned: Vec<String> = ["iron_dagger", "shortbow", "bronze_med_helm"].iter().map(|name| name.to_string()).collect();
    let worn = vec!["iron_dagger".to_string(), "bronze_med_helm".to_string()];
    assert_eq!(unworn_upgrade(Style::Ranged, &levels, &[], &owned, &worn).as_deref(), Some("shortbow"));
}

/// Live frank_lane 2026-11-14: a worn bronze med helm kept all 88 banked
/// copies off the market. One spare copy stays, the rest sells; a piece
/// the graph keeps whole stays whole.
#[test]
fn spare_copies_of_kept_gear_sell() {
    let stacks = vec![("bronze_med_helm".to_string(), 88), ("iron_dagger".to_string(), 41), ("black_battleaxe".to_string(), 1), ("staff".to_string(), 7)];
    let gear = vec!["bronze_med_helm".to_string(), "black_battleaxe".to_string(), "staff".to_string()];
    let keep = vec!["staff".to_string()];
    let spares = spare_gear_units(&stacks, &gear, &keep);
    assert_eq!(spares, vec![("bronze_med_helm".to_string(), 87)], "one helm stays, the lone axe stays, the kept staff stays whole");
}

/// User 2026-11-14 ("au moins 1 équipement de combat complet"): a thin
/// budget never drops an EMPTY slot while an upgrade of an owned slot
/// can give way. Equal prices: the old rule dropped the weapon (first
/// slot) and kept the helm upgrade.
#[test]
fn empty_slots_come_before_upgrades() {
    let owned = vec!["bronze_med_helm".to_string()];
    let flat = |_: &str| 10;
    let low = [("attack", 1), ("defence", 1), ("strength", 1)];
    let all = plan_gear(Style::Melee, &low, &[], &owned, 1_000_000, &flat);
    let helm = all.iter().find(|want| want.wearpos == lifescape_gamedata::gear::HEAD).expect("a helm upgrade at level 1");
    let budget = 10 * (all.len() as i64 - 1);
    let wants = plan_gear(Style::Melee, &low, &[], &owned, budget, &flat);
    assert_eq!(wants.len(), all.len() - 1, "{wants:?}");
    assert!(wants.iter().any(|want| want.wearpos == WEAPON), "the empty weapon slot stays: {wants:?}");
    assert!(!wants.iter().any(|want| want.item == helm.item), "the helm upgrade gives way: {wants:?}");
}

/// An Iron plans only what it can obtain without the market (shop-only
/// pieces, `plan_gear_from`'s filter).
#[test]
fn the_filter_bounds_the_candidates() {
    use super::plan_gear_from;
    let shop_only = |item: &str| lifescape_gamedata::obtain::merchant_route(item);
    let wants = plan_gear_from(Style::Melee, &FIGHTER, &[], &[], 1_000_000, &alch_price, &shop_only);
    assert!(!wants.is_empty(), "black pieces, a wooden shield...");
    assert!(wants.iter().all(|want| shop_only(&want.item)), "{wants:?}");
}
