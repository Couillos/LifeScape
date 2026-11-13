//! Gather-method gear gate: which carried tool (and bait) a method
//! needs, which names satisfy it, and whether the pack holds or can fund
//! it now. Split out of `expand` (rule 4); API re-exported there.

use crate::expand::ReadyCtx;

/// One carried tool a gather method needs: the cheapest canonical
/// buyable of its family (as `kit::tools::blocked_tool` names it),
/// plus the consumable bait when the tier burns one (the script funds
/// bait from the catch, so shop coverage gates it like the tool).
pub struct ToolNeed {
    pub tool: &'static str,
    pub bait: Option<&'static str>,
}

/// Required carried gear per gather method. Mirrors the `kit::tools`
/// ranks (best-first orders) and the fishing tiers (tool + bait per
/// product); the minimum duplicated planner-side, names only.
pub fn required_tool(method: &str) -> Option<ToolNeed> {
    // An obtain's `farm:` leg works the same gatherer: same gear (a trout
    // quota needs the fly rod AND feathers, b8jxrdd2 live 2026-11-13).
    if let Some(item) = method.strip_prefix("farm:") {
        if crate::mapping::is_fish(item) {
            return required_tool(&format!("gather:fishing/{item}"));
        }
        if item.ends_with("_ore") || item == "coal" || item == "clay" || item == "blankrune" {
            return required_tool(&format!("gather:mining/{item}"));
        }
        if item.ends_with("logs") {
            return required_tool(&format!("gather:woodcut/{item}"));
        }
        return None;
    }
    if let Some(product) = method.strip_prefix("gather:fishing/") {
        let need = match product {
            "raw_shrimp" | "raw_anchovies" => ToolNeed { tool: "net", bait: None },
            "raw_sardine" | "raw_herring" | "raw_pike" => {
                ToolNeed { tool: "fishing_rod", bait: Some("fishing_bait") }
            }
            "raw_trout" | "raw_salmon" => ToolNeed { tool: "fly_fishing_rod", bait: Some("feather") },
            "raw_lobster" => ToolNeed { tool: "lobster_pot", bait: None },
            "raw_tuna" | "raw_swordfish" => ToolNeed { tool: "harpoon", bait: None },
            _ => return None,
        };
        Some(need)
    } else if method.starts_with("gather:mining/") {
        Some(ToolNeed { tool: "bronze_pickaxe", bait: None })
    } else if method.starts_with("gather:woodcut/") {
        Some(ToolNeed { tool: "bronze_axe", bait: None })
    } else if method.starts_with("gather:sheep/") {
        // shear_sheep.rs2: shears used on the sheep (PRD3 lot 7).
        Some(ToolNeed { tool: "shears", bait: None })
    } else if method.starts_with("milk:") {
        Some(ToolNeed { tool: "bucket_empty", bait: None })
    } else if method.starts_with("fight:ranged/") {
        // PRD3 lot 5: ranged trains with a bow and arrows, never by
        // punching (the old script melee-attacked).
        Some(ToolNeed { tool: "shortbow", bait: Some("bronze_arrow") })
    } else if method.starts_with("fight:magic/") {
        // Wind strike: one mind rune and one air rune per cast.
        Some(ToolNeed { tool: "mindrune", bait: Some("airrune") })
    } else if method.starts_with("smith:") {
        // Every anvil recipe carries a hammer (obtain way tools).
        Some(ToolNeed { tool: "hammer", bait: None })
    } else if method.starts_with("fire:") {
        // Lighting logs takes a tinderbox in the pack.
        Some(ToolNeed { tool: "tinderbox", bait: None })
    } else if let Some(product) = method.strip_prefix("jewelry:") {
        // The furnace casts gold in the product's mould (kept, never used up).
        jewelry_mould(product).map(|mould| ToolNeed { tool: mould, bait: None })
    } else if let Some(rune) = method.strip_prefix("runecraft:") {
        // The altar ruins open to the rune's talisman.
        rune_talisman(rune).map(|talisman| ToolNeed { tool: talisman, bait: None })
    } else if let Some(product) = method.strip_prefix("craft:") {
        // Leather is sewn with a needle and thread (thread wears out).
        crate::mapping::is_leather_recipe(product).then_some(ToolNeed { tool: "needle", bait: Some("thread") })
    } else {
        None
    }
}

/// Mould a jewellery product is cast in (gold and gem rings, necklaces,
/// amulets; the unstrung holy symbol `nostringstar`).
pub fn jewelry_mould(product: &str) -> Option<&'static str> {
    if product == "nostringstar" {
        Some("holy_symbol_mould")
    } else if product.contains("necklace") {
        Some("necklace_mould")
    } else if product.contains("amulet") {
        Some("amulet_mould")
    } else if product.contains("ring") {
        Some("ring_mould")
    } else {
        None
    }
}

/// Talisman whose altar crafts `rune` (F2P altars).
pub fn rune_talisman(rune: &str) -> Option<&'static str> {
    Some(match rune {
        "airrune" => "air_talisman",
        "mindrune" => "mind_talisman",
        "waterrune" => "water_talisman",
        "earthrune" => "earth_talisman",
        "firerune" => "fire_talisman",
        "bodyrune" => "body_talisman",
        _ => return None,
    })
}

/// Every name that satisfies `tool`: the whole `kit::tools` family
/// (any carried pickaxe mines, any carried axe chops), the single
/// name otherwise (a harpoon never catches sardines).
pub fn tool_family(tool: &str) -> &'static [&'static str] {
    match tool {
        "bronze_pickaxe" => &[
            "rune_pickaxe",
            "adamant_pickaxe",
            "mithril_pickaxe",
            "steel_pickaxe",
            "iron_pickaxe",
            "bronze_pickaxe",
        ],
        "bronze_axe" => &[
            "rune_axe",
            "adamant_axe",
            "mithril_axe",
            "black_axe",
            "steel_axe",
            "iron_axe",
            "bronze_axe",
        ],
        "shortbow" => &["oak_longbow", "oak_shortbow", "longbow", "shortbow"],
        "bronze_arrow" => &["rune_arrow", "adamant_arrow", "mithril_arrow", "steel_arrow", "iron_arrow", "bronze_arrow"],
        "mindrune" => &["mindrune"],
        "airrune" => &["airrune"],
        "shears" => &["shears"],
        "bucket_empty" => &["bucket_empty"],
        "net" => &["net"],
        "fishing_rod" => &["fishing_rod"],
        "fly_fishing_rod" => &["fly_fishing_rod"],
        "lobster_pot" => &["lobster_pot"],
        "harpoon" => &["harpoon"],
        "fishing_bait" => &["fishing_bait"],
        "feather" => &["feather"],
        "hammer" => &["hammer"],
        "tinderbox" => &["tinderbox"],
        "needle" => &["needle"],
        "thread" => &["thread"],
        "ring_mould" => &["ring_mould"],
        "necklace_mould" => &["necklace_mould"],
        "amulet_mould" => &["amulet_mould"],
        "holy_symbol_mould" => &["holy_symbol_mould"],
        "air_talisman" => &["air_talisman"],
        "mind_talisman" => &["mind_talisman"],
        "water_talisman" => &["water_talisman"],
        "earth_talisman" => &["earth_talisman"],
        "fire_talisman" => &["fire_talisman"],
        "body_talisman" => &["body_talisman"],
        _ => &[],
    }
}

pub fn tool_price(item: &str) -> i64 {
    lifescape_gamedata::objs_by_name(item).into_iter().next().map(|entry| entry.price.max(1)).unwrap_or(i64::MAX)
}

/// One item the pack holds or can fund: carried (any family rank)
/// or a purchasable shop stock the wallet covers. Ground spawns do
/// not count: no script path picks up a ground-spawned tool (the
/// recoveries reattach, rebuy or restore from the bank). Bait rides
/// the same gate as the tool.
pub fn gear_ok(item: &str, ctx: &ReadyCtx) -> bool {
    if !ctx.bank_known {
        return true;
    }
    if tool_family(item).iter().any(|name| ctx.tools.iter().any(|held| held.eq_ignore_ascii_case(name))) {
        return true;
    }
    use lifescape_gamedata::item_sources::ItemSource;
    let stocked = lifescape_gamedata::item_sources::sources_for_name(item)
        .iter()
        .any(|source| matches!(source, ItemSource::Shop { .. }));
    // D-A: a producible tool (bronze axe: smithed) is never a merchant
    // purchase, so it never counts as fundable here.
    ctx.allow_shop && stocked && lifescape_gamedata::obtain::merchant_buy_allowed(item) && ctx.coins >= tool_price(item)
}

/// True when the bot can work `method` now: its quest done, no gear
/// needed or the tool (and bait) carried or fundable.
pub fn method_feasible(method: &str, ctx: &ReadyCtx) -> bool {
    // A method whose quest the bot has not finished never launches.
    if crate::quests::missing_quest(method, None, ctx).is_some() {
        return false;
    }
    match required_tool(method) {
        None => true,
        Some(need) => gear_ok(need.tool, ctx) && need.bait.is_none_or(|bait| gear_ok(bait, ctx)),
    }
}

#[cfg(test)]
#[path = "tools_tests.rs"]
mod tests;
