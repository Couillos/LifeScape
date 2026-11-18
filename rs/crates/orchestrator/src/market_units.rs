#![forbid(unsafe_code)]
//! Price units at the market boundary (plan prix lot D, user decision Q3
//! 2026-11-18). A cheap object is quoted per LOT (`lifescape_market::
//! lots`): the market engine, the carnet, the deals, the executions and
//! the candles hold coins per lot; the brain, the guides and the scripts
//! keep coins per unit. Every crossing goes through here:
//! - `lot_of`: the object's lot, fixed by its game value (its base form
//!   for a banknote), 1 while `LOT_QUOTATION` is off;
//! - `to_lot` / `to_unit`: a per-unit price into the market, a market
//!   price back to the brain (rounded, at least 1);
//! - `deal_coins`: what a deal's quantity costs at its market price.

use lifescape_market::lots;

/// Lot quotation switch: off, every object is quoted per unit (lot 1).
pub const LOT_QUOTATION: bool = true;

#[cfg(test)]
thread_local! {
    /// Unit tests price per unit unless one turns lots on for its thread
    /// (`lots_on_for_this_test`): the older tests cover the unit rules.
    static TEST_LOTS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Turns lot quotation on for the calling test thread.
#[cfg(test)]
pub fn lots_on_for_this_test() {
    TEST_LOTS.with(|cell| cell.set(true));
}

fn active() -> bool {
    #[cfg(test)]
    {
        TEST_LOTS.with(|cell| cell.get())
    }
    #[cfg(not(test))]
    {
        LOT_QUOTATION
    }
}

/// Lots decided for the books (base item id → lot), loaded by the market
/// service at its start (`decide_lots`).
fn lots_table() -> &'static std::sync::RwLock<std::collections::HashMap<i64, i64>> {
    static TABLE: std::sync::OnceLock<std::sync::RwLock<std::collections::HashMap<i64, i64>>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

/// Quotation lot of a book (`item`: any form, a banknote reads its base);
/// 1 for coins, an unknown object, or while lots are off.
pub fn lot_of(item: i64) -> i64 {
    if !active() {
        return 1;
    }
    let base = crate::market_audit::base_of(item);
    lots_table().read().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&base).copied().unwrap_or(1)
}

/// Objects traded in bulk (plan prix lot D): every input a recipe eats
/// (ores, logs, raw fish, bars, hides, essence...), never a carried tool
/// (hammer, chisel, needle, tinderbox, a mould, a talisman).
fn carried_tool(name: &str) -> bool {
    matches!(name, "hammer" | "chisel" | "needle" | "tinderbox") || name.ends_with("_mould") || name.ends_with("_talisman")
}

fn bulk_inputs() -> &'static std::collections::HashSet<String> {
    static SET: std::sync::OnceLock<std::collections::HashSet<String>> = std::sync::OnceLock::new();
    SET.get_or_init(|| {
        let carried = carried_tool;
        let recipes = lifescape_gamedata::recipes::all_recipes(None).into_iter().flat_map(|recipe| recipe.inputs.into_iter().map(|(name, _)| name));
        let extras = lifescape_gamedata::extra_recipes::all_extra().into_iter().flat_map(|recipe| recipe.inputs.into_iter().map(|(name, _)| name));
        recipes.chain(extras).filter(|name| !carried(name)).collect()
    })
}

/// The lot a book takes when it gets one (plan prix lot D): from its
/// observed unit price (median of its last fills, `observed`), else its
/// game value. Only what trades in bulk takes a lot: a stackable object
/// (runes, arrows, feathers, bait) or a recipe input; gear, tools and
/// coins are bought one at a time, per unit.
pub fn first_lot(item: i64, observed: Option<i64>) -> i64 {
    let obj = usize::try_from(item).ok().and_then(lifescape_gamedata::obj_by_id);
    let name = obj.as_ref().and_then(|obj| obj.name).unwrap_or_default();
    let stackable = obj.as_ref().is_some_and(|obj| obj.stackable);
    if name == "coins" || carried_tool(name) || lifescape_gamedata::equipment::by_name(name).is_some() || !(stackable || bulk_inputs().contains(name)) {
        return 1;
    }
    let value = usize::try_from(item).ok().and_then(lifescape_gamedata::obj_by_id).map(|obj| obj.price).unwrap_or(i64::MAX);
    lots::lot_for_price(observed.unwrap_or(value).max(1))
}

/// Records the lots the market service decided (`item` → lot).
pub fn load_lots(lots: &[(i64, i64)]) {
    let mut table = lots_table().write().unwrap_or_else(|poisoned| poisoned.into_inner());
    table.clear();
    table.extend(lots.iter().copied());
}

/// True while lots are quoted (production; a test thread that turned them
/// on).
pub fn lots_active() -> bool {
    active()
}

/// A per-unit price of `item` as the market holds it (per lot).
pub fn to_lot(item: i64, unit_price: i64) -> i64 {
    lots::lot_price(unit_price, lot_of(item))
}

/// A market price of `item` (per lot) as the brain reads it (per unit).
pub fn to_unit(item: i64, price: i64) -> i64 {
    lots::unit_price(price, lot_of(item))
}

/// Exact per-unit value of a market price of `item`.
pub fn to_unit_exact(item: i64, price: i64) -> f64 {
    lots::unit_price_exact(price, lot_of(item))
}

/// Coins `quantity` units of `item` cost at the market price `price`.
pub fn deal_coins(item: i64, quantity: i64, price: i64) -> i64 {
    lots::lot_coins(quantity, price, lot_of(item))
}

/// A book's levels `(price, units)` from the market into per-unit prices.
pub fn unit_levels(item: i64, levels: &[(i64, i64)]) -> Vec<(i64, i64)> {
    levels.iter().map(|(price, units)| (to_unit(item, *price), *units)).collect()
}

#[cfg(test)]
mod tests {
    /// Off, every object is quoted per unit: the boundary is the identity.
    #[test]
    fn off_the_boundary_is_the_identity() {
        assert_eq!(super::lot_of(556), 1);
        assert_eq!(super::to_lot(556, 2), 2);
        assert_eq!(super::to_unit(556, 2), 2);
        assert_eq!(super::deal_coins(556, 300, 2), 600);
    }

    /// A book's first lot follows its observed unit price, else its game
    /// value; coins stay per unit.
    #[test]
    fn the_first_lot_reads_the_observed_price() {
        let id = |name: &str| lifescape_gamedata::objs_by_name(name).into_iter().next().map(|obj| obj.obj_id as i64).unwrap_or(-1);
        assert_eq!(super::first_lot(id("bones"), Some(67)), 1, "bones trade at 67");
        assert_eq!(super::first_lot(id("waterrune"), Some(2)), 100);
        assert_eq!(super::first_lot(id("logs"), Some(12)), 10);
        assert_eq!(super::first_lot(id("coins"), Some(1)), 1);
        assert_eq!(super::first_lot(id("bronze_pickaxe"), Some(2)), 1, "a tool is bought one at a time");
        assert_eq!(super::first_lot(id("bronze_platebody"), Some(3)), 1, "gear too");
        assert_eq!(super::first_lot(id("chisel"), Some(2)), 1, "a carried tool");
        assert_eq!(super::first_lot(id("needle"), Some(1)), 1, "a carried tool, stackable or not");
        assert_eq!(super::first_lot(id("bucket_empty"), Some(2)), 1, "no recipe eats it, not stackable");
        assert_eq!(super::first_lot(id("copper_ore"), Some(2)), 100, "a smelting input");
        assert_eq!(super::first_lot(id("raw_sardine"), Some(2)), 100, "a cooking input");
        assert_eq!(super::first_lot(id("feather"), None), 100, "never traded: its game value");
    }
}

