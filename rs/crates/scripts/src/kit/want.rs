//! Want: the run's `--want <objet> N` goal read INSIDE a load, not only
//! between loads. `cycle::poll_stop` ends the run once N more units are
//! held (pack + known bank, the `--want` convention); these reads let the
//! work loops stop the moment the goal is met instead of finishing the
//! stand, the pack or the make-X chain first (live admin 2026-11-17:
//! `fishing --want raw_lobster 2` caught 11 lobsters, the stand chained
//! until the spot hopped).
//!
//! One primitive, every loop: gather stands (`harvest::watch_stand`),
//! the gather load driver (`load::run_load`), the produce core
//! (`produce::run_job`), the fight loop (`hunt::HuntStop`) and the
//! per-item loops of the thin scripts (cutting, stringing, sewing,
//! milling, ground sweeps, essence) all read `load_done` / `left` /
//! `cap`. Without `--want` (`None`) nothing changes: a load ends on a
//! full pack and a job on its own count, as before.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::cycle::{CycleState, TaskSpec, WantGoal};
use crate::kit::pack::{banked_usable, free_slots, pack_total};

impl WantGoal {
    /// Units still due: the goal's held total minus pack + known bank
    /// (0 once met).
    pub fn left(&self, bot: &Bot) -> u32 {
        self.held.saturating_sub(pack_total(bot, self.obj).saturating_add(banked_usable(bot, self.obj)))
    }
}

impl CycleState {
    /// True once the run's `--want` goal is met (false without one).
    pub fn want_met(&self, bot: &Bot, spec: &TaskSpec) -> bool {
        met(self.want_goal(bot, spec), bot)
    }

    /// Units of the `--want` object still due (`u32::MAX` without one).
    pub fn want_left(&self, bot: &Bot, spec: &TaskSpec) -> u32 {
        left(self.want_goal(bot, spec), bot)
    }
}

/// True once the goal is met; never without a goal.
pub fn met(goal: Option<WantGoal>, bot: &Bot) -> bool {
    goal.is_some_and(|goal| goal.left(bot) == 0)
}

/// Units still due; `u32::MAX` (unbounded) without a goal.
pub fn left(goal: Option<WantGoal>, bot: &Bot) -> u32 {
    goal.map_or(u32::MAX, |goal| goal.left(bot))
}

/// `count` bounded by the units still due (unchanged without a goal):
/// a make-X answer, a withdrawal, a press never asks past the order.
pub fn cap(count: u32, goal: Option<WantGoal>, bot: &Bot) -> u32 {
    count.min(left(goal, bot))
}

/// Inputs that cover `due` outputs when one input yields `per_input`
/// (runes per essence at the level): rounded up, never 0 while due,
/// unbounded (`u32::MAX`) passes through. Pure.
pub fn inputs_for(due: u32, per_input: u32) -> u32 {
    if due == u32::MAX {
        return u32::MAX;
    }
    due.div_ceil(per_input.max(1))
}

/// A transformation stage's input bound and the goal it re-reads per
/// click (`produce::run_job`). A goal on one of the stage's `products`
/// is live: the stage stops the moment it is met and every make-X answer
/// asks only what is still due (a burnt fish, a failed iron smelt keep
/// it going, never past N). A goal on a later stage (soft clay shaped
/// for a fired order) caps this one once, one unit in per unit due.
/// No goal: the stage's own `target` alone.
pub fn stage_bounds(target: u32, products: &[u16], goal: Option<WantGoal>, bot: &Bot) -> (u32, Option<WantGoal>) {
    match goal {
        Some(goal) if products.contains(&goal.obj) => (target, Some(goal)),
        Some(goal) => (target.min(goal.left(bot)), None),
        None => (target, None),
    }
}

/// True while a live goal still wants units (false without one: the
/// count alone ends a make-X chain, as before).
pub fn wants_more(goal: Option<WantGoal>, bot: &Bot) -> bool {
    goal.is_some() && !met(goal, bot)
}

/// A gather load is done: the pack is full, or the run's `--want` is met
/// (the caller banks it and the cycle stops, never a stand, a pack or a
/// chain past N).
pub fn load_done(bot: &Bot, goal: Option<WantGoal>) -> bool {
    free_slots(bot) == 0 || met(goal, bot)
}

#[cfg(test)]
mod tests {
    use super::{cap, inputs_for, left, load_done, met, stage_bounds, wants_more};
    use crate::kit::cycle::{parse_cli, CycleState, WantGoal};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    fn id(bot: &Bot, name: &str) -> u16 {
        crate::kit::pack::obj_id(bot, name).unwrap_or_else(|_| panic!("{name} resolves"))
    }

    /// Fill the pack: `stacks` of (object, count) one slot each.
    fn carry(bot: &mut Bot, stacks: &[(u16, u32)]) {
        let pack = (bot.tables.com_id)("inventory:inv").expect("inventory");
        bot.world.inv.insert(pack, stacks.iter().enumerate().map(|(slot, stack)| (slot as u8, *stack)).collect());
    }

    /// Live admin 2026-11-17: `fishing raw_lobster --want raw_lobster 2`
    /// banked 11 lobsters. Pot + coins carried, no lobster at the start:
    /// the first lobster keeps the load going, the second ends it with
    /// 25 free slots.
    #[test]
    fn fishing_want_two_stops_at_two() {
        let mut bot = offline_bot();
        let (lobster, pot, coins) = (id(&bot, "raw_lobster"), id(&bot, "lobster_pot"), id(&bot, "coins"));
        carry(&mut bot, &[(pot, 1), (coins, 30)]);
        let spec = parse_cli("fishing", &["raw_lobster", "--want", "raw_lobster", "2"].map(String::from)).expect("parses");
        let state = CycleState::begin(&bot, &spec);
        let goal = state.want_goal(&bot, &spec);
        assert_eq!(goal, Some(WantGoal { obj: lobster, held: 2 }));
        carry(&mut bot, &[(pot, 1), (coins, 30), (lobster, 1)]);
        assert!(!load_done(&bot, goal), "1 of 2: keep fishing");
        assert_eq!(state.want_left(&bot, &spec), 1);
        carry(&mut bot, &[(pot, 1), (coins, 30), (lobster, 1), (lobster, 1)]);
        assert!(load_done(&bot, goal), "2 of 2: the stand ends, 24 slots free");
        assert!(state.want_met(&bot, &spec));
        assert_eq!(state.want_left(&bot, &spec), 0);
    }

    /// Mining and woodcut: the goal counts the bank too (ores banked by
    /// an earlier load of the same run) and only the ordered object.
    #[test]
    fn mining_and_woodcut_count_pack_and_bank() {
        let mut bot = offline_bot();
        let (ore, logs, oak) = (id(&bot, "iron_ore"), id(&bot, "logs"), id(&bot, "oak_logs"));
        let bank = (bot.tables.com_id)("bank_main:inv").expect("bank");
        let ore_goal = Some(WantGoal { obj: ore, held: 5 });
        bot.world.inv.insert(bank, [(0u8, (ore, 3u32))].into_iter().collect());
        carry(&mut bot, &[(ore, 1)]);
        assert!(!load_done(&bot, ore_goal), "4 of 5 held");
        carry(&mut bot, &[(ore, 1), (ore, 1)]);
        assert!(load_done(&bot, ore_goal), "3 banked + 2 carried");
        let oak_goal = Some(WantGoal { obj: oak, held: 1 });
        carry(&mut bot, &[(logs, 1), (logs, 1)]);
        assert!(!load_done(&bot, oak_goal), "plain logs never count for oak");
        carry(&mut bot, &[(logs, 1), (oak, 1)]);
        assert!(load_done(&bot, oak_goal));
    }

    /// No `--want`: a load still ends only on a full pack, `left` and
    /// `cap` never bound anything.
    #[test]
    fn without_want_nothing_changes() {
        let mut bot = offline_bot();
        let logs = id(&bot, "logs");
        carry(&mut bot, &[(logs, 1); 27]);
        assert!(!load_done(&bot, None), "27 of 28: keep cutting");
        carry(&mut bot, &[(logs, 1); 28]);
        assert!(load_done(&bot, None), "full pack");
        assert!(!met(None, &bot));
        assert_eq!(left(None, &bot), u32::MAX);
        assert_eq!(cap(27, None, &bot), 27);
        let spec = parse_cli("woodcut", &["logs".to_string()]).expect("parses");
        let state = CycleState::begin(&bot, &spec);
        assert!(!state.want_met(&bot, &spec));
        assert_eq!(state.want_left(&bot, &spec), u32::MAX);
    }

    /// `--want` binds every transformation (`produce::run_job`): a goal
    /// on the stage's product is re-read per click (spin want 3 with 1
    /// ball held asks a make-X of 2 from 27 wool, then nothing; cook's
    /// burnt fish never count), a goal on a later stage caps this one
    /// once (2 fired pie dishes due: 2 soft clay shaped, never 27); no
    /// goal keeps the stage's own count.
    #[test]
    fn want_bounds_every_transformation() {
        let mut bot = offline_bot();
        let (wool, ball, shrimps) = (id(&bot, "wool"), id(&bot, "ball_of_wool"), id(&bot, "shrimps"));
        let burnt_name = crate::kit::produce::cook_recipe("raw_shrimp").and_then(|recipe| recipe.burnt).expect("burnt shrimp");
        let burnt = id(&bot, &burnt_name);
        let (unfired, fired) = (id(&bot, "piedish_unfired"), id(&bot, "piedish"));
        let mut stacks: Vec<(u16, u32)> = vec![(wool, 1); 27];
        stacks.push((ball, 1));
        carry(&mut bot, &stacks);
        let goal = Some(WantGoal { obj: ball, held: 3 });
        assert_eq!(stage_bounds(u32::MAX, &[ball], goal, &bot), (u32::MAX, goal), "live goal on the product");
        assert_eq!(cap(u32::MAX, goal, &bot), 2, "make-X of 2, never 27");
        assert!(wants_more(goal, &bot));
        assert_eq!(stage_bounds(u32::MAX, &[ball], None, &bot), (u32::MAX, None), "no --want: every carried wool");
        assert!(!wants_more(None, &bot), "no goal: the count alone ends a chain");
        assert_eq!(stage_bounds(5, &[ball], None, &bot).0, 5, "the stage's own count still binds");
        let cooked = Some(WantGoal { obj: shrimps, held: 2 });
        assert_eq!(stage_bounds(u32::MAX, &[shrimps, burnt], cooked, &bot).1, cooked, "burnt never meets the order: live per click");
        let fired_goal = Some(WantGoal { obj: fired, held: 2 });
        assert_eq!(stage_bounds(u32::MAX, &[unfired], fired_goal, &bot), (2, None), "fired order: shape only the 2 due");
        carry(&mut bot, &[(ball, 3)]);
        assert!(!wants_more(goal, &bot), "met: the chain ends");
    }

    /// Runecraft: one altar click binds every carried essence, so the
    /// load carries only the essence the due runes need (3 air runes per
    /// essence at level 22: 7 due = 3 essence, 9 runes, the least past N).
    #[test]
    fn inputs_cover_the_due_outputs() {
        let per = lifescape_gamedata::action_odds::runes_per_essence("airrune", 22).expect("air multiplier") as u32;
        assert_eq!(per, 3);
        assert_eq!(inputs_for(7, per), 3);
        assert_eq!(inputs_for(6, per), 2);
        assert_eq!(inputs_for(1, 1), 1);
        assert_eq!(inputs_for(0, 3), 0, "nothing due, nothing carried");
        assert_eq!(inputs_for(u32::MAX, 3), u32::MAX, "no --want: a full pack");
    }

    /// `cap` bounds a count (make-X answer, withdrawal) by what is due.
    #[test]
    fn cap_bounds_by_what_is_due() {
        let mut bot = offline_bot();
        let ball = id(&bot, "ball_of_wool");
        let goal = Some(WantGoal { obj: ball, held: 3 });
        carry(&mut bot, &[(ball, 1)]);
        assert_eq!(cap(27, goal, &bot), 2, "27 wool carried, 2 balls due");
        assert_eq!(cap(1, goal, &bot), 1);
        carry(&mut bot, &[(ball, 1), (ball, 1), (ball, 1), (ball, 1)]);
        assert_eq!(cap(27, goal, &bot), 0, "past the goal: nothing due");
    }
}
