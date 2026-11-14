//! Priced training (user 2026-11-14: "the brain knows the whole F2P game,
//! bots play like real players"). Smithing and magic tranches pick their
//! method at resolve with the market, like a player picks what to make:
//! smelt bars then hammer what pays best (bronze daggers, then scimitars,
//! warhammers and platebodies as levels open them, iron, steel...), alch
//! from magic 21 when it beats the combat spells.
//!
//! Score of one method = coins one XP costs the bot (lower wins):
//! `(ticks per action × coin rate + inputs − product value) / XP per action`
//! - ticks per action: the server action (a press 5 ticks, a smelt 5, a
//!   cast 5, a combat cast with its target swaps 7) plus one bank trip
//!   shared by the load (`BANK_TRIP_TICKS` over the slots it fills);
//! - inputs: units held at what selling them brings now, the rest at
//!   what buying or making them costs the bot, its time included
//!   (`bid_response::unit_cost`, brain side); an elemental staff pays its
//!   runes; an input the bot makes with the same skill (bars smelted for
//!   the anvil) adds its XP;
//! - product value: what the market absorbs at its current price
//!   (saturation of the tranche's batch), or the alchemy net when the bot
//!   casts and that pays more; an alchemy pays its coins.
//!
//! Pure: prices, holdings and levels come in through `TrainPrices`.
//! Cannonballs are members-only at 274 (`cannonballs.rs2`: `map_members`),
//! gold and silver never go on an anvil: neither is an anvil option.

use crate::alchemy::{alch_spell, alch_value, CAST_TICKS, FIRE_RUNE, FIRE_STAFF, MIN_ALCH_CASTS, NATURE_RUNE};

/// Ticks of one anvil press (`[proc,smithing_anvil]`: product after 1,
/// then 2 more, plus the click).
pub const SMITH_PRESS_TICKS: f64 = 5.0;
/// Ticks of one furnace smelt (`smelt_ore_weakqueue`, 3-4 ticks + click).
pub const SMELT_TICKS: f64 = 5.0;
/// Ticks of one combat cast with the walk to the next target.
pub const FIGHT_CAST_TICKS: f64 = 7.0;
/// One bank round trip from an anvil, a furnace or a casting spot.
pub const BANK_TRIP_TICKS: f64 = 60.0;
/// Pack slots one load fills (the last one holds the hammer or runes).
pub const LOAD_SLOTS: f64 = 27.0;
/// Most actions a tranche is priced over (book depth, saturation).
pub const MAX_PRICED_ACTIONS: i64 = 2000;
/// Magic level from which a tranche is priced (low alchemy).
pub const ALCH_FROM_LEVEL: u8 = 21;

/// F2P anvil metals: bar, product prefix (274 shortens adamant and rune),
/// smithing level of the dagger.
pub const METALS: [(&str, &str, u8); 6] =
    [("bronze_bar", "bronze", 1), ("iron_bar", "iron", 15), ("steel_bar", "steel", 30), ("mithril_bar", "mithril", 50), ("adamantite_bar", "adamant", 70), ("runite_bar", "rune", 85)];
/// Product suffixes of the four F2P smithing columns (the `smith`
/// script's cell table): anything else (nails, wire, members cells) is
/// never a training product.
pub const SMITH_CELLS: [&str; 17] = [
    "dagger", "sword", "scimitar", "longsword", "2h_sword", "axe", "mace", "warhammer", "battleaxe", "chainbody", "platelegs", "plateskirt", "platebody", "med_helm", "full_helm", "sq_shield",
    "kiteshield",
];

/// One anvil product: name, bar, bars per press, smithing level.
#[derive(Debug, Clone, PartialEq)]
pub struct SmithProduct {
    pub product: String,
    pub bar: &'static str,
    pub bars: i64,
    pub level: u8,
}

/// Every F2P anvil product of the smithing columns, from the dump.
pub fn smith_products() -> &'static [SmithProduct] {
    static TABLE: std::sync::OnceLock<Vec<SmithProduct>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut out = Vec::new();
        for recipe in lifescape_gamedata::recipes::all_recipes(Some("smith")) {
            if recipe.members != Some(false) {
                continue;
            }
            let Some((product, _)) = recipe.outputs.first() else { continue };
            let Some((bar, bars)) = recipe.inputs.first() else { continue };
            let Some((bar, prefix, _)) = METALS.iter().find(|(name, _, _)| name == bar) else { continue };
            let Some(suffix) = product.strip_prefix(&format!("{prefix}_")) else { continue };
            if !SMITH_CELLS.contains(&suffix) {
                continue;
            }
            out.push(SmithProduct { product: product.clone(), bar, bars: (*bars).max(1), level: u8::try_from(recipe.level.clamp(1, 99)).unwrap_or(99) });
        }
        out.sort_by(|left, right| left.level.cmp(&right.level).then_with(|| left.product.cmp(&right.product)));
        out
    })
}

/// Bar of the best metal open at `level` (bronze below 15).
pub fn default_smith_bar(level: u8) -> &'static str {
    METALS.iter().rev().find(|(_, _, from)| level >= *from).map(|(bar, _, _)| *bar).unwrap_or("bronze_bar")
}

/// Static tranche product at `level` (no prices: the skeleton's guess,
/// its bars filed ahead): the best metal open, the product with the most
/// bars per press (most XP per tick), the first unlocked on a tie.
pub fn default_smith_product(level: u8) -> String {
    let bar = default_smith_bar(level);
    smith_products()
        .iter()
        .filter(|row| row.bar == bar && row.level <= level)
        .max_by(|left, right| left.bars.cmp(&right.bars).then_with(|| right.level.cmp(&left.level)))
        .map(|row| row.product.clone())
        .unwrap_or_else(|| "bronze_dagger".to_string())
}

/// Smithing tranche cuts: every level that opens a better anvil product
/// for its metal (more bars per press), a new metal, or a new bar to
/// smelt. Computed once from the dump.
pub fn smithing_cuts() -> &'static [u8] {
    static CUTS: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    CUTS.get_or_init(|| {
        let mut cuts: Vec<u8> = Vec::new();
        for (bar, _, _) in METALS {
            let mut best = 0;
            for row in smith_products().iter().filter(|row| row.bar == bar) {
                if row.bars > best {
                    best = row.bars;
                    cuts.push(row.level);
                }
            }
        }
        for recipe in lifescape_gamedata::recipes::all_recipes(Some("smelt")) {
            if recipe.members == Some(false) {
                cuts.push(u8::try_from(recipe.level.clamp(1, 99)).unwrap_or(99));
            }
        }
        cuts.retain(|cut| *cut > 1 && *cut < 99);
        cuts.sort_unstable();
        cuts.dedup();
        cuts
    })
}

/// True when a tranche of `skill` from `start` picks its method at
/// resolve with prices: smithing always, magic from low alchemy. The
/// skeleton files no consumption for a magic one (the object to alch is
/// unknown until then).
pub fn priced_at_resolve(skill: &str, start: u8) -> bool {
    skill == "smithing" || (skill == "magic" && start >= ALCH_FROM_LEVEL)
}

/// What the bot pays for one unit it does not hold.
#[derive(Debug, Clone, PartialEq)]
pub struct Acquired {
    /// Coins plus the bot's time at its coin rate.
    pub coins_each: f64,
    /// The production method when the bot makes it (`smelt:bronze_bar`).
    pub made_by: Option<String>,
}

/// The bot's own numbers and the public market, read by the brain.
pub struct TrainPrices<'a> {
    /// Coins one tick of the bot's time is worth.
    pub coin_rate: f64,
    /// Free coins (pack + bank, minus resting buys).
    pub coins: i64,
    /// Free units held (pack, bank, worn; listings out).
    pub held: &'a dyn Fn(&str) -> i64,
    /// What selling one held unit brings now (0: no buyer).
    pub sale_value: &'a dyn Fn(&str) -> f64,
    /// Cost of each unit when `units` more are needed (`None`: neither
    /// bought nor made now).
    pub acquire: &'a dyn Fn(&str, i64) -> Option<Acquired>,
    /// What one product brings when `units` of it reach the market.
    pub product_value: &'a dyn Fn(&str, i64) -> f64,
}

/// One object the bot could alch: own stock (`own`) or resting asks.
#[derive(Debug, Clone, PartialEq)]
pub struct AlchSupply {
    pub item: String,
    pub units: i64,
    /// What one unit costs: its sale value when owned, the ask paid else.
    pub unit_cost: f64,
    pub own: bool,
}

/// The method a tranche launches with, priced.
#[derive(Debug, Clone, PartialEq)]
pub struct TrainPick {
    pub method: String,
    /// Units the launch works (alchemy casts), 0 = until the level.
    pub batch: i64,
    /// Inputs to obtain first (missing at resolve: obtain sub-tasks).
    pub missing: Vec<String>,
    pub cost_per_xp: f64,
    pub xp_per_tick: f64,
}

/// Cost of `units` of one input: held units at their sale value, the
/// rest acquired; `bonus_xp` per unit when the bot makes it with `skill`.
struct Costed {
    each: f64,
    bonus_xp: f64,
    short: bool,
}

/// An elemental staff pays its rune (`tools::tool_family`).
fn staff_pays(item: &str, prices: &TrainPrices) -> bool {
    crate::tools::tool_family(item).iter().any(|name| name.starts_with("staff_of_") && (prices.held)(name) > 0)
}

fn cost_input(item: &str, units: i64, first_load: i64, skill: &str, prices: &TrainPrices) -> Option<Costed> {
    if staff_pays(item, prices) {
        return Some(Costed { each: 0.0, bonus_xp: 0.0, short: false });
    }
    let units = units.max(1);
    let held = (prices.held)(item).max(0);
    let have = held.min(units);
    let rest = units - have;
    let sale = (prices.sale_value)(item).max(0.0);
    if rest == 0 {
        return Some(Costed { each: sale, bonus_xp: 0.0, short: false });
    }
    let acquired = (prices.acquire)(item, rest)?;
    let each = (have as f64 * sale + rest as f64 * acquired.coins_each.max(0.0)) / units as f64;
    let bonus = acquired
        .made_by
        .as_deref()
        .filter(|method| crate::method_way::method_way(method).and_then(|(_, way)| way.skill).is_some_and(|(trained, _)| trained == skill))
        .and_then(crate::method_way::action_xp)
        .map(|xp| xp * rest as f64 / units as f64)
        .unwrap_or(0.0);
    Some(Costed { each, bonus_xp: bonus, short: held < first_load.max(1) })
}

fn actions_for(xp_needed: f64, xp_each: f64) -> i64 {
    if xp_each <= 0.0 {
        return 0;
    }
    ((xp_needed / xp_each).ceil() as i64).clamp(1, MAX_PRICED_ACTIONS)
}

fn pick(method: String, batch: i64, missing: Vec<String>, cost_each: f64, xp_each: f64, ticks_each: f64) -> Option<TrainPick> {
    (xp_each > 0.0 && ticks_each > 0.0).then(|| TrainPick { method, batch, missing, cost_per_xp: cost_each / xp_each, xp_per_tick: xp_each / ticks_each })
}

/// Every smithing method at `level` toward `target`, priced, best first:
/// each open anvil product and each open bar to smelt.
pub fn rank_smithing(level: u8, target: u8, prices: &TrainPrices) -> Vec<TrainPick> {
    let xp_needed = crate::sizing::xp_between(level, target.max(level.saturating_add(1)));
    let mut out = Vec::new();
    for row in smith_products().iter().filter(|row| row.level <= level) {
        let method = format!("smith:{}", row.product);
        let Some(xp_each) = crate::method_way::action_xp(&method) else { continue };
        let actions = actions_for(xp_needed, xp_each);
        // Bars the climb melts are the same for every product of a metal
        // (XP is per bar): one acquisition price per metal (memoized).
        let bars_total = ((xp_needed * row.bars as f64 / xp_each).ceil() as i64).clamp(row.bars, MAX_PRICED_ACTIONS);
        let Some(bars) = cost_input(row.bar, bars_total, row.bars, "smithing", prices) else { continue };
        let ticks_each = SMITH_PRESS_TICKS + BANK_TRIP_TICKS * row.bars as f64 / LOAD_SLOTS;
        let value = (prices.product_value)(&row.product, actions).max(0.0);
        let cost_each = ticks_each * prices.coin_rate + row.bars as f64 * bars.each - value;
        let missing = if bars.short { vec![row.bar.to_string()] } else { Vec::new() };
        out.extend(pick(method, 0, missing, cost_each, xp_each + row.bars as f64 * bars.bonus_xp, ticks_each));
    }
    for recipe in lifescape_gamedata::recipes::all_recipes(Some("smelt")) {
        if recipe.members != Some(false) || recipe.level > i64::from(level) {
            continue;
        }
        let Some((bar, _)) = recipe.outputs.first() else { continue };
        let method = format!("smelt:{bar}");
        let Some((_, way)) = crate::method_way::method_way(&method) else { continue };
        let Some(xp_each) = crate::method_way::action_xp(&method) else { continue };
        let success = if way.success > 0.0 { way.success } else { 1.0 };
        let actions = actions_for(xp_needed, xp_each);
        let mut inputs_each = 0.0;
        let mut slots = 0.0;
        let mut missing = Vec::new();
        let mut feasible = true;
        for (ore, per) in &way.inputs {
            let per_bar = (*per).max(1) as f64 / success;
            let Some(costed) = cost_input(ore, (actions as f64 * per_bar).ceil() as i64, per_bar.ceil() as i64, "smithing", prices) else {
                feasible = false;
                break;
            };
            inputs_each += per_bar * costed.each;
            slots += per_bar;
            if costed.short {
                missing.push(ore.clone());
            }
        }
        if !feasible {
            continue;
        }
        let ticks_each = SMELT_TICKS / success + BANK_TRIP_TICKS * slots / (LOAD_SLOTS + 1.0);
        let value = (prices.product_value)(bar, actions).max(0.0);
        out.extend(pick(method, 0, missing, ticks_each * prices.coin_rate + inputs_each - value, xp_each, ticks_each));
    }
    sort_best_first(&mut out);
    out
}

/// Every magic method at `level` toward `target`, priced, best first:
/// the combat spell of the tranche (`tranches::reach_method`) and, from
/// 21, alchemy of each supplied object (low below 55, high from 55).
pub fn rank_magic(level: u8, target: u8, prices: &TrainPrices, supply: &[AlchSupply]) -> Vec<TrainPick> {
    let xp_needed = crate::sizing::xp_between(level, target.max(level.saturating_add(1)));
    let mut out = Vec::new();
    let fight = crate::tranches::reach_method("magic", level);
    if let (Some(xp_each), Some(spell)) = (crate::sizing::xp_per_action(&fight, level), crate::method_way::magic_spell(&fight)) {
        let casts = actions_for(xp_needed, xp_each);
        let mut runes_each = 0.0;
        let mut missing = Vec::new();
        let mut feasible = true;
        for (rune, per_cast) in crate::method_way::spell_runes(&spell) {
            let Some(costed) = cost_input(&rune, casts * per_cast, per_cast, "magic", prices) else {
                feasible = false;
                break;
            };
            runes_each += per_cast as f64 * costed.each;
            if costed.short {
                missing.push(rune);
            }
        }
        if feasible {
            out.extend(pick(fight, 0, missing, FIGHT_CAST_TICKS * prices.coin_rate + runes_each, xp_each, FIGHT_CAST_TICKS));
        }
    }
    if let Some(spell) = alch_spell(level) {
        let casts_needed = actions_for(xp_needed, spell.xp);
        for offer in supply {
            out.extend(alch_option(&spell, casts_needed, offer, prices));
        }
    }
    sort_best_first(&mut out);
    out
}

/// One alchemy option: casts = what the stock (or the asks), the runes
/// and the wallet cover, at most the tranche; fire from a staff held,
/// fire runes, or a staff bought when the batch's runes cost more.
fn alch_option(spell: &crate::alchemy::AlchSpell, casts_needed: i64, offer: &AlchSupply, prices: &TrainPrices) -> Option<TrainPick> {
    let value = alch_value(&offer.item, spell.high)? as f64;
    let mut casts = offer.units.min(casts_needed);
    let natures_per = spell.runes.iter().find(|(rune, _)| rune == NATURE_RUNE).map(|(_, count)| *count).unwrap_or(1);
    let fires_per = spell.runes.iter().find(|(rune, _)| rune == FIRE_RUNE).map(|(_, count)| *count).unwrap_or(0);
    // The wallet pays what the bot does not hold (object asks, runes).
    let natures_held = (prices.held)(NATURE_RUNE).max(0);
    let nature_price = match (prices.acquire)(NATURE_RUNE, 1) {
        Some(got) => got.coins_each,
        // Nature runes nobody sells now: the ones held bound the batch.
        None => {
            casts = casts.min(natures_held / natures_per.max(1));
            0.0
        }
    };
    let ask = if offer.own { 0.0 } else { offer.unit_cost.max(0.0) };
    let coins = prices.coins.max(0) as f64;
    let per = natures_per.max(1) as f64;
    // Casts the wallet pays: the asks while the natures held last, then
    // the asks and the natures together.
    let on_held = if ask > 0.0 { (coins / ask).floor() } else { f64::INFINITY };
    let affordable = if on_held * per <= natures_held as f64 {
        on_held
    } else if ask + per * nature_price > 0.0 {
        ((coins + natures_held as f64 * nature_price) / (ask + per * nature_price)).floor()
    } else {
        f64::INFINITY
    };
    if affordable.is_finite() {
        casts = casts.min(affordable as i64);
    }
    if casts < MIN_ALCH_CASTS {
        return None;
    }
    let natures = cost_input(NATURE_RUNE, casts * natures_per, casts * natures_per, "magic", prices)?;
    let mut missing = Vec::new();
    if !offer.own {
        missing.push(offer.item.clone());
    }
    if natures.short {
        missing.push(NATURE_RUNE.to_string());
    }
    let fire_each = if fires_per == 0 || staff_pays(FIRE_RUNE, prices) {
        0.0
    } else {
        let runes = cost_input(FIRE_RUNE, casts * fires_per, casts * fires_per, "magic", prices);
        // A staff serves the whole climb, not this batch alone.
        let staff = (prices.acquire)(FIRE_STAFF, 1).map(|got| got.coins_each / casts_needed.max(casts) as f64);
        match (runes, staff) {
            (Some(runes), Some(staff)) if staff < runes.each * fires_per as f64 => {
                missing.push(FIRE_STAFF.to_string());
                staff
            }
            (Some(runes), _) => {
                if runes.short {
                    missing.push(FIRE_RUNE.to_string());
                }
                runes.each * fires_per as f64
            }
            (None, Some(staff)) => {
                missing.push(FIRE_STAFF.to_string());
                staff
            }
            (None, None) => return None,
        }
    };
    let stackable = lifescape_gamedata::objs_by_name(&offer.item).into_iter().next().is_some_and(|obj| obj.stackable);
    let ticks_each = CAST_TICKS + if stackable { 0.0 } else { BANK_TRIP_TICKS / (LOAD_SLOTS - 1.0) };
    let cost_each = ticks_each * prices.coin_rate + offer.unit_cost.max(0.0) + natures_per as f64 * natures.each + fire_each - value;
    pick(format!("alch:{}", offer.item), casts, missing, cost_each, spell.xp, ticks_each)
}

fn sort_best_first(picks: &mut [TrainPick]) {
    picks.sort_by(|left, right| left.cost_per_xp.total_cmp(&right.cost_per_xp).then_with(|| left.method.cmp(&right.method)));
}

/// The pick for a tranche of `skill` at the live `level` toward
/// `target` (`None`: not a priced tranche, or nothing priced).
pub fn best_pick(skill: &str, level: u8, target: u8, prices: &TrainPrices, supply: &[AlchSupply]) -> Option<TrainPick> {
    if !priced_at_resolve(skill, level) {
        return None;
    }
    let ranked = match skill {
        "smithing" => rank_smithing(level, target, prices),
        "magic" => rank_magic(level, target, prices, supply),
        _ => Vec::new(),
    };
    ranked.into_iter().next()
}

/// Objects an open tranche of `skill` toward `target` keeps out of the
/// sales between launches: the alchemy runes (and the staff) of a magic
/// climb past 21.
pub fn kept_inputs(skill: &str, target: u8) -> Vec<String> {
    if skill == "magic" && target > ALCH_FROM_LEVEL {
        return vec![NATURE_RUNE.to_string(), FIRE_RUNE.to_string(), FIRE_STAFF.to_string()];
    }
    Vec::new()
}

#[cfg(test)]
#[path = "training_tests.rs"]
mod tests;
