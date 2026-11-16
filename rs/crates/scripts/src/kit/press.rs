//! Press: the Make 1/5/10 choice of the 274 grids (`inv_button1/2/3`
//! on the smithing columns and the jewellery moulds, `if_button` triples
//! on the leather grid). One press per load when the inputs bind, the
//! server chaining the items and stopping on them; never a press past
//! the order. Pure: the bricks that open the grids own the sends.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::pack::pack_total;
use crate::ScriptError;

/// How one pressed chain is followed: the input it eats, per item, the
/// items pressed, the silence that reads as a cut chain (level-up) and
/// the tail the last item still holds the player (`p_delay`).
pub struct Chain {
    pub input: u16,
    pub per_item: u32,
    pub items: u32,
    pub stall_ticks: u64,
    pub tail_ticks: u32,
}

/// Follow the server's chain after one press: over once the pressed
/// items are made or the input no longer makes one (`chain_over`), or
/// when `starved` says another input ran out (thread), plus the tail so
/// the next op never lands while the player is delayed; a silence of
/// `stall_ticks` first is a cut chain (the caller re-presses for the
/// rest). Items made by this press.
pub fn follow_chain(bot: &mut Bot, chain: &Chain, starved: impl Fn(&Bot) -> bool) -> Result<u32, ScriptError> {
    let start = pack_total(bot, chain.input);
    let mut left = start;
    let mut last_progress = bot.world.ticks;
    loop {
        bot.pump(1)?;
        let now = pack_total(bot, chain.input);
        if now < left {
            left = now;
            last_progress = bot.world.ticks;
        }
        let done = (start - left) / chain.per_item.max(1);
        if chain_over(done, chain.items, left, chain.per_item) || (done > 0 && starved(bot)) {
            bot.pump(chain.tail_ticks)?;
            return Ok(done);
        }
        if bot.world.ticks.saturating_sub(last_progress) >= chain.stall_ticks {
            return Ok(done);
        }
    }
}

/// The server's chain for one press is over: the pressed items are made,
/// or the input left no longer makes one (`smithing_anvil` and the
/// leather queue return). Pure.
pub fn chain_over(done: u32, items: u32, left: u32, per_item: u32) -> bool {
    done >= items || left < per_item.max(1)
}

/// Make press on a 274 Make 1/5/10 grid (`inv_button1/2/3`: smithing
/// columns, jewellery moulds), for `can_make` items the carried inputs
/// allow and `wanted` the order still asks. When the inputs bind
/// (`wanted >= can_make`) the server's own chain stops on them (« not
/// enough bars to make any more »): the smallest press covering them all
/// goes once (8 kiteshields from 24 bars = one Make 10, never 5+1+1+1,
/// live 2026-11-16: ~16 ticks per extra Make 1 visit). When the order
/// binds, the largest press within it (never past `--want`).
/// (button, items the press makes); (1, 0) when nothing is due. Pure.
pub fn make_press(can_make: u32, wanted: u32) -> (u8, u32) {
    let due = can_make.min(wanted);
    if due == 0 {
        return (1, 0);
    }
    if wanted >= can_make {
        return match due {
            1 => (1, 1),
            2..=5 => (2, due),
            _ => (3, due.min(10)),
        };
    }
    match due {
        10.. => (3, 10),
        5..=9 => (2, 5),
        _ => (1, 1),
    }
}

#[cfg(test)]
mod tests {
    use super::{chain_over, make_press};

    /// The drain ends on the server's own stop (items pressed, input out),
    /// never on a 12-tick silence after the last item (live 2026-11-16).
    #[test]
    fn chain_ends_on_items_or_inputs() {
        assert!(chain_over(8, 8, 0, 3), "24 bars, 8 kiteshields: done");
        assert!(chain_over(5, 5, 9, 3), "Make 5 pressed, 5 made");
        assert!(chain_over(3, 10, 2, 3), "bars out mid-press");
        assert!(!chain_over(3, 8, 15, 3), "still chaining");
        assert!(!chain_over(0, 1, 5, 5), "first item pending");
        assert!(chain_over(26, 26, 0, 1), "leather: the whole pack");
    }

    /// One press for a load the inputs bind (the server stops on them):
    /// 8 items = one Make 10, never 5 + 1 + 1 + 1; an order that binds
    /// never gets a press past it; nothing due presses nothing.
    #[test]
    fn make_press_covers_the_inputs_in_one_press() {
        assert_eq!(make_press(25, u32::MAX), (3, 10), "a full load chains ten, then re-presses");
        assert_eq!(make_press(8, u32::MAX), (3, 8), "24 bars of kiteshields: one Make 10");
        assert_eq!(make_press(7, u32::MAX), (3, 7));
        assert_eq!(make_press(4, u32::MAX), (2, 4), "Make 5 covers four");
        assert_eq!(make_press(1, u32::MAX), (1, 1));
        assert_eq!(make_press(6, 6), (3, 6), "bars withdrawn for the order: one press");
        assert_eq!(make_press(12, 12), (3, 10), "then (2, 2)");
        assert_eq!(make_press(2, 12), (2, 2));
        assert_eq!(make_press(25, 1), (1, 1), "--want 1 makes one");
        assert_eq!(make_press(25, 6), (2, 5), "the order binds: never past --want");
        assert_eq!(make_press(25, 3), (1, 1));
        assert_eq!(make_press(0, 10), (1, 0), "no input set, nothing due");
        assert_eq!(make_press(10, 0), (1, 0), "order met, nothing due");
    }

}
