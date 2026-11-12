//! Fight: target acquisition and the kill loop. Moved out of the
//! melee script: the attack itself, the wait for the death, the style
//! rotation (pressed through `kit::style`), eating (`kit::survival`),
//! re-wearing (`kit::gear`) and looting (the combat module until the
//! loot brick lands).

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::waits::wait_ticks;
use lifescape_sdk::Bot;

use crate::kit::gear::{current_weapon_category, equip_plan, gear_plan, melee_level, prepare_at_bank, rewear};
use crate::kit::style::{combat_xp, controlled_button, current_skill, fight_weapon_style, melee_stat, next_style, set_style, style_damage_key, weakest_melee};
use crate::kit::survival::{bank_stocks_food, eat, food_count, should_eat, EAT_FRACTION};
use crate::kit::spots::{attack_nearest, BLOCKED_BAN_TICKS, DOOR_BAN_TICKS};
use crate::kit::pack::free_slots;
use crate::kit::survival::{check_events, hitpoints};
use crate::skill_walk::walk_checked;
use crate::kit::bans::{same_side, BanSet};
use crate::ScriptError;

/// True when the fight started since the click: xp flowing, hp taken,
/// or the target animating (its attack anim streams — ours never does,
/// and no positive "you're fighting" message exists server-side, all
/// combat lines are refusals). Pure so the loop stays thin; the caller
/// owns the snapshots and the re-click throttle. Ready for smithing-
/// style loops too: any started-loop-vs-dead-click verdict.

/// Spots named on the CLI. An unknown key is a contract error (`Usage`):
/// the old silent drop sent every `chicken_lumbridge` run to the auto
/// pick (goblins, cows) where fresh bots died 11-18 times per run.
pub fn named_spots(keys: &[String]) -> Result<Vec<knowledge::combat_spots::CombatSpot>, ScriptError> {
    let unknown: Vec<&String> = keys.iter().filter(|key| knowledge::combat_spots::by_key(key).is_none()).collect();
    if !unknown.is_empty() {
        return Err(ScriptError::Usage(format!("unknown combat spot {unknown:?}")));
    }
    Ok(keys.iter().filter_map(|key| knowledge::combat_spots::by_key(key)).collect())
}

pub fn fight_engaged(
    xp_now: i64,
    xp_at_click: i64,
    hp_now: u8,
    hp_before: u8,
    anim_now: Option<u16>,
    anim_at_click: Option<u16>,
) -> bool {
    xp_now != xp_at_click || hp_now < hp_before || anim_now != anim_at_click
}


/// No combat xp and no steps for this long while the foe lives: not
/// fighting (fence, stale slot, single-way block). ~35 s at the measured
/// ~5.6 ticks/s; a real engagement lands xp within seconds of contact.
const FIGHT_STALL_TICKS: u64 = 200;

/// Cold-click budget: consecutive attack clicks thrown from an adjacent
/// tile that produced no engagement evidence (no xp, no hp taken, target
/// anim unchanged) before the target is declared unstartable (ban +
/// repick, same as the stall path). Our own chasing steps reset the
/// stall window, so silent clicks alone never escalated: t02 live,
/// woman3 slot 785 ate ~40 silent clicks over ~250t, then rode KILL_CAP
/// to 970t alive with 0 kills. 40 clicks ≈ one stall window at the 5t
/// re-click throttle.
const MAX_COLD_CLICKS: u32 = 40;

/// True once the cold-click budget is spent: ban and repick.
fn cold_clicks_spent(cold: u32) -> bool {
    cold >= MAX_COLD_CLICKS
}

/// True when the bot stands on a tile the server accepts NPC clicks
/// from: orthogonal adjacency, never a diagonal (see `approach_npc`).
/// Clicks thrown from further out are silently converted to walks, so
/// throwing them only burns the cold-click budget for nothing.
fn click_adjacent(bot_x: i32, bot_z: i32, npc_x: i32, npc_z: i32) -> bool {
    (npc_x - bot_x).abs() + (npc_z - bot_z).abs() == 1
}

/// One foe never holds the bot longer than this, xp trickle or not: a normal
/// goblin kill costs <= ~600 ticks all-in (t01 live: kills 1-6), so 900 is
/// 1.5x headroom. The 12-minute goblin 810 grind (xp flowing, nothing dying)
/// is by definition a bad use of time on an axe rush.
const KILL_CAP_TICKS: u64 = 900;


/// Deaths make the fight return instead of resuming the killer spot: the
/// caller re-picks excluding it (one death) or fails typed (two in a row).
/// Returns (kills, bank trip wanted, death seen).
pub fn fight(
    bot: &mut Bot,
    spot: &knowledge::combat_spots::CombatSpot,
    target: (i32, i32),
    kills_limit: u32,
    ticks_limit: u64,
    no_bank: bool,
) -> Result<(u32, bool, bool), ScriptError> {
    let mut kills = 0u32;
    let started = bot.world.ticks;
    let mut target_npc: Option<u16> = None;
    // Last tile the target was seen on: its drop (bones included) lands
    // there on death, after the slot already left the NPC list.
    let mut foe_tile = (bot.world.x, bot.world.z);
    let mut last_attack = 0u64;
    let mut last_event_check = bot.world.ticks;
    let mut max_hp = hitpoints(bot).max(10);
    let mut prev_hp = hitpoints(bot);
    let mut fighting = false;
    let mut acquire_xp = combat_xp(bot);
    let mut acquire_anim: Option<u16> = None;
    let mut seen_msgs = bot.world.messages.len();
    let mut unreachable: BanSet = BanSet::new();
    let mut progress_at = bot.world.ticks;
    let mut progress_xp = combat_xp(bot);
    let mut progress_pos = (bot.world.x, bot.world.z);
    let mut engaged_at = bot.world.ticks;
    let mut attempts = 0u32;
    let mut cold_clicks = 0u32;
    // Controlled (all three at once) when the wielded category offers it,
    // else the weakest skill right now: unarmed has no controlled entry.
    let mut style: String = if controlled_button(&current_weapon_category(bot)).is_some() {
        "controlled".to_string()
    } else {
        weakest_melee(bot).to_string()
    };
    let confirmed = set_style(bot, &style)?;
    bot.log(&format!(
        "style {style} (atk={} str={} def={}) live={} confirmed={confirmed}",
        melee_stat(bot, "attack"),
        melee_stat(bot, "strength"),
        melee_stat(bot, "defence"),
        current_skill(bot, &style),
    ));
    // One line per hunt: which flank we hold (or that no gate is mapped
    // yet, so the per-pick filter stays off and crossings stay possible).
    match crate::kit::bank::toll_gate_x(bot) {
        Some(line) => bot.log(&format!("toll fence line {line}, hunting the {} flank", if bot.world.x >= line { "east" } else { "west" })),
        None => bot.log("toll fence: no gates mapped, cross-fence picks unfiltered"),
    }
    loop {
        if ticks_limit > 0 && bot.world.ticks.saturating_sub(started) >= ticks_limit {
            break;
        }
        if kills_limit > 0 && kills >= kills_limit {
            break;
        }
        bot.pump(2)?;
        if bot.world.ticks.saturating_sub(last_event_check) >= 25 {
            last_event_check = bot.world.ticks;
            check_events(bot)?;
        }
        let hp = hitpoints(bot);
        if hp == 0 {
            bot.intend("dead, waiting for respawn");
            let target = bot.world.ticks + 40;
            while bot.world.ticks < target && hitpoints(bot) == 0 {
                bot.pump(5)?;
            }
            // Death drops the pack: re-wear from what survived, pack only,
            // never a buy loop. Then stop: the caller re-picks excluding
            // this spot (or fails typed on the second death in a row) —
            // walking straight back here is the death loop (t02 live:
            // warriors -> Lumbridge -> warriors on foot, forever).
            rewear(bot, "death")?;
            bot.log(&format!("died at {} respawned at {:?}", spot.key, bot.position()));
            return Ok((kills, false, true));
        }
        max_hp = max_hp.max(hp);
        if should_eat(hp, max_hp, EAT_FRACTION) {
            eat(bot)?;
        }
        if let Some(slot) = target_npc {
            if let Some(foe) = bot.world.npcs.get(&slot) {
                foe_tile = (foe.x, foe.z);
                // One foe never holds the bot past the cap, xp trickle or
                // not: ban, step toward it (door-aware), repick. A target at
                // 1 hp retried after the ban expiry, nothing is lost.
                if bot.world.ticks.saturating_sub(engaged_at) >= KILL_CAP_TICKS {
                    let (foe_x, foe_z) =
                        bot.world.npcs.get(&slot).map(|npc| (npc.x, npc.z)).unwrap_or((bot.world.x, bot.world.z));
                    bot.log(&format!("target {slot} alive {}t, moving on", bot.world.ticks.saturating_sub(engaged_at)));
                    unreachable.ban_for((foe_x, foe_z), bot.world.ticks, BLOCKED_BAN_TICKS);
                    let _ = bot.approach_npc(slot, 60);
                    target_npc = None;
                    attempts = 0;
                    progress_at = bot.world.ticks;
                    continue;
                }
                let blocked = bot.world.messages.get(seen_msgs..).unwrap_or(&[]).iter().any(|line| {
                    // Server refusals (player_combat.rs2 player_in_combat_check)
                    // or unreachable foe: repick instead of spamming the same
                    // click every 5 ticks forever (t01 baseline: 229 "can't
                    // reach" + 36 "already under attack" for 26 kills).
                    let lowered = line.to_lowercase();
                    lowered.contains("can't reach that")
                        || lowered.contains("already under attack")
                        || lowered.contains("someone else is fighting")
                        || lowered.contains("not after you")
                });
                let (foe_x, foe_z) =
                    bot.world.npcs.get(&slot).map(|npc| (npc.x, npc.z)).unwrap_or((bot.world.x, bot.world.z));
                if blocked {
                    unreachable.ban_for((foe_x, foe_z), bot.world.ticks, BLOCKED_BAN_TICKS);
                    // The step toward a door-blocked foe re-burns the door
                    // (click + watch, refused again): inspect it, and when
                    // the server proves the door uncrossable the tile stays
                    // out for the whole fight instead of 200 ticks.
                    if let Err(step_error) = bot.approach_npc(slot, 60) {
                        if matches!(step_error, lifescape_sdk::Error::DoorDidNotOpen(_)) {
                            bot.log(&format!("npc {slot} at ({foe_x},{foe_z}) behind uncrossable door, excluding"));
                            unreachable.ban_until((foe_x, foe_z), DOOR_BAN_TICKS);
                        }
                    }
                    seen_msgs = bot.world.messages.len();
                    target_npc = None;
                    attempts = 0;
                    progress_at = bot.world.ticks;
                    continue;
                }
                // Door-aware step: orthogonal adjacency via the router, which
                // opens shut doors on the way (goblin in a house, or bot
                // locked inside: the walk crosses the door instead of
                // spamming clicks). 3 failed bouts blacklist the foe.
                match bot.approach_npc(slot, 100) {
                    Ok(()) => {}
                    Err(error)
                        if matches!(
                            error,
                            lifescape_sdk::Error::Interrupted(_)
                                | lifescape_sdk::Error::Io(_)
                                | lifescape_sdk::Error::Login(_)
                        ) =>
                    {
                        return Err(error.into())
                    }
                    Err(error) => {
                        // Door proved uncrossable: no 3-bout retry (each bout
                        // is a click + watch the server refuses). Exclude the
                        // tile for the fight and repick at once.
                        if matches!(error, lifescape_sdk::Error::DoorDidNotOpen(_)) {
                            bot.log(&format!("npc {slot} at ({foe_x},{foe_z}) behind uncrossable door, excluding: {error}"));
                            unreachable.ban_until((foe_x, foe_z), DOOR_BAN_TICKS);
                            target_npc = None;
                            attempts = 0;
                            progress_at = bot.world.ticks;
                            continue;
                        }
                        attempts += 1;
                        bot.log(&format!("npc {slot} approach {attempts}/3 failed: {error}"));
                        if attempts >= 3 {
                            bot.log(&format!(
                                "blacklist npc {slot} at ({foe_x},{foe_z}): 3 failed approaches from {:?}",
                                bot.position()
                            ));
                            unreachable.ban_for((foe_x, foe_z), bot.world.ticks, BLOCKED_BAN_TICKS);
                            target_npc = None;
                            attempts = 0;
                            progress_at = bot.world.ticks;
                        }
                        continue;
                    }
                }
                // No xp and no steps for a whole stall window while the foe
                // lives: not fighting (locked in/out around a door the
                // router could not cross). Ban, step out toward the foe,
                // repick. Attempts reset on xp only: circling a blocked
                // house is movement, not progress, or 3 failed bouts would
                // never blacklist.
                if combat_xp(bot) != progress_xp {
                    progress_xp = combat_xp(bot);
                    progress_pos = (bot.world.x, bot.world.z);
                    progress_at = bot.world.ticks;
                    attempts = 0;
                } else if (bot.world.x, bot.world.z) != progress_pos {
                    progress_pos = (bot.world.x, bot.world.z);
                    progress_at = bot.world.ticks;
                } else if bot.world.ticks.saturating_sub(progress_at) >= FIGHT_STALL_TICKS {
                    bot.log(&format!("stall on npc {slot}, repicking"));
                    unreachable.ban_for((foe_x, foe_z), bot.world.ticks, BLOCKED_BAN_TICKS);
                    let _ = bot.approach_npc(slot, 60);
                    target_npc = None;
                    attempts = 0;
                    progress_at = bot.world.ticks;
                    continue;
                }
                // One click opens the fight; re-click only while no evidence
                // proves it started (a swallowed click recovers in <= 5
                // ticks). No positive "you're fighting" message exists
                // server-side (all combat lines are refusals), so evidence
                // is xp flowing, hp taken, or the target animating — its
                // attack anim streams, ours never does. A held fight
                // still probes reachability every 25 ticks: a
                // wall-separated target only refuses when clicked, and
                // that refusal is the fast exit (UnreachableSpot path).
                // FIGHT_STALL/KILL_CAP stay the backstops.
                if !fighting && bot.world.ticks.saturating_sub(last_attack) >= 5 {
                    if fight_engaged(
                        combat_xp(bot),
                        acquire_xp,
                        hitpoints(bot),
                        prev_hp,
                        bot.world.npcs.get(&slot).and_then(|npc| npc.anim),
                        acquire_anim,
                    ) {
                        fighting = true;
                        bot.log(&format!("engaged npc {slot}, holding clicks"));
                    } else if bot.world.npcs.get(&slot).is_some_and(|foe| {
                        click_adjacent(bot.world.x, bot.world.z, foe.x, foe.z)
                    }) {
                        // Adjacent throws only: clicks from further out are
                        // silently converted to walks and prove nothing, so
                        // they must not burn the cold-click budget (the
                        // approach above is still closing).
                        attack_npc(bot, slot)?;
                        last_attack = bot.world.ticks;
                        cold_clicks += 1;
                        if cold_clicks_spent(cold_clicks) {
                            let (foe_x, foe_z) = bot.world.npcs.get(&slot).map(|npc| (npc.x, npc.z)).unwrap_or((bot.world.x, bot.world.z));
                            bot.log(&format!("{cold_clicks} silent clicks on npc {slot}, repicking"));
                            unreachable.ban_for((foe_x, foe_z), bot.world.ticks, BLOCKED_BAN_TICKS);
                            let _ = bot.approach_npc(slot, 60);
                            target_npc = None;
                            attempts = 0;
                            progress_at = bot.world.ticks;
                        }
                    }
                } else if fighting && bot.world.ticks.saturating_sub(last_attack) >= reattack_ticks(bot) {
                    attack_npc(bot, slot)?;
                    last_attack = bot.world.ticks;
                }
                prev_hp = hitpoints(bot);
                continue;
            }
            kills += 1;
            let loot_from = bot.world.ticks;
            // Out of combat: a ranged run picks its arrows back up first
            // and merges them into the quiver (its reserved slot is free
            // again), then the kill's bones are picked up and buried on
            // the spot (every style, user rule), then the regular loot.
            let mut taken = 0;
            if bot.combat_style == "ranged" {
                taken += crate::kit::loot::recover_arrows(bot)?;
                crate::kit::loot::merge_arrows(bot)?;
            }
            let buried = crate::kit::loot::bury_kill_bones(bot, foe_tile)?;
            taken += crate::kit::loot::loot_ground(bot)?;
            bot.log(&format!(
                "kill {kills} at {:?} (+{}t loot, bones {})",
                bot.position(),
                bot.world.ticks.saturating_sub(loot_from),
                if buried { "buried" } else { "none" }
            ));
            // Loot can change the best: one re-evaluation when something
            // was picked up, never on empty kills.
            if taken > 0 {
                rewear(bot, "loot")?;
            }
            seen_msgs = bot.world.messages.len();
            progress_at = bot.world.ticks;
            attempts = 0;
            target_npc = None;
            // Trust the server, not the last string sent: a dropped press
            // shows up here as live != assumed and self-corrects instead
            // of locking rotation forever.
            let live = current_skill(bot, &style).to_string();
            if live != style {
                bot.log(&format!(
                    "style sync live={live} assumed={style} (atk={} str={} def={})",
                    melee_stat(bot, "attack"),
                    melee_stat(bot, "strength"),
                    melee_stat(bot, "defence"),
                ));
                style = live;
            }
            if let Some(want) = next_style(bot, &style) {
                if set_style(bot, want)? {
                    style = want.to_string();
                    bot.log(&format!(
                        "style -> {style} (atk={} str={} def={})",
                        melee_stat(bot, "attack"),
                        melee_stat(bot, "strength"),
                        melee_stat(bot, "defence")
                    ));
                }
            }
            if free_slots(bot) <= 1 || (!no_bank && food_count(bot) == 0 && bank_stocks_food(bot)) {
                return Ok((kills, true, false));
            }
            continue;
        }
        unreachable.retain(bot.world.ticks);
        target_npc = attack_nearest(bot, spot, &mut unreachable)?;
        if let Some(slot) = target_npc {
            // Fresh engagement baseline per target: the click in
            // attack_nearest just went out, evidence starts now.
            fighting = false;
            acquire_xp = combat_xp(bot);
            prev_hp = hitpoints(bot);
            acquire_anim = bot.world.npcs.get(&slot).and_then(|npc| npc.anim);
            if let Some(foe) = bot.world.npcs.get(&slot) {
                foe_tile = (foe.x, foe.z);
            }
            last_attack = bot.world.ticks;
            engaged_at = bot.world.ticks;
            attempts = 0;
            cold_clicks = 0;
            let _ = slot;
        } else if (bot.world.x - target.0).abs() + (bot.world.z - target.1).abs() > 8 {
            // Dead socket, not an empty field: typed instead of pacing.
            if let Err(error) = bot.approach(target.0, target.1, bot.world.level, 1, 1500) {
                if matches!(
                    error,
                    lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)
                ) {
                    return Err(error.into());
                }
            }
            wait_ticks(bot, 5)?;
        } else {
            wait_ticks(bot, 5)?;
        }
    }
    Ok((kills, false, false))
}




/// Clusters on the bot's toll-fence flank (see kit::bans::same_side): the
/// crossing is never paid in melee, so an eastern cluster from a western
/// hunt is a ~220-tile detour per visit, never a shortcut. When every
/// cluster is across and the run did not opt into `toll`, the list stays
/// empty and the caller fails named instead of burning NoRoute dossiers.
/// Falls back to every cluster only when paid (an all-across spot still
/// hunts instead of idling once the gate is crossable).
fn same_flank_clusters(line: i32, home: i32, targets: &[(i32, i32, usize)], allow_paid: bool) -> Vec<(i32, i32, usize)> {
    let kept: Vec<(i32, i32, usize)> =
        targets.iter().filter(|(center_x, _, _)| same_side(line, *center_x, home)).copied().collect();
    if kept.is_empty() && allow_paid {
        targets.to_vec()
    } else {
        kept
    }
}

/// Kills still wanted: 0 means unlimited, so a plain subtraction panics
/// on the second cluster once anything died (t02 live).
fn remaining_kills(kills_limit: u32, total: u32) -> u32 {
    if kills_limit == 0 {
        return 0;
    }
    kills_limit.saturating_sub(total)
}

/// Portion of a run budget still unspent (ticks left, saturated).
/// Pure so the budget arithmetic stays testable without a game.
pub fn budget_left(ticks_limit: u64, started: u64, now: u64) -> u64 {
    if ticks_limit == 0 {
        return 0;
    }
    ticks_limit.saturating_sub(now.saturating_sub(started))
}

/// One attack on `slot` for the run's combat style (PRD3 lot 6). Melee
/// and ranged click Attack: the server shoots when a bow and arrows are
/// worn. Magic casts the strongest damage spell the pack funds; no runes
/// left blocks typed so the brain files the runes as an obtain.
fn attack_npc(bot: &mut Bot, slot: u16) -> Result<(), ScriptError> {
    if bot.combat_style == "magic" {
        let Some(spell) = crate::activities::magic_style::castable_spell(bot) else {
            bot.log("out of runes for any damage spell");
            return Err(ScriptError::Blocked { missing: vec!["mindrune".to_string(), "airrune".to_string()] });
        };
        let com = crate::kit::magic::spell_com(bot, spell)?;
        bot.op_npc_t(slot, com)?;
        return Ok(());
    }
    if bot.combat_style == "ranged" && !crate::kit::gear::refill_quiver(bot)? {
        // Empty quiver and none carried: the arrows on the ground first,
        // then block typed for the brain to obtain more.
        crate::kit::loot::recover_arrows(bot)?;
        if !crate::kit::gear::refill_quiver(bot)? {
            bot.log("out of arrows");
            return Err(ScriptError::Blocked { missing: vec!["bronze_arrow".to_string()] });
        }
    }
    bot.op_npc(2, slot)?;
    Ok(())
}

/// Ticks between re-attacks while engaged: a manual cast never repeats
/// (one spell per click, 5-tick cast speed); melee and ranged keep
/// fighting on their own and only need a nudge.
fn reattack_ticks(bot: &Bot) -> u64 {
    if bot.combat_style == "magic" { 5 } else { 25 }
}

pub fn run_spot(
    bot: &mut Bot,
    spot: &knowledge::combat_spots::CombatSpot,
    kills_limit: u32,
    ticks_limit: u64,
    no_bank: bool,
) -> Result<(u32, bool, bool), ScriptError> {
    if spot.entrance.is_some() {
        // shortcut: trapdoors/ladders to underground areas are not traversed
        // (single-level router); surface spots only.
        return Err(ScriptError::Usage(format!(
            "spot {} is underground, not traversable from {:?}",
            spot.key,
            bot.position()
        )));
    }
    let targets = crate::kit::spots::ranked_clusters(bot, spot);
    if targets.is_empty() {
        bot.log(&format!("no spawn for {}", spot.key));
        return Ok((0, false, false));
    }
    // One flank per hunt: cluster centers across the toll fence would
    // each cost the ~220-tile foot detour (paid doors stay off all
    // hunt). The per-pick filter in attack_nearest stops the same
    // oscillation inside the fight; this stops it between clusters.
    let mut targets = targets;
    if let Some(line) = crate::kit::bank::toll_gate_x(bot) {
        let kept = same_flank_clusters(line, bot.world.x, &targets, bot.allow_paid);
        if kept.len() != targets.len() {
            bot.log(&format!("toll fence line {line}: hunting {} of {} clusters on this flank", kept.len(), targets.len()));
        }
        // Unpaid and nothing on this flank: the router refuses the paid
        // gate, so every cluster would burn a NoRoute dossier and end
        // "ok killed 0" (t02 live). Fail named instead.
        if kept.is_empty() {
            return Err(ScriptError::Usage(format!(
                "spot {} is across the toll fence (line {line}) from {:?}: re-run with `toll` once the bot carries coins",
                spot.key,
                bot.position()
            )));
        }
        targets = kept;
    }
    let mut total = 0u32;
    let mut banked_trip = false;
    // Run-level deadline: `fight` below budgets each call from its own
    // start, so passing it the full limit every cluster multiplies the
    // overshoot by the cluster count (~2.5x seen live, L7 debt). The
    // remainder shrinks as the run spends ticks anywhere (bank trips,
    // walks, fights alike).
    let run_started = bot.world.ticks;
    for (center_x, center_z, count) in targets {
        // Pace retries: instant walk failures consume no ticks, so an
        // unpaced loop machine-guns the server into kicking us.
        bot.pump(2)?;
        if ticks_limit > 0 && bot.world.ticks.saturating_sub(run_started) >= ticks_limit {
            bot.log(&format!("run budget {ticks_limit}t spent across clusters, stopping"));
            break;
        }
        bot.log(&format!("spot {} target ({center_x},{center_z}) spawns={count}", spot.key));
        let mut walk_fails = 0u32;
        while kills_limit == 0 || total < kills_limit {
            if ticks_limit > 0 && bot.world.ticks.saturating_sub(run_started) >= ticks_limit {
                bot.log(&format!("run budget {ticks_limit}t spent inside the hunt, stopping"));
                break;
            }
            // Hunt entry: the one full evaluation (pack + bank when near
            // one, pack only with --no-bank). Inside the hunt only a loot
            // that was picked up re-evaluates; a death ends the fight and
            // returns to the run for the re-pick (see fight()).
            if !no_bank {
                if prepare_at_bank(bot)? {
                    banked_trip = true;
                }
            } else {
                let available = crate::kit::gear::available_gear(bot);
                equip_plan(bot, &gear_plan(bot, &available, fight_weapon_style, style_damage_key))?;
            }
            // A dead socket must reconnect, not limp to the next cluster:
            // propagate connection failures, swallow the rest as before.
            match walk_checked(bot, center_x, center_z, 15000) {
                Err(error) if crate::kit::survival::is_conn_abort(&error) => return Err(error),
                Err(_) | Ok(false) => {
                    // A sticking door (or a step refused for a tick) must
                    // not exile the hunt across the map: re-bank and retry
                    // the same cluster, move on only on the third failure
                    // in a row (t02 live: Lumbridge pen doors).
                    walk_fails += 1;
                    if walk_fails >= 3 {
                        break;
                    }
                    wait_ticks(bot, 25)?;
                    continue;
                }
                Ok(true) => {
                    walk_fails = 0;
                }
            }
            // The status line reads the last intent: without this the
            // whole hunt showed "banking at (x,z)" from prepare_at_bank.
            bot.intend(&format!("hunting {} near ({center_x},{center_z})", spot.key));
            {
                let (kills, bank_wanted, died) =
                    crate::kit::fight::fight(bot, spot, (center_x, center_z), remaining_kills(kills_limit, total), budget_left(ticks_limit, run_started, bot.world.ticks), no_bank)?;
                total += kills;
                if died {
                    return Ok((total, true, banked_trip));
                }
                if !bank_wanted || (kills_limit > 0 && total >= kills_limit) {
                    return Ok((total, false, banked_trip));
                }
            }
        }
    }
    Ok((total, false, banked_trip))
}

/// Auto spot: broke beginners (no food carried, none banked) cannot
/// trade hits, so chickens until the account can afford to bleed.
/// Stocked accounts keep best_spot() exactly (t02 live: foodless at
/// melee 2 died to cow chip damage every few kills).
pub fn auto_spot(bot: &Bot) -> knowledge::combat_spots::CombatSpot {
    let level = melee_level(bot);
    if level <= 2 && food_count(bot) == 0 && !bank_stocks_food(bot) {
        if let Some(chickens) = knowledge::combat_spots::by_key("chickens") {
            return chickens;
        }
    }
    knowledge::combat_spots::best_spot(level)
}


/// Fallback after a death at `killer`: nearest surface spot that fits the
/// current melee level and is no harder than the killer (existing
/// `min_level` data only, no new threshold), killers excluded. Gear is
/// unchanged since the death (re-wear from survivors only), so an
/// easier-or-equal level range stays within gear reach. Nearest cluster to
/// the respawn position wins so the walk back stays short. None when
/// nothing reasonable is on foot: the caller banks instead.
pub fn fallback_spot(
    bot: &Bot,
    killer: &knowledge::combat_spots::CombatSpot,
    excluded: &[&str],
) -> Option<knowledge::combat_spots::CombatSpot> {
    let level = melee_level(bot);
    let here = (bot.world.x, bot.world.z);
    knowledge::combat_spots::all_spots()
        .into_iter()
        .filter(|spot| spot.entrance.is_none())
        .filter(|spot| !excluded.iter().any(|key| *key == spot.key))
        .filter(|spot| spot.min_level <= level && spot.min_level <= killer.min_level)
        .filter_map(|spot| {
            crate::kit::spots::ranked_clusters(bot, &spot)
                .into_iter()
                .map(|(spot_x, spot_z, _)| (spot_x - here.0).abs() + (spot_z - here.1).abs())
                .min()
                .map(|dist| (dist, spot))
        })
        .min_by_key(|(dist, _)| *dist)
        .map(|(_, spot)| spot)
}

#[cfg(test)]
mod tests {
    use super::{auto_spot, fallback_spot, remaining_kills};
    use crate::kit::survival::{danger_error, DeathTally};
    use crate::tables_live::attach_live_tables;
    use lifescape_gamedata as knowledge;
    use lifescape_sdk::Bot;

    fn stat_bot(attack: u8, defence: u8, strength: u8) -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.stats.insert(0, (attack, 0));
        bot.world.stats.insert(1, (defence, 0));
        bot.world.stats.insert(2, (strength, 0));
        bot
    }



    /// Unlimited (0) stays unlimited past the first cluster; a bound
    /// never goes negative.
    #[test]
    fn remaining_kills_never_underflows() {
        assert_eq!(remaining_kills(0, 0), 0);
        assert_eq!(remaining_kills(0, 5), 0, "unlimited hunt survives kills");
        assert_eq!(remaining_kills(10, 3), 7);
        assert_eq!(remaining_kills(10, 10), 0);
    }

    /// The run budget is cumulative, not per cluster: each fight call
    /// receives only what the run has not spent yet.
    #[test]
    fn budget_left_shrinks_with_spent_ticks() {
        assert_eq!(super::budget_left(0, 100, 99999), 0, "unlimited stays unlimited");
        assert_eq!(super::budget_left(6000, 100, 100), 6000, "nothing spent yet");
        assert_eq!(super::budget_left(6000, 100, 3100), 3000, "half the run left");
        assert_eq!(super::budget_left(6000, 100, 6100), 0, "spent budget saturates, never wraps");
        assert_eq!(super::budget_left(6000, 5000, 100), 6000, "clock skew never grants extra");
    }


    /// Broke beginners hunt chickens, stocked ones keep best_spot.
    #[test]
    fn auto_spot_shelters_the_foodless() {
        let broke = stat_bot(2, 2, 2);
        assert_eq!(auto_spot(&broke).key, "chickens");
        let mut stocked = stat_bot(2, 2, 2);
        let bank_com = (stocked.tables.com_id)("bank_main:inv").expect("bank com in dumps");
        stocked.world.inv.insert(bank_com, [(0u8, (315u16, 3u32))].into_iter().collect());
        assert_eq!(auto_spot(&stocked).key, "cows");
    }







    /// Cluster flank filter: mixed flanks keep the bot's side only; an
    /// all-across list falls back to everything only when paid, and
    /// stays empty when unpaid so the caller fails named (t02 live:
    /// unpaid al_kharid_warriors from the west burned NoRoute dossiers
    /// and ended "ok killed 0").
    #[test]
    fn clusters_stay_on_the_hunting_flank() {
        let mixed = vec![(3252, 3243, 16), (3274, 3213, 4)];
        assert_eq!(super::same_flank_clusters(3268, 3260, &mixed, false), vec![(3252, 3243, 16)], "east cluster dropped");
        assert_eq!(super::same_flank_clusters(3268, 3270, &mixed, true), vec![(3274, 3213, 4)], "west cluster dropped");
        let east_only = vec![(3274, 3213, 4)];
        assert_eq!(super::same_flank_clusters(3268, 3270, &east_only, true), east_only, "paid all-across still hunts");
        assert!(super::same_flank_clusters(3268, 3260, &east_only, false).is_empty(), "unpaid all-across stays empty");
    }

    /// Auto re-pick from the west never idles on an across-toll pick: the
    /// fallback is a surface spot at or below the killer tier with a
    /// same-flank cluster to hunt (t02 live: auto warriors from Lumbridge
    /// ended the run with Usage instead of hunting).
    #[test]
    fn auto_toll_fallback_hunts_the_home_flank() {
        let mut bot = stat_bot(17, 17, 17);
        bot.world.x = 3219;
        bot.world.z = 3219;
        bot.world.level = 0;
        let killer = knowledge::combat_spots::by_key("al_kharid_warriors").expect("warriors");
        let excluded = vec![killer.key];
        let fallback = fallback_spot(&bot, &killer, &excluded).expect("a walkable repli exists");
        assert_ne!(fallback.key, killer.key, "never re-pick the across spot");
        assert!(fallback.entrance.is_none(), "surface only, the router never rides down");
        assert!(fallback.min_level <= killer.min_level, "no harder than the across pick, got: {}", fallback.key);
        let targets = crate::kit::spots::ranked_clusters(&bot, &fallback);
        assert!(!targets.is_empty(), "fallback has same-level spawns");
        let kept = super::same_flank_clusters(3268, bot.world.x, &targets, false);
        assert!(!kept.is_empty(), "{} has no west-flank cluster to hunt", fallback.key);
    }





    /// One death excludes the killer from every later re-pick: the
    /// fallback is never the spot that just killed (t02 live: warriors
    /// -> Lumbridge -> warriors on foot, forever).
    #[test]
    fn death_excludes_the_killer_spot() {
        let bot = stat_bot(9, 9, 9);
        let killer = knowledge::combat_spots::by_key("cows").expect("cows");
        let mut tally = DeathTally::fresh();
        tally.note_death(killer.key);
        assert!(tally.excludes("cows"));
        let fallback =
            fallback_spot(&bot, &killer, tally.excluded()).expect("a same-tier repli exists");
        assert_ne!(fallback.key, "cows", "never re-pick the killer identically");
        assert!(fallback.entrance.is_none(), "surface only, the router never rides down");
        assert!(
            fallback.min_level <= killer.min_level,
            "no harder than what just killed, got: {}",
            fallback.key
        );
    }

    /// Nothing safer on foot (every surface spot excluded or above the
    /// melee level): no fallback, the caller banks instead of re-picking.
    #[test]
    fn no_fallback_when_nothing_reasonable() {
        let bot = stat_bot(2, 2, 2);
        let killer = knowledge::combat_spots::by_key("chickens").expect("chickens");
        let all: Vec<&str> =
            knowledge::combat_spots::all_spots().iter().map(|spot| spot.key).collect();
        assert!(
            fallback_spot(&bot, &killer, &all).is_none(),
            "everything excluded: bank, never re-pick"
        );
    }

    /// Two deaths in a row (no kill, no bank trip between) fail typed
    /// Unreachable instead of walking back a third time.
    #[test]
    fn two_deaths_fail_typed_unreachable() {
        let mut tally = DeathTally::fresh();
        tally.note_death("al_kharid_warriors");
        assert!(!tally.fatal(), "one death re-picks, never fails");
        tally.note_death("cows");
        assert!(tally.fatal(), "two consecutive deaths are fatal");
        let error = danger_error("cows", tally.deaths());
        assert!(
            matches!(&error, crate::ScriptError::Sdk(lifescape_sdk::Error::Unreachable(_))),
            "typed Unreachable, got: {error}"
        );
        assert!(error.to_string().contains("too dangerous after 2 deaths"), "explicit, got: {error}");
        assert!(error.to_string().contains("re-gear"), "points at the fix, got: {error}");
    }

    /// A kill or a bank trip between deaths starts a new streak (deaths
    /// with progress between are not consecutive); the killer stays out
    /// for the run.
    #[test]
    fn progress_resets_the_death_streak() {
        let mut tally = DeathTally::fresh();
        tally.note_death("cows");
        tally.note_progress();
        assert_eq!(tally.deaths(), 0, "kill/bank resets the streak");
        assert!(!tally.fatal());
        assert!(tally.excludes("cows"), "the killer stays out for the run");
        tally.note_death("goblins");
        assert!(!tally.fatal(), "progress between deaths: new streak, not fatal");
    }
}

#[cfg(test)]
mod engaged_tests {
    use super::fight_engaged;


    /// Engagement evidence: each channel alone proves the fight (xp
    /// only flows on our hits, hp only drops on damage taken, target
    /// anim only changes when it acts); all quiet means the click
    /// never landed.
    #[test]
    fn fight_engaged_reads_any_channel() {
        assert!(!fight_engaged(100, 100, 18, 18, None, None), "silence is not engagement");
        assert!(fight_engaged(120, 100, 18, 18, None, None), "xp flow is engagement");
        assert!(fight_engaged(100, 100, 15, 18, None, None), "damage taken is engagement");
        assert!(fight_engaged(100, 100, 18, 18, Some(7), None), "target animating is engagement");
        assert!(!fight_engaged(100, 100, 19, 18, None, None), "regen/heal is not engagement");
    }

    /// Cold-click budget: 39 silent clicks hold, the 40th repicks.
    #[test]
    fn cold_click_budget_spent_at_forty() {
        assert!(!super::cold_clicks_spent(0), "fresh target holds");
        assert!(!super::cold_clicks_spent(39), "39 silent clicks hold");
        assert!(super::cold_clicks_spent(40), "40 silent clicks repick");
    }

    /// Clicks only land from orthogonal adjacency: diagonals and range
    /// never start combat (the server silently walks instead).
    #[test]
    fn clicks_gate_on_orthogonal_adjacency() {
        assert!(super::click_adjacent(0, 0, 1, 0), "side adjacent clicks");
        assert!(super::click_adjacent(0, 0, 0, 1), "side adjacent clicks");
        assert!(!super::click_adjacent(0, 0, 1, 1), "diagonal never clicks");
        assert!(!super::click_adjacent(0, 0, 2, 0), "range never clicks");
        assert!(!super::click_adjacent(0, 0, 0, 0), "same tile never clicks");
    }
}
