//! Survival: eating, death tally and the safe-point predicate. Every
//! archetype needs it (Wilderness open to all): eat under a threshold,
//! count consecutive deaths, resume after respawn, and expose the single
//! place that knows what "safe to pause" means for the market pause and
//! the end of engagement (L2 wires the callers).

#![forbid(unsafe_code)]

use lifescape_sdk::waits::wait_backpack;
use lifescape_sdk::Bot;

use crate::{ScriptError, ScriptResult};

/// Eat when hp falls to this fraction of the tracked max (fight loop
/// owns the max; beginners without food bank before hunting instead).
pub const EAT_FRACTION: f64 = 0.55;
/// Food carried after a bank restock.
pub const FOOD_TARGET: usize = 10;
/// Ticks waited flat after a death before resuming (respawn + resync).
pub const DEATH_WAIT_TICKS: u64 = 40;

/// True when hp sits at or under the eat line.
pub fn should_eat(hp: u8, max_hp: u8, fraction: f64) -> bool {
    f64::from(hp) <= f64::from(max_hp) * fraction
}

/// Eat one bite, wasting the fewest hitpoints (`kit::meals`): the
/// biggest heal that fits the missing hitpoints, else the smallest heal.
/// True when something was eaten (alcoholic drinks are never food).
pub fn eat(bot: &mut Bot) -> Result<bool, ScriptError> {
    let foods = crate::kit::meals::pack_foods(bot);
    let heals: Vec<i64> = foods.iter().map(|(_, _, heal)| *heal).collect();
    let (hp, max_hp) = (hitpoints(bot), max_hitpoints(bot).max(hitpoints(bot)));
    let pick = crate::kit::meals::pick_food(hp, max_hp, &heals, false)
        .or_else(|| heals.iter().enumerate().min_by_key(|(_, heal)| **heal).map(|(index, _)| index));
    let Some((slot, held, _)) = pick.and_then(|index| foods.get(index).copied()) else {
        return Ok(false);
    };
    let Some(pack_com) = bot.backpack().0 else {
        return Ok(false);
    };
    bot.op_held(1, held, slot, pack_com)?;
    bot.pump(3)?;
    Ok(true)
}

/// Edible count in the pack.
pub fn food_count(bot: &Bot) -> u32 {
    bot.backpack()
        .1
        .into_iter()
        .filter(|(_, (held, _))| {
            (bot.tables.obj_name)(*held).is_some_and(|name| lifescape_gamedata::food::heal(&name) > 0)
        })
        .map(|(_, (_, count))| count)
        .sum()
}

/// Withdraw the best banked food up to the target. Returns food carried.
pub fn withdraw_food(bot: &mut Bot) -> Result<u32, ScriptError> {
    withdraw_food_to(bot, FOOD_TARGET as u32)
}

/// Withdraw banked food up to `target` units (the hunt's sized load),
/// the food that fits the bot's health first (`meals::carry_rank`: heals
/// up to half its max hitpoints, best first; oversized heals last,
/// smallest first), the next food when one runs short. Returns food
/// carried.
pub fn withdraw_food_to(bot: &mut Bot, target: u32) -> Result<u32, ScriptError> {
    let max_hp = max_hitpoints(bot);
    let mut banked: Vec<(i64, u16)> = bot
        .bank_items()
        .keys()
        .filter_map(|id| (bot.tables.obj_name)(*id).map(|name| (lifescape_gamedata::food::heal(&name), *id)))
        .filter(|(heal, _)| *heal > 0)
        .collect();
    let rank = |heal: i64| crate::kit::meals::carry_rank(heal, max_hp);
    banked.sort_by(|left, right| rank(left.0).cmp(&rank(right.0)).then(left.1.cmp(&right.1)));
    for (_, food_id) in banked {
        let need = target.saturating_sub(food_count(bot));
        if need == 0 || crate::kit::pack::free_slots(bot) == 0 {
            break;
        }
        let _ = bot.bank_withdraw(food_id, need, 50);
    }
    Ok(food_count(bot))
}

/// True when the bank itself holds edible food: a foodless pack is only
/// worth a bank trip when the bank can restock it. A broke beginner
/// (empty bank) otherwise ping-pongs bank<->field after every kill and
/// dies crossing the map instead of hunting.
pub fn bank_stocks_food(bot: &Bot) -> bool {
    let banked: Vec<String> =
        bot.bank_items().keys().filter_map(|id| (bot.tables.obj_name)(*id)).collect();
    let refs: Vec<&str> = banked.iter().map(String::as_str).collect();
    lifescape_gamedata::food::best_food(&refs).is_some()
}

/// Deaths a risk camp (`combat_spots::risk_spot`: ice camps, demons)
/// may cost within `RISK_WINDOW_TICKS` before it leaves the run: the
/// guard against a death loop (user 2026-11-20: a death there is the
/// price of the drops, three in half an hour is a camp out of reach).
pub const RISK_DEATHS: usize = 3;
/// Window of the risk guard, in ticks (30 minutes at 600 ms a tick).
pub const RISK_WINDOW_TICKS: u64 = 3000;

/// Run-local death tally: consecutive deaths (a kill or a bank trip is
/// progress and resets the streak) plus the killer spots excluded from
/// every later re-pick. Local to one run, never global, never persisted:
/// an ordinary spot that killed once stays out for the run; a risk camp
/// stays in (the bot re-gears at the bank and goes back) until it kills
/// `RISK_DEATHS` times within `RISK_WINDOW_TICKS`.
pub struct DeathTally {
    deaths: u32,
    excluded: Vec<&'static str>,
    /// Risk-camp deaths of the window (spot, tick).
    risk_deaths: Vec<(&'static str, u64)>,
}

impl DeathTally {
    pub fn fresh() -> DeathTally {
        DeathTally { deaths: 0, excluded: Vec::new(), risk_deaths: Vec::new() }
    }

    pub fn excludes(&self, spot_key: &str) -> bool {
        self.excluded.iter().any(|key| *key == spot_key)
    }

    /// Raw consecutive-death count (the typed error quotes it).
    pub fn deaths(&self) -> u32 {
        self.deaths
    }

    /// Killer spots excluded from every later re-pick.
    pub fn excluded(&self) -> &[&'static str] {
        &self.excluded
    }

    /// Exclude a spot without a death (auto-mode flank fallback).
    pub fn exclude(&mut self, spot_key: &'static str) {
        if !self.excludes(spot_key) {
            self.excluded.push(spot_key);
        }
    }

    pub fn fatal(&self) -> bool {
        self.deaths >= 2
    }

    /// A kill or a bank trip is progress: the next death starts a new
    /// streak. Exclusions stay: a spot that killed once stays out.
    pub fn note_progress(&mut self) {
        self.deaths = 0;
    }

    /// A death at `spot_key` on tick `now`: one more in the streak; the
    /// spot leaves the run, unless it is a risk camp still under its
    /// guard (`RISK_DEATHS` within `RISK_WINDOW_TICKS`).
    pub fn note_death(&mut self, spot_key: &'static str, now: u64) {
        self.deaths += 1;
        if lifescape_gamedata::combat_spots::risk_spot(spot_key) {
            self.risk_deaths.retain(|(_, at)| now.saturating_sub(*at) < RISK_WINDOW_TICKS);
            self.risk_deaths.push((spot_key, now));
            if self.risk_deaths.iter().filter(|(key, _)| *key == spot_key).count() < RISK_DEATHS {
                return;
            }
        }
        if !self.excludes(spot_key) {
            self.excluded.push(spot_key);
        }
    }

    /// True when the death just noted at `spot_key` is the camp's price:
    /// a risk camp still in the run (re-gear at the bank, go back).
    pub fn accepts(&self, spot_key: &str) -> bool {
        lifescape_gamedata::combat_spots::risk_spot(spot_key) && !self.excludes(spot_key)
    }
}

/// After a death noted at `spot_key` in an ordered queue: true when the
/// spot is a risk camp the run takes back (`DeathTally::accepts`; user
/// 2026-11-20: the death is its price, the hunt re-gears at the bank).
pub fn takes_back(bot: &mut Bot, tally: &DeathTally, spot_key: &str) -> bool {
    let back = tally.accepts(spot_key);
    if back {
        bot.log(&format!("{spot_key} killed us: a risk camp, the death is its price; back after the bank"));
    }
    back
}

/// Second consecutive death with no kill or bank trip between: loud and
/// typed instead of walking back a third time.
pub fn danger_error(spot_key: &str, deaths: u32) -> ScriptError {
    ScriptError::Sdk(lifescape_sdk::Error::Unreachable(format!(
        "spot {spot_key} too dangerous after {deaths} deaths, re-gear or pick easier foes"
    )))
}

/// An ordered combat spot (the brain named it) killed the bot: bank the
/// run's loot unless `--no-bank`, then the typed danger error. Never a
/// swap to a "safer" spot: a drop farm must stay on its dropper.
pub fn ordered_spot_lost(bot: &mut Bot, spot_key: &str, deaths: u32, no_bank: bool) -> Result<ScriptError, ScriptError> {
    if !no_bank {
        crate::kit::gear::prepare_at_bank(bot)?;
    }
    Ok(danger_error(spot_key, deaths))
}

/// An ordered combat spot sits across the toll fence and the brain sent
/// no `toll`: the typed access gap (`access:al_kharid_toll`), never Usage.
pub fn ordered_spot_tolled(bot: &mut Bot, spot_key: &str, detail: &str) -> ScriptError {
    bot.log(&format!("{spot_key} across the toll fence: {detail}"));
    crate::kit::cycle::blocked_access(crate::kit::site::TOLL_AREA)
}

/// Death resume: bounded wait for the respawn, then resync the loop
/// watches (hp flatlined, message index consumed). Never billed to a
/// stall budget by the caller. This is the `death_resume` the pause and
/// engagement code (L2) reuses; `kit::load` wraps it for the driver.
pub fn death_resume(bot: &mut Bot, wait_ticks: u64, last_hp: &mut u8, seen: &mut usize) -> Result<(), ScriptError> {
    bot.log("died, respawning");
    bot.intend("died, respawning");
    let target = bot.world.ticks + wait_ticks;
    while bot.world.ticks < target && hitpoints(bot) == 0 {
        bot.pump(5)?;
    }
    *last_hp = hitpoints(bot);
    *seen = bot.world.messages.len();
    Ok(())
}

/// Safe-point predicate for the market pause and the end of engagement:
/// alive with no modal open (no dialogue, no trade screen; an overlay
/// layer is a HUD such as the Wilderness one, never a modal). The combat
/// check (no target) lives with the fight loop, which owns its target;
/// the pause gate ANDs this with "not fighting" at the call site (L2).
pub fn at_safe_point(bot: &Bot) -> bool {
    hitpoints(bot) > 0 && bot.world.modal.as_ref().is_none_or(|modal| modal.layer == "overlay")
}


/// Live level of a stat, or the last known value before it streamed.
pub fn learn_level(bot: &Bot, stat: u8, known: i64) -> i64 {
    bot.world.stats.get(&stat).map(|(level, _)| i64::from(*level)).unwrap_or(known)
}

/// Current hitpoints (10 before the stats stream).
pub fn hitpoints(bot: &Bot) -> u8 {
    bot.world.stats.get(&3).map(|(level, _)| *level).unwrap_or(10)
}

/// Base hitpoints (the level the XP buys, i.e. full health): 10 before
/// the stats stream.
pub fn max_hitpoints(bot: &Bot) -> u8 {
    bot.world.stats.get(&3).map(|(_, exp)| lifescape_sdk::snapshot::level_for_xp(*exp)).unwrap_or(10)
}

/// Hurt loads in a row a gatherer heals from before it gives the site up
/// (decision D3, 2026-11-17: flee, heal, come back; live arthur_hart on
/// the Wilderness iron rocks relaunched at hp 29, 17, then 9).
pub const HURT_RETURNS: u32 = 2;

/// What a gatherer does after a load that ended without its resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HurtReturn {
    /// Not hurt: the load's own failure stands.
    NotHurt,
    /// Hurt (under half its hitpoints): bank, heal, come back.
    HealAndReturn,
    /// Hurt again past `HURT_RETURNS`: the site is too dangerous now, a
    /// typed failure (the brain cools the site for this bot).
    GiveUp,
}

/// Pure verdict of a failed load: `hp` / `max_hp` now, `streak` = hurt
/// loads already healed from in a row.
pub fn hurt_return(hp: u8, max_hp: u8, streak: u32) -> HurtReturn {
    // The meal rule's eat line (tour 3 point 8: half health let tim_wolfe
    // relaunch at 22/42 on the Wilderness iron before the rule saw 12).
    if !should_eat(hp, max_hp.max(1), EAT_FRACTION) {
        return HurtReturn::NotHurt;
    }
    if streak >= HURT_RETURNS {
        HurtReturn::GiveUp
    } else {
        HurtReturn::HealAndReturn
    }
}

/// Smallest hp a hurt gatherer heals to before it walks back: the first
/// hp above the eat line `hurt_return` reads (never above full health).
/// The bank heal's `HEAL_TARGET_HP` (12) sits under that line for any
/// bot past 21 max hp, so a heal to it left the next check as hurt.
pub fn hurt_heal_target(max_hp: u8) -> u8 {
    let max_hp = max_hp.max(1);
    (1..=max_hp).find(|hp| !should_eat(*hp, max_hp, EAT_FRACTION)).unwrap_or(max_hp)
}

/// Regen ticks of one hitpoint (content `health_regen` 100, measured
/// live 9->12 in 300 ticks, see `kit::bank::HEAL_TARGET_HP`).
pub const REGEN_TICKS_PER_HP: u64 = 100;

/// True when the server announced death since `seen`: "Oh dear you are
/// dead!" arrives even when the hitpoints stream lags behind.
pub fn death_message_seen(bot: &Bot, seen: usize) -> bool {
    bot.world.messages.get(seen..).unwrap_or(&[]).iter().any(|message| message.to_lowercase().contains("oh dear you are dead"))
}

/// Random-event gate: greets hosts on sight, rubs the lamp, solves what
/// resolves inline; an unresolved event aborts loudly so the caller stops
/// before the next step. The worker pause gate runs the mapped rescue
/// inline and the loop retries after the next pump.
pub fn check_events(bot: &mut Bot) -> ScriptResult {
    // Routes planned from here on avoid the hunters near the bot.
    crate::kit::danger::arm(bot);
    if !crate::randomevent_full::check(bot)? {
        return Err(ScriptError::Failed(format!("random event unresolved at {:?}", bot.position())));
    }
    Ok(0)
}

/// Wait until the backpack streams after login (tick-bounded).
pub fn wait_pack_streamed(bot: &mut Bot) -> Result<(), ScriptError> {
    if !wait_backpack(bot, 150)? {
        return Err(ScriptError::Failed(format!("backpack never streamed at {:?}", bot.position())));
    }
    Ok(())
}

/// True for connection failures (socket, login): these must propagate
/// all the way to the session's reconnect instead of degrading into a
/// limp-along (a dead socket freezes ticks, so every tick-bounded wait
/// below would spin forever). Hurt, stalls and blockages stay local.
pub fn is_conn_abort(error: &ScriptError) -> bool {
    matches!(
        error,
        ScriptError::Sdk(lifescape_sdk::Error::Io(_)) | ScriptError::Sdk(lifescape_sdk::Error::Login(_))
    )
}

/// True for errors a walk must never downgrade into Ok(false)/Unreachable:
/// dead socket (the session reconnects) or operator stop (the run unwinds).
/// Callers match this before degrading anything else to a limp-along.
pub fn is_walk_abort(error: &ScriptError) -> bool {
    is_conn_abort(error) || matches!(error, ScriptError::Sdk(lifescape_sdk::Error::Interrupted(_)))
}
/// True for the loud death-abort errors raised instead of limping on:
/// `bank_trip` ("died en route") and `walk_checked` ("died walking")
/// both mean the load was lost to a respawn. Callers match on this to
/// re-gear and continue the job rather than failing it.
pub fn is_death_abort(error: &ScriptError) -> bool {
    error.to_string().contains("died ")
}

#[cfg(test)]
mod hurt_tests {
    use super::{hurt_return, HurtReturn, HURT_RETURNS};

    /// D3 red test: a load that failed at 9/40 hp heals and returns, a
    /// third hurt load in a row gives the site up; a healthy failure is
    /// left to the load's own verdict.
    #[test]
    fn a_hurt_gatherer_heals_before_it_returns() {
        assert_eq!(hurt_return(9, 40, 0), HurtReturn::HealAndReturn);
        assert_eq!(hurt_return(17, 40, 1), HurtReturn::HealAndReturn);
        assert_eq!(hurt_return(9, 40, HURT_RETURNS), HurtReturn::GiveUp);
        assert_eq!(hurt_return(30, 40, 0), HurtReturn::NotHurt);
        assert_eq!(hurt_return(23, 40, 5), HurtReturn::NotHurt, "above the eat line is no hurt");
        // Tour 3 point 8 (tim_wolfe 12:54): 22/42 is under the eat line
        // (0.55): heal now, not at 12.
        assert_eq!(hurt_return(22, 42, 0), HurtReturn::HealAndReturn);
    }

    /// Live 2026-11-21 (lily_brook, hill_eric, rae_stone: 104 « hurt on
    /// the site » failures at the Edgeville booth): the heal of a hurt
    /// load stopped at the bank heal's 12 hp, under the hurt line, so
    /// the next check saw the same hp and gave the site up in the same
    /// tick. The heal target clears the line, the bank's 12 does not.
    #[test]
    fn the_hurt_heal_clears_the_hurt_line() {
        for max_hp in [10u8, 25, 42, 43, 47, 50, 73, 99] {
            let target = super::hurt_heal_target(max_hp);
            assert_eq!(hurt_return(target, max_hp, HURT_RETURNS), HurtReturn::NotHurt, "{target}/{max_hp} clears");
            assert_ne!(hurt_return(target - 1, max_hp, 0), HurtReturn::NotHurt, "{target} is the smallest clear hp of {max_hp}");
            assert!(target <= max_hp);
        }
        assert_eq!(super::hurt_heal_target(47), 26, "lily_brook 20/47 heals to 26");
        let bank_heal = crate::kit::bank::heal_target(47);
        assert_ne!(hurt_return(bank_heal, 47, 0), HurtReturn::NotHurt, "the bank's 12 leaves a 47-hp miner hurt");
    }
}

#[cfg(test)]
mod tests {
    use super::{at_safe_point, death_message_seen, is_conn_abort, is_walk_abort, should_eat, DeathTally, EAT_FRACTION, FOOD_TARGET};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    /// Eat line: at or under the fraction eats, above holds.
    #[test]
    fn eat_line_holds_above_falls_at_or_under() {
        assert!(should_eat(5, 10, 0.55), "5 of 10 at 0.55 eats");
        assert!(should_eat(6, 10, 0.6), "6 of 10 at 0.6 eats");
        assert!(!should_eat(6, 10, EAT_FRACTION), "6 of 10 above 0.55 holds");
        assert!(!should_eat(10, 10, EAT_FRACTION), "full never eats");
    }

    /// Tally: progress resets the streak, exclusions stay, two in a row
    /// without progress is fatal.
    #[test]
    fn tally_resets_on_progress_never_forgets_killers() {
        let mut tally = DeathTally::fresh();
        assert!(!tally.fatal());
        tally.note_death("goblins", 0);
        assert!(tally.excludes("goblins"));
        assert!(!tally.fatal(), "one death is not fatal");
        tally.note_progress();
        tally.note_death("goblins", 0);
        assert!(tally.excludes("goblins"), "exclusion survives progress");
        assert!(!tally.fatal(), "progress broke the streak");
        tally.note_death("cows", 0);
        assert!(tally.fatal(), "two consecutive deaths are fatal");
    }

    /// Safe point: alive with no modal; dead or mid-dialogue never is.
    #[test]
    fn safe_point_needs_hp_and_no_modal() {        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.stats.insert(3, (10, 0));
        assert!(at_safe_point(&bot), "alive with no modal is safe");
        bot.world.stats.insert(3, (0, 0));
        assert!(!at_safe_point(&bot), "dead is never safe");
        assert_eq!(FOOD_TARGET, 10, "restock target");
    }

    /// Live 2026-11-22 (N0valyfe, ground pickup at the Graveyard of
    /// Shadows): the Wilderness HUD (`wilderness_overlay`, 193) is an
    /// overlay layer, not a dialogue; counted as a modal it kept the
    /// market gate shut for hours (poses every 5 min, 18 `no_show`
    /// deals). Dialogues and trade screens still close the gate.
    #[test]
    fn a_hud_overlay_is_no_modal_for_the_safe_point() {
        use lifescape_sdk::world::Modal;
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.stats.insert(3, (10, 0));
        bot.world.modal = Some(Modal { layer: "overlay".to_string(), interface_id: 193 });
        assert!(at_safe_point(&bot), "the Wilderness overlay is a HUD");
        for layer in ["chat", "main", "main_side", "side"] {
            bot.world.modal = Some(Modal { layer: layer.to_string(), interface_id: 1 });
            assert!(!at_safe_point(&bot), "{layer} modal is a screen the bot must close first");
        }
    }

    /// A foodless pack forces a bank trip only when the bank can restock:
    /// broke beginners stay on the field instead of ping-ponging.
    #[test]
    fn bank_trip_needs_bank_food() {
        use super::bank_stocks_food;
        use crate::tables_live::attach_live_tables;
        use lifescape_sdk::Bot;
        let mut empty = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut empty);
        assert!(!bank_stocks_food(&empty), "no bank open: no trip for food");
        let mut stocked = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut stocked);
        let bank_com = (stocked.tables.com_id)("bank_main:inv").expect("bank com in dumps");
        stocked.world.inv.insert(bank_com, [(0u8, (315u16, 3u32))].into_iter().collect());
        assert!(bank_stocks_food(&stocked), "banked shrimps restock");
    }

    #[test]
    fn death_message_seen_matches_case_insensitive() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.messages.push("Oh dear you are dead!".to_string());
        assert!(death_message_seen(&bot, 0), "exact line detected");
        assert!(!death_message_seen(&bot, 1), "index tracks seen messages");
        bot.world.messages.push("OH DEAR you ARE dead".to_string());
        assert!(death_message_seen(&bot, 1), "case-insensitive");
    }

    /// Walk-abort typing: dead socket and operator stop never degrade,
    /// stalls and blockages still do.
    #[test]
    fn walk_abort_covers_conn_and_stop_only() {
        use crate::ScriptError;
        use lifescape_sdk::Error;
        assert!(is_walk_abort(&ScriptError::Sdk(Error::Io("x".to_string()))), "socket");
        assert!(is_walk_abort(&ScriptError::Sdk(Error::Login("x".to_string()))), "login");
        assert!(is_walk_abort(&ScriptError::Sdk(Error::Interrupted("x".to_string()))), "stop");
        assert!(!is_walk_abort(&ScriptError::Sdk(Error::Unreachable("x".to_string()))), "blockage stays local");
        assert!(!is_walk_abort(&ScriptError::Sdk(Error::Timeout("x".to_string()))), "stall stays local");
        assert!(!is_walk_abort(&ScriptError::Usage("x".to_string())), "usage stays local");
        assert!(is_conn_abort(&ScriptError::Sdk(Error::Io("x".to_string()))), "conn subset");
        assert!(!is_conn_abort(&ScriptError::Sdk(Error::Interrupted("x".to_string()))), "stop is not conn");
    }

}

#[cfg(test)]
mod risk_tests {
    use super::{DeathTally, RISK_DEATHS, RISK_WINDOW_TICKS};

    /// User 2026-11-20: a death at a risk camp is a cost, never a reason
    /// to leave: the camp stays in the run, the bot re-gears and goes
    /// back; three deaths within the window are a death loop and the
    /// camp leaves. An ordinary spot still leaves at its first death.
    #[test]
    fn risk_camps_accept_deaths_until_the_guard() {
        let mut tally = DeathTally::fresh();
        tally.note_death("lesser_demons", 100);
        assert!(!tally.excludes("lesser_demons") && tally.accepts("lesser_demons"), "one death: back to the demons");
        tally.note_progress();
        tally.note_death("lesser_demons", 900);
        assert!(!tally.excludes("lesser_demons"), "two deaths with kills between: still the price");
        tally.note_progress();
        tally.note_death("lesser_demons", 1500);
        assert_eq!(RISK_DEATHS, 3);
        assert!(tally.excludes("lesser_demons") && !tally.accepts("lesser_demons"), "three in the window: a death loop");
        let mut spaced = DeathTally::fresh();
        for index in 0..5u64 {
            spaced.note_death("greater_demons", index * RISK_WINDOW_TICKS);
            spaced.note_progress();
        }
        assert!(!spaced.excludes("greater_demons"), "one death per window never trips the guard");
        let mut plain = DeathTally::fresh();
        plain.note_death("hill_giants", 10);
        assert!(plain.excludes("hill_giants") && !plain.accepts("hill_giants"), "an ordinary spot leaves at once");
        // Two deaths in a row with nothing between stay fatal anywhere.
        let mut streak = DeathTally::fresh();
        streak.note_death("lesser_demons", 10);
        streak.note_death("lesser_demons", 50);
        assert!(streak.fatal(), "no kill between: the loop guard holds");
    }
}
