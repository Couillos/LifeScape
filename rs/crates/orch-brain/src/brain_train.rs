#![forbid(unsafe_code)]
//! Training method of every level climb, brain side (plan rendements R6,
//! R7, D10, D13, D14, D17): harvest, combat, recipes, spells — a
//! `reach_level` tranche picks its method from the TRAINING GUIDE the
//! guides store precomputed (`guides_store::current()`: the shared `Arc`
//! of the last cycle, never a build on demand):
//! - the bot's lines: the guide read at its live level, its quests done
//!   and its own free stock (`GuideSet::training`: the recipes on its
//!   stock as `stock` lines, R7 (a) — the alchemy of its own objects),
//!   its skill, only the lines a level climb launches
//!   (`mapping::map_method`; quest shortcuts are quest nodes; a melee
//!   line in the climb's own style, the script fights in it);
//! - each line judged on the bot's LIVE state (`guide_feasible::
//!   feasible_for`: levels, combat, quests, stock, free wallet, the
//!   cycle's asks): feasible now (class 0), after a lot's coins (1), after
//!   a tool to obtain (2), else never; quarantined, cooled or excluded
//!   methods never;
//! - each line scored for THIS bot (`train_score`): ticks per XP plus
//!   coins per XP converted at its time value (`Climb::coin_rate`: what
//!   its work really earns per tick, D1, never its D17), weighed by
//!   thrift and market_affinity;
//! - A1 (a): a pending climb that opens a lucrative way (the unserved
//!   demand, `brain_value::pending_investments`) lifts the coin rate to
//!   its gain per tick (`investment::time_value`): a line that costs coins
//!   and saves ticks is taken for it;
//! - the choice: the draw of `train_score::pick` (`guide_pick`: the first
//!   non-empty class, a weighted draw among the lines within 15 % of its
//!   best score, D10, the unmeasured lines' 5 % of exploration, D14), the
//!   bot's seed, node and draw counter; the node's previous pick (its
//!   block's `blocked_by`) taken again once it works now (the tool it
//!   blocked on is used); the consumables it lacks for one action
//!   obtained first (`training::missing_inputs_with`, the measured
//!   yields), handed to the resolver (`ReadyCtx::training`), the choice
//!   left in `ReadyCtx::drawn` for the record (`brain_picks`).
//!
//! No guide published yet (the first cycle after a start) or no line
//! within reach: no pick, logged; the static tranche method runs (its
//! skeleton deps filed: `brain_plan_read::with_plan_read` tells the
//! skeleton which tranches the guide covers). Called once per launch of
//! a climb, right before `resolve_ready`.

use lifescape_json::Value;
use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::guide_feasible::{feasible_for, BotState, Feasibility, Pending};
use lifescape_planner::guide_model::{GuideFilter, Supply, TrainingRow};
use lifescape_planner::guide_pick::Choice;
use lifescape_planner::train_score::{pick, rank_classed, ticks_per_coin, Classed, LineScore};
use lifescape_planner::training::{missing_inputs_with, TrainPick};
use lifescape_planner::work_rate::RateReader;
use lifescape_planner::{Life, Node, NodeKind};

use crate::guides_store::GuideSet;

/// The line a bot trains with and why.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GuidePick {
    pub pick: TrainPick,
    pub supply: Supply,
    pub score: LineScore,
    /// Lines scored within the bot's reach (every class).
    pub scored: usize,
    /// The bot's D17 price (best accessible money line per 6000): the
    /// log's figure, never its time value (`Climb::coin_rate`).
    pub best_profit: Option<f64>,
    pub choice: Choice,
}

/// One climb to pick for.
pub(crate) struct Climb<'a> {
    pub skill: &'a str,
    /// The bot's live level in the skill.
    pub level: u8,
    pub target: u8,
    /// The node's previous pick (`blocked_by`): taken again once it works
    /// now.
    pub previous: Option<&'a str>,
    /// The food a fight lacks before it can run (`food_gate::food_short`
    /// on the bot's context: a spot that bites, no food in hand): such a
    /// line works after its food (class 1 at best), the food obtained
    /// first. `None`: not judged (offline).
    pub food: Option<&'a dyn Fn(&str) -> Option<String>>,
    /// Coins one tick of the bot's time is worth (D1, plan équilibre
    /// step 1: its measured gain, `brain_d17::coin_rate`): the coins of a
    /// line convert to ticks at it.
    pub coin_rate: f64,
}

/// Levels of the skill one `reach_combat` launch reads its line for (its
/// consumables are sized on that tranche; the node re-resolves after).
pub(crate) const COMBAT_TRANCHE_LEVELS: u8 = 5;

/// The skill a `reach_combat` node climbs on the training guide (R7
/// suite, 2026-11-17: the measured lines, never the static chickens of
/// `combat::training_method`): its style's own skill (the static method
/// names it: ranged, magic), melee the lowest of attack, strength and
/// defence (the combat level rises the most evenly; attack on a tie).
pub(crate) fn combat_climb_skill(static_method: &str, level_of: &dyn Fn(&str) -> u8) -> &'static str {
    if static_method.starts_with("fight:ranged/") {
        return "ranged";
    }
    if static_method.starts_with("fight:magic/") {
        return "magic";
    }
    ["attack", "strength", "defence"].into_iter().min_by_key(|skill| level_of(skill)).unwrap_or("attack")
}

/// True for a guide line a `skill` level climb can launch (module doc).
fn launchable(row: &TrainingRow, skill: &str) -> bool {
    if row.method.starts_with("quest:") || row.skill != skill {
        return false;
    }
    // A melee fight runs in the climb's own style (`mapping`: the
    // script's style follows the climb's skill); hitpoints come of any.
    if let Some(style) = row.style.strip_prefix("melee:") {
        if style != skill && skill != "hitpoints" {
            return false;
        }
    }
    let probe = Node::new("probe", NodeKind::ReachLevel { skill: skill.to_string(), level: 99 }, Vec::new(), 0);
    lifescape_planner::map_method(&row.method, &probe, None).is_some()
}

/// The class of a line for the bot (`guide_pick::class_order`): feasible
/// now 0, after a lot's coins 1, after a tool to obtain 2; `None` out of
/// its reach (a level, a quest, a stock, an object nobody sells...).
pub(crate) fn class_for(row: &TrainingRow, bot: &BotState) -> Option<u8> {
    match feasible_for(row, bot) {
        Feasibility::Feasible => Some(0),
        Feasibility::AfterPrereqs(pending) if pending.iter().all(Pending::is_supply) => Some(if pending.iter().any(|need| matches!(need, Pending::Tool { .. })) { 2 } else { 1 }),
        _ => None,
    }
}

/// The bot's launchable lines of a `skill` climb at `level`, its quests
/// and its own `stock` (the stock lines, R7 (a)).
fn climb_lines(set: &GuideSet, skill: &str, level: u8, quests_done: &[String], stock: &[(String, f64)]) -> Vec<TrainingRow> {
    let filter = GuideFilter { skill: Some(skill.to_string()), level: Some(level), quests_done: quests_done.to_vec(), held: stock.to_vec(), ..GuideFilter::default() };
    let (rows, _) = set.training(&filter);
    rows.into_iter().filter(|row| launchable(row, skill)).collect()
}

/// The bot's D17 price at `level` (best accessible money line, read in
/// the money view of its band with its quests).
fn d17_of(set: &GuideSet, level: u8, quests_done: &[String]) -> Option<f64> {
    let filter = GuideFilter { level: Some(level), quests_done: quests_done.to_vec(), ..GuideFilter::default() };
    lifescape_planner::guide_read::best_profit_in(set.money_view(Some(level)), &filter)
}

/// True when the guide holds a line of `skill` from `from` within the
/// bot's reach (the skeleton's oracle, `brain_plan_read`): the climb will
/// pick it at resolve.
pub(crate) fn guide_covers(set: &GuideSet, skill: &str, from: u8, to: u8, profile: &lifescape_planner::Profile, bot: &BotState) -> bool {
    if from >= to {
        return false;
    }
    let lines = climb_lines(set, skill, from, bot.quests_done, &[]);
    let per_coin = ticks_per_coin(d17_of(set, from, bot.quests_done));
    !rank_classed(&lines, profile, per_coin, &|row| class_for(row, bot)).is_empty()
}

/// Units of `item` the bot holds for its work, its tier family counted
/// (iron arrows meet `bronze_arrow`, like the obtain node's own
/// `family_held`: 2026-11-23, an obtain "done without launch" on iron
/// arrows while the pick found `bronze_arrow` missing, 507 loops in 34
/// min). Staffs are left out: `missing_inputs_with` already lets a staff
/// stand for its runes, and counting one here would size a stock batch
/// without bound.
pub(crate) fn family_units(item: &str, count_of: &dyn Fn(&str) -> i64) -> i64 {
    lifescape_planner::tools::tool_family(item).iter().filter(|name| !name.starts_with("staff_of_")).map(|name| count_of(name).max(0)).fold(0, i64::saturating_add)
}

/// Units one action of `method` at `level` burns of each stock input the
/// bot holds: the batch of a stock line (what its stock casts or makes).
fn stock_batch(row: &TrainingRow, level: u8, held: &dyn Fn(&str) -> i64, rate: RateReader) -> i64 {
    row.prereqs
        .stock
        .iter()
        .map(|(item, _)| held(item) / lifescape_planner::sizing::per_action_need_with(rate, &row.method, level, item).unwrap_or(1).max(1))
        .min()
        .unwrap_or(0)
        .max(0)
}

/// The pick of a climb for `life` read in `set` (module doc). `bot`: the
/// live judgement; `stock`: its own free stock; `seed`: the draw's
/// (`guide_pick::seed_for`); `sick`: methods that never launch now.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pick_from_guide(set: &GuideSet, climb: &Climb, life: &Life, bot: &BotState, stock: &[(String, f64)], seed: u64, sick: &dyn Fn(&str) -> bool, rate: RateReader) -> Result<GuidePick, String> {
    let lines: Vec<TrainingRow> = climb_lines(set, climb.skill, climb.level, bot.quests_done, stock).into_iter().filter(|row| !sick(&row.method)).collect();
    let best_profit = d17_of(set, climb.level, bot.quests_done);
    let food_short = |method: &str| climb.food.and_then(|food| food(method));
    let per_coin = Some(1.0 / climb.coin_rate.max(lifescape_planner::market_fit::MIN_COIN_RATE));
    let ranked: Vec<Classed> = rank_classed(&lines, &life.profile, per_coin, &|row| class_for(row, bot).map(|class| if food_short(&row.method).is_some() { class.max(1) } else { class }));
    // The previous pick comes back only while it stays within the band of
    // the best line feasible now (D10, 2026-11-20): a pick the score now
    // ranks far behind is drawn again (live: bronze bars smelted for
    // smithing 59 -> 66, kept by every re-resolve).
    // The best line of all (a class behind included, `drawn_class`): a
    // pick far behind a line that needs a needle first is drawn again.
    let best_now = ranked.iter().map(|line| line.score.score).min_by(f64::total_cmp);
    let again = climb.previous.and_then(|method| {
        ranked.iter().position(|line| line.class == 0 && line.row.method == method && best_now.is_some_and(|best| lifescape_planner::guide_pick::in_band(-line.score.score, -best)))
    });
    let (index, drawn, sticky) = match again {
        Some(index) => (index, lifescape_planner::guide_pick::Drawn { index: 0, band: 1, candidates: ranked.iter().filter(|line| line.class == 0).count(), explored: false }, true),
        None => {
            let Some((index, drawn)) = pick(&ranked, seed) else {
                return Err(format!("no {} line within reach at {} in the training guide ({} lines read)", climb.skill, climb.level, lines.len()));
            };
            (index, drawn, false)
        }
    };
    let chosen = &ranked[index];
    let row = chosen.row;
    let held = |item: &str| (bot.held)(item).max(0.0).floor() as i64;
    let batch = if row.supply == Supply::Stock { stock_batch(row, climb.level, &held, rate) } else { 0 };
    let mut missing = missing_inputs_with(rate, &row.method, climb.level, climb.target, &held);
    missing.extend(food_short(&row.method).filter(|food| !missing.contains(food)));
    let pick = TrainPick {
        method: row.method.clone(),
        batch,
        missing,
        // A market line buys its inputs at the tranche's volume (user
        // lever 2026-11-17, `replan::buys_by_market`).
        buys_inputs: row.supply == Supply::Market,
        cost_per_xp: row.coins_per_xp,
        xp_per_tick: row.effective_xp_6000 / lifescape_planner::guide_model::WINDOW_TICKS,
        profit_per_tick: row.coins_6000 / lifescape_planner::guide_model::WINDOW_TICKS,
    };
    let choice = Choice { kind: "training", method: row.method.clone(), score: chosen.score.score, class: chosen.class, drawn, sticky };
    Ok(GuidePick { pick, supply: row.supply, score: chosen.score.clone(), scored: ranked.len(), best_profit, choice })
}

/// Fills `ctx.training` for a climb about to resolve (module doc),
/// logging the pick with its numbers.
pub(crate) fn fill_ctx(ctx: &mut ReadyCtx, node: &Node, life: &Life, live: &Value, deps: &crate::brain_deps::BrainDeps) {
    if node.method.is_some() {
        return;
    }
    let (skill, target): (String, u8) = match &node.kind {
        NodeKind::ReachLevel { skill, level } => (skill.clone(), *level),
        // A combat level climbs one of its skills on the guide (R7
        // suite): the next tranche of it, the node re-resolves after.
        NodeKind::ReachCombat { .. } => {
            let static_method = node.alternatives.first().map(|alt| alt.method.as_str()).unwrap_or("");
            let skill = combat_climb_skill(static_method, &|name: &str| crate::brain_succession::live_level(live, name));
            let level = crate::brain_succession::live_level(live, skill);
            (skill.to_string(), level.saturating_add(COMBAT_TRANCHE_LEVELS).min(99))
        }
        _ => return,
    };
    let (skill, target) = (skill.as_str(), &target);
    let level = crate::brain_succession::live_level(live, skill);
    if level >= *target {
        return;
    }
    let Some(set) = deps.guides.clone() else {
        crate::logln!("brain {}: {} {skill} {level}->{target}: no training guide published yet, static tranche", life.account, node.id);
        return;
    };
    let locks = crate::brain_locks::locked_items_in(deps.market.as_deref(), &life.account);
    let free_of = |item: &str| crate::brain_locks::free_count(crate::brain_ambition::live_count(live, item), crate::brain_locks::locked_in(&locks, item));
    let held = |item: &str| family_units(item, &free_of) as f64;
    let levels = ctx.levels.clone();
    let level_of = |name: &str| levels.iter().find(|(known, _)| known == name).map(|(_, known)| *known).unwrap_or(1);
    let asks = |item: &str| crate::brain_trade::cycle_asks(&set, item);
    let quests = ctx.quests_done.clone();
    let bot = BotState { level_of: &level_of, combat: ctx.combat, quests_done: &quests, held: &held, coins: Some(ctx.coins.max(0) as f64), ironman: life.profile.ironman, asks: &asks };
    let stock = crate::brain_alch::own_stock(live, &life.account, &crate::brain_alch::kept(life, live), deps);
    let sick = |method: &str| ctx.sick(method).is_some() || node.excluded.iter().any(|excluded| excluded == method);
    let rate = crate::brain_plan_read::live_rate(deps);
    let food = |method: &str| lifescape_planner::food_gate::food_short(method, ctx);
    // A1 (a): a pending climb that opens a lucrative way makes a tick worth
    // that way's gain per tick (`investment::time_value`).
    let own_rate = crate::brain_d17::coin_rate(&life.account);
    let pending = crate::brain_value::pending_investments(skill, &level_of, deps);
    let coin_rate = lifescape_planner::investment::time_value(own_rate, &pending);
    if coin_rate > own_rate {
        let top = pending.iter().max_by(|left, right| left.value_6000.total_cmp(&right.value_6000)).map(|found| format!("{} (climb {:.0} ticks)", found.item, found.climb_ticks)).unwrap_or_default();
        crate::logln!("brain {}: {} {skill} {level}->{target}: a tick is worth {coin_rate:.2} coins (the climb opens {top}), not {own_rate:.2}", life.account, node.id);
    }
    let climb = Climb { skill, level, target: *target, previous: node.blocked_by.as_deref(), food: Some(&food), coin_rate };
    match pick_from_guide(&set, &climb, life, &bot, &stock, ctx.draw_seed, &sick, &rate) {
        Ok(found) => {
            crate::logln!(
                "brain {}: {} {skill} {level}->{target} trains by {} ({}, score {:.3} = {:.3} ticks/XP + {:.3} coin ticks/XP, thrift {:.2}, affinity {:.2}, {:.1} XP/tick, {:+.2} coins/tick, class {}, {} in band of {} drawn, {} lines in reach{}{}, guide {} s old{})",
                life.account,
                node.id,
                found.pick.method,
                found.supply.as_str(),
                found.score.score,
                found.score.own_ticks_per_xp,
                found.score.coin_ticks_per_xp,
                life.profile.thrift,
                life.profile.market_affinity,
                found.pick.xp_per_tick,
                found.pick.profit_per_tick,
                found.choice.class,
                found.choice.drawn.band,
                found.choice.drawn.candidates,
                found.scored,
                if found.choice.drawn.explored { ", exploration" } else { "" },
                if found.choice.sticky { ", previous pick again" } else { "" },
                set.age().as_secs(),
                if found.pick.missing.is_empty() { String::new() } else { format!(", obtains {}", found.pick.missing.join(",")) }
            );
            *ctx.drawn.borrow_mut() = Some(found.choice);
            ctx.training = Some(found.pick);
        }
        Err(error) => crate::logln!("brain {}: {} {skill} {level}->{target}: {error}, static tranche", life.account, node.id),
    }
}

#[cfg(test)]
#[path = "brain_train_tests.rs"]
mod tests;
