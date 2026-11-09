//! Woodcutter: best trees for the level, banks full loads, sells logs to fund
//! axe upgrades, reattaches lost heads instead of rebuying.
//! (Gather loop, bank trip, sell, upgrade.)

#![forbid(unsafe_code)]

use std::collections::HashSet;

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::skill_common::{
    bank_trip, bank_trip_resilient, buy_stock, check_events, free_slots, hitpoints, learn_level, nearest_bank, obj_id,
    pack_total, sell_for_target, travel_ticks, walk_checked, WorkOutcome,
    GONE_BAN_TICKS, STRUCTURAL_BAN_TICKS,
};
use crate::kit::harvest::{watch_stand, HarvestSource, HarvestTarget};
use crate::skill_gather::{
    reattach_tool, BanSet, ToolSpec, WOODCUT_ACKS,
};
use crate::kit::observe::Observe;
use crate::skill_woodcutting_spot::{default_respawn_ticks, is_stump, live_loc_id, operable_ring, operable_stand, pick_tier, pick_tree_spot, refusal_ban_ticks, spot_footprint, tree_still_there, walk_fail_ban, DIRECT_REFUSALS_TO_CONDEMN};
use crate::{ScriptError, ScriptResult};

const WC_STAT: u8 = 8;
pub(crate) const HANDLE_NAME: &str = "macro_hatchethandle";

/// Best axe first.
pub const AXE_ORDER: [&str; 7] =
    ["rune_axe", "adamant_axe", "mithril_axe", "black_axe", "steel_axe", "iron_axe", "bronze_axe"];

/// Shared rank: the server takes the best carried axe at any level, so
/// rank alone decides (see `ToolSpec`). The level-gated shortlist is
/// commented out server-side (woodcut.rs2 woodcutting_axe_checker, 2004
/// behaviour).
pub(crate) const AXE_SPEC: ToolSpec = ToolSpec { order: &AXE_ORDER };

/// Adapter for the `fn`-pointer rank API (`best_banked_by_rank` takes a
/// plain function, never a closure over the spec).
fn axe_rank(name: &str) -> Option<usize> {
    AXE_SPEC.rank(name)
}





/// Pure wait-vs-walk call: Some(wait) when standing still wins, None when
/// leaving wins. Wait wins only when strictly cheaper than the walk to the
/// best alternative (`travel_ticks`, Manhattan lower bound — biased toward
/// leaving) and under the Gone-ban window: waiting longer than the ban
/// that would have applied is never better than leaving, so over-cap
/// expectations degrade to None, logged by the caller. No alternative
/// (None) always waits: there is nowhere cheaper to go.
fn respawn_wait_ticks(expected: u64, walk_alternative: Option<u64>) -> Option<u64> {
    if expected > GONE_BAN_TICKS {
        return None;
    }
    match walk_alternative {
        None => Some(expected),
        Some(walk) if expected < walk => Some(expected),
        Some(_) => None,
    }
}

/// Depleted stump under us: wait for the regrow or walk to the next spot,
/// whichever is cheaper. A wait sets the SHORT ban (regrow ETA) so the
/// repick returns to this tree; leaving keeps the Gone ban (250). The
/// wait is threat-checked every pump (hurt or corpse breaks it, long ban,
/// the loop-top flee/death arms handle the rest) and billed to
/// `idle_since` like any other non-chop time, never to the stall budget.
#[allow(clippy::too_many_arguments)]
fn wait_for_respawn(
    bot: &mut Bot,
    tier: &knowledge::trees::TreeTier,
    spot_x: i32,
    spot_z: i32,
    banned: &mut BanSet,
    tiers: &[knowledge::trees::TreeTier],
    bank: &knowledge::banks::Bank,
    level: i64,
    observe: &mut Observe,
    idle_since: &mut u64,
    except: Option<(i32, i32)>,
    leave_ban: u64,
) -> Result<(), ScriptError> {
    let expected = observe.respawn.get(&tier.product).copied().unwrap_or_else(|| default_respawn_ticks(&tier.product));
    // Sitting out a failed prediction (leave_ban, backed off by dud
    // count) is longer than the ETA; sitting out our own fresh stump is
    // exactly the ETA, so a genuinely regrown tree is never skipped.
    banned.ban_for((spot_x, spot_z), bot.world.ticks, leave_ban);
    // Ticks advanced during the work: drop expired bans so the walk
    // alternative is measured against live spots, not stale clocks.
    banned.retain(bot.world.ticks);
    let alternative = pick_tree_spot(&mut *bot, tiers, bank, level, &mut *banned, except, observe)
        .map(|(_, found_x, found_z)| travel_ticks((found_x - bot.world.x).abs() as u64 + (found_z - bot.world.z).abs() as u64));
    let Some(wait) = respawn_wait_ticks(expected, alternative) else {
        bot.log(&format!(
            "stump at ({spot_x},{spot_z}): regrow ~{expected}t vs walk {}t, leaving",
            alternative.map(|walk| walk.to_string()).unwrap_or_else(|| "none".to_string())
        ));
        return Ok(());
    };
    // Short ban: the repick after the wait may return to this tree.
    banned.ban_for((spot_x, spot_z), bot.world.ticks, wait);
    bot.log(&format!(
        "waiting respawn {wait}t at ({spot_x},{spot_z}) (regrow ~{expected}t vs walk {}t)",
        alternative.map(|walk| walk.to_string()).unwrap_or_else(|| "none".to_string())
    ));
    bot.intend(&format!("waiting for the tree at ({spot_x},{spot_z}) to regrow"));
    let start = bot.world.ticks;
    let hitpoints_before = hitpoints(bot);
    while bot.world.ticks - start < wait {
        bot.pump(2)?;
        if hitpoints(bot) < hitpoints_before || hitpoints(bot) == 0 {
            bot.log("threat during respawn wait, leaving the stump");
            banned.ban_for((spot_x, spot_z), bot.world.ticks, GONE_BAN_TICKS);
            break;
        }
        if tree_still_there(bot, spot_x, spot_z, &tier.trees) {
            break;
        }
    }
    *idle_since += bot.world.ticks - start;
    let elapsed = bot.world.ticks - start;
    if elapsed > 0 && tree_still_there(bot, spot_x, spot_z, &tier.trees) {
        // Remaining time, not the full delay (arrival reads mid-regrow),
        // so the ETA only ratchets up toward the true server rate.
        observe.learn_respawn(&tier.product, elapsed);
        bot.log(&format!("regrew at ({spot_x},{spot_z}) after {elapsed}t"));
    }
    Ok(())
}

/// Click once per stand, then only re-click after a quiet round (the server
/// chains attempts itself while in range). Stands rotate on refusal: the
/// arrival tile first when footprint-operable, else the footprint ring
/// nearest-first. A click refusal from an inoperable stand proves nothing
/// about the tree (a 3x3 yew always refuses diagonal-to-anchor corners, yet
/// cuts fine from a side) — rotate, never ban. Only refusals from operable
/// stands report UnreachableSpot (the strike path); stands that cannot even
/// be reached report Unreachable (transient walk failure, short ban).
///
/// Ring-pass verdict after every stand had its click: an operable click
/// refusal anywhere is the strike path (UnreachableSpot); when no stand
/// could even be clicked from, the pass is transient (Unreachable).
/// Pure so tests lock the rotation rule: one walled stand must never
/// shadow the good ones into a condemn.
fn ring_pass_verdict(refused_operable: bool) -> WorkOutcome {
    if refused_operable {
        WorkOutcome::UnreachableSpot
    } else {
        WorkOutcome::Unreachable
    }
}

fn work_spot(
    bot: &mut Bot,
    tree_x: i32,
    tree_z: i32,
    log_ids: &[u16],
    had_logs: u32,
    banned: &mut BanSet,
    travel_dead: &mut Option<((i32, i32), (i32, i32))>,
) -> Result<(WorkOutcome, u32), ScriptError> {
    if (bot.world.x - tree_x).abs() + (bot.world.z - tree_z).abs() > 4 {
        // Dead socket or operator stop, not an unreachable tree: typed
        // all the way to the session instead of a quiet Unreachable.
        match walk_checked(bot, tree_x, tree_z, 6000) {
            Ok(true) => {}
            Ok(false) => return Ok((WorkOutcome::Unreachable, 0)),
            Err(error) if crate::skill_common::is_walk_abort(&error) => return Err(error),
            Err(ScriptError::Sdk(lifescape_sdk::Error::Travel(detail))) if detail.contains("nav-down") => {
                // Transitory service outage (unknown-ticket under load,
                // L1 full): not a spot verdict — fail loud with the typed
                // error, no ban, no travel_dead; the session stays alive.
                return Err(ScriptError::Sdk(lifescape_sdk::Error::Travel(detail)));
            }
            Err(ScriptError::Sdk(lifescape_sdk::Error::Travel(_))) => {
                // Planner proof: disconnected from here. Long ban (re-plan
                // from the same tile is futile) + never re-pick from this
                // tile until the bot moves.
                let ban = walk_fail_ban(true);
                bot.log(&format!("no route to ({tree_x},{tree_z}) from {:?}, long-banning {ban}t", bot.position()));
                banned.ban_for((tree_x, tree_z), bot.world.ticks, ban);
                *travel_dead = Some(((tree_x, tree_z), (bot.world.x, bot.world.z)));
                return Ok((WorkOutcome::Unreachable, 0));
            }
            Err(_) => return Ok((WorkOutcome::Unreachable, 0)),
        }
    }
    if (bot.world.x - tree_x).abs() + (bot.world.z - tree_z).abs() > 1 {
        if !crate::skill_walk::step_adjacent(bot, tree_x, tree_z)? {
            return Ok((WorkOutcome::Unreachable, 0));
        }
    }
    let tree_loc = live_loc_id(bot, tree_x, tree_z)?;
    let (foot_w, foot_l) = spot_footprint(bot, tree_x, tree_z);
    let mut stands: Vec<((i32, i32), bool)> = Vec::new();
    if operable_stand(bot.world.x, bot.world.z, tree_x, tree_z, foot_w, foot_l) {
        stands.push(((bot.world.x, bot.world.z), true));
    } else {
        bot.log(&format!(
            "stand ({},{}) not footprint-operable for ({tree_x},{tree_z}), rotating stands",
            bot.world.x, bot.world.z
        ));
    }
    let mut ring = operable_ring(tree_x, tree_z, foot_w, foot_l);
    ring.sort_by_key(|tile| (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs());
    for tile in ring {
        if !stands.iter().any(|(known, _)| *known == tile) {
            stands.push((tile, true));
        }
    }
    let mut gained = 0u32;
    // A refused operable stand no longer ends the pass: the click can be
    // refused by a wall edge between a geometrically adjacent stand and
    // the trunk (the server also wants the wall edge open), while another
    // ring stand cuts fine. Returning on the first refusal let the nearest
    // (walled) stand shadow the good ones, and two strikes condemned a
    // cuttable tree (Edgeville yews, structural bans).
    let mut refused_operable = false;
    let mut source = TreeSource { loc: tree_loc, x: tree_x, z: tree_z };
    for ((stand_x, stand_z), operable) in stands {
        if (bot.world.x, bot.world.z) != (stand_x, stand_z) {
            match bot.walk_to(stand_x, stand_z, bot.world.level, 100) {
                Ok(()) => {}
                Err(error)
                    if matches!(
                        error,
                        lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)
                    ) =>
                {
                    return Err(error.into())
                }
                Err(_) => continue,
            }
        }
        let (end, got) = watch_stand(bot, &mut source, tree_x, tree_z, log_ids, had_logs + gained)?;
        gained += got;
        match end {
            WorkOutcome::Gone => return Ok((WorkOutcome::Gone, gained)),
            WorkOutcome::UnreachableSpot => {
                refused_operable |= operable;
                bot.log(&format!(
                    "click refused from {}operable stand at {:?}, rotating ring",
                    if operable { "" } else { "in" },
                    bot.position()
                ));
            }
            other => return Ok((other, gained)),
        }
    }
    // Verdict after the full ring rotation: an operable click refusal
    // anywhere is the strike path (one strike per pass, as before); when
    // every stand failed to walk there, the pass is transient, short ban,
    // never a strike.
    Ok((ring_pass_verdict(refused_operable), gained))
}

/// Re-click decision after a quiet round: never when a swing was acked
/// on a still-live tree (the player is cutting — a re-click restarts the
/// swing server-side); otherwise probe reachability (a wall-separated
/// tree only refuses when clicked). Pure so the cadence rule is locked
/// by tests without a live connection (`op_loc`/`pump` need a login).
fn probe_after_silence(acked: bool, live: bool) -> bool {
    !(acked && live)
}

/// Tree click-and-watch on the shared harvest core: the loc is resolved
/// once per spot (stream first, template fallback), the ring stands rotate
/// around it, the watch loop is `kit::harvest`.
struct TreeSource {
    loc: u16,
    x: i32,
    z: i32,
}

impl HarvestSource for TreeSource {
    fn acks(&self) -> &[&str] {
        &WOODCUT_ACKS
    }

    fn click(&mut self, bot: &mut Bot, x: i32, z: i32) -> Result<HarvestTarget, ScriptError> {
        bot.log(&format!("click tree at ({x},{z})"));
        bot.intend(&format!("chopping a tree at ({x},{z})"));
        bot.op_loc(1, x, z, self.loc)?;
        Ok(HarvestTarget::Loc { loc: self.loc })
    }

    fn slot_gone(&self, _bot: &mut Bot, _target: &HarvestTarget) -> bool {
        false
    }

    fn slot_moved(&self, _bot: &mut Bot, _target: &HarvestTarget) -> bool {
        false
    }

    fn target_live(&self, bot: &mut Bot, _target: &HarvestTarget) -> bool {
        !is_stump(bot, self.x, self.z) && live_loc_id(bot, self.x, self.z).is_ok()
    }

    /// Stump on the first quiet round: the tree is depleted, Gone at once
    /// instead of waiting out a second round on a stump.
    fn depleted_now(&self, bot: &mut Bot, _x: i32, _z: i32, silences: u32) -> bool {
        silences == 1 && is_stump(bot, self.x, self.z)
    }

    fn reprobe(&self, acked: bool, live: bool, _since_probe: u64) -> bool {
        probe_after_silence(acked, live)
    }

    fn poll_events(&self) -> bool {
        true
    }

    fn note_dry_streak(&mut self, bot: &mut Bot, x: i32, z: i32, acked: bool, live: bool, silences: u32) {
        if silences == 2 && acked && live {
            bot.log(&format!("still cutting at ({x},{z}), waiting out the dry streak"));
        }
    }
}

fn recover_axe(bot: &mut Bot, coins: u16, bank: &knowledge::banks::Bank) -> Result<bool, ScriptError> {
    if AXE_SPEC.best_held(bot).is_some() {
        return Ok(true);
    }
    if reattach_tool(bot, HANDLE_NAME, "_hatchethead", &AXE_SPEC)? {
        return Ok(true);
    }
    if pack_total(bot, coins) >= 16 && buy_stock(bot, "bronze_axe", 1)? > 0 {
        bot.log("ok replacement axe bought");
        return Ok(true);
    }
    let keep = vec![coins];
    if bank_trip(bot, bank, &keep, 16u32.saturating_sub(pack_total(bot, coins)))? == 0 {
        return Ok(false);
    }
    if AXE_SPEC.best_held(bot).is_some() {
        bot.log("ok replacement axe restored");
        return Ok(true);
    }
    Ok(pack_total(bot, coins) >= 16 && buy_stock(bot, "bronze_axe", 1)? > 0)
}

/// Woodcut policy on the shared driver: the pack-full gate ends the
/// load, a lost axe exits for caller-side recovery (reattach, shop,
/// bank), tier upgrades and cleared trees drop the spot, stumps wait for
/// the regrow, refusal strikes condemn, pocket landings escape to the
/// bank. Bans, regrow memory and strike counts outlive one load.
struct WoodOps<'a> {
    tiers: &'a [knowledge::trees::TreeTier],
    bank: &'a mut knowledge::banks::Bank,
    coins: u16,
    level: i64,
    log_ids: Vec<u16>,
    observe: &'a mut Observe,
    streak: u32,
    travel_dead: Option<((i32, i32), (i32, i32))>,
}

impl crate::skill_loop::LoopOps for WoodOps<'_> {
    type Spot = knowledge::trees::TreeTier;

    fn is_full(&self, bot: &Bot) -> bool {
        free_slots(bot) <= usize::from(pack_total(bot, self.coins) > 0)
    }

    fn has_tool(&self, bot: &Bot) -> bool {
        AXE_SPEC.best_held(bot).is_some()
    }

    fn gain_of(&self, bot: &Bot, _spot: &Self::Spot) -> u32 {
        self.log_ids.iter().map(|id| pack_total(bot, *id)).sum()
    }

    fn pick(&mut self, bot: &mut Bot, state: &mut crate::skill_loop::LoopState) -> Option<(Self::Spot, i32, i32)> {
        self.level = learn_level(bot, WC_STAT, self.level);
        // Cluster hop: the spot may sit near another bank, so re-sync on
        // every pick (cheap, no walking) to keep the trip short. The
        // driver also clears planner-dead picks through `travel_dead`.
        if let Ok(fresh) = nearest_bank(bot) {
            if (fresh.x, fresh.z, fresh.level) != (self.bank.x, self.bank.z, self.bank.level) {
                bot.log(&format!(
                    "cluster change: bank re-synced to ({},{}) near {:?}",
                    fresh.x,
                    fresh.z,
                    bot.position()
                ));
                *self.bank = fresh;
            }
        }
        let except = self.travel_dead.and_then(|(dead, from)| {
            (from == (bot.world.x, bot.world.z) && state.banned.contains(dead)).then_some(dead)
        });
        match pick_tree_spot(bot, self.tiers, &*self.bank, self.level, &mut state.banned, except, self.observe) {
            Some((ref tier, spot_x, spot_z)) => {
                bot.log(&format!("chopping {} at ({spot_x},{spot_z})", tier.product));
                bot.intend(&format!("chopping {} at ({spot_x},{spot_z})", tier.product));
                Some((tier.clone(), spot_x, spot_z))
            }
            None => {
                bot.log("no tree spot, waiting for respawns");
                bot.intend("waiting for a safe tree spot");
                None
            }
        }
    }

    fn keep_spot(
        &mut self,
        bot: &mut Bot,
        state: &mut crate::skill_loop::LoopState,
        spot: &Self::Spot,
        at: (i32, i32),
    ) -> bool {
        self.level = learn_level(bot, WC_STAT, self.level);
        if let Some(best) = pick_tier(self.tiers, self.level) {
            if spot.level < best.level {
                bot.log("level up, switching tier");
                return false;
            }
        }
        if !tree_still_there(bot, at.0, at.1, &spot.trees) {
            bot.log(&format!("tree gone at ({},{})", at.0, at.1));
            if is_stump(bot, at.0, at.1) {
                // Timestamp the sighting: the pick predicts this regrow
                // instead of trusting the stale stream state forever.
                self.observe.note_stump(at, bot.world.ticks);
            }
            state.banned.ban_for(at, bot.world.ticks, GONE_BAN_TICKS);
            return false;
        }
        true
    }

    fn work(
        &mut self,
        bot: &mut Bot,
        state: &mut crate::skill_loop::LoopState,
        _spot: &Self::Spot,
        at: (i32, i32),
        had: u32,
    ) -> Result<(WorkOutcome, u32), ScriptError> {
        work_spot(bot, at.0, at.1, &self.log_ids, had, &mut state.banned, &mut self.travel_dead)
    }

    fn on_outcome(
        &mut self,
        bot: &mut Bot,
        cfg: &crate::skill_loop::LoadCfg,
        state: &mut crate::skill_loop::LoopState,
        spot: &Self::Spot,
        at: (i32, i32),
        outcome: WorkOutcome,
        gained: u32,
    ) -> Result<crate::skill_loop::LoadExit, ScriptError> {
        use crate::skill_loop::LoadExit;
        let (spot_x, spot_z) = at;
        // Strikes only survive short bans and walk failures: any other
        // outcome re-proves the stand (chopped, stumped, fled, locked).
        if !matches!(outcome, WorkOutcome::Unreachable | WorkOutcome::UnreachableSpot) {
            self.observe.refusals.remove(&at);
        }
        if outcome != WorkOutcome::Unreachable {
            self.streak = 0;
            // Cluster hop: the spot may sit near another bank, so re-sync
            // on arrival (cheap, no walking) to keep the trip short.
            let fresh = nearest_bank(bot)?;
            if (fresh.x, fresh.z, fresh.level) != (self.bank.x, self.bank.z, self.bank.level) {
                bot.log(&format!(
                    "cluster change: bank re-synced to ({},{}) near {:?}",
                    fresh.x,
                    fresh.z,
                    bot.position()
                ));
                *self.bank = fresh;
            }
        }
        match outcome {
            WorkOutcome::Gone => {
                if is_stump(bot, spot_x, spot_z) {
                    // Depleted under us: timestamp the sighting so the
                    // pick predicts this regrow instead of trusting the
                    // stale stream state after the ban expires. Gainless
                    // means the prediction failed (still a stump on
                    // arrival): back the sit-out off so repeats empty the
                    // pool and the loop degrades instead of touring stumps.
                    self.observe.note_stump(at, bot.world.ticks);
                    let expected = self.observe.respawn.get(&spot.product).copied().unwrap_or_else(|| default_respawn_ticks(&spot.product));
                    let leave_ban = self.observe.dud_ban(at, gained, expected);
                    // Waiting for the regrow beats re-banning blind when
                    // the walk costs more.
                    let except = self.travel_dead.and_then(|(dead, from)| {
                        (from == (bot.world.x, bot.world.z) && state.banned.contains(dead)).then_some(dead)
                    });
                    wait_for_respawn(
                        bot,
                        spot,
                        spot_x,
                        spot_z,
                        &mut state.banned,
                        self.tiers,
                        self.bank,
                        self.level,
                        self.observe,
                        &mut state.idle_since,
                        except,
                        leave_ban,
                    )?;
                } else {
                    bot.log("tree quiet, next one");
                    state.banned.ban_for(at, bot.world.ticks, GONE_BAN_TICKS);
                }
                state.spot = None;
            }
            WorkOutcome::NoTool => {
                state.spot = None;
            }
            WorkOutcome::Unreachable => {
                // Walk-leg failure (never arrived, leg timeout, step refused
                // far away): short ban + retry, NEVER structural or long.
                bot.log(&format!("unreachable, short-banning ({spot_x},{spot_z}) for {GONE_BAN_TICKS}t (walk fell short)"));
                state.banned.ban_for(at, bot.world.ticks, GONE_BAN_TICKS);
                state.spot = None;
                self.streak += 1;
                // Pocket landing (yank mid-run): every spot fails at the
                // walk. Travel back to the bank once, then re-pick from
                // the new region; the stall bound stays the loud exit.
                if self.streak >= 8 {
                    let dest = lifescape_sdk::plan::Tile { x: self.bank.x as i32, z: self.bank.z as i32, level: self.bank.level as i32 };
                    match bot.travel_to(dest) {
                        Ok(()) => {}
                        Err(error @ (lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_))) => {
                            return Err(error.into());
                        }
                        Err(error) => bot.log(&format!("pocket escape refused: {error}")),
                    }
                    // One escape per streak: a refused escape re-arms by
                    // counting, it never re-fires on the next spot.
                    self.streak = 0;
                }
            }
            WorkOutcome::UnreachableSpot => {
                // Direct server refusal of the chop click itself, from a
                // footprint-operable stand (work_spot rotates inoperable
                // stands away before reporting). Structural ONLY on a
                // repeated operable refusal of a standing tree.
                let (foot_w, foot_l) = spot_footprint(bot, spot_x, spot_z);
                let operable = operable_stand(bot.world.x, bot.world.z, spot_x, spot_z, foot_w, foot_l);
                let stump_here = is_stump(bot, spot_x, spot_z);
                let ban = if stump_here {
                    // Gainless stump: a failed regrow prediction, backed
                    // off like the Gone path so repeats empty the pool.
                    // Gains prove we cut it ourselves: plain regrow ETA.
                    // Timestamp it too: the pick predicts this regrow.
                    self.observe.note_stump(at, bot.world.ticks);
                    let expected = self.observe.respawn.get(&spot.product).copied().unwrap_or_else(|| default_respawn_ticks(&spot.product));
                    self.observe.dud_ban(at, gained, expected)
                } else {
                    refusal_ban_ticks(operable, false, true, self.observe.refusals.get(&at).copied().unwrap_or(0))
                };
                if stump_here {
                    bot.log(&format!("tree stumped at ({spot_x},{spot_z}), back in ~{ban}t"));
                } else if operable && ban == STRUCTURAL_BAN_TICKS {
                    bot.log(&format!("structurally unreachable, long-banning ({spot_x},{spot_z})"));
                    // Condemned: the ban is the memory now, drop the count
                    // so a starvation release re-proves permanence cleanly.
                    self.observe.refusals.remove(&at);
                } else if operable {
                    let strikes = self.observe.refusals.entry(at).or_insert(0);
                    *strikes += 1;
                    bot.log(&format!(
                        "spot refused once at ({spot_x},{spot_z}), short-banning {ban}t (strike {}/{DIRECT_REFUSALS_TO_CONDEMN}), retry before condemning",
                        *strikes
                    ));
                } else {
                    bot.log(&format!("click refused off-stand at ({spot_x},{spot_z}), walk fell short, short ban {ban}t"));
                }
                state.banned.ban_for(at, bot.world.ticks, ban);
                state.spot = None;
            }
            WorkOutcome::LevelLocked => {
                state.banned.ban_for(at, bot.world.ticks, 500);
                state.spot = None;
            }
            WorkOutcome::Threat => {
                bot.log("hunter close, re-picking a spot");
                state.banned.ban_for(at, bot.world.ticks, GONE_BAN_TICKS);
                state.spot = None;
            }
            WorkOutcome::Blocked => {
                bot.log("unknown dialog blocking chop, aborting load");
                return Ok(LoadExit::Abort);
            }
            WorkOutcome::Full | WorkOutcome::Moved | WorkOutcome::NoBait => {
                state.spot = None;
            }
        }
        Ok(crate::skill_loop::stall_or_continue(cfg, state, gained, bot.world.ticks))
    }
}

fn chop_load(
    bot: &mut Bot,
    tiers: &[knowledge::trees::TreeTier],
    bank: &mut knowledge::banks::Bank,
    coins: u16,
    level: i64,
    banned: &mut BanSet,
    observe: &mut Observe,
) -> Result<(bool, i64), ScriptError> {
    let log_ids: Vec<u16> = tiers.iter().filter_map(|tier| obj_id(bot, &tier.product).ok()).collect();
    // Threat bans outlive one load: move the caller's set into the driver
    // state, run, move back (restored on every exit below, errors too).
    let mut state = crate::skill_loop::LoopState::resume(bot, std::mem::take(banned));
    let out = chop_inner(bot, tiers, bank, coins, level, &mut state, log_ids, observe);
    *banned = std::mem::take(&mut state.banned);
    out
}

#[allow(clippy::too_many_arguments)]
fn chop_inner(
    bot: &mut Bot,
    tiers: &[knowledge::trees::TreeTier],
    bank: &mut knowledge::banks::Bank,
    coins: u16,
    level: i64,
    state: &mut crate::skill_loop::LoopState,
    log_ids: Vec<u16>,
    observe: &mut Observe,
) -> Result<(bool, i64), ScriptError> {
    let mut ops = WoodOps { tiers, bank, coins, level, log_ids, observe, streak: 0, travel_dead: None };
    loop {
        if AXE_SPEC.best_held(bot).is_none() {
            // Recovery walks (shop, bank) are not chopping: don't bill
            // them to the no-log stall budget.
            let spent = bot.world.ticks;
            if !recover_axe(bot, coins, ops.bank)? {
                bot.log("axe lost, banking load");
                return Ok((true, learn_level(bot, WC_STAT, ops.level)));
            }
            state.idle_since += bot.world.ticks - spent;
        }
        if crate::skill_loop::run_load(bot, &crate::skill_loop::LoadCfg::mining(), state, &mut ops)? {
            return Ok((true, learn_level(bot, WC_STAT, ops.level)));
        }
        if AXE_SPEC.best_held(bot).is_none() {
            continue;
        }
        return Ok((false, learn_level(bot, WC_STAT, ops.level)));
    }
}




/// Cut banked loads in the open session (0 = forever).
pub fn run_woodcut(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let loads: u32 = args.first().and_then(|arg| arg.parse().ok()).unwrap_or(0);
    if args.iter().any(|arg| arg == "toll") {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    if args.iter().any(|arg| arg == "wild") {
        bot.allow_wild = true;
        bot.log("hunter zones allowed for this run");
    }
    crate::skill_common::wait_pack_streamed(bot)?;
    check_events(bot)?;
    let coins = obj_id(bot, "coins")?;
    let tiers = knowledge::trees::all_tiers();
    let mut level = learn_level(bot, WC_STAT, 1);
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let log_ids: HashSet<u16> = tiers.iter().filter_map(|tier| obj_id(bot, &tier.product).ok()).collect();
    let mut bank_coins = 0u32;
    let mut done = 0u32;
    // Threat bans outlive one load: a killer tree stays banned while its
    // neighbours get cut, instead of repicking the same death every load.
    let mut banned: BanSet = BanSet::new();
    // Observation memory for the run (regrow ETAs, stump sightings,
    // refusal strikes, gainless visits): owned by the bot, never shared,
    // never persisted — it dies with the run.
    let mut observe = Observe::fresh();
    while loads == 0 || done < loads {
        // Death may have moved us across the map: re-sync the bank cheaply
        // (no walking) so spots and treks start from the real position.
        bank = nearest_bank(bot)?;
        check_events(bot)?;
        // Death abort inside means the load was lost: re-gear (rebuy the
        // axe when missing) and retry the load, for as long as it takes.
        // Broke is not an error either: no axe means hunt again next
        // iteration (combat xp always progresses). Only conn/interrupt
        // and structural failures (toll, unreachable bank) fail loud.
        // Anything else fails loud as before.
        match crate::legacy::bootstrap::ensure_axe(bot, &mut bank, coins, &log_ids) {
            Ok(Some(coins_)) => bank_coins = coins_,
            Ok(None) => {}
            Err(error) if crate::skill_common::is_death_abort(&error) => {
                bot.log("died re-gearing, retrying the load");
                continue;
            }
            Err(error) => return Err(error),
        }
        if AXE_SPEC.best_held(bot).is_none() {
            bot.log("still axeless after re-gear, hunting again");
            continue;
        }
        let (ok, new_level) = chop_load(bot, &tiers, &mut bank, coins, level, &mut banned, &mut observe)?;
        level = new_level;
        // A failed load (threats, stumps, stall) retries with bans kept:
        // stumps regrow and hunters wander off.
        if !ok {
            bot.log("chop load failed, retrying with fresh bans");
            continue;
        }
        let (cands, cheapest) = AXE_SPEC.cheapest_upgrade(bot, |_| true);
        if !cands.is_empty() {
            let have = pack_total(bot, coins);
            sell_for_target(
                bot,
                &log_ids.iter().copied().collect::<Vec<_>>(),
                coins,
                (cheapest as u32).saturating_sub(bank_coins + have),
            )?;
            if crate::legacy::bootstrap::can_buy_now(pack_total(bot, coins), cheapest, &cands) {
                match crate::legacy::bootstrap::buy_axe_upgrade(bot, &cands)? {
                    Some(_) => {}
                    None => {
                        bot.log(&format!(
                            "upgrade missed: {} too costly or keeper unreachable at {:?}, {} coins banked next trip, never carried to spot",
                            cands.join("+"),
                            bot.position(),
                            pack_total(bot, coins)
                        ));
                    }
                }
            }
        }
        let keep: Vec<u16> = AXE_SPEC.keep_ids(bot);
        // Death on the bank trip loses the load: re-gear next iteration
        // and retry it instead of failing the run. Anything else fails
        // loud as before.
        match bank_trip_resilient(bot, &mut bank, &keep, cheapest as u32, &log_ids.iter().copied().collect::<Vec<_>>()) {
            Ok(coins_) => bank_coins = coins_,
            Err(error) if crate::skill_common::is_death_abort(&error) => {
                bot.log("died on the bank trip, re-gearing to continue");
                continue;
            }
            Err(error) => return Err(error),
        }
        // Post-bank upgrade: only what the pack affords, best-first with
        // bronze fallback inside. No coins means no walk to Bob at all.
        // The shop trip can leave two axes in the pack (old + upgrade):
        // stash then, so the spot sees one axe and no coins.
        if pack_total(bot, coins) > 0 {
            let _ = crate::legacy::bootstrap::buy_axe_upgrade(bot, &cands)?;
        }
        let axes_held: u32 = AXE_SPEC.known_ids(bot).iter().map(|id| pack_total(bot, *id)).sum();
        if pack_total(bot, coins) > 0 || axes_held > 1 {
            // Failed buys must not ride to the spot: stash now or abort.
            let keep_now: Vec<u16> = AXE_SPEC.keep_ids(bot);
            bank_trip(bot, &bank, &keep_now, 0)?;
            if pack_total(bot, coins) > 0 {
                return Err(ScriptError::Usage(format!(
                    "coins left after banking at {:?}, refusing to carry to spot",
                    bot.position()
                )));
            }
        }
        done += 1;
        bot.log(&format!("ok load {done} banked (wc={level})"));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::{respawn_wait_ticks, AXE_SPEC};
    use crate::skill_common::{death_message_seen, WorkOutcome};    use crate::tables_live::attach_live_tables;
    use crate::skill_gather::death_since;
    use lifescape_sdk::Bot;

    /// Death since an index: the mid-work message fires on every work
    /// outcome path (Threat/Gone/Full), the resync never replays it.
    #[test]
    fn death_since_detects_message_on_every_work_outcome() {
        for outcome in [WorkOutcome::Threat, WorkOutcome::Gone, WorkOutcome::Full] {
            let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
            attach_live_tables(&mut bot);
            bot.world.stats.insert(3, (10, 0));
            let seen = bot.world.messages.len();
            bot.world.messages.push("You get some oak logs.".to_string());
            bot.world.messages.push("Oh dear you are dead!".to_string());
            let (died, fresh) = death_since(&bot, seen);
            assert!(died, "death message detected on {outcome:?} path");
            assert_eq!(fresh, 2, "resync covers everything observed");
            let (again, _) = death_since(&bot, fresh);
            assert!(!again, "resync never replays on {outcome:?} path");
        }
    }

    /// Flatlined hp detects death with no message at all.
    #[test]
    fn death_since_detects_hp_zero_without_message() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.stats.insert(3, (0, 0));
        let (died, fresh) = death_since(&bot, 0);
        assert!(died, "hp 0 detects death alone");
        assert_eq!(fresh, 0, "no messages, index stays");
    }

    /// Ordinary chatter never trips the detector.
    #[test]
    fn death_since_ignores_unrelated_chatter() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.stats.insert(3, (10, 0));
        bot.world.messages.push("You get some oak logs.".to_string());
        let (died, fresh) = death_since(&bot, 0);
        assert!(!died, "loot chatter is not death");
        assert_eq!(fresh, 1, "chatter still resyncs the index");
    }





    /// Bootstrap fallback order: with 16 coins only bronze is affordable
    /// (bronze 16, iron 56, steel 200 from the dump). The upgrade loop must
    /// skip the dearer axes instead of fatally trying iron first.
    #[test]
    fn fallback_order_skips_unaffordable_with_16() {
        use crate::skill_common::obj_price;
        let bronze = obj_price("bronze_axe");
        let iron = obj_price("iron_axe");
        let steel = obj_price("steel_axe");
        assert_eq!((bronze, iron, steel), (16, 56, 200), "dump prices, got: {bronze}/{iron}/{steel}");
        let cands = ["steel_axe", "iron_axe", "bronze_axe"];
        let affordable: Vec<&&str> = cands.iter().filter(|name| obj_price(name) <= 16).collect();
        assert_eq!(affordable, vec![&"bronze_axe"], "16 coins fund bronze only, got: {affordable:?}");
    }

    /// Bank keep holds one axe only: the best carried. Coins always land
    /// in the bank, never ride to the spot; inferior axes are banked, not
    /// doubled up (the server chops with the best carried axe at any
    /// level, so a second axe is dead weight).
    #[test]
    fn chop_keep_never_holds_coins() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let coins = crate::skill_common::obj_id(&bot, "coins").expect("coins in tables");
        let bronze = crate::skill_common::obj_id(&bot, "bronze_axe").expect("bronze_axe in tables");
        let steel = crate::skill_common::obj_id(&bot, "steel_axe").expect("steel_axe in tables");
        let com = (bot.tables.com_id)("inventory:inv").expect("inventory:inv in dumps");
        bot.world.inv.insert(com, [(0u8, (bronze, 1u32)), (1, (steel, 1))].into_iter().collect());
        let keep = AXE_SPEC.keep_ids(&bot);
        assert_eq!(keep, vec![steel], "one axe in the pack: the best, got: {keep:?}");
        assert!(!keep.contains(&coins), "coins always banked, never carried");
        assert!(!keep.contains(&bronze), "inferior axe banked, never doubled");
    }

    /// Restore withdraws the best banked axe, never a second or a worse
    /// one (rank order is AXE_ORDER best-first).
    #[test]
    fn bank_restore_withdraws_best_rank() {
        use super::{axe_rank, AXE_SPEC};
        use crate::skill_gather::best_banked_by_rank;
        use std::collections::HashMap;
        assert!(axe_rank("steel_axe") < axe_rank("bronze_axe"), "steel outranks bronze");
        assert_eq!(axe_rank("no_such_axe"), None, "unknown axes never rank");
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let bronze = crate::skill_common::obj_id(&bot, "bronze_axe").expect("bronze_axe in tables");
        let steel = crate::skill_common::obj_id(&bot, "steel_axe").expect("steel_axe in tables");
        let axes = AXE_SPEC.known_ids(&bot);
        let slots: HashMap<u8, (u16, u32)> = [(3u8, (bronze, 1u32)), (7, (steel, 1))].into_iter().collect();
        assert_eq!(best_banked_by_rank(&slots, &axes, axe_rank), Some((7, steel)), "best banked axe wins");
        assert_eq!(best_banked_by_rank(&HashMap::new(), &axes, axe_rank), None, "empty bank restores nothing");
    }

    /// Death by message: hp stream can lag, the chat line still resets the
    /// spot and the death/respawn ticks are not billed to the stall budget
    /// (same `idle_since += now - spent` as the axe-recovery branch).
    #[test]
    fn death_message_detects_with_hp_up() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.stats.insert(3, (10, 0));
        assert_eq!(crate::skill_common::hitpoints(&bot), 10, "hp up, message path must trigger alone");
        bot.world.messages.push("Oh dear you are dead!".to_string());
        assert!(death_message_seen(&bot, 0), "exact line detected");
        assert!(!death_message_seen(&bot, 1), "index tracks seen messages");
        bot.world.messages.push("OH DEAR you ARE dead".to_string());
        assert!(death_message_seen(&bot, 1), "case-insensitive");
        // Chop-load handling for that detection: spot reset, idle not billed.
        let mut idle_since = 100u64;
        let spent = 100u64;
        let waited_until = 140u64;
        idle_since += waited_until - spent;
        assert_eq!(idle_since, 140, "death wait not billed to the stall budget");
        let mut spot: Option<(i32, i32)> = Some((3086, 3243));
        spot = None;
        assert!(spot.is_none(), "spot reset after death");
    }

    /// Wait-vs-walk on chiffré cases (GONE_BAN_TICKS = 250 cap): oak 30
    /// vs a 200-tile walk waits 30; yew 200 vs a 50-tile walk leaves;
    /// magic 400 over the cap leaves even with nowhere better to go
    /// (the ban repick revisits); a tie walks; no alternative waits.
    #[test]
    fn respawn_wait_wins_only_when_strictly_cheaper() {
        use crate::skill_common::GONE_BAN_TICKS;
        assert_eq!(respawn_wait_ticks(30, Some(200)), Some(30), "oak 30 < walk 200 waits");
        assert_eq!(respawn_wait_ticks(200, Some(50)), None, "yew 200 > walk 50 leaves");
        assert_eq!(respawn_wait_ticks(400, Some(500)), None, "magic 400 over cap leaves");
        assert_eq!(respawn_wait_ticks(400, None), None, "magic 400 over cap even with nowhere to go");
        assert_eq!(respawn_wait_ticks(30, Some(30)), None, "tie walks");
        assert_eq!(respawn_wait_ticks(30, None), Some(30), "nowhere to go waits");
        assert_eq!(respawn_wait_ticks(GONE_BAN_TICKS, Some(1000)), Some(GONE_BAN_TICKS), "cap itself still waits");
        assert_eq!(respawn_wait_ticks(GONE_BAN_TICKS + 1, Some(1000)), None, "one over cap leaves");
    }

    /// Cadence lock: silent rounds with an acked swing on a live tree
    /// never re-click (one `op_loc` per stand — the player is cutting, a
    /// re-click would restart the swing server-side); silent rounds
    /// without an ack keep the re-click probe (a lost click or a wall
    /// only shows on refusal). A dead/stumped tree still probes: the Gone
    /// verdict (`silences >= 2 && !(acked && live)`) owns that path.
    #[test]
    fn silence_probe_never_reclicks_an_acked_live_tree() {
        assert!(
            !super::probe_after_silence(true, true),
            "acked swing on a live tree waits, exactly one op_loc per stand"
        );
        assert!(
            super::probe_after_silence(false, true),
            "no ack on a live tree re-clicks the reachability probe"
        );
        assert!(
            super::probe_after_silence(true, false),
            "acked swing on a gone tree re-clicks: Gone owns the verdict"
        );
        assert!(
            super::probe_after_silence(false, false),
            "no ack on a gone tree re-clicks the probe before Gone fires"
        );
    }

    /// Ring-pass verdict: an operable click refusal anywhere is the strike
    /// path (UnreachableSpot); a pass where no stand could even be clicked
    /// from is transient (Unreachable). Locks the Edgeville-yew rule: one
    /// walled stand (west of the trunk, oldwall in between) must never
    /// shadow the good ones into a structural condemn.
    #[test]
    fn ring_pass_verdict_strikes_only_on_operable_refusal() {
        assert!(
            matches!(super::ring_pass_verdict(true), WorkOutcome::UnreachableSpot),
            "operable refusal anywhere strikes"
        );
        assert!(
            matches!(super::ring_pass_verdict(false), WorkOutcome::Unreachable),
            "pure walk failures stay transient"
        );
    }

    /// Service outage is not a spot verdict: `nav-down` from the walk
    /// passes through typed (loud run failure, live session) with no ban
    /// and no `travel_dead`. Mapless bot, no service handle — the first
    /// plan step fails `nav-down` before any socket exists.
    #[test]
    fn nav_down_walk_fails_loud_without_ban() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3092;
        bot.world.z = 3230;
        let (tree_x, tree_z) = (3092, 3246);
        let mut banned = crate::skill_gather::BanSet::new();
        let mut travel_dead: Option<((i32, i32), (i32, i32))> = None;
        let error = super::work_spot(&mut bot, tree_x, tree_z, &[], 0, &mut banned, &mut travel_dead)
            .expect_err("nav-down fails loud, never a quiet Unreachable");
        assert!(error.to_string().contains("nav-down"), "typed nav-down, got: {error}");
        assert!(!banned.contains((tree_x, tree_z)), "outage bans nothing");
        assert!(travel_dead.is_none(), "outage poisons no pick");
    }

}
