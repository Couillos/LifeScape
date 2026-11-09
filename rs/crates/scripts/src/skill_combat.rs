//! Melee farmer: best F2P spot for the level, best banked gear, banked food.
//! Rotates attack/strength/defence, eats when hurt, loots every drop except
//! bones, banks when the pack fills (gather loop, bank trip, sell, upgrade).

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::waits::wait_ticks;
use lifescape_sdk::Bot;

use crate::kit::gear::{available_gear, current_weapon_category, equip_plan, gear_plan, melee_level, prepare_at_bank, rewear};
use crate::kit::survival::{bank_stocks_food, danger_error, eat, food_count, should_eat, DeathTally, EAT_FRACTION};
use crate::skill_common::{
    bury_bones, check_events, free_slots, hitpoints, walk_checked,
};
use crate::skill_gather::{fight_engaged, BanSet, same_side};
use crate::{ScriptError, ScriptResult};

const LOOT_RADIUS: i32 = 6;
/// World ticks a "can't reach" goblin tile stays banned before retry.
const BLOCKED_BAN_TICKS: u64 = 200;
/// Ban for a foe behind a door the server proved uncrossable (door opens,
/// the step through is refused): the tile stays out for the whole fight.
/// A spot that fails in a loop is worse than a spot skipped: each new
/// indoor goblin burns one approach, never a campaign.
const DOOR_BAN_TICKS: u64 = u64::MAX;
/// Worst-case take: the drop lands on the death tick (goblin.rs2 obj_add
/// right after npc_death), walk <= 6 tiles + click. 600 burned 12 s on every
/// vanished/unreachable item; failures now log so the bound stays honest.
const TAKE_TICKS: u64 = 100;
/// No combat xp and no steps for this long while the foe lives: not
/// fighting (fence, stale slot, single-way block). ~35 s at the measured
/// ~5.6 ticks/s; a real engagement lands xp within seconds of contact.
const FIGHT_STALL_TICKS: u64 = 200;
/// One foe never holds the bot longer than this, xp trickle or not: a normal
/// goblin kill costs <= ~600 ticks all-in (t01 live: kills 1-6), so 900 is
/// 1.5x headroom. The 12-minute goblin 810 grind (xp flowing, nothing dying)
/// is by definition a bad use of time on an axe rush.
const KILL_CAP_TICKS: u64 = 900;

/// Controlled style button per weapon category, when this revision offers
/// one: slash/spiked/claws index 2, spear 0-2, polearm 0. Controlled trains
/// attack + strength + defence at once (combat.rs2:151-154, each 133/100 of
/// base xp). Unarmed has NO controlled entry (unarmed_table), so bare fists
/// rotate instead of sitting on accurate.
/// Source: content/scripts/skill_combat/configs/combat.dbrow (slash_table,
/// spear_table, polearm_table, spiked_table, claws_table) crossed with
/// content/scripts/skill_combat/scripts/player/player_attackstyles.rs2:37-76
/// (button suffix -> style index).
fn controlled_button(category: &str) -> Option<&'static str> {
    match category {
        "weapon_slash" => Some("combat_hacksword:hack2"),
        "weapon_spiked" => Some("combat_spiked:spiked2"),
        "weapon_claws" => Some("combat_claw:claw2"),
        "weapon_spear" => Some("combat_spear:spear0"),
        "weapon_polearm" => Some("combat_polearm:polearm0"),
        _ => None,
    }
}

/// Combat xp across the four fighting stats (attack 0, defence 1, strength 2,
/// hitpoints 3): any landed hit moves it, so a flat line means no combat.
fn combat_xp(bot: &Bot) -> i64 {
    [0u8, 1, 2, 3].iter().filter_map(|index| bot.world.stats.get(index)).map(|(_, xp)| i64::from(*xp)).sum()
}

/// Level of one melee skill by stat id (0 attack, 1 defence, 2 strength).
fn melee_stat(bot: &Bot, skill: &str) -> i64 {
    let index = match skill {
        "attack" => 0u8,
        "defence" => 1u8,
        _ => 2u8,
    };
    bot.world.stats.get(&index).map(|(level, _)| i64::from(*level)).unwrap_or(1)
}

/// Style to switch to, if any: controlled never rotates, otherwise the
/// weakest skill when it sits a full level below the current one (ties and
/// balanced skills hold, no click spam).
fn next_style(bot: &Bot, current: &str) -> Option<&'static str> {
    if current == "controlled" {
        return None;
    }
    let want = weakest_melee(bot);
    (want != current && melee_stat(bot, want) < melee_stat(bot, current)).then_some(want)
}

/// Weakest of attack/strength/defence right now.
fn weakest_melee(bot: &Bot) -> &'static str {

    let (attack, strength, defence) =
        (melee_stat(bot, "attack"), melee_stat(bot, "strength"), melee_stat(bot, "defence"));
    if attack <= strength && attack <= defence {
        "attack"
    } else if strength <= defence {
        "strength"
    } else {
        "defence"
    }
}

/// The style `fight` presses for this category: controlled when the
/// revision offers it, else the weakest skill right now. Shared with the
/// gear planner so the scored bonus is the trained bonus.
pub(crate) fn fight_weapon_style(bot: &Bot, category: &str) -> &'static str {
    if controlled_button(category).is_some() { "controlled" } else { weakest_melee(bot) }
}

/// Combat style buttons per weapon category (player_attackstyles data).
fn style_buttons(category: &str, skill: &str) -> Option<&'static str> {
    match (category, skill) {
        ("weapon_slash", "attack") => Some("combat_hacksword:hack0"),
        ("weapon_slash", "strength") => Some("combat_hacksword:hack1"),
        ("weapon_slash", "defence") => Some("combat_hacksword:hack3"),
        ("weapon_stab", "attack") => Some("combat_stabsword:stab0"),
        ("weapon_stab", "strength") => Some("combat_stabsword:stab1"),
        ("weapon_stab", "defence") => Some("combat_stabsword:stab3"),
        ("weapon_blunt", "attack") => Some("combat_blunt:blunt0"),
        ("weapon_blunt", "strength") => Some("combat_blunt:blunt1"),
        ("weapon_blunt", "defence") => Some("combat_blunt:blunt2"),
        ("weapon_axe", "attack") => Some("combat_axe:axe0"),
        ("weapon_axe", "strength") => Some("combat_axe:axe1"),
        ("weapon_axe", "defence") => Some("combat_axe:axe3"),
        ("weapon_2h_sword", "attack") => Some("combat_heavysword:heavy0"),
        ("weapon_2h_sword", "strength") => Some("combat_heavysword:heavy1"),
        ("weapon_2h_sword", "defence") => Some("combat_heavysword:heavy3"),
        ("weapon_pickaxe", "attack") => Some("combat_pickaxe:pickaxe0"),
        ("weapon_pickaxe", "strength") => Some("combat_pickaxe:pickaxe1"),
        ("weapon_pickaxe", "defence") => Some("combat_pickaxe:pickaxe3"),
        ("weapon_scythe", "attack") => Some("combat_scythe:scythe0"),
        ("weapon_scythe", "strength") => Some("combat_scythe:scythe1"),
        ("weapon_scythe", "defence") => Some("combat_scythe:scythe3"),
        ("weapon_spear", "attack" | "strength") => Some("combat_spear:spear0"),
        ("weapon_spear", "defence") => Some("combat_spear:spear3"),
        ("unarmed", "attack") => Some("combat_unarmed:unarmed0"),
        ("unarmed", "strength") => Some("combat_unarmed:unarmed1"),
        ("unarmed", "defence") => Some("combat_unarmed:unarmed2"),
        _ => None,
    }
}

/// Server attack-style varp (`gamedata/data/varps.json` id 43,
/// transmit=true): the ground truth presses verify against. A sent
/// `if_button` is silently dropped when its component is not visible
/// (engine `IfButtonHandler.ts`) or the player is delayed (normal queue,
/// see player_attackstyles.rs2:1-4), so open-loop tracking locked
/// rotation forever on one dropped press (t02 live: tracker said
/// defence, server stayed aggressive, str 5->12 while def sat at 4).
const COM_MODE_VARP: u16 = 43;

/// Live `%com_mode` index, when the server has transmitted it yet.
fn live_style_index(bot: &Bot) -> Option<i32> {
    bot.world.varps.get(&COM_MODE_VARP).copied()
}

/// com_mode index the button for `skill` should produce: the trailing
/// number of the dotted component (`stab3` -> 3). Single source stays
/// `style_buttons`/`controlled_button`.
fn style_button_index(dotted: &str) -> Option<i32> {
    let tail = dotted.rsplit(':').next()?;
    let digits: String =
        tail.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Skill the live `%com_mode` index trains with the wielded category
/// (damagestyle rows of combat.dbrow in button order): blunt, unarmed,
/// staff and polearm tables hold 3 rows, the rest 4; stab/axe/2h
/// sword/pickaxe/scythe index 2 is a second aggressive.
fn live_skill(category: &str, index: i32) -> Option<&'static str> {
    match category {
        "weapon_spear" => match index {
            0 | 1 | 2 => Some("controlled"),
            3 => Some("defence"),
            _ => None,
        },
        "weapon_polearm" => match index {
            0 => Some("controlled"),
            1 => Some("strength"),
            2 => Some("defence"),
            _ => None,
        },
        "weapon_slash" | "weapon_spiked" | "weapon_claws" => match index {
            0 => Some("attack"),
            1 => Some("strength"),
            2 => Some("controlled"),
            3 => Some("defence"),
            _ => None,
        },
        "weapon_blunt" | "unarmed" | "weapon_staff" => match index {
            0 => Some("attack"),
            1 => Some("strength"),
            2 => Some("defence"),
            _ => None,
        },
        _ => match index {
            0 => Some("attack"),
            1 | 2 => Some("strength"),
            3 => Some("defence"),
            _ => None,
        },
    }
}

/// Skill the server is actually training: its `%com_mode` when known,
/// else the last style we asked for. Every kill re-derives this, so a
/// dropped press self-corrects on the next kill instead of pinning the
/// tracker on a style the server never took.
fn current_skill<'a>(bot: &Bot, assumed: &'a str) -> &'a str {
    let category = current_weapon_category(bot);
    live_style_index(bot).and_then(|index| live_skill(&category, index)).unwrap_or(assumed)
}
/// A scimitar's speed beats a 2h sword's raw bonus: one-handed slashers win
/// ties (strategy, not game data).
pub(crate) fn weapon_weight(category: &str) -> f64 {
    match category {
        "weapon_slash" => 1.0,
        "weapon_stab" => 0.9,
        "weapon_axe" | "weapon_blunt" => 0.85,
        "weapon_pickaxe" => 0.7,
        "weapon_2h_sword" | "weapon_spear" => 0.6,
        _ => 0.5,
    }
}

/// Attack bonus the bot's next button actually trains with, per weapon
/// category: the revision ties each combat button to one damagetype
/// (slash/stab/crush), and the bot presses the button for the skill it
/// trains (style_buttons) or the controlled button when the category
/// offers one. Unknown pairs fall back to the best bonus.
/// Sources (read-only): content/scripts/skill_combat/configs/combat.dbrow
/// (weapon_*_table damagestyle/damagetype rows: slash_table is
/// acc/agg/con/def on slash/slash/stab/slash, stab_table acc/agg/agg/def
/// on stab/stab/slash/stab, blunt_table all crush, axe_table
/// acc/agg/agg/def on slash/slash/crush/slash, pickaxe_table on
/// stab/stab/crush/stab, spear_table all controlled on
/// stab/slash/crush/stab, spiked_table acc/agg/con/def on
/// crush/crush/stab/crush, claws_table like slash, polearm_table
/// con/agg/def on stab/slash/stab, unarmed/staff all crush) crossed with
/// content/.../player/player_attackstyles.rs2 (button suffix -> style).
pub(crate) fn style_damage_key(category: &str, style: &str) -> Option<&'static str> {
    match (category, style) {
        ("weapon_slash" | "weapon_claws", "controlled") => Some("stab"),
        ("weapon_slash" | "weapon_claws", _) => Some("slash"),
        ("weapon_stab", _) => Some("stab"),
        ("weapon_blunt", _) => Some("crush"),
        ("weapon_axe" | "weapon_2h_sword", _) => Some("slash"),
        ("weapon_scythe", "strength") => Some("stab"),
        ("weapon_scythe", _) => Some("slash"),
        ("weapon_pickaxe", _) => Some("stab"),
        ("weapon_spear", _) => Some("stab"),
        ("weapon_spiked", "controlled") => Some("stab"),
        ("weapon_polearm", "controlled") => Some("stab"),
        ("weapon_polearm", "defence") => Some("stab"),
        ("weapon_polearm", _) => Some("slash"),
        ("unarmed", _) => Some("crush"),
        _ => None,
    }
}


/// Press the button for `skill` and confirm the server took it: true
/// when live `%com_mode` matches the button's index. A dropped press
/// (invisible tab, delay queue) is retried twice, then logged loudly so
/// the next kill retries instead of training the wrong skill in
/// silence. False when the pair has no button at all (also logged).
fn set_style(bot: &mut Bot, skill: &str) -> Result<bool, ScriptError> {
    let category = current_weapon_category(bot);
    let dotted = if skill == "controlled" {
        controlled_button(&category)
    } else {
        style_buttons(&category, skill)
    };
    let Some(dotted) = dotted else {
        bot.log(&format!("no {skill} button for {category}, holding server style"));
        return Ok(false);
    };
    let Some(com) = (bot.tables.com_id)(dotted) else {
        bot.log(&format!("{dotted} missing from component table, holding server style"));
        return Ok(false);
    };
    let want = style_button_index(dotted);
    for _ in 0..3 {
        bot.if_button(com)?;
        bot.pump(3)?;
        if want.is_none() || live_style_index(bot) == want {
            return Ok(true);
        }
    }
    bot.log(&format!(
        "style {skill} ({dotted}) not confirmed (live={:?} want={want:?}), next kill retries",
        live_style_index(bot)
    ));
    Ok(false)
}


fn nearest_bank_tile(bot: &Bot) -> Option<(i32, i32, i32)> {
    knowledge::banks::all_banks()
        .into_iter()
        .filter(|bank| !bank.members && bank.level == i64::from(bot.world.level))
        .min_by_key(|bank| (bank.x - i64::from(bot.world.x)).abs() + (bank.z - i64::from(bot.world.z)).abs())
        .map(|bank| (bank.level as i32, bank.x as i32, bank.z as i32))
}


/// NPC spawn clusters for a spot (same-level spawns only).
fn ranked_clusters(bot: &Bot, spot: &knowledge::combat_spots::CombatSpot) -> Vec<(i32, i32, usize)> {
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

fn loot_ground(bot: &mut Bot) -> Result<u32, ScriptError> {
    let mut taken = 0u32;
    let mut items: Vec<((i32, i32, i32), u16)> = bot
        .world
        .ground_items(bot.world.x, bot.world.z, bot.world.level, LOOT_RADIUS, None)
        .into_iter()
        .map(|((tile, oid), _)| (tile, oid))
        .collect();
    items.sort_by_key(|(tile, _)| (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs());
    // Coins first: the goblin bootstrap exists to fund the axe, junk never
    // starves the coin pickup when the pack is nearly full.
    let coins = (bot.tables.find_obj_by_name)("coins");
    items.sort_by_key(|(_, oid)| if Some(*oid) == coins { 0 } else { 1 });
    for (_, oid) in items {
        if free_slots(bot) == 0 {
            break;
        }
        let name = (bot.tables.obj_name)(oid).unwrap_or_default();
        if name.is_empty() || name.to_lowercase().contains("bones") {
            continue;
        }
        // Dead socket, not a missed pickup: typed instead of skipping.
        match bot.take(Some(oid), None, TAKE_TICKS) {
            Ok(true) => taken += 1,
            Ok(false) => bot.log(&format!("loot {name} gave nothing at {:?}", bot.position())),
            Err(error)
                if matches!(
                    error,
                    lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)
                ) =>
            {
                return Err(error.into())
            }
            Err(error) => bot.log(&format!("loot {name} failed at {:?}: {error}", bot.position())),
        }
    }
    bury_bones(bot)?;
    Ok(taken)
}


/// Deaths make the fight return instead of resuming the killer spot: the
/// caller re-picks excluding it (one death) or fails typed (two in a row).
/// Returns (kills, bank trip wanted, death seen).
fn fight(
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
            let taken = loot_ground(bot)?;
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

/// Clusters on the bot's toll-fence flank (see skill_gather::same_side): the
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

fn run_spot(
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
    let targets = ranked_clusters(bot, spot);
    if targets.is_empty() {
        bot.log(&format!("no spawn for {}", spot.key));
        return Ok((0, false, false));
    }
    // One flank per hunt: cluster centers across the toll fence would
    // each cost the ~220-tile foot detour (paid doors stay off all
    // hunt). The per-pick filter in attack_nearest stops the same
    // oscillation inside the fight; this stops it between clusters.
    let mut targets = targets;
    if let Some(line) = crate::skill_common::toll_gate_x(bot) {
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
    for (center_x, center_z, count) in targets {
        // Pace retries: instant walk failures consume no ticks, so an
        // unpaced loop machine-guns the server into kicking us.
        bot.pump(2)?;
        bot.log(&format!("spot {} target ({center_x},{center_z}) spawns={count}", spot.key));
        let mut walk_fails = 0u32;
        while kills_limit == 0 || total < kills_limit {
            // Hunt entry: the one full evaluation (pack + bank when near
            // one, pack only with --no-bank). Inside the hunt only a loot
            // that was picked up re-evaluates; a death ends the fight and
            // returns to the run for the re-pick (see fight()).
            if !no_bank {
                if prepare_at_bank(bot)? {
                    banked_trip = true;
                }
            } else {
                let available = available_gear(bot);
                equip_plan(bot, &gear_plan(bot, &available, fight_weapon_style, style_damage_key))?;
            }
            // A dead socket must reconnect, not limp to the next cluster:
            // propagate connection failures, swallow the rest as before.
            match walk_checked(bot, center_x, center_z, 15000) {
                Err(error) if crate::skill_common::is_conn_abort(&error) => return Err(error),
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
            {
                let (kills, bank_wanted, died) =
                    fight(bot, spot, (center_x, center_z), remaining_kills(kills_limit, total), ticks_limit, no_bank)?;
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
fn auto_spot(bot: &Bot) -> knowledge::combat_spots::CombatSpot {
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
fn fallback_spot(
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
            ranked_clusters(bot, &spot)
                .into_iter()
                .map(|(spot_x, spot_z, _)| (spot_x - here.0).abs() + (spot_z - here.1).abs())
                .min()
                .map(|dist| (dist, spot))
        })
        .min_by_key(|(dist, _)| *dist)
        .map(|(_, spot)| spot)
}

/// Melee farmer: `melee [spot ...] [--kills N] [--ticks N] [--no-bank]`.
pub fn run_melee(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let mut spots: Vec<String> = Vec::new();
    let mut kills = 0u32;
    let mut ticks = 0u64;
    let mut no_bank = false;
    if args.iter().any(|arg| arg == "toll") {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--kills" => {
                kills = args.get(index + 1).and_then(|arg| arg.parse().ok()).unwrap_or(0);
                index += 2;
            }
            "--ticks" => {
                ticks = args.get(index + 1).and_then(|arg| arg.parse().ok()).unwrap_or(0);
                index += 2;
            }
            "--no-bank" => {
                no_bank = true;
                index += 1;
            }
            other => {
                spots.push(other.to_string());
                index += 1;
            }
        }
    }
    crate::skill_common::wait_pack_streamed(bot)?;
    let target = bot.world.ticks + 60;
    while bot.world.ticks < target && (!bot.world.stats.contains_key(&0) || !bot.world.stats.contains_key(&3)) {
        bot.pump(5)?;
    }
    check_events(bot)?;
    let picked: Vec<knowledge::combat_spots::CombatSpot> = {
        let named: Vec<_> = spots.iter().filter_map(|key| knowledge::combat_spots::by_key(key)).collect();
        if named.is_empty() {
            vec![auto_spot(bot)]
        } else {
            named
        }
    };
    bot.log(&format!("melee={} spots={}", melee_level(bot), picked.iter().map(|spot| spot.key).collect::<Vec<_>>().join(",")));
    // Deaths re-pick, never identically: the killer spot is excluded for
    // the run (one death hunts the fallback or the next picked spot, two
    // consecutive deaths fail typed). Kills and bank trips reset the
    // streak: only back-to-back deaths with no progress between are fatal.
    let mut queue = picked;
    let mut tally = DeathTally::fresh();
    // Auto mode (no explicit spot): the pick is a suggestion, never an
    // order. Unpaid and across the toll fence, it re-picks walkable
    // instead of ending the run idle; explicit picks keep the Usage.
    let auto = spots.is_empty();
    while !queue.is_empty() {
        let next = queue.iter().position(|queued| !tally.excludes(queued.key));
        let Some(next) = next else {
            bot.log("all remaining spots excluded after deaths, stopping");
            break;
        };
        let spot = queue.remove(next);
        bot.log(&format!("=== {} ===", spot.key));
        bot.intend(&format!("melee at {}", spot.key));
        let (total, died, banked) = match run_spot(bot, &spot, kills, ticks, no_bank) {
            Err(crate::ScriptError::Usage(detail)) if auto && detail.contains("toll fence") => {
                if !tally.excludes(spot.key) {
                    tally.exclude(spot.key);
                }
                match fallback_spot(bot, &spot, tally.excluded()) {
                    Some(fallback) => {
                        bot.log(&format!("{} across the toll fence, falling back to {}", spot.key, fallback.key));
                        queue.insert(0, fallback);
                        continue;
                    }
                    None => return Err(crate::ScriptError::Usage(detail)),
                }
            }
            other => other?,
        };
        if total > 0 || banked {
            tally.note_progress();
        }
        bot.log(&format!("ok {} killed {total}", spot.key));
        if !died {
            continue;
        }
        tally.note_death(spot.key);
        if tally.fatal() {
            return Err(danger_error(spot.key, tally.deaths()));
        }
        if queue.iter().any(|queued| !tally.excludes(queued.key)) {
            bot.log(&format!("{} killed us, re-picking without it", spot.key));
            continue;
        }
        match fallback_spot(bot, &spot, tally.excluded()) {
            Some(fallback) => {
                bot.log(&format!("{} killed us, falling back to {}", spot.key, fallback.key));
                queue.insert(0, fallback);
            }
            None => {
                if !no_bank {
                    prepare_at_bank(bot)?;
                }
                bot.log(&format!("{} killed us, nothing safer on foot: banked and stopping", spot.key));
                break;
            }
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::{COM_MODE_VARP, auto_spot, controlled_button, current_skill, fallback_spot, live_skill, live_style_index, melee_stat, next_style, rank_foes, remaining_kills, style_button_index, style_buttons, style_damage_key, weakest_melee};
    use crate::kit::survival::{DeathTally, danger_error};
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

    /// t01's live state (atk 10, str/def 1): the laggard trains, never the
    /// leader. Previously the blind 3-kill rotation sat on accurate.
    #[test]
    fn weakest_melee_picks_laggard() {
        let bot = stat_bot(10, 1, 1);
        assert_eq!(weakest_melee(&bot), "strength");
        assert_eq!(melee_stat(&bot, "attack"), 10);
        assert_eq!(next_style(&bot, "attack"), Some("strength"));
        assert_eq!(next_style(&bot, "strength"), None, "already training the weakest: hold");
        assert_eq!(next_style(&bot, "controlled"), None, "controlled never rotates");
    }

    /// Balanced skills hold the current style (no click spam on ties), a
    /// one-level gap rotates to the laggard.
    #[test]
    fn rotation_holds_ties_switches_on_gap() {
        let tied = stat_bot(5, 5, 5);
        assert_eq!(next_style(&tied, "attack"), None);
        let gapped = stat_bot(6, 5, 5);
        assert_eq!(next_style(&gapped, "attack"), Some("strength"), "5 < 6 trains first");
        assert_eq!(next_style(&gapped, "strength"), None, "weakest held");
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

    /// Controlled exists exactly where the revision's combat.dbrow puts a
    /// ^style_melee_controlled entry (slash/spear/polearm/spiked/claws);
    /// unarmed and plain axes have none.
    #[test]
    fn controlled_only_where_the_revision_has_it() {
        assert_eq!(controlled_button("weapon_slash"), Some("combat_hacksword:hack2"));
        assert_eq!(controlled_button("weapon_spear"), Some("combat_spear:spear0"));
        assert_eq!(controlled_button("weapon_polearm"), Some("combat_polearm:polearm0"));
        assert_eq!(controlled_button("unarmed"), None);
        assert_eq!(controlled_button("weapon_axe"), None);
        assert_eq!(controlled_button("weapon_blunt"), None);
    }




    /// Style map covers the buttons the bot presses (dbrow damagetypes x
    /// player_attackstyles suffixes); unknown categories fall back to max.
    #[test]
    fn style_damage_follows_the_pressed_button() {
        assert_eq!(style_damage_key("weapon_stab", "attack"), Some("stab"));
        assert_eq!(style_damage_key("weapon_blunt", "defence"), Some("crush"));
        assert_eq!(style_damage_key("weapon_axe", "strength"), Some("slash"));
        assert_eq!(style_damage_key("weapon_pickaxe", "attack"), Some("stab"));
        assert_eq!(style_damage_key("weapon_spear", "attack"), Some("stab"));
        assert_eq!(style_damage_key("weapon_polearm", "controlled"), Some("stab"));
        assert_eq!(style_damage_key("unarmed", "defence"), Some("crush"));
        assert_eq!(style_damage_key("weapon_bow", "attack"), None, "ranged: max fallback");
    }


    /// Toll-fence side test: the gate line splits flanks (Al-Kharid gate
    /// halves 2882/2883); same flank hunts together, never across.
    #[test]
    fn fence_side_splits_at_the_gate_line() {
        use crate::skill_gather::same_side;
        assert!(same_side(3268, 3262, 3255), "west flank together");
        assert!(same_side(3268, 3270, 3300), "east flank together");
        assert!(!same_side(3268, 3262, 3270), "across never targets");
        assert!(!same_side(3268, 3270, 3262), "across either way");
    }

    /// Shop staff are excluded by op category, never by name: the
    /// Lumbridge general store pair (520/521), Bob (519) and bankers
    /// carry Trade/Bank; every valid foe (goblin 100, chicken 41, man 1)
    /// carries Attack instead.
    #[test]
    fn shop_staff_never_targeted_by_category() {
        assert!(super::is_shop_staff(520), "Lumbridge keeper");
        assert!(super::is_shop_staff(521), "Lumbridge assistant");
        assert!(super::is_shop_staff(519), "Bob the axe seller");
        assert!(super::is_shop_staff(494), "banker");
        assert!(!super::is_shop_staff(100), "goblin fights");
        assert!(!super::is_shop_staff(41), "chicken fights");
        assert!(!super::is_shop_staff(1), "man fights");
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
        let targets = super::ranked_clusters(&bot, &fallback);
        assert!(!targets.is_empty(), "fallback has same-level spawns");
        let kept = super::same_flank_clusters(3268, bot.world.x, &targets, false);
        assert!(!kept.is_empty(), "{} has no west-flank cluster to hunt", fallback.key);
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

    /// Every pressed button resolves to the com_mode index the server
    /// takes (dbrow row order x player_attackstyles suffixes): the
    /// verifier compares against this, never a second table.
    #[test]
    fn button_index_matches_pressed_button() {
        let cases = [
            ("weapon_slash", "attack", 0),
            ("weapon_slash", "strength", 1),
            ("weapon_slash", "defence", 3),
            ("weapon_stab", "attack", 0),
            ("weapon_stab", "strength", 1),
            ("weapon_stab", "defence", 3),
            ("weapon_blunt", "attack", 0),
            ("weapon_blunt", "strength", 1),
            ("weapon_blunt", "defence", 2),
            ("weapon_axe", "attack", 0),
            ("weapon_axe", "strength", 1),
            ("weapon_axe", "defence", 3),
            ("weapon_2h_sword", "attack", 0),
            ("weapon_2h_sword", "strength", 1),
            ("weapon_2h_sword", "defence", 3),
            ("weapon_pickaxe", "attack", 0),
            ("weapon_pickaxe", "strength", 1),
            ("weapon_pickaxe", "defence", 3),
            ("weapon_scythe", "attack", 0),
            ("weapon_scythe", "strength", 1),
            ("weapon_scythe", "defence", 3),
            ("weapon_spear", "attack", 0),
            ("weapon_spear", "strength", 0),
            ("weapon_spear", "defence", 3),
            ("unarmed", "attack", 0),
            ("unarmed", "strength", 1),
            ("unarmed", "defence", 2),
        ];
        for (category, skill, want) in cases {
            let dotted = style_buttons(category, skill).unwrap_or_else(|| panic!("no button for {category}/{skill}"));
            assert_eq!(style_button_index(dotted), Some(want), "{dotted}");
        }
        assert_eq!(style_button_index("combat_hacksword:hack2"), Some(2), "controlled slash");
        assert_eq!(style_button_index("combat_spear:spear0"), Some(0), "controlled spear");
        assert_eq!(style_button_index("combat_polearm:polearm0"), Some(0), "controlled polearm");
        assert_eq!(style_button_index("combat_x:foo"), None, "no trailing number");
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

    /// Live index decodes to the trained skill per revision tables:
    /// stab/axe/2h/pickaxe/scythe 2 is a second aggressive, spear is
    /// all-controlled but its defensive, blunt/unarmed/staff stop at 2.
    #[test]
    fn live_skill_follows_dbrow_tables() {
        assert_eq!(live_skill("weapon_stab", 2), Some("strength"), "second aggressive");
        assert_eq!(live_skill("weapon_stab", 3), Some("defence"));
        assert_eq!(live_skill("weapon_spear", 1), Some("controlled"));
        assert_eq!(live_skill("weapon_spear", 3), Some("defence"));
        assert_eq!(live_skill("weapon_polearm", 0), Some("controlled"));
        assert_eq!(live_skill("weapon_polearm", 2), Some("defence"));
        assert_eq!(live_skill("weapon_slash", 2), Some("controlled"));
        assert_eq!(live_skill("weapon_blunt", 2), Some("defence"));
        assert_eq!(live_skill("unarmed", 2), Some("defence"));
        assert_eq!(live_skill("unarmed", 0), Some("attack"));
        assert_eq!(live_skill("weapon_stab", 9), None, "out of table");
    }

    /// t02 live lock (assumed defence, server aggressive, atk5/str12/def4
    /// bare-fisted): trusting the live varp re-arms the rotation to the
    /// real laggard instead of holding a style never taken.
    #[test]
    fn live_varp_unsticks_a_dropped_press() {
        let mut bot = stat_bot(5, 4, 12);
        bot.world.varps.insert(COM_MODE_VARP, 1);
        assert_eq!(live_style_index(&bot), Some(1));
        assert_eq!(current_skill(&bot, "defence"), "strength", "server trains strength, not the assumed defence");
        assert_eq!(next_style(&bot, current_skill(&bot, "defence")), Some("defence"), "real laggard re-armed");
        let mut quiet = stat_bot(5, 5, 5);
        assert_eq!(current_skill(&quiet, "attack"), "attack", "no varp yet: keep assumed");
        quiet.world.varps.insert(COM_MODE_VARP, 3);
        assert_eq!(current_skill(&quiet, "attack"), "attack", "index 3 is out of the unarmed table: keep assumed");
    }
}
