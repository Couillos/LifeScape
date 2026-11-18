//! Hunt: one combat spot, cluster after cluster, bank trips between
//! fights. Split out of `kit::fight` (rule 4; `fight::run_spot` stays
//! the public entry). The ladder adds a `HuntPlan`: its seeded cluster
//! order (the Edgeville dungeon giants included, the router reaching
//! them by the Edgeville trapdoor, or the brass-key house when the key
//! rides along), the sized food load, the food rule (no
//! dangerous spot without food: typed `Blocked` instead) and the items
//! kept through deposits.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::waits::wait_ticks;
use lifescape_sdk::Bot;

use crate::kit::bans::same_side;
use crate::kit::gear::{equip_plan, gear_plan, prepare_at_bank_for};
use crate::kit::style::{fight_weapon_style, style_damage_key};
use crate::kit::survival::{food_count, FOOD_TARGET};
use crate::skill_walk::walk_checked_at;
use crate::ScriptError;

/// What a hunt carries and where it goes.
#[derive(Debug, Clone, PartialEq)]
pub struct HuntPlan {
    /// Explicit cluster order (x, z, spawns); `None` ranks the spot's
    /// clusters (`kit::foes::ranked_clusters`) and keeps the surface rule.
    pub clusters: Option<Vec<(i32, i32, usize)>>,
    /// Food units withdrawn per bank trip.
    pub food_target: u32,
    /// The spot bites: never fight it without food (`Blocked`).
    pub food_required: bool,
    /// Items kept through every deposit (and withdrawn when banked).
    pub keep: Vec<(String, u32)>,
    /// Level target of the task (stat id, level, the `--level` flag):
    /// the hunt and the fight stop the moment it is reached instead of
    /// running the budget out past the goal.
    pub stop_level: Option<(u8, u8)>,
    /// The run's `--want` goal (`farm:<drop>`): the hunt and the fight
    /// stop once the looted drop meets it, never a spot's worth of kills
    /// (a whole pack, a whole budget) past N.
    pub stop_want: Option<crate::kit::cycle::WantGoal>,
}

/// Goal checks of one fight: the `--level` target and the `--want` drop.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HuntStop {
    pub level: Option<(u8, u8)>,
    pub want: Option<crate::kit::cycle::WantGoal>,
}

impl HuntStop {
    /// True once either goal is met (never without one).
    pub fn reached(&self, bot: &Bot) -> bool {
        level_reached(&bot.world.stats, self.level) || crate::kit::want::met(self.want, bot)
    }
}

impl HuntPlan {
    /// The pre-ladder hunt: ranked clusters, the usual 10 food when the
    /// bank has some, food optional, no level target.
    pub fn plain() -> HuntPlan {
        HuntPlan { clusters: None, food_target: FOOD_TARGET as u32, food_required: false, keep: Vec::new(), stop_level: None, stop_want: None }
    }

    /// The same plan stopping at the task's `--level` target.
    pub fn until(self, spec: &crate::kit::cycle::TaskSpec) -> HuntPlan {
        HuntPlan { stop_level: stop_level_of(spec), ..self }
    }

    /// The same plan stopping at the run's `--want` goal
    /// (`CycleState::want_goal`).
    pub fn want(self, goal: Option<crate::kit::cycle::WantGoal>) -> HuntPlan {
        HuntPlan { stop_want: goal, ..self }
    }

    /// The goal checks the fight loop reads between foes.
    pub fn stop(&self) -> HuntStop {
        HuntStop { level: self.stop_level, want: self.stop_want }
    }
}

/// (stat id, level) of a task's `--level SKILL N` target, if any.
pub fn stop_level_of(spec: &crate::kit::cycle::TaskSpec) -> Option<(u8, u8)> {
    let skill = spec.targets.level_skill.as_deref()?;
    let level = spec.targets.level?;
    lifescape_sdk::provenance::skill_id(skill).map(|stat| (stat, level))
}

/// True once the live level reaches the target (the `poll_stop` rule:
/// current level, read from the stats stream).
pub fn level_reached(stats: &std::collections::HashMap<u8, (u8, i32)>, stop_level: Option<(u8, u8)>) -> bool {
    stop_level.is_some_and(|(stat, level)| stats.get(&stat).is_some_and(|(now, _)| *now >= level))
}

/// Typed food requirement: the brain files an obtain for `count` units
/// of `item` (cooking, market) before the spot is hunted again.
pub fn food_blocked(item: &str) -> ScriptError {
    ScriptError::Blocked { missing: vec![item.to_string()] }
}

/// True when a hunt that needs food stands without any: the pack is
/// empty after the bank trip (or with `--no-bank`).
pub fn foodless(plan: &HuntPlan, carried: u32) -> bool {
    plan.food_required && carried == 0
}

/// Clusters on the bot's toll-fence flank (see kit::bans::same_side): the
/// crossing is never paid in melee, so an eastern cluster from a western
/// hunt is a ~220-tile detour per visit, never a shortcut. When every
/// cluster is across and the run did not opt into `toll`, the list stays
/// empty and the caller fails named instead of burning NoRoute dossiers.
/// Falls back to every cluster only when paid (an all-across spot still
/// hunts instead of idling once the gate is crossable).
pub(crate) fn same_flank_clusters(home: (i32, i32), targets: &[(i32, i32, usize)], allow_paid: bool) -> Vec<(i32, i32, usize)> {
    let kept: Vec<(i32, i32, usize)> =
        targets.iter().filter(|(center_x, center_z, _)| same_side((*center_x, *center_z), home)).copied().collect();
    if kept.is_empty() && allow_paid {
        targets.to_vec()
    } else {
        kept
    }
}

/// Where the hunt's flank is read from: the brain's site when it fixed
/// one (it priced the route there), else the bot. rose_young
/// 2026-11-18: read from the bot at the Al Kharid bank, it dropped the
/// Lumbridge site's camp and kept a wall centroid in Varrock.
pub(crate) fn flank_home(site: Option<(i32, i32, i32)>, here: (i32, i32)) -> (i32, i32) {
    site.map(|(x, z, _)| (x, z)).unwrap_or(here)
}

/// End of a hunt over every cluster: no kill and no cluster ever
/// reached is a typed failure (the brain counts it), never "ok killed
/// 0" relaunched every pass.
pub(crate) fn hunt_end(total: u32, reached: bool, spot: &str, at: (i32, i32, i32)) -> Result<u32, ScriptError> {
    if total == 0 && !reached {
        return Err(ScriptError::Failed(format!("no {spot} cluster reached from {at:?}: every walk failed")));
    }
    Ok(total)
}

/// Kills still wanted: 0 means unlimited, so a plain subtraction panics
/// on the second cluster once anything died (t02 live).
pub(crate) fn remaining_kills(kills_limit: u32, total: u32) -> u32 {
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

/// Typed failure of a spot with no free spawn on the hunt's level: never
/// a 0-tick "goal reached".
pub fn no_spawn_error(bot: &Bot, spot: &str) -> ScriptError {
    ScriptError::Failed(format!("no spawn for {spot} on level {} near {:?} (site {:?})", bot.world.level, bot.position(), bot.assigned_site))
}

/// Typed failure of a fight run that killed nothing, banked nothing and
/// never died: every cluster walk failed. Reported as `goal_reached` it
/// read as a success and the brain relaunched it at once.
pub fn nothing_hunted_error(bot: &Bot, style: &str, spots: &[&str]) -> ScriptError {
    ScriptError::Failed(format!("{style} {}: nothing killed, no cluster reached from {:?}", spots.join(","), bot.position()))
}

/// True when a hunt must refuse a spot whose camp of record is
/// underground (`has_entrance`): without the ladder's clusters (route
/// verified offline) and without a brain site on the surface. A surface
/// `--site` (the Wilderness hill giants, user 2026-11-16: open to every
/// bot) hunts the surface spawns of the same foes.
pub fn underground_refused(has_entrance: bool, has_clusters: bool, site: Option<(i32, i32, i32)>) -> bool {
    let surface_site = site.is_some_and(|(_, z, _)| !knowledge::underground::is_underground(i64::from(z)));
    has_entrance && !has_clusters && !surface_site
}

/// Clusters on the brain site's side of the surface (a surface site
/// never walks to a dungeon camp, which needs the ladder's route); no
/// site: unchanged.
pub fn site_side_clusters(clusters: Vec<(i32, i32, usize)>, site: Option<(i32, i32, i32)>) -> Vec<(i32, i32, usize)> {
    let Some((_, site_z, _)) = site else { return clusters };
    let below = knowledge::underground::is_underground(i64::from(site_z));
    clusters.into_iter().filter(|(_, z, _)| knowledge::underground::is_underground(i64::from(*z)) == below).collect()
}

/// The pre-ladder hunt (`HuntPlan::plain`).
pub fn run_spot(
    bot: &mut Bot,
    spot: &knowledge::combat_spots::CombatSpot,
    kills_limit: u32,
    ticks_limit: u64,
    no_bank: bool,
) -> Result<(u32, bool, bool), ScriptError> {
    run_spot_with(bot, spot, &HuntPlan::plain(), kills_limit, ticks_limit, no_bank)
}

/// Hunt one spot under a plan. Returns (kills, died, banked trip).
pub fn run_spot_with(
    bot: &mut Bot,
    spot: &knowledge::combat_spots::CombatSpot,
    plan: &HuntPlan,
    kills_limit: u32,
    ticks_limit: u64,
    no_bank: bool,
) -> Result<(u32, bool, bool), ScriptError> {
    if underground_refused(spot.entrance.is_some(), plan.clusters.is_some(), bot.assigned_site) {
        // Underground camps only ride with the ladder's clusters (route
        // verified offline, approach rated); a bare key never walks down.
        // A surface site of the same foes hunts there (`underground_refused`).
        return Err(ScriptError::Usage(format!(
            "spot {} is underground, not traversable from {:?}",
            spot.key,
            bot.position()
        )));
    }
    // The brain's site (`--site`) anchors the hunt: clusters are ranked
    // around it, on its level, and walked to on that level, never around
    // wherever the last run left the bot (live 2026-11-14: `melee men
    // --site 3100,3279,0` from the mill top (3165,3306,2) found "no spawn
    // for men" on level 2 and reported goal_reached after 0 ticks, in a
    // loop). Ladder clusters keep the bot's level.
    let hunt_level = match (&plan.clusters, bot.assigned_site) {
        (None, Some((_, _, site_level))) => site_level,
        _ => bot.world.level,
    };
    let targets = match &plan.clusters {
        Some(clusters) => clusters.clone(),
        None => site_side_clusters(crate::kit::foes::ranked_clusters(bot, spot), bot.assigned_site),
    };
    if targets.is_empty() {
        bot.log(&format!("no spawn for {}", spot.key));
        if plan.clusters.is_none() {
            return Err(no_spawn_error(bot, spot.key));
        }
        return Ok((0, false, false));
    }
    // One flank per hunt: cluster centers across the toll fence would
    // each cost the ~220-tile foot detour (paid doors stay off all
    // hunt). The per-pick filter in attack_nearest stops the same
    // oscillation inside the fight; this stops it between clusters.
    let mut targets = targets;
    if let Some(line) = crate::kit::bank::toll_gate_x(bot) {
        let kept = same_flank_clusters(flank_home(bot.assigned_site, (bot.world.x, bot.world.z)), &targets, bot.allow_paid);
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
    let mut reached = false;
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
            // Level goal met (between fights, after a bank trip): stop
            // here, the caller's poll_stop reads it as goal_reached.
            if plan.stop().reached(bot) {
                bot.log(&format!("goal reached (level {:?}, want {:?}), ending the hunt", plan.stop_level, plan.stop_want));
                return Ok((total, false, banked_trip));
            }
            // Hunt entry: the one full evaluation (pack + bank when near
            // one, pack only with --no-bank). Inside the hunt only a loot
            // that was picked up re-evaluates; a death ends the fight and
            // returns to the run for the re-pick (see fight()).
            if !no_bank {
                if prepare_at_bank_for(bot, plan.food_target, &plan.keep)? {
                    banked_trip = true;
                }
            } else {
                let available = crate::kit::gear::available_gear(bot);
                equip_plan(bot, &gear_plan(bot, &available, fight_weapon_style, style_damage_key))?;
            }
            // Never walk into a spot that bites without food (user rule:
            // food everywhere but cows and chickens).
            if foodless(plan, food_count(bot)) {
                bot.log(&format!("no food for {} after the bank trip, blocking", spot.key));
                return Err(food_blocked(crate::kit::ladder::asked_food(bot)));
            }
            // A dead socket must reconnect, not limp to the next cluster:
            // propagate connection failures, swallow the rest as before.
            match walk_checked_at(bot, center_x, center_z, hunt_level, 15000) {
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
                    reached = true;
                }
            }
            // The status line reads the last intent: without this the
            // whole hunt showed "banking at (x,z)" from prepare_at_bank.
            bot.intend(&format!("hunting {} near ({center_x},{center_z})", spot.key));
            let (kills, bank_wanted, died) = crate::kit::fight::fight(
                bot,
                spot,
                (center_x, center_z),
                remaining_kills(kills_limit, total),
                budget_left(ticks_limit, run_started, bot.world.ticks),
                no_bank,
                plan.food_required,
                plan.stop(),
            )?;
            total += kills;
            if died {
                return Ok((total, true, banked_trip));
            }
            if !bank_wanted || (kills_limit > 0 && total >= kills_limit) {
                return Ok((total, false, banked_trip));
            }
        }
    }
    let total = hunt_end(total, reached, spot.key, bot.position())?;
    Ok((total, false, banked_trip))
}

#[cfg(test)]
mod tests {
    use super::{flank_home, food_blocked, foodless, hunt_end, level_reached, same_flank_clusters, stop_level_of, HuntPlan};

    /// rose_young / Yogosun 2026-11-18: at the Al Kharid bank, sent to
    /// the Lumbridge castle men (`--site 3206,3219,1`, a 311-tile walk
    /// without the toll), the flank filter read the BOT's side and kept
    /// only the Varrock palace centroid (3232,3392,1), a brick wall: two
    /// NoRoute a run, "ok men killed 0", relaunched every 10 s for hours.
    /// The flank is the site's; a hunt that reached no cluster fails typed.
    #[test]
    fn the_hunt_flank_is_the_sites_and_a_hunt_reaching_nothing_fails() {
        let targets = vec![(3206, 3219, 1usize), (3232, 3392, 2usize)];
        let home = flank_home(Some((3206, 3219, 1)), (3269, 3169));
        let kept = same_flank_clusters(home, &targets, false);
        assert!(kept.iter().any(|(x, z, _)| (*x, *z) == (3206, 3219)), "the site's camp stays: {kept:?}");
        assert_eq!(flank_home(None, (3269, 3169)), (3269, 3169), "no site: the bot's side");
        assert!(hunt_end(0, false, "men", (3269, 3169, 0)).is_err(), "no cluster reached, no kill: typed failure");
        assert_eq!(hunt_end(0, true, "men", (3269, 3169, 0)).ok(), Some(0), "reached but nothing died: a quiet camp, ok");
        assert_eq!(hunt_end(3, false, "men", (3269, 3169, 0)).ok(), Some(3));
    }

    /// Live 2026-11-17 (george_97, lucy_rhodes, ryan_park, frank_lane…):
    /// `melee hill_giants --site 3308,3661,0` (the Wilderness surface
    /// giants, open to every bot) was refused "spot hill_giants is
    /// underground, not traversable" because the spot's camp of record is
    /// the Edgeville dungeon: 21 Usage, abandons, attack/defence bans in
    /// 40 minutes. A surface site hunts the surface spawns; only a hunt
    /// that would go underground without the ladder's route is refused.
    #[test]
    fn a_surface_site_of_an_underground_spot_hunts_on_the_surface() {
        use super::{site_side_clusters, underground_refused};
        assert!(!underground_refused(true, false, Some((3308, 3661, 0))), "surface Wilderness giants: hunt");
        assert!(underground_refused(true, false, Some((3100, 9830, 0))), "dungeon site without the ladder route: refused");
        assert!(underground_refused(true, false, None), "no site: the camp of record is underground, refused");
        assert!(!underground_refused(true, true, None), "the ladder's clusters ride down");
        assert!(!underground_refused(false, false, None), "a surface spot");
        let clusters = vec![(3300, 3650, 4), (3105, 9830, 9), (3100, 3850, 3)];
        assert_eq!(site_side_clusters(clusters.clone(), Some((3308, 3661, 0))), vec![(3300, 3650, 4), (3100, 3850, 3)], "surface site: no dungeon camp");
        assert_eq!(site_side_clusters(clusters.clone(), None), clusters, "no site: unchanged");
    }

    /// `farm:cowhide --want cowhide 2`: the fight stops between foes once
    /// the looted hides meet the order (before: a whole spot visit, a
    /// pack or the budget, past N); no `--want` never stops on drops.
    #[test]
    fn want_drop_stops_the_fight() {
        let mut bot = lifescape_sdk::Bot::new("127.0.0.1", 43594, ".", ".");
        crate::tables_live::attach_live_tables(&mut bot);
        let hide = crate::kit::pack::obj_id(&bot, "cow_hide").expect("cow_hide");
        let pack = (bot.tables.com_id)("inventory:inv").expect("inventory");
        let goal = Some(crate::kit::cycle::WantGoal { obj: hide, held: 2 });
        let plan = HuntPlan::plain().want(goal);
        assert_eq!(plan.stop().want, goal);
        bot.world.inv.insert(pack, [(0u8, (hide, 1u32))].into_iter().collect());
        assert!(!plan.stop().reached(&bot), "1 hide of 2: keep fighting");
        bot.world.inv.insert(pack, [(0u8, (hide, 1u32)), (1u8, (hide, 1u32))].into_iter().collect());
        assert!(plan.stop().reached(&bot), "2 hides: stop between foes");
        assert!(!HuntPlan::plain().stop().reached(&bot), "no --want, no --level: never");
    }

    /// `--level strength 40` rides into the hunt plan and stops the
    /// hunt and the fight once the live level reaches it (agent A: a
    /// climb used to burn up to 18 000 ticks past its goal inside
    /// run_spot); no target never stops.
    #[test]
    fn level_target_stops_the_hunt() {
        let args: Vec<String> = ["ladder", "--level", "strength", "40"].iter().map(|arg| arg.to_string()).collect();
        let spec = crate::kit::cycle::parse_cli("melee", &args).expect("melee flags parse");
        assert_eq!(stop_level_of(&spec), Some((2, 40)), "strength is stat 2");
        let plan = HuntPlan::plain().until(&spec);
        assert_eq!(plan.stop_level, Some((2, 40)));
        let mut stats = std::collections::HashMap::new();
        stats.insert(2u8, (39u8, 0i32));
        assert!(!level_reached(&stats, plan.stop_level), "39 keeps fighting");
        stats.insert(2u8, (40u8, 0i32));
        assert!(level_reached(&stats, plan.stop_level), "40 stops");
        assert!(!level_reached(&stats, HuntPlan::plain().stop_level), "no target never stops");
        assert!(!level_reached(&std::collections::HashMap::new(), plan.stop_level), "stats not streamed yet: keep going");
        let plain = crate::kit::cycle::parse_cli("melee", &["cows".to_string()]).expect("plain parses");
        assert_eq!(stop_level_of(&plain), None);
    }

    /// The plain hunt never requires food (pre-ladder behaviour); a
    /// ladder hunt on a biting spot with an empty pack blocks typed.
    #[test]
    fn food_rule_blocks_only_required_hunts() {
        let plain = HuntPlan::plain();
        assert!(!foodless(&plain, 0), "plain hunts keep fighting foodless");
        let biting = HuntPlan { food_required: true, food_target: 8, ..HuntPlan::plain() };
        assert!(foodless(&biting, 0), "no food, no biting spot");
        assert!(!foodless(&biting, 1), "some food fights, the bank trip refills");
        let error = food_blocked("shrimp");
        assert!(crate::is_blocked(&error), "typed Blocked, got {error}");
        assert_eq!(error.to_string(), "blocked, missing: shrimp");
    }
}
