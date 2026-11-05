//! Tick-based waits: pacing in server ticks behaves the same at 600 ms
//! as at 20 ms tick rates. `world.ticks` advances on every player-info
//! packet, which the server sends each tick while logged in.

use crate::client::Bot;
use crate::error::Result;

/// 20 ms server tick length; matches node.tickRate in world.json.
pub const TICK_MS: u64 = 20;

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
