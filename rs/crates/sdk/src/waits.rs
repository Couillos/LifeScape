//! Tick-based waits: pacing in server ticks behaves the same whatever the
//! tick length. `world.ticks` advances on every player-info packet, which
//! the server sends each tick while logged in.
//!
//! Tick length follows the server: 60 ms in dev, 600 ms in prod, set by
//! `NODE_TICK_RATE` (default 600, the rate the server ships with). Waits
//! count ticks, never milliseconds, so dev and prod behave the same; only
//! failure dossiers annotate a wall-clock duration, via `tick_ms()`.

use crate::client::Bot;
use crate::error::Result;

/// Real server tick length in milliseconds: `NODE_TICK_RATE` when set,
/// 600 (prod default) otherwise. Dev runs the server with 60.
pub fn tick_ms() -> u64 {
    std::env::var("NODE_TICK_RATE")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|rate| *rate > 0)
        .unwrap_or(600)
}

/// Pump until `n` ticks pass.
pub fn wait_ticks(bot: &mut Bot, n: u64) -> Result<()> {
    wait_ticks_pump(bot, n, 2)
}

fn wait_ticks_pump(bot: &mut Bot, n: u64, per_pump: u32) -> Result<()> {
    let target = bot.world.ticks + n;
    while bot.world.ticks < target {
        bot.pump(per_pump)?;
    }
    Ok(())
}

/// Pump until hitpoints reach `target`, or the tick bound. True when healed.
pub fn wait_hitpoints(bot: &mut Bot, target: u8, timeout: u64) -> Result<bool> {
    let deadline = bot.world.ticks + timeout;
    while bot.world.ticks < deadline {
        bot.pump(5)?;
        if bot.world.stats.get(&3).is_some_and(|(level, _)| *level >= target) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Pump until the backpack com streams after (re)login. Acting on an
/// unstreamed empty pack misreads the whole inventory.
pub fn wait_backpack(bot: &mut Bot, timeout: u64) -> Result<bool> {
    let (com, _) = bot.backpack();
    if com.is_some() {
        return Ok(true);
    }
    let deadline = bot.world.ticks + timeout;
    while bot.world.ticks < deadline {
        bot.pump(5)?;
        if bot.backpack().0.is_some() {
            return Ok(true);
        }
    }
    Ok(bot.backpack().0.is_some())
}

#[cfg(test)]
mod tests {
    use super::tick_ms;

    /// The annotated tick length tracks the server rate, defaulting to
    /// the 600 ms prod rate when the variable is absent or invalid.
    #[test]
    fn tick_ms_follows_node_tick_rate() {
        let prior = std::env::var("NODE_TICK_RATE").ok();
        std::env::set_var("NODE_TICK_RATE", "60");
        assert_eq!(tick_ms(), 60);
        std::env::set_var("NODE_TICK_RATE", "bogus");
        assert_eq!(tick_ms(), 600);
        std::env::remove_var("NODE_TICK_RATE");
        assert_eq!(tick_ms(), 600);
        if let Some(value) = prior {
            std::env::set_var("NODE_TICK_RATE", value);
        }
    }
}
