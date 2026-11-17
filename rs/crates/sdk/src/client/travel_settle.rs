//! Settle after a scripted traversal (plan P9, 2026-11-17).
//!
//! A toll gate, a teleporting door or a ride lands the player through a
//! server script that still holds it after the arrival: the Al Kharid
//! gate (`border_gate.rs2`) ends on `p_teleport; p_stopaction;
//! p_delay(0)`, so the player stays delayed until the next tick's script
//! pass, and a MOVE the client sends in that window is dropped. Live:
//! 406 "step refused" dossiers at (3267,3227), the leg after "You pay the
//! guard." sent one tick after the arrival. The executor now waits
//! `SCRIPT_SETTLE_TICKS` after any scripted arrival before its next
//! send: the delay(0) tick, then the tick its packets are read in.

use super::Bot;
use crate::error::Result;

/// Ticks a scripted arrival holds the player (delay(0) clears on the
/// next tick's script pass, after that tick's client packets).
pub(crate) const SCRIPT_SETTLE_TICKS: u64 = 2;

/// Pure: first tick a walk may be sent after an arrival seen at
/// `arrived`.
pub(crate) fn settled_at(arrived: u64) -> u64 {
    arrived + SCRIPT_SETTLE_TICKS
}

impl Bot {
    /// Pump until a scripted arrival seen now has released the player.
    pub(crate) fn settle_after_script(&mut self) -> Result<()> {
        let until = settled_at(self.world.ticks);
        while self.world.ticks < until {
            self.pump(1)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{settled_at, SCRIPT_SETTLE_TICKS};

    /// P9: a toll arrival seen at tick T leaves the next walk to T+2 (the
    /// delay(0) tick T+1 would drop it).
    #[test]
    fn a_scripted_arrival_settles_two_ticks() {
        assert_eq!(SCRIPT_SETTLE_TICKS, 2);
        assert_eq!(settled_at(100), 102);
        assert!(settled_at(100) > 101, "never on the delay(0) tick");
    }
}
