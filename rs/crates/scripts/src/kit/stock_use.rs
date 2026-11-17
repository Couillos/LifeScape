//! Stock use: the run's `--use <objet> N` goal (repeatable), a money lot
//! on the bot's OWN stock (plan rendements R7, recipes on stock beyond
//! alchemy, 2026-11-17): transform N units of the stock the brain judged
//! free, then stop. The goal counts the INPUT consumed (pack + known
//! bank below its level at the start), never the product: a burnt fish,
//! a failed iron smelt, a pot cracked in the oven still spend their
//! input, the lot ends all the same.
//!
//! Two ends, both `goal_reached` (exit 0), never a failure:
//! - `use_met`: one `--use` object has dropped by N since the start
//!   (`cycle::poll_stop`, between two loads; a load already started
//!   finishes, like a full pack);
//! - `stock_spent`: a step stopped `Blocked` while one `--use` object is
//!   spent (none left in pack + known bank, and the run used at least
//!   one): the lot is the stock, the stock is gone (a lot judged on the
//!   brain's free stock that a sale or a lock shrank since). Whatever the
//!   script names as missing (pottery names `clay` when its soft clay is
//!   gone), the order is done (`cycle::run`, `spell_load::run_casts`).
//! A `Blocked` before any unit of the stock was used stays a block (no
//! stock at all: the brain's judgement was wrong, it must know).

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::cycle::{held_total, CycleState, TaskSpec};

/// Held totals of the `--use` objects at the start of the run, in the
/// order of `spec.targets.uses`.
pub fn bases(bot: &Bot, spec: &TaskSpec) -> Vec<u32> {
    spec.targets.uses.iter().map(|(item, _)| held_total(bot, item)).collect()
}

impl CycleState {
    /// True once one `--use` object has dropped by its count (false
    /// without `--use`).
    pub fn use_met(&self, bot: &Bot, spec: &TaskSpec) -> bool {
        spec.targets.uses.iter().zip(self.use_bases()).any(|((item, count), base)| base.saturating_sub(held_total(bot, item)) >= *count)
    }

    /// True when one `--use` object is spent: none left, at least one
    /// used by this run (module doc).
    pub fn stock_spent(&self, bot: &Bot, spec: &TaskSpec) -> bool {
        spec.targets.uses.iter().zip(self.use_bases()).any(|((item, _), base)| *base > 0 && held_total(bot, item) == 0)
    }
}

#[cfg(test)]
mod tests {
    use crate::kit::cycle::{parse_cli, parse_order, poll_stop, run, CycleState, StepControl, StopReason};
    use crate::kit::pack::obj_id;
    use crate::kit::spell_load::tests::stocked_bot;
    use crate::ScriptError;

    fn empty_pack(bot: &mut lifescape_sdk::Bot) {
        let pack = (bot.tables.com_id)("inventory:inv").expect("pack com");
        bot.world.inv.insert(pack, Default::default());
    }

    /// The money lot's order parses on every transformation script
    /// (`--use` repeatable) and is an objective (`goal_reached` at its
    /// end, never `budget_exhausted`).
    #[test]
    fn use_parses_as_an_objective() {
        let spec = parse_cli("cook", &["raw_trout", "--use", "raw_trout", "200"].map(String::from)).expect("cook parses");
        assert_eq!(spec.targets.uses, vec![("raw_trout".to_string(), 200)]);
        assert!(spec.targets.any());
        assert_eq!(crate::kit::cycle::ended_code_for(&spec, true), 0, "a met lot is the goal");
        let smelt = parse_order("smelt", &["bronze_bar", "--use", "copper_ore", "14", "--use", "tin_ore", "14", "--node", "n3"].map(String::from)).expect("smelt parses");
        assert_eq!(smelt.targets.uses, vec![("copper_ore".to_string(), 14), ("tin_ore".to_string(), 14)]);
        assert!(parse_cli("cook", &["--use", "raw_trout"].map(String::from)).is_err(), "a count is due");
    }

    /// 200 raw trout banked, `--use raw_trout 200`: 150 cooked is not the
    /// end, 200 is (burnt ones count: the input is what the lot spends).
    #[test]
    fn the_lot_ends_when_its_stock_is_used() {
        let mut bot = stocked_bot(&[], &[("raw_trout", 200)]);
        let spec = parse_cli("cook", &["raw_trout", "--use", "raw_trout", "200"].map(String::from)).expect("parses");
        let state = CycleState::begin(&bot, &spec);
        assert_eq!(poll_stop(&bot, &state, &spec), None);
        let pack = (bot.tables.com_id)("inventory:inv").expect("pack com");
        let trout = obj_id(&bot, "raw_trout").expect("trout");
        bot.world.inv_closed.clear();
        bot.world.inv.insert(pack, [(0u8, (trout, 50u32))].into_iter().collect());
        assert_eq!(poll_stop(&bot, &state, &spec), None, "50 left");
        assert!(!state.stock_spent(&bot, &spec));
        empty_pack(&mut bot);
        assert_eq!(poll_stop(&bot, &state, &spec), Some(StopReason::GoalReached));
        assert!(state.stock_spent(&bot, &spec));
    }

    /// A step blocked on its input once the stock is spent ends the run
    /// `goal_reached` (exit 0); blocked before any unit was used, the
    /// block stands (no stock at all).
    #[test]
    fn a_block_on_a_spent_stock_is_the_end_of_the_lot() {
        let mut bot = stocked_bot(&[("raw_trout", 1)], &[]);
        // The brain judged 200 free; a sale left one: the run cooks it.
        let spec = parse_cli("cook", &["raw_trout", "--use", "raw_trout", "200"].map(String::from)).expect("parses");
        let mut steps = 0;
        let ended = run(&mut bot, &spec, |step_bot, _| {
            steps += 1;
            if steps == 1 {
                empty_pack(step_bot);
                return Ok(StepControl::UnitDone);
            }
            Err(ScriptError::Blocked { missing: vec!["raw_trout".to_string()] })
        });
        assert_eq!(ended.expect("goal, not blocked"), 0);
        let mut bare = stocked_bot(&[], &[]);
        let blocked = run(&mut bare, &spec, |_, _| Err(ScriptError::Blocked { missing: vec!["raw_trout".to_string()] }));
        assert!(matches!(blocked, Err(ScriptError::Blocked { .. })), "no stock at all: {blocked:?}");
        // Pottery names clay when its soft clay is spent: still the end.
        let mut potter = stocked_bot(&[("softclay", 3)], &[]);
        let pots = parse_order("pottery", &["pot_unfired", "--use", "softclay", "3"].map(String::from)).expect("parses");
        let mut first = true;
        let ended = run(&mut potter, &pots, |step_bot, _| {
            if std::mem::take(&mut first) {
                empty_pack(step_bot);
                return Ok(StepControl::Continue);
            }
            Err(ScriptError::Blocked { missing: vec!["clay".to_string(), "water".to_string()] })
        });
        assert_eq!(ended.expect("pottery lot done"), 0);
    }
}
