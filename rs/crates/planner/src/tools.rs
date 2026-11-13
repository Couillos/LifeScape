//! Method gear gate: which carried tools (and consumables) a method
//! needs — derived from the server data for every production and gather
//! (`method_way`) —, which names satisfy them, and whether the pack holds
//! or can fund them now. Split out of `expand` (rule 4); API re-exported
//! there.

use crate::expand::ReadyCtx;

/// Gear one method needs, from the server data (`Way.tools` of the way
/// the method works, `method_way`): the carried tools (pickaxe, hammer,
/// mould, talisman, needle...) and the consumables each action burns
/// (bait, feathers, thread, a milking bucket, a fight's arrows or runes).
/// Fights have no way: ranged takes a bow and arrows, magic the runes of
/// the spell it casts (`fight:magic/<spell>/<spot>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolNeed {
    pub tools: Vec<String>,
    pub consumables: Vec<String>,
}

impl ToolNeed {
    /// Every name, tools first.
    pub fn names(&self) -> Vec<String> {
        self.tools.iter().chain(self.consumables.iter()).cloned().collect()
    }

    /// The first name, tools first (the block a broke bot raises).
    pub fn first(&self) -> &str {
        self.tools.first().or_else(|| self.consumables.first()).map(String::as_str).unwrap_or("")
    }

    /// First item the pack neither holds nor can fund: a tool first, then
    /// a consumable (feathers once the fly rod is in hand).
    pub fn missing(&self, ctx: &ReadyCtx) -> Option<String> {
        self.tools.iter().chain(self.consumables.iter()).find(|item| !gear_ok(item, ctx)).cloned()
    }
}

/// Carried objects the data lists as tools but each action wears out
/// (`leather.rs2`: one thread per five sewings).
fn worn_out(tool: &str) -> bool {
    tool == "thread"
}

/// Required gear of one method (`None`: none). Derived from the method's
/// way for every production and gather (user rule 2026-11-13: no hand
/// list), fights from their style.
pub fn required_tool(method: &str) -> Option<ToolNeed> {
    if method.starts_with("fight:ranged/") {
        // PRD3 lot 5: ranged trains with a bow and arrows, never by
        // punching (the old script melee-attacked).
        return Some(ToolNeed { tools: vec!["shortbow".to_string()], consumables: vec!["bronze_arrow".to_string()] });
    }
    if let Some(spell) = crate::method_way::magic_spell(method) {
        // The runes of the spell the tranche casts (wind strike: one mind,
        // one air), sized per cast by `sizing`.
        let runes: Vec<String> = crate::method_way::spell_runes(&spell).into_iter().map(|(rune, _)| rune).collect();
        return (!runes.is_empty()).then_some(ToolNeed { tools: Vec::new(), consumables: runes });
    }
    let (_, way) = crate::method_way::method_way(method)?;
    let mut need = ToolNeed { tools: Vec::new(), consumables: Vec::new() };
    for tool in &way.tools {
        if worn_out(tool) {
            need.consumables.push(tool.clone());
        } else {
            need.tools.push(tool.clone());
        }
    }
    // A gather's inputs are its bait (one per catch); milking fills the
    // bucket it carries in. Recipe inputs are the work itself (sized by
    // `need_count`), never gear.
    let consumed_gear = way.kind == lifescape_gamedata::obtain::WayKind::Gather || way.detail.starts_with("milk");
    if consumed_gear {
        need.consumables.extend(way.inputs.iter().map(|(name, _)| name.clone()));
    }
    (!need.tools.is_empty() || !need.consumables.is_empty()).then_some(need)
}

/// Mould a jewellery product is cast in (from its recipe's carried tools).
pub fn jewelry_mould(product: &str) -> Option<String> {
    required_tool(&format!("jewelry:{product}")).and_then(|need| need.tools.into_iter().find(|tool| tool.ends_with("_mould")))
}

/// Talisman whose altar crafts `rune` (from its recipe's carried tools).
pub fn rune_talisman(rune: &str) -> Option<String> {
    required_tool(&format!("runecraft:{rune}")).and_then(|need| need.tools.into_iter().find(|tool| tool.ends_with("_talisman")))
}

/// Every name that satisfies `tool`: the whole `kit::tools` family
/// (any carried pickaxe mines, any carried axe chops, any bow shoots),
/// an elemental staff for its rune (the staff casts unlimited runes of
/// its element), the single name otherwise (a harpoon never catches
/// sardines).
pub fn tool_family(tool: &str) -> Vec<String> {
    let family: &[&str] = match tool {
        "bronze_pickaxe" => &["rune_pickaxe", "adamant_pickaxe", "mithril_pickaxe", "steel_pickaxe", "iron_pickaxe", "bronze_pickaxe"],
        "bronze_axe" => &["rune_axe", "adamant_axe", "mithril_axe", "black_axe", "steel_axe", "iron_axe", "bronze_axe"],
        "shortbow" => &["oak_longbow", "oak_shortbow", "longbow", "shortbow"],
        "bronze_arrow" => &["rune_arrow", "adamant_arrow", "mithril_arrow", "steel_arrow", "iron_arrow", "bronze_arrow"],
        "airrune" => &["airrune", "staff_of_air"],
        "waterrune" => &["waterrune", "staff_of_water"],
        "earthrune" => &["earthrune", "staff_of_earth"],
        "firerune" => &["firerune", "staff_of_fire"],
        _ => return vec![tool.to_string()],
    };
    family.iter().map(|name| name.to_string()).collect()
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
    required_tool(method).is_none_or(|need| need.missing(ctx).is_none())
}

#[cfg(test)]
#[path = "tools_tests.rs"]
mod tests;
