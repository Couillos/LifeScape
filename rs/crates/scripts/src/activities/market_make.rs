//! `market make --post x,z,level [--budget-max N] [--node ID]`: a market
//! maker's post (plan marché Q6, wave 3). The daemon quotes the maker's
//! slice for it (`orchestrator::market_maker`, public data only); this
//! script only stands at its post by the Varrock fountain for the
//! engagement window: the market pauses deliver its deals inline (the
//! physical handover of every bot), it walks back to the post after a
//! delivery took it away, eats when hurt, and exits on budget so the
//! session reports `budget_exhausted`. Stateless: the post rides in the
//! arguments.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::survival::{eat, hitpoints, max_hitpoints, should_eat, EAT_FRACTION};
use crate::{ScriptError, ScriptResult};

/// Default engagement window (ticks): one money window.
pub const MAKE_BUDGET_TICKS: u64 = 6000;
/// Tiles a delivery may leave the maker off its post before it walks back.
pub const POST_RADIUS: i32 = 6;
/// Ticks between two checks of the post (deliveries run inside the pump).
const CHECK_TICKS: u32 = 5;

/// Parsed `market make` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MakeOrder {
    pub post: (i32, i32, i32),
    pub budget_max: u64,
}

fn usage() -> ScriptError {
    ScriptError::Usage("market make --post x,z,level [--budget-max N] [--node ID]".to_string())
}

/// `market make` arguments: the post is required (the brain's, never
/// guessed), the window defaults to `MAKE_BUDGET_TICKS`.
pub fn parse_make(args: &[String]) -> Result<MakeOrder, ScriptError> {
    let mut post = None;
    let mut budget_max = MAKE_BUDGET_TICKS;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--post" => {
                let raw = args.get(index + 1).ok_or_else(usage)?;
                let cells: Vec<i32> = raw.split(',').map(|cell| cell.trim().parse::<i32>()).collect::<Result<_, _>>().map_err(|_| usage())?;
                let [x, z, level] = cells[..] else { return Err(usage()) };
                post = Some((x, z, level));
                index += 2;
            }
            "--budget-max" => {
                budget_max = args.get(index + 1).and_then(|raw| raw.parse().ok()).ok_or_else(usage)?;
                index += 2;
            }
            "--node" | "--budget-min" => index += 2,
            _ => return Err(usage()),
        }
    }
    Ok(MakeOrder { post: post.ok_or_else(usage)?, budget_max })
}

fn off_post(bot: &Bot, post: (i32, i32, i32)) -> bool {
    bot.world.level != post.2 || (bot.world.x - post.0).abs() > POST_RADIUS || (bot.world.z - post.1).abs() > POST_RADIUS
}

pub fn run_make(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let order = parse_make(args)?;
    let (x, z, level) = order.post;
    bot.log(&format!("make: post ({x},{z},{level}) for {} ticks", order.budget_max));
    bot.travel_to(lifescape_sdk::plan::Tile { x, z, level })?;
    let start_tick = bot.world.ticks;
    let start_pause = bot.pause_ticks;
    loop {
        bot.pump(CHECK_TICKS)?;
        if should_eat(hitpoints(bot), max_hitpoints(bot), EAT_FRACTION) {
            eat(bot)?;
        }
        let work = bot.world.ticks.saturating_sub(start_tick).saturating_sub(bot.pause_ticks.saturating_sub(start_pause));
        if work >= order.budget_max {
            break;
        }
        if off_post(bot, order.post) {
            bot.travel_to(lifescape_sdk::plan::Tile { x, z, level })?;
        }
    }
    Ok(crate::kit::cycle::BUDGET_EXIT)
}

#[cfg(test)]
mod tests {
    use super::{parse_make, MakeOrder, MAKE_BUDGET_TICKS};

    fn strings(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|text| text.to_string()).collect()
    }

    #[test]
    fn the_post_rides_in_the_arguments() {
        let order = parse_make(&strings(&["make", "--post", "3210,3427,0", "--node", "n4", "--budget-max", "9000"])).expect("parses");
        assert_eq!(order, MakeOrder { post: (3210, 3427, 0), budget_max: 9000 });
        let plain = parse_make(&strings(&["make", "--post", "3216,3431,0"])).expect("parses");
        assert_eq!(plain.budget_max, MAKE_BUDGET_TICKS);
        assert!(parse_make(&strings(&["make"])).is_err(), "no post: never guessed");
        assert!(parse_make(&strings(&["make", "--post", "3210,3427"])).is_err(), "x,z,level");
        assert!(parse_make(&strings(&["make", "--post", "3210,3427,0", "feather"])).is_err(), "no item: the daemon quotes the slice");
    }
}
