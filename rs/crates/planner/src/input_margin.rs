//! Margin on the inputs of a random recipe. A method whose action
//! succeeds with odds below 1 (iron smelting, 50 %: the ore is lost on a
//! miss) burns a random number of inputs for a given output; sized at
//! the expected value the lot runs dry a few products before its goal
//! (live 2026-11-22: 142 ores for 71 bars made 69 bars, blocked on the
//! last 2). Every place that derives an input quantity for an obtain or
//! a trade lot sizes it through `sized_inputs`: expected inputs plus
//! 10 %, never less than one more input set. A certain recipe (success
//! 1) is sized at its expected value, unchanged. The leftover inputs
//! stay in the bank.

/// Extra share of inputs a random recipe is sized with.
pub const MARGIN: f64 = 0.10;

/// Inputs a lot needs: `expected` is the expected units (products times
/// inputs per action over the expected output), `per_set` the inputs one
/// action burns of this item, `success` the action's odds.
pub fn sized_inputs(expected: f64, per_set: i64, success: f64) -> i64 {
    let base = (expected - 1e-9).ceil().max(1.0) as i64;
    if success >= 1.0 {
        return base;
    }
    let margined = (expected * (1.0 + MARGIN) - 1e-9).ceil() as i64;
    margined.max(base.saturating_add(per_set.max(1)))
}

/// Odds that one action of the recipe `method` yields (1 when unknown).
pub fn recipe_success(method: &str) -> f64 {
    crate::method_way::method_way(method).map(|(_, way)| way.success).filter(|odds| *odds > 0.0).unwrap_or(1.0)
}

#[cfg(test)]
mod tests {
    use super::{recipe_success, sized_inputs};

    #[test]
    fn seventy_one_iron_bars_ask_at_least_157_ores() {
        // 71 bars at 0.5 per smelt: 142 expected ores.
        assert_eq!(sized_inputs(142.0, 1, 0.5), 157);
        assert_eq!(recipe_success("smelt:iron_bar"), 0.5);
    }

    #[test]
    fn a_certain_recipe_is_unchanged() {
        assert_eq!(sized_inputs(71.0, 1, 1.0), 71);
        assert_eq!(recipe_success("smelt:bronze_bar"), 1.0);
    }

    #[test]
    fn a_small_lot_gets_at_least_one_more_set() {
        // One bar: 2 expected ores, 10 % rounds to 3 = one set more.
        assert_eq!(sized_inputs(2.0, 1, 0.5), 3);
        // Two inputs per action: a whole extra action.
        assert_eq!(sized_inputs(4.0, 2, 0.5), 6);
    }
}
