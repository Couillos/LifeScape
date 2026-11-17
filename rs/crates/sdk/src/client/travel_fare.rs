//! Coins a trip will take from the pack, decided by the planned route
//! (user rule 2026-11-17: a bot takes travel money only when its trip
//! really costs money, and only that amount). The plan is asked with the
//! fares payable (paid paths open, every `coins:<n>` tag held): the route
//! the run walks once it carries them. `nav::fare::route_fare` sums its
//! toll doors and fare edges; a route without any costs 0.

use lifescape_nav::plan::Tile;

use super::travel::RemoteOutcome;
use crate::client::Bot;
use crate::error::{Error, Result};

impl Bot {
    /// Coins the route `from` → `dest` takes from the pack once the
    /// fares are carried: the Al Kharid gate, the Karamja boat, 0 for a
    /// free route. A route the planner refuses is the walk's own typed
    /// failure (`fail_no_route`), never a guessed amount.
    pub fn route_fare(&mut self, from: Tile, dest: Tile) -> Result<u32> {
        if from == dest {
            return Ok(0);
        }
        let mut options = self.nav_options_remote()?;
        options.allow_paid = true;
        if let Some((_, tags)) = &self.wants_cache {
            options.satisfied.extend(tags.iter().filter(|tag| tag.starts_with("coins:")).cloned());
        }
        match self.plan_remote(&options, from, dest) {
            RemoteOutcome::Ready(plan, _) => Ok(lifescape_nav::fare::route_fare(&plan)),
            RemoteOutcome::NoRoute(no) => Err(self.fail_no_route(from, dest, &options, no)),
            RemoteOutcome::Interrupted(message) => Err(Error::Interrupted(message)),
            RemoteOutcome::NavDown(reason) => Err(Error::Travel(format!("nav-down: {reason}"))),
            RemoteOutcome::Session(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::client::Bot;
    use lifescape_nav::plan::Tile;

    /// The bot's own tile costs nothing, with no service at all.
    #[test]
    fn a_trip_to_the_same_tile_costs_nothing() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        let tile = Tile { x: 3222, z: 3218, level: 0 };
        assert_eq!(bot.route_fare(tile, tile).expect("no plan needed"), 0);
    }

    /// No plan service: `nav-down`, never a guessed fare.
    #[test]
    fn no_service_is_nav_down_never_a_guess() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        let error = bot.route_fare(Tile { x: 3222, z: 3218, level: 0 }, Tile { x: 3270, z: 3167, level: 0 }).expect_err("no service");
        assert!(error.to_string().contains("nav-down"), "{error}");
    }
}
