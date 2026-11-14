//! Alchemy in the brain (user 2026-11-14: "pourquoi personne n'a l'idée
//! d'alch des objets ?"). High alchemy (magic 55) turns an object into
//! its `alch_high` coins, low alchemy (magic 21) into `alch_low`, for one
//! nature rune and five (high) or three (low) fire runes; a staff of fire
//! pays the fire runes. Spell runes and XP come from the server spell
//! table (`gamedata::spells`), alchemy values from the obj configs
//! (`cost × 0.6 / 0.4`).
//!
//! - Net value of one cast = alchemy value − nature rune − fire runes
//!   (market prices, never a shop): a caster never sells under it (the
//!   natural price floor of the market, `brain_reserve`).
//! - An object pays to alch when its net beats what selling it brings
//!   (best bid, else its current price): its own produce (a smith's steel
//!   platebodies), or an object bought under the net (an ask below it).
//! - Rate: one cast per `CAST_TICKS`; profit per 6000 ticks = casts ×
//!   (net − object price), magic XP per 6000 = casts × spell XP. The
//!   brain offers it as a money method (`alch:<object>`, script
//!   `alchemy <object>`) beside the gathering and coin spots.
//!
//! Pure: prices and levels come in as arguments.

/// Ticks between two casts of an alchemy spell (server animation delay).
pub const CAST_TICKS: f64 = 5.0;
/// Fewest casts an alchemy goal is worth starting for.
pub const MIN_ALCH_CASTS: i64 = 10;
/// Rune names of the alchemy spells and the staff that pays fire runes.
pub const NATURE_RUNE: &str = "naturerune";
pub const FIRE_RUNE: &str = "firerune";
pub const FIRE_STAFF: &str = "staff_of_fire";

/// One alchemy spell the caster can cast.
#[derive(Debug, Clone, PartialEq)]
pub struct AlchSpell {
    pub spell: &'static str,
    pub level: u8,
    /// Display XP per cast.
    pub xp: f64,
    /// Runes per cast, (name, count).
    pub runes: Vec<(String, i64)>,
    pub high: bool,
}

fn spell_row(spell: &'static str, level: u8, high: bool) -> AlchSpell {
    let row = lifescape_gamedata::spells::all_spells().into_iter().find(|row| row.spell == spell);
    let xp = row.as_ref().map(|row| row.xp as f64 / 10.0).unwrap_or(if high { 65.0 } else { 31.0 });
    let runes = row.map(|row| row.runes).filter(|runes| !runes.is_empty()).unwrap_or_else(|| {
        vec![(NATURE_RUNE.to_string(), 1), (FIRE_RUNE.to_string(), if high { 5 } else { 3 })]
    });
    AlchSpell { spell, level, xp, runes, high }
}

/// The best alchemy spell at `magic_level` (high at 55, low at 21).
pub fn alch_spell(magic_level: u8) -> Option<AlchSpell> {
    if magic_level >= 55 {
        Some(spell_row("highlvl_alchemy", 55, true))
    } else if magic_level >= 21 {
        Some(spell_row("lowlvl_alchemy", 21, false))
    } else {
        None
    }
}

/// What a cast costs and pays for one caster.
#[derive(Debug, Clone, PartialEq)]
pub struct Caster {
    pub spell: AlchSpell,
    /// Coins of the runes one cast burns (fire runes free with a staff).
    pub rune_cost: f64,
}

/// The caster at `magic_level`: rune prices from `price_of` (market,
/// never a shop), fire runes free when it holds a staff of fire.
pub fn caster(magic_level: u8, fire_staff: bool, price_of: &dyn Fn(&str) -> f64) -> Option<Caster> {
    let spell = alch_spell(magic_level)?;
    let rune_cost = spell
        .runes
        .iter()
        .filter(|(rune, _)| !(fire_staff && rune == FIRE_RUNE))
        .map(|(rune, count)| *count as f64 * price_of(rune).max(1.0))
        .sum();
    Some(Caster { spell, rune_cost })
}

/// Alchemy value of `item` for this spell (`None`: not alchable, coins,
/// or worth nothing).
pub fn alch_value(item: &str, high: bool) -> Option<i64> {
    if item == "coins" {
        return None;
    }
    let obj = lifescape_gamedata::objs_by_name(item).into_iter().next()?;
    let value = if high { obj.alch_high } else { obj.alch_low };
    (value > 0).then_some(value)
}

impl Caster {
    /// Coins one cast on `item` leaves after its runes.
    pub fn net(&self, item: &str) -> Option<f64> {
        alch_value(item, self.spell.high).map(|value| value as f64 - self.rune_cost)
    }

    /// Profit of alching one `item` worth `price` (what selling it, or
    /// buying it, costs): positive when the cast beats the market.
    pub fn profit(&self, item: &str, price: f64) -> Option<f64> {
        self.net(item).map(|net| net - price.max(0.0))
    }
}

/// An alchemy the bot should run: the object, how many casts, profit and
/// XP per 6000 ticks.
#[derive(Debug, Clone, PartialEq)]
pub struct AlchPick {
    pub item: String,
    pub casts: i64,
    pub profit_each: f64,
    pub profit_per_6000: f64,
    pub xp_per_6000: f64,
}

/// Casts per 6000 ticks.
pub fn casts_per_6000() -> f64 {
    6000.0 / CAST_TICKS
}

/// The best alchemy among `stock` (object, free units, what one sells
/// for now): the highest profit per cast with at least `MIN_ALCH_CASTS`
/// units, `None` when nothing beats selling.
pub fn best_alch(caster: &Caster, stock: &[(String, i64, f64)]) -> Option<AlchPick> {
    let mut best: Option<AlchPick> = None;
    for (item, count, price) in stock {
        if *count < MIN_ALCH_CASTS {
            continue;
        }
        let Some(profit) = caster.profit(item, *price) else { continue };
        if profit <= 0.0 {
            continue;
        }
        let pick = AlchPick {
            item: item.clone(),
            casts: *count,
            profit_each: profit,
            profit_per_6000: profit * casts_per_6000(),
            xp_per_6000: caster.spell.xp * casts_per_6000(),
        };
        if best.as_ref().is_none_or(|known| pick.profit_each > known.profit_each) {
            best = Some(pick);
        }
    }
    best
}

/// Highest price a caster pays for `item` to alch it: the net less the
/// time of one cast at `coin_rate` (never a loss), `None` when the cast
/// itself loses.
pub fn alch_bid_ceiling(caster: &Caster, item: &str, coin_rate: f64) -> Option<i64> {
    let ceiling = caster.net(item)? - CAST_TICKS * coin_rate.max(0.0);
    (ceiling >= 1.0).then(|| ceiling.floor() as i64)
}

#[cfg(test)]
#[path = "alchemy_tests.rs"]
mod tests;
