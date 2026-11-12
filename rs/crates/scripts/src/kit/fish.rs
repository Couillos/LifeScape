//! Fish driver: the fishing work loop (LoopOps) on the shared harvest
//! core, gear and bait restocking. The pool (tiers, live spots, pinned
//! bank) lives in `kit::spots`; the activity keeps only the CLI and the
//! loads loop.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::waits::wait_ticks;
use lifescape_sdk::Bot;

use crate::kit::spots::{nearest_bank_at, nearest_live, pick_hop_fish_spot, pick_near_fish_spot};
use crate::kit::tools::{blocked_tool, FISH_SPEC};
use crate::kit::bank::bank_trip;
use crate::kit::pack::{free_slots, obj_id, obj_price, pack_total, shop_price};
use crate::kit::shop::buy_stock;
use crate::kit::harvest::WorkOutcome;
use crate::kit::survival::learn_level;
use crate::skill_walk::walk_checked;
use crate::ScriptError;

pub const FISH_STAT: u8 = 10;
/// A live spot NPC farther than this from the chosen tile is another
/// spot: the chosen one moved (spots hop every 280-530 ticks).
const SPOT_MOVED_TILES: i32 = 6;
const BAIT_MIN: u32 = 20;
const BAIT_WANT: u32 = 200;

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
            let price = shop_price(tier.tool).unwrap_or(i64::MAX).min(i64::from(u32::MAX)) as u32;
            if pack_total(bot, coins) < price {
                return Ok(false);
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
    let price = shop_price(bait).unwrap_or(i64::MAX).clamp(1, i64::from(u32::MAX)) as u32;
    if pack_total(bot, coins) < price {
        let keep: Vec<u16> = FISH_SPEC.known_ids(bot).into_iter().collect();
        bank_trip(bot, bank, &keep, (need * price).min(5000))?;
    }
    let affordable = pack_total(bot, coins) / price;
    buy_stock(bot, bait, need.min(affordable))?;
    Ok(pack_total(bot, bait_id) >= BAIT_MIN)
}

/// Gear-first tier choice: the best tier whose tool is carried (bait
/// ready or fundable) fishes before any higher tier we'd have to buy;
/// buyable tiers follow; nothing fishable blocks with the missing tool
/// or bait, never Usage. Pass 0 fishes now, 1 restocks bait, 2 buys the
/// tool — porté first, achetable second.
fn tier_pass(bot: &Bot, tier: &knowledge::fishing::FishTier) -> Option<u8> {
    let tool_held = obj_id(bot, tier.tool).is_ok_and(|id| pack_total(bot, id) > 0);
    let coins = obj_id(bot, "coins").map(|id| pack_total(bot, id)).unwrap_or(0);
    let tool_fundable = shop_price(tier.tool).is_some_and(|price| i64::from(coins) >= price);
    let (bait_ready, bait_fundable) = match tier.bait {
        None => (true, true),
        Some(bait) => {
            let held = obj_id(bot, bait).map(|id| pack_total(bot, id)).unwrap_or(0);
            let fundable = shop_price(bait).is_some_and(|price| i64::from(coins) >= i64::from(BAIT_MIN) * price);
            (held >= BAIT_MIN, fundable)
        }
    };
    if tool_held && bait_ready {
        Some(0)
    } else if tool_held && bait_fundable {
        Some(1)
    } else if tool_fundable && bait_fundable {
        Some(2)
    } else {
        None
    }
}

/// Best funded tier by (pass, level): a carried net beats a rod to buy
/// (live t04: net + 0 coins fishes shrimp, never sardine).
pub(crate) fn select_fish_tier(bot: &Bot, level: i64) -> Result<knowledge::fishing::FishTier, ScriptError> {
    let mut best: Option<(u8, knowledge::fishing::FishTier)> = None;
    for tier in knowledge::fishing::affordable_tiers(level) {
        if let Some(pass) = tier_pass(bot, &tier) {
            let better = best.is_none_or(|(known_pass, known): (u8, knowledge::fishing::FishTier)| {
                (pass, known.level) < (known_pass, tier.level)
            });
            if better {
                best = Some((pass, tier));
            }
        }
    }
    if let Some((_, tier)) = best {
        return Ok(tier);
    }
    // Nothing fishable: the bait when the best tool is already held,
    // else the cheapest affordable tool (net) — the brain funds that.
    let mut by_level = knowledge::fishing::affordable_tiers(level);
    by_level.sort_by_key(|tier| -tier.level);
    if let Some(top) = by_level.first() {
        if let Some(bait) = top.bait {
            if obj_id(bot, top.tool).is_ok_and(|id| pack_total(bot, id) > 0) {
                return Err(blocked_tool(bait));
            }
        }
    }
    let mut by_price = knowledge::fishing::affordable_tiers(level);
    by_price.sort_by_key(|tier| obj_price(tier.tool));
    if let Some(cheapest) = by_price.first() {
        return Err(blocked_tool(cheapest.tool));
    }
    Err(blocked_tool("net"))
}

/// Affordable tiers, funded (pass, level) first, unfunded after for the
/// blocked path (water with no gear blocks typed, never reads dry).
/// Bait-banned tiers are skipped: the repick lands on net water instead
/// of retrying the baited tier in a loop.
pub(crate) fn ordered_fish_tiers(
    bot: &Bot,
    level: i64,
    bait_ban_until: u64,
    now: u64,
) -> Vec<knowledge::fishing::FishTier> {
    let mut funded: Vec<(u8, knowledge::fishing::FishTier)> = Vec::new();
    let mut rest: Vec<knowledge::fishing::FishTier> = Vec::new();
    for tier in knowledge::fishing::affordable_tiers(level) {
        if tier.bait.is_some() && now < bait_ban_until {
            continue;
        }
        match tier_pass(bot, &tier) {
            Some(pass) => funded.push((pass, tier)),
            None => rest.push(tier),
        }
    }
    funded.sort_by(|left, right| left.0.cmp(&right.0).then(right.1.level.cmp(&left.1.level)));
    rest.sort_by_key(|tier| -tier.level);
    funded.into_iter().map(|(_, tier)| tier).chain(rest).collect()
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
        let fence = if bot.allow_paid { None } else { crate::kit::bank::toll_gate_x(&mut *bot) };
        for tier in ordered_fish_tiers(bot, self.level, self.bait_ban, now) {
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
                // Broke without a tool: typed blocked (the brain funds an
                // obtain task), never a silent drop or a shop Usage.
                return Err(blocked_tool(spot.tool));
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
pub(crate) fn structurally_dry(bot: &mut Bot, level: i64, bank: &knowledge::banks::Bank) -> bool {
    let now = bot.world.ticks;
    let clean = crate::kit::bans::BanSet::new();
    crate::kit::spots::pick_fish_spot(bot, level, &clean, now, bank).is_none()
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

#[cfg(test)]
mod gear_tests {
    use super::{ordered_fish_tiers, select_fish_tier};
    use crate::kit::pack::obj_id;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn pack_bot(items: &[(&str, u32)]) -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let ids: Vec<(u16, u32)> = items
            .iter()
            .map(|(name, count)| {
                (obj_id(&bot, name).unwrap_or_else(|_| panic!("{name} in tables")), *count)
            })
            .collect();
        let com = (bot.tables.com_id)("inventory:inv").expect("inventory:inv in dumps");
        bot.world.inv.insert(
            com,
            ids.into_iter().enumerate().map(|(slot, (id, count))| (slot as u8, (id, count))).collect(),
        );
        bot
    }

    /// Live t04: net carried, 0 coins, level 5 — the shrimp tier (net)
    /// fishes, never the sardine tier (rod to buy).
    #[test]
    fn net_held_broke_picks_shrimp_not_sardine() {
        let bot = pack_bot(&[("net", 1)]);
        let tier = select_fish_tier(&bot, 5).expect("net held, shrimp fishes");
        assert_eq!((tier.tool, tier.level), ("net", 1), "crevette au filet, pas sardine, got: {}", tier.tool);
    }

    /// Rod without bait or coins: shrimp when the net is also carried,
    /// typed blocked otherwise — never Usage, never a silent Ok(false).
    #[test]
    fn rod_without_bait_or_coins_falls_back_or_blocks() {
        let both = pack_bot(&[("fishing_rod", 1), ("net", 1)]);
        let tier = select_fish_tier(&both, 5).expect("net fallback fishes");
        assert_eq!(tier.tool, "net", "crevette si filet, got: {}", tier.tool);
        let bare = pack_bot(&[("fishing_rod", 1)]);
        let error = select_fish_tier(&bare, 5).expect_err("rod without bait or coins blocks");
        assert!(matches!(error, crate::ScriptError::Blocked { .. }), "blocked, got: {error}");
        assert!(!matches!(error, crate::ScriptError::Usage(_)), "never Usage, got: {error}");
        assert!(error.to_string().contains("fishing_bait"), "names the bait, got: {error}");
    }

    /// Level 1 only knows shrimp: net held fishes it, empty and broke
    /// blocks on the net itself (the cheapest family tool), never Usage.
    #[test]
    fn level_1_picks_shrimp() {
        let bot = pack_bot(&[("net", 1)]);
        let tier = select_fish_tier(&bot, 1).expect("level 1 fishes shrimp");
        assert_eq!((tier.tool, tier.level), ("net", 1));
        let broke = pack_bot(&[]);
        let error = select_fish_tier(&broke, 1).expect_err("nothing held blocks");
        assert!(matches!(error, crate::ScriptError::Blocked { .. }), "blocked, got: {error}");
        assert!(error.to_string().contains("net"), "cheapest tool named, got: {error}");
    }

    /// Fully geared rod (tool + bait + coins) still takes the best tier.
    #[test]
    fn rod_and_bait_held_picks_sardine() {
        let bot = pack_bot(&[("fishing_rod", 1), ("fishing_bait", 200), ("coins", 100)]);
        let tier = select_fish_tier(&bot, 5).expect("geared rod fishes sardine");
        assert_eq!((tier.tool, tier.level), ("fishing_rod", 5));
    }

    /// Funded but tool-less: the buyable best tier still wins (porté
    /// first, achetable second — never blocked while coins cover it).
    #[test]
    fn funded_but_toolless_buys_best() {
        let bot = pack_bot(&[("coins", 100)]);
        let tier = select_fish_tier(&bot, 5).expect("100 coins fund rod+bait");
        assert_eq!(tier.tool, "fishing_rod", "achetable après le porté, got: {}", tier.tool);
    }

    /// A bait ban (restock failed) drops baited tiers from the order so
    /// the repick lands on net water instead of retrying sardine in a loop.
    #[test]
    fn bait_ban_drops_baited_tiers() {
        let bot = pack_bot(&[("fishing_rod", 1), ("fishing_bait", 200)]);
        let plain: Vec<&str> = ordered_fish_tiers(&bot, 5, 0, 0).iter().map(|tier| tier.tool).collect();
        assert_eq!(plain.first(), Some(&"fishing_rod"), "unbanned rod first, got: {plain:?}");
        let banned: Vec<&str> = ordered_fish_tiers(&bot, 5, 1000, 10).iter().map(|tier| tier.tool).collect();
        assert!(!banned.contains(&"fishing_rod"), "banned rod out of the order, got: {banned:?}");
        assert_eq!(banned.first(), Some(&"net"), "net water next, got: {banned:?}");
    }
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
        assert!(structurally_dry(&mut bot, 1, &bank), "no service means no verifiable water");
        let error = dry_water_error(&mut bot);
        assert!(error.to_string().contains("no fishing water in range"), "named, got: {error}");
        assert!(error.to_string().contains("3092"), "names where, got: {error}");
        assert!(!error.to_string().contains("toll"), "no fence proof offline, no hint, got: {error}");
    }
}
