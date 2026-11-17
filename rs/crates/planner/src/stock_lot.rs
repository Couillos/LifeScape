//! Money lots on the bot's OWN stock (plan rendements R7, D5, decision
//! 2026-11-17: recipes on stock beyond alchemy): a bot that holds raw
//! fish, ores, bars, wool, hides... may choose "transform my stock and
//! sell" as a money line, its lot = its free stock. Pure, from the game
//! data (`method_way`):
//! - `stock_inputs`: what one action of the method consumes of the stock
//!   (its recipe inputs, never a rune: the cast's runes are consumables
//!   the kit stocks); `None` for a method whose run counts its batch
//!   otherwise (alchemy, teleports, bones to bananas, curses: a cast
//!   count), a gather or a fight (its bait, feathers, arrows are
//!   consumables) or no stock at all;
//! - `lot_of`: actions the stock covers (the scarcest input decides);
//! - `uses`: the `--use <item> N` goals of a lot of `batch` actions
//!   (`mapping`: the script stops once its lot of stock is used, or the
//!   stock is spent, `goal_reached`, `scripts::kit::stock_use`).

use lifescape_gamedata::obtain::WayKind;

/// Methods whose run takes its batch as a cast count (`mapping`).
const CAST_COUNTED: [&str; 4] = ["alch:", "teleport:", "bananas:", "curse:"];

/// Stock one action of `method` consumes, per input (module doc).
pub fn stock_inputs(method: &str) -> Option<Vec<(String, i64)>> {
    if CAST_COUNTED.iter().any(|prefix| method.starts_with(prefix)) || crate::trade_craft::is_trade(method) {
        return None;
    }
    let (_, way) = crate::method_way::method_way(method)?;
    // A recipe, a spell or a service on an object (tanning, filling,
    // milking, dyeing); never a gather, a drop or a pickup.
    if !matches!(way.kind, WayKind::Make | WayKind::Spell | WayKind::Harvest) {
        return None;
    }
    let inputs: Vec<(String, i64)> = way.inputs.into_iter().filter(|(item, _)| crate::method_rate_units::input_cause(item) == "input").map(|(item, units)| (item, units.max(1))).collect();
    (!inputs.is_empty()).then_some(inputs)
}

/// Actions of `method` the stock `held` covers (0: none, or no stock
/// method).
pub fn lot_of(method: &str, held: &dyn Fn(&str) -> f64) -> i64 {
    let Some(inputs) = stock_inputs(method) else { return 0 };
    inputs.iter().map(|(item, per_action)| (held(item).max(0.0).floor() as i64) / per_action).min().unwrap_or(0).max(0)
}

/// The `--use` goals of a lot of `batch` actions (empty for no batch or
/// no stock method).
pub fn uses(method: &str, batch: i64) -> Vec<(String, i64)> {
    if batch <= 0 {
        return Vec::new();
    }
    stock_inputs(method).unwrap_or_default().into_iter().map(|(item, per_action)| (item, per_action * batch)).collect()
}

#[cfg(test)]
mod tests {
    use super::{lot_of, stock_inputs, uses};

    fn held<'a>(stock: &'a [(&'a str, f64)]) -> impl Fn(&str) -> f64 + 'a {
        move |item: &str| stock.iter().filter(|(name, _)| *name == item).map(|(_, units)| *units).sum()
    }

    /// 200 raw trout: a lot of 200 cooks, whatever burns (the input is
    /// what the lot spends); a pair recipe is bound by its scarcest input;
    /// a smith's piece by the bars one piece takes.
    #[test]
    fn the_lot_is_what_the_stock_covers() {
        assert_eq!(lot_of("cook:trout", &held(&[("raw_trout", 200.0)])), 200);
        assert_eq!(uses("cook:trout", 200), vec![("raw_trout".to_string(), 200)]);
        assert_eq!(lot_of("smelt:bronze_bar", &held(&[("copper_ore", 200.0), ("tin_ore", 50.0)])), 50);
        assert_eq!(uses("smelt:bronze_bar", 50), vec![("copper_ore".to_string(), 50), ("tin_ore".to_string(), 50)]);
        assert_eq!(lot_of("smith:adamant_2h_sword", &held(&[("adamantite_bar", 28.0)])), 9);
        assert_eq!(lot_of("spin:ball_of_wool", &held(&[("wool", 0.0)])), 0);
    }

    /// Every family of transformation is a stock method, runes are never
    /// stock (an enchant's amulet is); a cast-counted spell or a farm is
    /// none.
    #[test]
    fn stock_methods_cover_every_transformation() {
        for method in ["cook:trout", "smelt:iron_bar", "smith:bronze_scimitar", "spin:ball_of_wool", "craft:leather_gloves", "tan:cow_hide", "pottery:pot_unfired", "cut:sapphire", "jewelry:gold_ring", "string:unstrung_gold_amulet", "runecraft:airrune", "fill:jug_empty", "dye:reddye", "superheat:iron_bar", "enchant:strung_sapphire_amulet"] {
            assert!(stock_inputs(method).is_some_and(|inputs| !inputs.is_empty()), "{method}");
        }
        assert_eq!(stock_inputs("superheat:iron_bar"), Some(vec![("iron_ore".to_string(), 1)]), "runes are no stock");
        assert_eq!(stock_inputs("enchant:strung_sapphire_amulet"), Some(vec![("strung_sapphire_amulet".to_string(), 1)]));
        for method in ["alch:iron_platebody", "bananas:bones", "gather:fishing/raw_trout", "trade:craft/smelt:bronze_bar"] {
            assert_eq!(stock_inputs(method), None, "{method}");
        }
    }
}
