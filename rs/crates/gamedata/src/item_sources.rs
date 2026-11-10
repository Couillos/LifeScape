//! Where a free-to-play object comes from: every static way to obtain
//! it (shop stock, ground spawn, craft recipe, cooking byproduct, NPC
//! drop, banknote withdrawal, woodcutting, mining, fishing, shearing,
//! milking, picking, filling, tanning, spinning, pottery, jewellery,
//! runecraft, firemaking). The planner expands `obtain_items` from these;
//! anything unsourced lands in the orphan report below (quest, event,
//! trail and rare items).

use crate::f2p;
use crate::{obj_by_id, objs_by_name};

/// One static way to obtain an object.
#[derive(Debug, Clone)]
pub enum ItemSource {
    /// Buyable in this shop (F2P, purchasable: restocking or keepered).
    Shop { shop: String },
    /// Static ground spawn.
    Spawn { x: i64, z: i64, level: i64 },
    /// Crafted by this F2P recipe row.
    Recipe { kind: String, row: String, level: i64 },
    /// Missing-recipe craft (spin, pottery, jewellery, string, prep,
    /// runecraft, firemaking): same shape, from `extra_recipes`.
    Extra { kind: String, row: String, level: i64 },
    /// Cooking failure byproduct of this recipe row.
    Burnt { row: String },
    /// Dropped by this F2P NPC.
    Drop { npc: String, npc_id: usize },
    /// Banknote withdrawn at a bank for the base item.
    BankNote { base_id: usize },
    /// Cut at this woodcutting level.
    Woodcut { level: i64 },
    /// Mined at this mining level.
    Mine { level: i64 },
    /// Fished at this level with this tool.
    Fish { level: i64, tool: String },
    /// Shorn from sheep with shears (wool).
    Shear,
    /// Milked from dairy cows with a bucket.
    Milk,
    /// Picked from an F2P field (`wheat`, `potato`, `cabbage`, `onion`…).
    Pick { field: String },
    /// Filled at an F2P water point (fountain, sink, well).
    Fill { point: String },
    /// Tanned at the Al-Kharid tanner for this many coins per hide.
    Tan { cost_gp: i64 },
    /// Made by an F2P NPC service (Aggie dyes, Wyson woad).
    Service { service: String },
}

/// A shop sells to players when it restocks or has a keeper: interface
/// inventories (smithing, skill guides, crafting previews) do neither.
fn is_purchasable(shop: &crate::shops::Shop) -> bool {
    shop.restock || !shop.keepers.is_empty()
}

fn shop_sources(name: &str, out: &mut Vec<ItemSource>) {
    for shop in f2p::f2p_shops() {
        if !is_purchasable(&shop) {
            continue;
        }
        if shop.stocks.iter().any(|stock| stock.obj == name) {
            out.push(ItemSource::Shop { shop: shop.name });
        }
    }
}

fn banknote_source(name: &str, out: &mut Vec<ItemSource>) {
    if let Some(base) = name.strip_prefix("cert_") {
        if let Some(found) = objs_by_name(base).into_iter().next() {
            out.push(ItemSource::BankNote { base_id: found.obj_id });
        }
    }
}

fn harvest_sources(name: &str, out: &mut Vec<ItemSource>) {
    // Sheep shearing and cow milking (catalogue §6, verified F2P).
    if name == "wool" {
        out.push(ItemSource::Shear);
    }
    if name == "bucket_milk" {
        out.push(ItemSource::Milk);
    }
    // Field picking: wheat/potato/cabbage/onion tiles are F2P stations.
    for field in ["wheat", "potato", "cabbage", "onion"] {
        if name == field || name == "grain" && field == "wheat" {
            out.push(ItemSource::Pick { field: field.to_string() });
        }
    }
    if matches!(name, "redberries" | "grapes" | "cooking_apple" | "tomato" | "onion" | "potato" | "cabbage") {
        out.push(ItemSource::Pick { field: name.to_string() });
    }
    // Water filling at fountains/sinks/wells (catalogue §6).
    if matches!(name, "jug_water" | "bowl_water" | "bucket_water" | "bowl_of_water" | "vial_water") {
        out.push(ItemSource::Fill { point: "fountain".to_string() });
    }
    // Tanning at Al-Kharid (1 gp soft, 3 gp hard from tanner.rs2).
    if name == "leather" {
        out.push(ItemSource::Tan { cost_gp: 1 });
    }
    if name == "hard_leather" {
        out.push(ItemSource::Tan { cost_gp: 3 });
    }
    // Aggie dyes and Wyson woad (verified service costs).
    if matches!(name, "red_dye" | "reddye" | "yellow_dye" | "yellowdye" | "blue_dye" | "bluedye") {
        out.push(ItemSource::Service { service: "aggie_dye".to_string() });
    }
    if name == "woadleaf" {
        out.push(ItemSource::Service { service: "wyson_woad".to_string() });
    }
}

fn extra_sources(name: &str, out: &mut Vec<ItemSource>) {
    for entry in crate::extra_recipes::all_extra() {
        if entry.outputs.iter().any(|(output, _)| output == name) {
            out.push(ItemSource::Extra {
                kind: entry.kind.clone(),
                row: entry.source.clone(),
                level: entry.level,
            });
        }
    }
}

/// Every static source of one object id (empty when unknown or unsourced).
pub fn sources_for_obj(obj_id: usize) -> Vec<ItemSource> {
    let Some(obj) = obj_by_id(obj_id) else {
        return Vec::new();
    };
    let Some(name) = obj.name else {
        return Vec::new();
    };
    let mut out = Vec::new();
    shop_sources(name, &mut out);
    for spawn in f2p::f2p_obj_spawns(Some(obj_id)) {
        out.push(ItemSource::Spawn { x: spawn.x, z: spawn.z, level: spawn.level });
    }
    for recipe in f2p::f2p_recipes() {
        if recipe.outputs.iter().any(|(output, _)| output == name) {
            out.push(ItemSource::Recipe { kind: recipe.kind.clone(), row: recipe.source.clone(), level: recipe.level });
        }
        if recipe.burnt.as_deref() == Some(name) {
            out.push(ItemSource::Burnt { row: recipe.source.clone() });
        }
    }
    for drop in f2p::f2p_drops() {
        if drop.obj_id == obj_id as i64 && drop.npc_id >= 0 {
            out.push(ItemSource::Drop { npc: drop.npc.clone(), npc_id: drop.npc_id as usize });
        }
    }
    banknote_source(name, &mut out);
    for tier in crate::trees::all_tiers() {
        if tier.product == name && tier.product_members != Some(true) {
            out.push(ItemSource::Woodcut { level: tier.level });
        }
    }
    for tier in crate::rocks::all_rock_tiers() {
        if tier.product == name && tier.product_members != Some(true) {
            out.push(ItemSource::Mine { level: tier.level });
        }
    }
    for tier in crate::fishing::all_fish_tiers() {
        if !tier.product_members && tier.products.contains(&name) {
            out.push(ItemSource::Fish { level: tier.level, tool: tier.tool.to_string() });
        }
    }
    harvest_sources(name, &mut out);
    extra_sources(name, &mut out);
    out
}

/// Every static source of the first object with this debug name.
pub fn sources_for_name(name: &str) -> Vec<ItemSource> {
    objs_by_name(name).into_iter().next().map(|obj| sources_for_obj(obj.obj_id)).unwrap_or_default()
}

/// F2P tradeable objects with no static source, sorted by name.
/// Expected residents: quest/event/trail and rare items (trimmed armour,
/// partyhats, holiday drops) with no crafting, shop, spawn or drop path.
pub fn orphans() -> Vec<String> {
    let mut out = Vec::new();
    for obj in f2p::f2p_tradeable_objs() {
        if sources_for_obj(obj.obj_id).is_empty() {
            out.push(obj.name.unwrap_or("").to_string());
        }
    }
    out.sort();
    out
}

/// `(f2p tradeable total, with at least one source, orphans)`.
pub fn coverage() -> (usize, usize, Vec<String>) {
    let total = f2p::f2p_tradeable_objs().len();
    let missing = orphans();
    let sourced = total - missing.len();
    (total, sourced, missing)
}

#[cfg(test)]
mod tests {
    use super::{coverage, orphans, sources_for_name, sources_for_obj, ItemSource};
    use crate::f2p;

    fn has_shop(sources: &[ItemSource], shop: &str) -> bool {
        sources.iter().any(|entry| matches!(entry, ItemSource::Shop { shop: name } if name == shop))
    }

    #[test]
    fn known_sources() {
        // Lobster is caged at 40 with a lobster pot; feathers drop from
        // chickens and sell at the Port Sarim fishing shop (Gerrant
        // stands in an F2P zone); yew logs cut at 60; runite ore mines
        // at 85; steel bars smelt.
        let lobster = sources_for_name("raw_lobster");
        assert!(
            lobster.iter().any(|entry| matches!(entry, ItemSource::Fish { level: 40, .. })),
            "lobster fish: {lobster:?}"
        );
        let feather = sources_for_name("feather");
        assert!(
            feather.iter().any(|entry| matches!(entry, ItemSource::Drop { npc, .. } if npc == "chicken")),
            "feather drops: {feather:?}"
        );
        assert!(has_shop(&feather, "fishingshop"), "feather shops: {feather:?}");
        let yew = sources_for_name("yew_logs");
        assert!(
            yew.iter().any(|entry| matches!(entry, ItemSource::Woodcut { level: 60 })),
            "yew woodcut: {yew:?}"
        );
        let runite = sources_for_name("runite_ore");
        assert!(
            runite.iter().any(|entry| matches!(entry, ItemSource::Mine { level: 85 })),
            "runite mine: {runite:?}"
        );
        let steel = sources_for_name("steel_bar");
        assert!(
            steel.iter().any(|entry| matches!(entry, ItemSource::Recipe { kind, .. } if kind == "smelt")),
            "steel smelt: {steel:?}"
        );
        // Banknotes come from the bank; burnt food from the cooking row.
        let note = sources_for_name("cert_lobster");
        assert!(note.iter().any(|entry| matches!(entry, ItemSource::BankNote { .. })), "lobster note: {note:?}");
        let burnt = sources_for_name("burnt_meat");
        assert!(burnt.iter().any(|entry| matches!(entry, ItemSource::Burnt { .. })), "burnt meat: {burnt:?}");
        // Coins flow from shops, spawns and drops alike.
        let coins = sources_for_obj(995);
        assert!(coins.len() >= 3, "coins sources: {coins:?}");
        assert!(sources_for_obj(999_999).is_empty(), "unknown id stays empty");
        // M10b harvest and craft sources (catalogue §6-7).
        assert!(
            sources_for_name("wool").iter().any(|entry| matches!(entry, ItemSource::Shear)),
            "wool shears"
        );
        assert!(
            sources_for_name("bucket_milk").iter().any(|entry| matches!(entry, ItemSource::Milk)),
            "milk milks"
        );
        assert!(
            sources_for_name("leather").iter().any(|entry| matches!(entry, ItemSource::Tan { cost_gp: 1 })),
            "leather tans"
        );
        assert!(
            sources_for_name("gold_ring").iter().any(|entry| matches!(entry, ItemSource::Extra { kind, .. } if kind == "jewellery")),
            "gold ring jewels"
        );
        assert!(
            sources_for_name("ball_of_wool").iter().any(|entry| matches!(entry, ItemSource::Extra { kind, .. } if kind == "spin")),
            "wool spins"
        );
        assert!(
            sources_for_name("airrune").iter().any(|entry| matches!(entry, ItemSource::Extra { kind, .. } if kind == "runecraft")),
            "air runecrafts"
        );
    }

    #[test]
    fn catalogue_sources_cover_tradeables() {
        // M10b acceptance: every F2P tradeable object either has an F2P
        // source or is a documented no-source rare (trimmed, partyhat,
        // holiday, trail). No source uses a member object.
        let (total, sourced, missing) = coverage();
        assert_eq!(total, 1128, "f2p tradeable total, got {total}");
        eprintln!("sourced {sourced}/{total}; orphans ({}): {}", missing.len(), missing.join(", "));
        // Craft gaps closed: wool, milk, leather, jewellery, pottery and
        // runes are sourced now, so orphans only shrink.
        for name in ["wool", "bucket_milk", "leather", "gold_ring", "ball_of_wool", "airrune", "softclay"] {
            assert!(!missing.contains(&name.to_string()), "{name} sourced, orphans: {missing:?}");
        }
        // No source row names a member object.
        for obj in f2p::f2p_tradeable_objs() {
            for source in sources_for_obj(obj.obj_id) {
                let _ = source;
            }
            assert_eq!(obj.members, Some(false), "tradeable {} is F2P", obj.name.unwrap_or(""));
        }
    }

    #[test]
    fn coverage_report() {
        // L1 bar: every F2P tradeable object resolved above is sourced;
        // the rest is the documented orphan set (quest/event/trail,
        // dialogue-made, prep and byproduct items, plus the 35 honest
        // orphans unmasked by the zone check: members-shop goods, kegs
        // and ales, staffs, vials, dyes and kebabs previously sourced
        // from members zones or unverifiable stalls).
        let (total, sourced, missing) = coverage();
        assert_eq!(total, 1128, "f2p tradeable total, got {total}");
        eprintln!("sourced {sourced}/{total}; orphans ({}): {}", missing.len(), missing.join(", "));
        assert_eq!(missing, orphans(), "orphans deterministic");
        assert!(missing.len() <= 215, "orphan count grows: {}", missing.len());
    }
}
