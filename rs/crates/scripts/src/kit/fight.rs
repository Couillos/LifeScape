//! Fight: target acquisition and the kill loop. Moved out of the
//! melee script: the attack itself, the wait for the death, the style
//! rotation (pressed through `kit::style`), eating (`kit::survival`),
//! re-wearing (`kit::gear`) and looting (the combat module until the
//! loot brick lands).

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::waits::wait_ticks;
use lifescape_sdk::Bot;

use crate::kit::gear::{current_weapon_category, rewear};
use crate::kit::style::{combat_xp, controlled_button, current_skill, melee_stat, next_style, set_style, weakest_melee};
use crate::kit::survival::{bank_stocks_food, eat, food_count, should_eat, EAT_FRACTION};
use crate::skill_common::{check_events, free_slots, hitpoints};
use crate::skill_gather::{fight_engaged, same_side, BanSet};
use crate::ScriptError;

/// World ticks a "can't reach" goblin tile stays banned before retry.
const BLOCKED_BAN_TICKS: u64 = 200;

/// Ban for a foe behind a door the server proved uncrossable (door opens,
/// the step through is refused): the tile stays out for the whole fight.
/// A spot that fails in a loop is worse than a spot skipped: each new
/// indoor goblin burns one approach, never a campaign.
const DOOR_BAN_TICKS: u64 = u64::MAX;

/// No combat xp and no steps for this long while the foe lives: not
/// fighting (fence, stale slot, single-way block). ~35 s at the measured
/// ~5.6 ticks/s; a real engagement lands xp within seconds of contact.
const FIGHT_STALL_TICKS: u64 = 200;

/// One foe never holds the bot longer than this, xp trickle or not: a normal
/// goblin kill costs <= ~600 ticks all-in (t01 live: kills 1-6), so 900 is
/// 1.5x headroom. The 12-minute goblin 810 grind (xp flowing, nothing dying)
/// is by definition a bad use of time on an axe rush.
const KILL_CAP_TICKS: u64 = 900;

fn nearest_bank_tile(bot: &Bot) -> Option<(i32, i32, i32)> {
    knowledge::banks::all_banks()
        .into_iter()
        .filter(|bank| !bank.members && bank.level == i64::from(bot.world.level))
        .min_by_key(|bank| (bank.x - i64::from(bot.world.x)).abs() + (bank.z - i64::from(bot.world.z)).abs())
        .map(|bank| (bank.level as i32, bank.x as i32, bank.z as i32))
}

/// NPC spawn clusters for a spot (same-level spawns only).
pub fn ranked_clusters(bot: &Bot, spot: &knowledge::combat_spots::CombatSpot) -> Vec<(i32, i32, usize)> {
    let mut spawns: Vec<(i64, i64)> = Vec::new();
    for entry in knowledge::all_npcs() {
        let name = entry.name.unwrap_or_default();
        if !spot.npc_names.contains(&name) || entry.members == Some(true) {
            continue;
        }
        for spawn in entry.spawns {
            // shortcut: same-level spawns only; the fighter never rides
            // down to a dungeon camp. Revisit if one is wanted.
            if spawn.level != i64::from(bot.world.level) {
                continue;
            }
            spawns.push((spawn.x, spawn.z));
        }
    }
    let mut groups: Vec<Vec<(i64, i64)>> = Vec::new();
    for spawn in spawns {
        let mut placed = false;
        for group in groups.iter_mut() {
            let center_x: i64 = group.iter().map(|item| item.0).sum::<i64>() / group.len() as i64;
            let center_z: i64 = group.iter().map(|item| item.1).sum::<i64>() / group.len() as i64;
            if (spawn.0 - center_x).abs().max((spawn.1 - center_z).abs()) <= 15 {
                group.push(spawn);
                placed = true;
                break;
            }
        }
        if !placed {
            groups.push(vec![spawn]);
        }
    }
    let mut ranked: Vec<(f64, i32, i32, usize)> = groups
        .into_iter()
        .filter(|group| !group.is_empty())
        .map(|group| {
            let center_x = (group.iter().map(|item| item.0).sum::<i64>() / group.len() as i64) as i32;
            let center_z = (group.iter().map(|item| item.1).sum::<i64>() / group.len() as i64) as i32;
            let dist = nearest_bank_tile(bot).map(|(_, bank_x, bank_z)| {
                (bank_x - center_x).abs() + (bank_z - center_z).abs()
            }).unwrap_or(10000) as f64;
            (group.len() as f64 - dist / 40.0, center_x, center_z, group.len())
        })
        .collect();
    ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    ranked.into_iter().map(|(_, x, z, count)| (x, z, count)).collect()
}

/// Shop and bank staff are never targets: keepers, assistants, Bob and
/// bankers (some at 1 HP) must survive our hunts. Excluded by op
/// category — any Trade/Bank right-click — never by name, so a new shop
/// inherits the guard. Valid foes carry Attack instead (goblin npc 100:
/// [None, Attack, None, ...]).
/// Source (read-only): gamedata npcs.json ops, e.g. generalshopkeeper1
/// (npc 520) and generalassistant1 (521) carry Trade at op index 2.
fn is_shop_staff(npc_type: u16) -> bool {
    knowledge::npc_by_id(usize::from(npc_type)).is_some_and(|entry| {
        entry.ops.iter().flatten().any(|op| {
            let lowered = op.to_lowercase();
            lowered.contains("trade") || lowered.contains("bank")
        })
    })
}

/// Acquisition window bound: Manhattan ranks the pool, only the top-K get
/// a pure plan (see `lifescape_nav::walk_rank`), so never more plans than
/// approaches. K=3: foes die fast, the window re-ranks every pick.
const FOE_SHORTLIST: usize = 3;

/// Walking-cost order for the acquisition window (remote batch, no
/// walk, no send): Manhattan lies across rivers and fences, so the
/// window is reordered by service plan cost before the door-aware
/// approach verifies reachability by walking. Unwired or unplannable
/// keeps the Manhattan order; the sort itself lives in
/// `lifescape_nav::walk_rank`.
fn rank_foes(bot: &mut Bot, window: Vec<(u16, i32, i32)>) -> Vec<(u16, i32, i32)> {
    let level = bot.world.level;
    crate::skill_walk::refine_walking(bot, window, FOE_SHORTLIST, "foe", |(_, x, z)| {
        lifescape_nav::plan::Tile { x: *x, z: *z, level }
    })
    .into_iter()
    .map(|(_, cand)| cand)
    .collect()
}

fn attack_nearest(
    bot: &mut Bot,
    spot: &knowledge::combat_spots::CombatSpot,
    avoid: &mut BanSet,
) -> Result<Option<u16>, ScriptError> {
    let names: Vec<&str> = spot.npc_names.to_vec();
    let mut cands = bot.npcs_named(&names);
    cands.retain(|(_, x, z)| !avoid.is_banned((*x, *z), bot.world.ticks));
    // Shop staff are never foes, by op category (Trade/Bank), not name:
    // a keeper next to the hunt (Bob by the axe shop, the Lumbridge
    // general store pair) is never a valid click.
    cands.retain(|(slot, _, _)| {
        bot.world.npcs.get(slot).is_none_or(|npc| !is_shop_staff(npc.npc_type))
    });
    // Never target across the toll fence: goblins spawn on both flanks,
    // so the nearest flips sides every kill and each crossing walks the
    // ~200-tile foot detour (paid doors stay off all hunt). One side per
    // pick; the flank follows the bot, not the plan, so no replan loop.
    if let Some(line) = crate::skill_common::toll_gate_x(bot) {
        let home = bot.world.x;
        cands.retain(|(_, x, _)| same_side(line, *x, home));
    }
    cands.sort_by_key(|(_, x, z)| (x - bot.world.x).abs() + (z - bot.world.z).abs());
    // Door-aware acquisition, first reachable wins (max 3 foes): the window
    // above is walking-cost ordered (rank_foes), so the first approach is
    // the nearest by foot, not by air. Stand orthogonally adjacent via the
    // router, which opens shut doors on the way (travel Segment::OpenDoor).
    // NoRoute or a blocked ring bans the tile and tries the next goblin
    // instead of insisting on this one.
    for (slot, x, z) in rank_foes(&mut *bot, cands.into_iter().take(FOE_SHORTLIST).collect()) {
        match bot.approach_npc(slot, 100) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error,
                    lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)
                ) =>
            {
                return Err(error.into())
            }
            Err(error) => {
                // Uncrossable door (verified open, step refused): the house
                // is out for this fight, not the foe. One approach burned
                // per tile, never a retry campaign.
                if matches!(error, lifescape_sdk::Error::DoorDidNotOpen(_)) {
                    bot.log(&format!("goblin {slot} at ({x},{z}) behind uncrossable door, excluding: {error}"));
                    avoid.ban_until((x, z), DOOR_BAN_TICKS);
                    continue;
                }
                bot.log(&format!("goblin {slot} at ({x},{z}) unreachable: {error}"));
                avoid.ban_for((x, z), bot.world.ticks, BLOCKED_BAN_TICKS);
                continue;
            }
        }
        bot.op_npc(2, slot)?;
        return Ok(Some(slot));
    }
    Ok(None)
}

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
    match crate::skill_common::toll_gate_x(bot) {
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
            if bot.world.npcs.contains_key(&slot) {
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
                    } else {
                        bot.op_npc(2, slot)?;
                        last_attack = bot.world.ticks;
                    }
                } else if fighting && bot.world.ticks.saturating_sub(last_attack) >= 25 {
                    bot.op_npc(2, slot)?;
                    last_attack = bot.world.ticks;
                }
                prev_hp = hitpoints(bot);
                continue;
            }
            kills += 1;
            let loot_from = bot.world.ticks;
            let taken = crate::skill_combat::loot_ground(bot)?;
            bot.log(&format!("kill {kills} at {:?} (+{}t loot)", bot.position(), bot.world.ticks.saturating_sub(loot_from)));
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
            last_attack = bot.world.ticks;
            engaged_at = bot.world.ticks;
            attempts = 0;
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

#[cfg(test)]
mod tests {
    use super::{is_shop_staff, rank_foes};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    /// Shop staff are excluded by op category, never by name: the
    /// Lumbridge general store pair (520/521), Bob (519) and bankers
    /// carry Trade/Bank; every valid foe (goblin 100, chicken 41, man 1)
    /// carries Attack instead.
    #[test]
    fn shop_staff_never_targeted_by_category() {
        assert!(is_shop_staff(520), "Lumbridge keeper");
        assert!(is_shop_staff(521), "Lumbridge assistant");
        assert!(is_shop_staff(519), "Bob the axe seller");
        assert!(is_shop_staff(494), "banker");
        assert!(!is_shop_staff(100), "goblin fights");
        assert!(!is_shop_staff(41), "chicken fights");
        assert!(!is_shop_staff(1), "man fights");
    }

    /// Walking-cost order inside the take(3) window: slot 7 at (3252,3237)
    /// is Manhattan-nearest from (3230,3241) but walks ~50 (houses/doors)
    /// while slot 9 at (3203,3246) walks ~36 — the lure loses, and no walk
    /// was spent to learn it (one remote batch, L1-cached on repeat).
    #[test]
    fn rank_foes_prefers_walk_cheap() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        crate::wire_nav(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3230;
        bot.world.z = 3241;
        let world = bot.nav_world.clone().expect("wired");
        let tables = bot.nav_tables.clone().expect("wired");
        let service = lifescape_nav::service::Service::start(
            lifescape_nav::service::Snapshot::new(world, tables, "test-foes".to_string()),
            &lifescape_nav::service::test_socket("foes-walk"),
            1,
        )
        .expect("service starts");
        bot.plan_client = Some(lifescape_nav::service::PlanClient::new(service.socket_path(), "foes"));
        bot.plan_data_version = Some("test-foes".to_string());
        bot.last_keepalive = Some(std::time::Instant::now());
        let window = vec![(7u16, 3252, 3237), (9u16, 3203, 3246)];
        let ranked = rank_foes(&mut bot, window);
        assert_eq!(ranked, vec![(9u16, 3203, 3246), (7u16, 3252, 3237)], "walk-cheap foe first, got: {ranked:?}");
        // L1 hit: the same pair costs no new compute.
        let before = service.stats();
        let window = vec![(7u16, 3252, 3237), (9u16, 3203, 3246)];
        let again = rank_foes(&mut bot, window);
        assert_eq!(again, vec![(9u16, 3203, 3246), (7u16, 3252, 3237)], "repeat ranks identically");
        let after = service.stats();
        assert_eq!(after.misses - before.misses, 0, "second ranking hits L1, never recomputes");
        assert!(after.hits + after.trunc_hits > before.hits + before.trunc_hits, "L1 served the repeat");
    }

    /// No planner wired (or nothing plannable): the window keeps its
    /// Manhattan order, never empties — ranking changes, picking never.
    #[test]
    fn rank_foes_unwired_keeps_manhattan() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3230;
        bot.world.z = 3241;
        let window = vec![(7u16, 3252, 3237), (9u16, 3203, 3246)];
        assert_eq!(rank_foes(&mut bot, window.clone()), window, "unwired keeps Manhattan order");
    }

    /// Operator stop during ranking: cancel, no block, Manhattan order.
    #[test]
    fn rank_foes_stop_returns_manhattan_without_blocking() {
        use std::sync::Arc;
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        crate::wire_nav(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3230;
        bot.world.z = 3241;
        let world = bot.nav_world.clone().expect("wired");
        let tables = bot.nav_tables.clone().expect("wired");
        let service = lifescape_nav::service::Service::start(
            lifescape_nav::service::Snapshot::new(world, tables, "test-foes-stop".to_string()),
            &lifescape_nav::service::test_socket("foes-stop"),
            1,
        )
        .expect("service starts");
        bot.plan_client = Some(lifescape_nav::service::PlanClient::new(service.socket_path(), "foes-stop"));
        bot.plan_data_version = Some("test-foes-stop".to_string());
        bot.last_keepalive = Some(std::time::Instant::now());
        bot.on_pump = Some(Arc::new(|_: &mut Bot| false));
        let start = std::time::Instant::now();
        let window = vec![(7u16, 3252, 3237), (9u16, 3203, 3246)];
        assert_eq!(rank_foes(&mut bot, window.clone()), window, "stop keeps Manhattan order");
        assert!(start.elapsed() < std::time::Duration::from_secs(30), "stop cancels, never blocks");
    }
}
