//! Where a free-to-play object comes from: every static way to obtain
//! it (shop stock, ground spawn, craft recipe, cooking byproduct, NPC
//! drop, banknote withdrawal, woodcutting, mining, fishing). The planner
//! expands `obtain_items` from these; anything unsourced lands in the
//! orphan report below (quest, event, trail and dialogue-only items).

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
    out
}

/// Every static source of the first object with this debug name.
pub fn sources_for_name(name: &str) -> Vec<ItemSource> {
    objs_by_name(name).into_iter().next().map(|obj| sources_for_obj(obj.obj_id)).unwrap_or_default()
}

/// F2P tradeable objects with no static source, sorted by name.
/// Expected residents: quest/event/trail items, dialogue-made goods
/// (dyes, tanned leather), cooking prep intermediates and eat-byproducts.
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

    fn has_shop(sources: &[ItemSource], shop: &str) -> bool {
        sources.iter().any(|entry| matches!(entry, ItemSource::Shop { shop: name } if name == shop))
    }

    #[test]
    fn known_sources() {
        // Lobster buys at the fish restaurant; feathers drop from chickens;
        // yew logs cut at 60; runite ore mines at 85; steel bars smelt.
        let lobster = sources_for_name("lobster");
        assert!(has_shop(&lobster, "fishrestaurant"), "lobster shops: {lobster:?}");
        let feather = sources_for_name("feather");
        assert!(
            feather.iter().any(|entry| matches!(entry, ItemSource::Drop { npc, .. } if npc == "chicken")),
            "feather drops: {feather:?}"
        );
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
    }

    #[test]
    fn coverage_report() {
        // L1 bar: every F2P tradeable object resolved above is sourced;
        // the rest is the documented orphan set (quest/event/trail,
        // dialogue-made, prep and byproduct items).
        let (total, sourced, missing) = coverage();
        assert_eq!(total, 1128, "f2p tradeable total, got {total}");
        eprintln!("sourced {sourced}/{total}; orphans ({}): {}", missing.len(), missing.join(", "));
        assert_eq!(missing, orphans(), "orphans deterministic");
        assert!(missing.len() <= 180, "orphan count grows: {}", missing.len());
    }
}
