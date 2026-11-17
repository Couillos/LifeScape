//! Priced training (user 2026-11-14: "the brain knows the whole F2P game,
//! bots play like real players"). Smithing and magic tranches pick their
//! method at resolve with the market, like a player picks what to make:
//! smelt bars then hammer what pays best (bronze daggers, then scimitars,
//! warhammers and platebodies as levels open them, iron, steel...), alch
//! from magic 21 when it beats the combat spells, a teleport, a curse or
//! bones to bananas when one of them does (D18).
//!
//! Score of one method = coins one XP costs the bot (lower wins):
//! `(ticks per XP × coin rate + inputs per XP − product value per XP)`,
//! everything per XP read from the method's YIELD (`TrainPrices::rate`,
//! plan rendements R3.3: the measured blend the caller holds, else the
//! game prior), never from a constant:
//! - ticks per XP = 6000 / XP per 6000 (the work, its failures and its
//!   bank walks as measured or as the prior models them);
//! - inputs: each consumable the rate burns per XP (bars, ores, raw
//!   fish, runes, bones, thread), units held at what selling them brings
//!   now, the rest at what buying or making them costs the bot, its
//!   time included (`bid_response::unit_cost`, brain side); an
//!   elemental staff pays its runes; an input the bot makes with the
//!   same skill (bars smelted for the anvil) adds its XP;
//! - product value: each product the rate yields per XP at what the
//!   market absorbs at its current price (saturation of the tranche's
//!   batch), or the alchemy net when the bot casts and that pays more;
//!   an alchemy pays its coins.
//!
//! Pure: prices, holdings, levels and rates come in through `TrainPrices`.
//! Cannonballs are members-only at 274 (`cannonballs.rs2`: `map_members`),
//! gold and silver never go on an anvil: neither is an anvil option.

use crate::alchemy::{alch_spell, alch_value, FIRE_RUNE, FIRE_STAFF, MIN_ALCH_CASTS, NATURE_RUNE};
use crate::work_rate::WorkRate;

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
/// resolve with prices: smithing always, magic from the first level that
/// opens a second method (confuse at 3, D18: every F2P spell competes).
/// The skeleton files no consumption for a magic one (the spell, its
/// runes and the object to alch are unknown until then).
pub fn priced_at_resolve(skill: &str, start: u8) -> bool {
    skill == "smithing" || (skill == "magic" && start >= crate::training_spells::magic_priced_from()) || crate::training_production::PRODUCTION_SKILLS.contains(&skill)
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
    /// The yield of (method, level) the caller trusts (R3.3: measured
    /// blend, else `work_rate::prior_rate`); `None`: the method is not
    /// priced (no rate, never a default figure).
    pub rate: &'a dyn Fn(&str, u8) -> Option<WorkRate>,
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
    /// A market line of the training guide (user lever 2026-11-17): the
    /// climb buys its inputs (`Node::buys_inputs`), never farms them.
    pub buys_inputs: bool,
    pub cost_per_xp: f64,
    pub xp_per_tick: f64,
    /// Coins the method earns per tick, time not counted (product value
    /// less inputs; negative = a loss). The ranking key (user
    /// 2026-11-15: the most profitable method, never a loss when one pays).
    pub profit_per_tick: f64,
}

/// Cost of `units` of one input: held units at their sale value, the
/// rest acquired; `bonus_xp` per unit when the bot makes it with `skill`.
pub(crate) struct Costed {
    pub(crate) each: f64,
    pub(crate) bonus_xp: f64,
    pub(crate) short: bool,
}

/// One method of a `skill` climb priced from its rate at `level`
/// (module doc): its time per XP, each consumable it burns per XP at
/// what the bot pays for it (the climb's units, its first action's
/// units held or not), each product it yields per XP at what the market
/// absorbs (coins at face value), its fees. `None`: no rate, no XP of
/// the skill, or an input neither held, bought nor made.
pub(crate) fn rated_option(method: &str, skill: &str, level: u8, xp_needed: f64, prices: &TrainPrices) -> Option<TrainPick> {
    let rate = (prices.rate)(method, level)?;
    let ticks_per_xp = rate.ticks_per_xp(skill)?;
    let xp_each = rate.xp_per_action(skill)?;
    let xp_6000 = rate.xp_of(skill);
    let actions = actions_for(xp_needed, xp_each);
    let climb_xp = actions as f64 * xp_each;
    let mut cost_per_xp = 0.0;
    let mut bonus_per_xp = 0.0;
    let mut missing = Vec::new();
    for (item, per_xp) in rate.burned_per_xp(skill) {
        let units = (per_xp * climb_xp - 1e-6).ceil().max(1.0) as i64;
        let first = (per_xp * xp_each - 1e-6).ceil().max(1.0) as i64;
        let costed = cost_input(&item, units, first, skill, prices)?;
        cost_per_xp += per_xp * costed.each;
        bonus_per_xp += per_xp * costed.bonus_xp;
        if costed.short {
            missing.push(item);
        }
    }
    let mut value_per_xp = 0.0;
    for (item, cause, qty) in &rate.flows_6000 {
        let per_xp = qty / xp_6000;
        match cause.as_str() {
            "product" if item == "coins" => value_per_xp += per_xp,
            "product" | "byproduct" if *qty > 0.0 => value_per_xp += per_xp * (prices.product_value)(item, (per_xp * climb_xp).ceil() as i64).max(0.0),
            "fee" if *qty < 0.0 => cost_per_xp -= per_xp,
            _ => {}
        }
    }
    let cost_each = xp_each * (ticks_per_xp * prices.coin_rate + cost_per_xp - value_per_xp);
    pick(method.to_string(), 0, missing, cost_each, xp_each * (1.0 + bonus_per_xp), xp_each * ticks_per_xp, prices.coin_rate)
}

/// `sort_best_first` for the sibling pricers.
pub(crate) fn sort_picks(picks: &mut [TrainPick]) {
    sort_best_first(picks);
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

/// `cost_each` carries the bot's time (`ticks_each × coin_rate`): the
/// profit adds it back, coins only.
fn pick(method: String, batch: i64, missing: Vec<String>, cost_each: f64, xp_each: f64, ticks_each: f64, coin_rate: f64) -> Option<TrainPick> {
    (xp_each > 0.0 && ticks_each > 0.0).then(|| TrainPick {
        method,
        batch,
        missing,
        buys_inputs: false,
        cost_per_xp: cost_each / xp_each,
        xp_per_tick: xp_each / ticks_each,
        profit_per_tick: coin_rate - cost_each / ticks_each,
    })
}

/// Every smithing method at `level` toward `target`, priced from its
/// rate, best first: each open anvil product and each open bar to smelt.
/// Bars the climb melts are the same for every product of a metal (XP is
/// per bar): one acquisition price per metal (memoized by the caller).
pub fn rank_smithing(level: u8, target: u8, prices: &TrainPrices) -> Vec<TrainPick> {
    let xp_needed = crate::sizing::xp_between(level, target.max(level.saturating_add(1)));
    let mut out = Vec::new();
    for row in smith_products().iter().filter(|row| row.level <= level) {
        out.extend(rated_option(&format!("smith:{}", row.product), "smithing", level, xp_needed, prices));
    }
    for recipe in lifescape_gamedata::recipes::all_recipes(Some("smelt")) {
        if recipe.members != Some(false) || recipe.level > i64::from(level) {
            continue;
        }
        let Some((bar, _)) = recipe.outputs.first() else { continue };
        out.extend(rated_option(&format!("smelt:{bar}"), "smithing", level, xp_needed, prices));
    }
    sort_best_first(&mut out);
    out
}

/// Every magic method at `level` toward `target`, priced, best first:
/// the combat spell of the tranche (`tranches::reach_method`), from 21
/// alchemy of each supplied object (low below 55, high from 55), and the
/// D18 spells open at the level (`training_spells`: teleports, curses,
/// bones to bananas, crumble undead on the undead).
pub fn rank_magic(level: u8, target: u8, prices: &TrainPrices, supply: &[AlchSupply]) -> Vec<TrainPick> {
    let xp_needed = crate::sizing::xp_between(level, target.max(level.saturating_add(1)));
    let mut out = Vec::new();
    out.extend(rated_option(&crate::tranches::reach_method("magic", level), "magic", level, xp_needed, prices));
    if let Some(spell) = alch_spell(level) {
        let casts_needed = actions_for(xp_needed, spell.xp);
        for offer in supply {
            out.extend(alch_option(&spell, casts_needed, offer, level, prices));
        }
    }
    out.extend(crate::training_spells::spell_options(level, target, prices));
    sort_best_first(&mut out);
    out
}

/// One alchemy option: casts = what the stock (or the asks), the runes
/// and the wallet cover, at most the tranche; fire from a staff held,
/// fire runes, or a staff bought when the batch's runes cost more.
fn alch_option(spell: &crate::alchemy::AlchSpell, casts_needed: i64, offer: &AlchSupply, level: u8, prices: &TrainPrices) -> Option<TrainPick> {
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
    // The cast and its bank walks (a non-stackable object fills the
    // pack), from the rate: one cast = its XP × ticks per XP.
    let method = format!("alch:{}", offer.item);
    let ticks_each = spell.xp * (prices.rate)(&method, level)?.ticks_per_xp("magic")?;
    let cost_each = ticks_each * prices.coin_rate + offer.unit_cost.max(0.0) + natures_per as f64 * natures.each + fire_each - value;
    pick(method, casts, missing, cost_each, spell.xp, ticks_each, prices.coin_rate)
}

/// Most profitable first (user 2026-11-15): every paying method (coins
/// per tick > 0, time not counted) before any loss, the best profit per
/// tick first; when all lose, the smallest coin loss per XP (a loss per
/// tick would crown the slowest method); then the fastest XP, the name.
fn sort_best_first(picks: &mut [TrainPick]) {
    picks.sort_by(|left, right| rank_key(left).total_cmp(&rank_key(right)).then_with(|| right.xp_per_tick.total_cmp(&left.xp_per_tick)).then_with(|| left.method.cmp(&right.method)));
}

/// Ascending key: a paying method keys below every loss (its negated
/// profit per tick), a loss keys at its coin loss per XP (>= 0).
fn rank_key(pick: &TrainPick) -> f64 {
    if pick.profit_per_tick > 0.0 {
        -pick.profit_per_tick
    } else {
        -pick.profit_per_tick / pick.xp_per_tick.max(f64::MIN_POSITIVE)
    }
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
        other if crate::training_production::PRODUCTION_SKILLS.contains(&other) => crate::training_production::rank_production(other, level, target, prices),
        _ => Vec::new(),
    };
    ranked.into_iter().next()
}

/// Consumables a climb with `method` from `level` to `target` burns
/// (`sizing::tranche_consumption`, from its yield) of which the bot
/// holds less than one action's worth: what the tranche obtains first
/// (the resolver blocks on them at once). An elemental staff held pays
/// its rune.
pub fn missing_inputs(method: &str, level: u8, target: u8, held: &dyn Fn(&str) -> i64) -> Vec<String> {
    missing_inputs_with(&crate::work_rate::prior_rate, method, level, target, held)
}

/// `missing_inputs` reading the yields from `rate` (R7, `plan_read`).
pub fn missing_inputs_with(rate: crate::work_rate::RateReader, method: &str, level: u8, target: u8, held: &dyn Fn(&str) -> i64) -> Vec<String> {
    let staff = |item: &str| crate::tools::tool_family(item).iter().any(|name| name.starts_with("staff_of_") && held(name) > 0);
    let mut out = Vec::new();
    for (item, _) in crate::sizing::tranche_consumption_with(rate, method, level, target.max(level.saturating_add(1))) {
        let first = crate::sizing::per_action_need_with(rate, method, level, &item).unwrap_or(1).max(1);
        if !staff(&item) && held(&item) < first {
            out.push(item);
        }
    }
    out
}

/// Objects an open tranche of `skill` toward `target` keeps out of the
/// sales between launches: the alchemy runes (and the staff) of a magic
/// climb past 21, the runes of every D18 spell it may pick (teleports,
/// curses, bones to bananas, crumble undead) and the bones bananas eat.
pub fn kept_inputs(skill: &str, target: u8) -> Vec<String> {
    if skill != "magic" {
        return Vec::new();
    }
    let mut out = Vec::new();
    if target > ALCH_FROM_LEVEL {
        out.extend([NATURE_RUNE.to_string(), FIRE_RUNE.to_string(), FIRE_STAFF.to_string()]);
    }
    for item in crate::training_spells::kept_spell_inputs(target) {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

#[cfg(test)]
#[path = "training_tests.rs"]
mod tests;
