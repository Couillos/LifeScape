//! Fish driver: the fishing work loop (LoopOps) on the shared harvest
//! core. Tier and gear (pack/bank only, exact gaps) live in
//! `kit::fish_gear`, the pool (tiers, live spots, pinned bank) in
//! `kit::spots`; the activity keeps only the CLI and the loads loop.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::waits::wait_ticks;
use lifescape_sdk::Bot;

use crate::kit::fish_gear::{ensure_gear, ordered_fish_tiers, restock_bait, select_fish_tier};
use crate::kit::spots::{nearest_bank_at, nearest_live, pick_hop_fish_spot, pick_near_fish_spot};
use crate::kit::pack::{free_slots, obj_id, pack_total};
use crate::kit::harvest::WorkOutcome;
use crate::kit::survival::learn_level;
use crate::skill_walk::walk_checked;
use crate::ScriptError;

pub const FISH_STAT: u8 = 10;
/// A live spot NPC farther than this from the chosen tile is another
/// spot: the chosen one moved (spots hop every 280-530 ticks).
const SPOT_MOVED_TILES: i32 = 6;

/// Server start-of-work acks: the action loop runs on the server's side.
/// Fishing-only vocabulary (point 1: skill-specific stays with the skill).
/// Net and harpoon spots ack with their own lines (content skill_fishing).
pub const FISHING_ACKS: [&str; 4] = ["cast out your line", "attempt to catch", "cast out your net", "start harpooning"];

/// One click, then watch the chain: re-clicking restarts it, so never
/// spam. A cast acked on a still-live spot is a dry streak, not a dead
/// spot — wait, no time cap; the load-level stall stays the ultimate
/// backstop. Quiet without any ack gives up fast: the click itself never
/// landed. Re-clicks probe reachability every ~30 quiet ticks: only a
/// click surfaces a refusal.
struct FishSource<'a> {
    tier: &'a knowledge::fishing::FishTier,
    slot: u16,
    x: i32,
    z: i32,
    patient: bool,
}

impl<'a> crate::kit::harvest::HarvestSource for FishSource<'a> {
    fn acks(&self) -> &[&str] {
        &FISHING_ACKS
    }

    fn click(&mut self, bot: &mut Bot, x: i32, z: i32) -> Result<crate::kit::harvest::HarvestTarget, ScriptError> {
        bot.log(&format!("click {} at ({x},{z})", self.tier.action));
        bot.intend(&format!("fishing {}", self.tier.products.first().unwrap_or(&"?")));
        bot.op_npc(self.tier.op as u8, self.slot)?;
        Ok(crate::kit::harvest::HarvestTarget::Npc { slot: self.slot })
    }

    fn slot_gone(&self, bot: &mut Bot, _target: &crate::kit::harvest::HarvestTarget) -> bool {
        nearest_live(bot, self.tier.category, self.x, self.z).is_none()
    }

    fn slot_moved(&self, bot: &mut Bot, _target: &crate::kit::harvest::HarvestTarget) -> bool {
        nearest_live(bot, self.tier.category, self.x, self.z).is_some_and(|(_, live_x, live_z)| {
            (live_x - self.x).abs() + (live_z - self.z).abs() > SPOT_MOVED_TILES
        })
    }

    fn target_live(&self, bot: &mut Bot, target: &crate::kit::harvest::HarvestTarget) -> bool {
        !self.slot_gone(bot, target)
    }

    fn depleted_now(&self, _bot: &mut Bot, _x: i32, _z: i32, _silences: u32) -> bool {
        false
    }

    /// The old loop's 60 unacked ticks in 12-tick rounds.
    fn unacked_rounds(&self) -> u32 {
        5
    }

    /// The old loop counts 60 ticks from the last fish, not from the
    /// click: a gain restarts the quiet budget.
    fn silence_resets_on_gain(&self) -> bool {
        true
    }

    /// Reachability probe every ~30 quiet ticks while acked: a
    /// wall-separated spot only refuses when clicked.
    fn reprobe(&self, acked: bool, _live: bool, since_probe: u64) -> bool {
        acked && since_probe >= 30
    }

    fn note_dry_streak(&mut self, bot: &mut Bot, _x: i32, _z: i32, acked: bool, _live: bool, silences: u32) {
        if acked && silences >= 5 && !self.patient {
            self.patient = true;
            bot.log("still fishing on a live spot, waiting out the dry streak");
        }
    }
}
fn work_spot(
    bot: &mut Bot,
    tier: &knowledge::fishing::FishTier,
    spot_x: i32,
    spot_z: i32,
    product_ids: &[u16],
    had_fish: u32,
) -> Result<(WorkOutcome, u32), ScriptError> {
    if (bot.world.x - spot_x).abs() + (bot.world.z - spot_z).abs() > 4 {
        // Dead socket or operator stop, not an unreachable spot: typed
        // all the way to the session instead of a quiet Unreachable.
        match walk_checked(bot, spot_x, spot_z, 6000) {
            Ok(true) => {}
            Ok(false) => return Ok((WorkOutcome::Unreachable, 0)),
            Err(error) if crate::kit::survival::is_walk_abort(&error) => return Err(error),
            Err(_) => return Ok((WorkOutcome::Unreachable, 0)),
        }
    }
    if (bot.world.x - spot_x).abs() + (bot.world.z - spot_z).abs() > 2 {
        if !crate::skill_walk::step_adjacent(bot, spot_x, spot_z)? {
            return Ok((WorkOutcome::Unreachable, 0));
        }
    }
    let Some((slot, live_x, live_z)) = nearest_live(bot, tier.category, spot_x, spot_z) else {
        return Ok((WorkOutcome::Gone, 0));
    };
    // Only the chosen spot: a same-category NPC across the bay is another
    // spot (the picker never vetted it for danger or its bank), so a
    // moved spot is a repick, never a click on whatever is in view.
    if (live_x - spot_x).abs() + (live_z - spot_z).abs() > SPOT_MOVED_TILES {
        bot.log(&format!("spot ({spot_x},{spot_z}) moved away (nearest live at ({live_x},{live_z})), repicking"));
        return Ok((WorkOutcome::Gone, 0));
    }
    let mut source = FishSource { tier, slot, x: spot_x, z: spot_z, patient: false };
    crate::kit::harvest::watch_stand(bot, &mut source, spot_x, spot_z, product_ids, had_fish)
}

/// Fishing policy on the shared driver: the pack-full gate ends the
/// load, gear (tool + bait, pack or bank) is ensured per spot inside
/// `work`, the trek retries in `work`, bait restocks (bank only) resume
/// the same spot through `on_no_bait`. An ordered fish (`target`) is the
/// only tier ever picked; a missing bait blocks by name.
struct FishOps<'a> {
    bank: &'a mut knowledge::banks::Bank,
    coins: u16,
    bait_ban: u64,
    /// The bait whose shortage set `bait_ban` (named in `blocked`).
    short_bait: Option<&'static str>,
    level: i64,
    target: Option<knowledge::fishing::FishTier>,
}

impl crate::kit::load::LoopOps for FishOps<'_> {
    type Spot = knowledge::fishing::FishTier;

    fn is_full(&self, bot: &Bot) -> bool {
        free_slots(bot) <= usize::from(pack_total(bot, self.coins) > 0)
    }

    /// Always geared at the gate: the tool check runs per spot in `work`
    /// (a broke pack aborts through `Blocked`, never here).
    fn has_tool(&self, _bot: &Bot) -> bool {
        true
    }

    fn gain_of(&self, bot: &Bot, spot: &Self::Spot) -> u32 {
        spot.products.iter().filter_map(|product| obj_id(bot, product).ok()).map(|id| pack_total(bot, id)).sum()
    }

    fn pick(&mut self, bot: &mut Bot, state: &mut crate::kit::load::LoopState) -> Option<(Self::Spot, i32, i32)> {
        self.level = learn_level(bot, FISH_STAT, self.level);
        let now = bot.world.ticks;
        let fence = if bot.allow_paid { None } else { crate::kit::bank::toll_gate_x(&mut *bot) };
        for tier in ordered_fish_tiers(bot, self.level, self.bait_ban, now, self.target.as_ref()) {
            if let Some((spot_x, spot_z)) = pick_near_fish_spot(bot, &tier, fence, &state.banned, now, &*self.bank) {
                let bank = nearest_bank_at(spot_x, spot_z, bot.world.level).expect("bank just found");
                *self.bank = bank;
                bot.log(&format!("fishing {} at ({spot_x},{spot_z})", tier.products.first().unwrap_or(&"?")));
                bot.intend(&format!("walking to the fishing spot at ({spot_x},{spot_z})"));
                return Some((tier, spot_x, spot_z));
            }
            if let Some((spot_x, spot_z)) = pick_hop_fish_spot(bot, &tier, fence, &state.banned, now) {
                bot.log(&format!(
                    "no {} near bank ({},{}), hopping same tier to ({spot_x},{spot_z})",
                    tier.products.first().unwrap_or(&"?"),
                    self.bank.x,
                    self.bank.z
                ));
                let bank = nearest_bank_at(spot_x, spot_z, bot.world.level).expect("bank just found");
                *self.bank = bank;
                bot.log(&format!("fishing {} at ({spot_x},{spot_z})", tier.products.first().unwrap_or(&"?")));
                bot.intend(&format!("walking to the fishing spot at ({spot_x},{spot_z})"));
                return Some((tier, spot_x, spot_z));
            }
        }
        bot.log("no fish spot, waiting for respawns");
        bot.intend("waiting for a safe fishing spot");
        None
    }

    fn keep_spot(
        &mut self,
        bot: &mut Bot,
        _state: &mut crate::kit::load::LoopState,
        spot: &Self::Spot,
        _at: (i32, i32),
    ) -> bool {
        self.level = learn_level(bot, FISH_STAT, self.level);
        if self.target.is_some() {
            return true;
        }
        if let Ok(top) = select_fish_tier(bot, self.level) {
            if spot.level < top.level {
                bot.log("level up, switching tier");
                return false;
            }
        }
        true
    }

    /// A lost tool condemns the spot like any other refusal (the old loop
    /// banned 500 here, never a clean drop).
    fn on_no_tool(&self) -> fn(&mut crate::kit::bans::BanSet, (i32, i32), u64, &crate::kit::load::LoadCfg) {
        crate::kit::load::ban_refusal
    }

    /// The old loop waited flat after a flee, never heal-gated.
    fn on_hurt(
        &mut self,
        bot: &mut Bot,
        cfg: &crate::kit::load::LoadCfg,
        state: &mut crate::kit::load::LoopState,
    ) -> Result<bool, ScriptError> {
        bot.log(&format!("hurt, fleeing to the bank ({},{})", self.bank.x, self.bank.z));
        bot.intend("fleeing a hunter");
        let abort = crate::kit::load::register_hurt(cfg, state, bot.world.ticks);
        // Run, never stand: waiting in place under a ranged hunter is how
        // the Draynor bots died. The bank is the known-safe tile.
        match walk_checked(bot, self.bank.x as i32, self.bank.z as i32, 3000) {
            Ok(true) => {}
            Ok(false) => bot.log("flee walk did not arrive, waiting out the flee window"),
            Err(error) if crate::kit::survival::is_walk_abort(&error) => return Err(error),
            Err(error) => bot.log(&format!("flee walk failed ({error}), waiting out the flee window")),
        }
        wait_ticks(bot, cfg.flee_wait.min(50))?;
        Ok(abort)
    }

    /// Bait ran out mid-click: restock walks bill outside the no-fish
    /// stall budget, exactly like the old `NoBait` arm.
    fn on_no_bait(
        &mut self,
        bot: &mut Bot,
        state: &mut crate::kit::load::LoopState,
        spot: &Self::Spot,
        _at: (i32, i32),
    ) -> Result<bool, ScriptError> {
        let spent = bot.world.ticks;
        let restocked = restock_bait(bot, spot, self.bank)?;
        state.idle_since += bot.world.ticks - spent;
        if restocked {
            bot.log("bait restocked, resuming");
            return Ok(true);
        }
        if let (Some(bait), Some(_)) = (spot.bait, self.target.as_ref()) {
            return Err(ScriptError::Blocked { missing: vec![bait.to_string()] });
        }
        bot.log("bait out, dropping to an unbaited tier");
        self.bait_ban = bot.world.ticks + 1000;
        self.short_bait = spot.bait;
        Ok(false)
    }

    /// Nothing to pick after a bait shortage: the bait is the gap, named
    /// (b3gybcev waited 16 rounds at the Varrock bank with no feathers).
    fn no_spot_error(&mut self, bot: &mut Bot, _state: &crate::kit::load::LoopState) -> Option<ScriptError> {
        let bait = self.short_bait.filter(|_| bot.world.ticks < self.bait_ban)?;
        Some(ScriptError::Blocked { missing: vec![bait.to_string()] })
    }

    fn work(
        &mut self,
        bot: &mut Bot,
        _state: &mut crate::kit::load::LoopState,
        spot: &Self::Spot,
        at: (i32, i32),
        had: u32,
    ) -> Result<(WorkOutcome, u32), ScriptError> {
        let (spot_x, spot_z) = at;
        if !ensure_gear(bot, spot, self.bank)? {
            // Bait short after the bank: an ordered fish blocks on it by
            // name; a manual run drops the baited tier for a while.
            let bait = spot.bait.unwrap_or("bait");
            if self.target.is_some() {
                return Err(ScriptError::Blocked { missing: vec![bait.to_string()] });
            }
            self.bait_ban = bot.world.ticks + 1000;
            self.short_bait = spot.bait;
            return Ok((WorkOutcome::NoBait, 0));
        }
        // Trek to the spot: three tries, then the refusal ban. The old
        // loop repicked between tries; retrying the same spot skips the
        // repick without changing the ban or the waits.
        let mut arrived = false;
        for _ in 0..3 {
            match walk_checked(bot, spot_x, spot_z, 15000) {
                Ok(true) => {
                    arrived = true;
                    break;
                }
                Ok(false) => wait_ticks(bot, 200)?,
                Err(error) if crate::kit::survival::is_walk_abort(&error) => return Err(error),
                Err(_) => return Ok((WorkOutcome::Gone, 0)),
            }
        }
        if !arrived {
            return Ok((WorkOutcome::Unreachable, 0));
        }
        let wanted: Vec<u16> = spot.products.iter().filter_map(|product| obj_id(bot, product).ok()).collect();
        work_spot(bot, spot, spot_x, spot_z, &wanted, had)
    }
}

/// Named failure when no fishing water sits in range: toll hint when the
/// fence is the reason (mirrors the melee toll-fence Usage), plain dry
/// reason otherwise. The brain 3-strikes named failures instead of
/// re-fishing a dry pit in a loop.
pub(crate) fn dry_water_error(bot: &mut Bot) -> ScriptError {
    let mut detail = format!("no fishing water in range of {:?} (level {})", bot.position(), bot.world.level);
    if !bot.allow_paid {
        if let Some(line) = crate::kit::bank::toll_gate_x(bot) {
            detail.push_str(&format!(": only water sits across the toll fence (line {line}), re-run with `toll` once the bot carries coins"));
        }
    }
    ScriptError::Failed(detail)
}

/// True when no fishing water sits in range even with a clean slate (no
/// bans): structural dry (a pit with no water), never a transient lull
/// (crowded or banned spots still read wet here).
pub(crate) fn structurally_dry(bot: &mut Bot, level: i64, bank: &knowledge::banks::Bank, target: Option<&knowledge::fishing::FishTier>) -> bool {
    let now = bot.world.ticks;
    let clean = crate::kit::bans::BanSet::new();
    if let Some(tier) = target {
        let fence = if bot.allow_paid { None } else { crate::kit::bank::toll_gate_x(&mut *bot) };
        return pick_near_fish_spot(bot, tier, fence, &clean, now, bank).is_none() && pick_hop_fish_spot(bot, tier, fence, &clean, now).is_none();
    }
    crate::kit::spots::pick_fish_spot(bot, level, &clean, now, bank).is_none()
}

/// One load: `target` is the brain's fish tier (only tier fished), `None`
/// on manual runs (best geared tier by level).
pub fn fish_load(
    bot: &mut Bot,
    level: i64,
    bank: &mut knowledge::banks::Bank,
    coins: u16,
    target: Option<&knowledge::fishing::FishTier>,
) -> Result<(bool, i64), ScriptError> {
    let mut ops = FishOps { bank, coins, bait_ban: 0, short_bait: None, level, target: target.cloned() };
    let ok = crate::kit::load::run_load(
        bot,
        &crate::kit::load::LoadCfg::gather(),
        &mut crate::kit::load::LoopState::fresh(bot),
        &mut ops,
    )?;
    Ok((ok, learn_level(bot, FISH_STAT, ops.level)))
}

#[cfg(test)]
mod dry_tests {
    use super::{dry_water_error, structurally_dry};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    /// Without a nav service the F2P batch never answers, so the pool is
    /// dry: the guard fires instead of burning dead repicks (live, a
    /// served batch distinguishes dry pits from transient lulls).
    #[test]
    fn dry_without_service_fails_fast() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3092;
        bot.world.z = 3243;
        let bank = crate::kit::bank::nearest_bank(&mut bot).expect("a bank resolves unfenced");
        assert!(structurally_dry(&mut bot, 1, &bank, None), "no service means no verifiable water");
        let error = dry_water_error(&mut bot);
        assert!(error.to_string().contains("no fishing water in range"), "named, got: {error}");
        assert!(error.to_string().contains("3092"), "names where, got: {error}");
        assert!(!error.to_string().contains("toll"), "no fence proof offline, no hint, got: {error}");
    }

    /// Live admin (2026-11-13): `fishing raw_shrimp --site 2924,3178,0
    /// boat` sailed to Musa Point, then "no fishing water in range": the
    /// site's bank is the Draynor one (mainland anchor), 230 tiles from
    /// the island water, and the pickers dropped every spot past the bank
    /// trek limit. Musa Point holds both saltfish (net/bait) and rarefish
    /// (cage/harpoon) spots (server map 0_45_49): the brain's site wins.
    #[test]
    fn karamja_site_fishes_around_the_site() {
        let (mut bot, _service) = crate::served_nav_bot("fish-karamja-site");
        bot.world.x = 2924;
        bot.world.z = 3178;
        bot.allow_paid = true;
        bot.assigned_site = Some((2925, 3181, 0));
        let bank = crate::kit::site::site_bank(&bot).expect("a mainland bank serves Karamja");
        for fish in ["raw_shrimp", "raw_tuna"] {
            let tier = crate::kit::fish_gear::ordered_tier(fish, 40).expect("tier at level 40");
            assert!(!structurally_dry(&mut bot, 40, &bank, Some(&tier)), "{fish}: Musa Point water is in range");
            let banned = crate::kit::bans::BanSet::new();
            let (spot_x, spot_z) = crate::kit::spots::pick_near_fish_spot(&mut bot, &tier, None, &banned, 0, &bank).expect("near pick at the site");
            assert!(crate::kit::spots::in_assigned_site(&bot, spot_x, spot_z), "{fish}: picked inside the site ({spot_x},{spot_z})");
        }
    }
}
