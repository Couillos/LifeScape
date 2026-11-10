//! Fish driver: the fishing work loop (LoopOps) on the shared harvest
//! core, gear and bait restocking. The pool (tiers, live spots, pinned
//! bank) lives in `kit::spots`; the activity keeps only the CLI and the
//! loads loop.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::waits::wait_ticks;
use lifescape_sdk::Bot;

use crate::kit::spots::{nearest_live, pick_fish_spot};
use crate::kit::tools::FISH_SPEC;
use crate::kit::bank::bank_trip;
use crate::kit::pack::{free_slots, obj_id, obj_price, pack_total};
use crate::kit::shop::{buy_stock, sell_for_target};
use crate::kit::harvest::WorkOutcome;
use crate::kit::survival::learn_level;
use crate::skill_walk::walk_checked;
use crate::ScriptError;

pub const FISH_STAT: u8 = 10;
const BAIT_MIN: u32 = 20;
const BAIT_WANT: u32 = 200;

/// Server start-of-work acks: the action loop runs on the server's side.
/// Fishing-only vocabulary (point 1: skill-specific stays with the skill).
pub const FISHING_ACKS: [&str; 2] = ["cast out your line", "attempt to catch"];

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
            (live_x - self.x).abs() + (live_z - self.z).abs() > 6
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
    let _ = (live_x, live_z);
    let mut source = FishSource { tier, slot, x: spot_x, z: spot_z, patient: false };
    crate::kit::harvest::watch_stand(bot, &mut source, spot_x, spot_z, product_ids, had_fish)
}

fn ensure_gear(
    bot: &mut Bot,
    tier: &knowledge::fishing::FishTier,
    side_shop: u16,
    main_shop: u16,
    coins: u16,
    bank: &knowledge::banks::Bank,
    bait_ban_until: &mut u64,
) -> Result<bool, ScriptError> {
    let tool = obj_id(bot, tier.tool)?;
    if pack_total(bot, tool) == 0 {
        let keep: Vec<u16> = {
            let mut keep = vec![coins];
            keep.extend(FISH_SPEC.known_ids(bot));
            keep
        };
        let side_bank = (bot.tables.com_id)("bank_side:inv").ok_or_else(|| ScriptError::Usage("bank com missing".to_string()))?;
        let main_bank = (bot.tables.com_id)("bank_main:inv").ok_or_else(|| ScriptError::Usage("bank com missing".to_string()))?;
        if bank_trip(bot, bank, &keep, 0)? == 0 && pack_total(bot, tool) == 0 {
            let price = obj_price(tier.tool) as u32;
            if pack_total(bot, coins) < price {
                sell_for_target(bot, &[], coins, price - pack_total(bot, coins))?;
            }
            if buy_stock(bot, tier.tool, 1)? == 0 {
                return Ok(false);
            }
        }
        let _ = (side_shop, main_shop, side_bank, main_bank);
    }
    if let Some(bait) = tier.bait {
        let bait_id = obj_id(bot, bait)?;
        if pack_total(bot, bait_id) < BAIT_MIN && !restock_bait(bot, tier, coins, bank)? {
            *bait_ban_until = bot.world.ticks + 1000;
            return Ok(false);
        }
    }
    Ok(pack_total(bot, tool) > 0)
}

fn restock_bait(
    bot: &mut Bot,
    tier: &knowledge::fishing::FishTier,
    coins: u16,
    bank: &knowledge::banks::Bank,
) -> Result<bool, ScriptError> {
    let Some(bait) = tier.bait else {
        return Ok(true);
    };
    let bait_id = obj_id(bot, bait)?;
    let need = BAIT_WANT.saturating_sub(pack_total(bot, bait_id));
    if need == 0 {
        return Ok(true);
    }
    let price = obj_price(bait).max(1) as u32;
    let shortfall = need * price - pack_total(bot, coins).min(need * price);
    if shortfall > 0 {
        sell_for_target(bot, &[], coins, shortfall)?;
    }
    if pack_total(bot, coins) < price {
        let keep: Vec<u16> = FISH_SPEC.known_ids(bot).into_iter().collect();
        bank_trip(bot, bank, &keep, (need * price).min(5000))?;
    }
    let affordable = pack_total(bot, coins) / price;
    buy_stock(bot, bait, need.min(affordable))?;
    Ok(pack_total(bot, bait_id) >= BAIT_MIN)
}

/// Fishing policy on the shared driver: the pack-full gate ends the
/// load, gear (tool + bait) is ensured per spot inside `work` (bank and
/// shop walks bill like the old pick phase), the trek retries in `work`,
/// bait restocks resume the same spot through `on_no_bait`.

/// Fishing policy on the shared driver: the pack-full gate ends the
/// load, gear (tool + bait) is ensured per spot inside `work` (bank and
/// shop walks bill like the old pick phase), the trek retries in `work`,
/// bait restocks resume the same spot through `on_no_bait`.
struct FishOps<'a> {
    bank: &'a mut knowledge::banks::Bank,
    coins: u16,
    side_shop: u16,
    main_shop: u16,
    bait_ban: u64,
    level: i64,
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
        match pick_fish_spot(bot, self.level, &state.banned, now, &*self.bank) {
            Some((tier, spot_x, spot_z, use_bank)) => {
                *self.bank = use_bank;
                bot.log(&format!("fishing {} at ({spot_x},{spot_z})", tier.products.first().unwrap_or(&"?")));
                bot.intend(&format!("walking to the fishing spot at ({spot_x},{spot_z})"));
                Some((tier, spot_x, spot_z))
            }
            None => {
                bot.log("no fish spot, waiting for respawns");
                bot.intend("waiting for a safe fishing spot");
                None
            }
        }
    }

    fn keep_spot(
        &mut self,
        bot: &mut Bot,
        _state: &mut crate::kit::load::LoopState,
        spot: &Self::Spot,
        _at: (i32, i32),
    ) -> bool {
        self.level = learn_level(bot, FISH_STAT, self.level);
        if let Some(top) = knowledge::fishing::best_fish_tier(self.level) {
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
        bot.log("hurt, fleeing");
        bot.intend("fleeing a hunter");
        let abort = crate::kit::load::register_hurt(cfg, state, bot.world.ticks);
        wait_ticks(bot, 200)?;
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
        let restocked = restock_bait(bot, spot, self.coins, self.bank)?;
        state.idle_since += bot.world.ticks - spent;
        if restocked {
            bot.log("bait restocked, resuming");
            return Ok(true);
        }
        bot.log("bait out, dropping to net tier");
        self.bait_ban = bot.world.ticks + 1000;
        Ok(false)
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
        if !ensure_gear(bot, spot, self.side_shop, self.main_shop, self.coins, self.bank, &mut self.bait_ban)? {
            if pack_total(bot, obj_id(bot, spot.tool).unwrap_or(0)) == 0 {
                bot.log(&format!("no gear for {}", spot.tool));
                // Broke without a tool: the load aborts, like the old loop.
                return Ok((WorkOutcome::Blocked, 0));
            }
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

#[allow(clippy::too_many_arguments)]

pub fn fish_load(
    bot: &mut Bot,
    level: i64,
    bank: &mut knowledge::banks::Bank,
    coins: u16,
    side_shop: u16,
    main_shop: u16,
) -> Result<(bool, i64), ScriptError> {
    let mut ops = FishOps { bank, coins, side_shop, main_shop, bait_ban: 0, level };
    let ok = crate::kit::load::run_load(
        bot,
        &crate::kit::load::LoadCfg::gather(),
        &mut crate::kit::load::LoopState::fresh(bot),
        &mut ops,
    )?;
    Ok((ok, learn_level(bot, FISH_STAT, ops.level)))
}
