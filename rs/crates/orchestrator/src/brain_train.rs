#![forbid(unsafe_code)]
//! Training method of a priced climb, brain side (plan rendements R6,
//! D13, D17): smithing, magic from confuse and the consuming skills
//! (`training::priced_at_resolve`) pick their method from the TRAINING
//! GUIDE the guides store precomputed (`guides_store::current()`: the
//! shared `Arc` of the last cycle, never a build on demand):
//! - the bot's lines: the guide read at its live level, its quests done
//!   (`GuideSet::training`, filters applied by reading), its skill, only
//!   the lines a level climb launches (`mapping::map_method`; quest
//!   shortcuts are quest nodes, never a tranche method);
//! - each line scored for THIS bot (`train_score`): its ticks per XP plus
//!   its coins per XP converted at the bot's D17 price (best accessible
//!   money line of the same bot, read in the money view), the coins
//!   weighed by the thrift trait (rapide ↔ économe), a market line by
//!   market_affinity like `choose_market_or_farm`;
//! - the best score kept (`train_score::choose`: R7's weighted draw takes
//!   that place), its consumables the bot lacks for one action obtained
//!   first (`training::missing_inputs`), handed to the resolver
//!   (`ReadyCtx::training`).
//!
//! No guide published yet (the first cycle after a start) or no runnable
//! line: no pick, logged; the static tranche method runs (as without a
//! market before). Called once per launch of such a climb, right before
//! `resolve_ready`.

use lifescape_json::Value;
use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::guide_model::{GuideFilter, Supply};
use lifescape_planner::train_score::{choose, rank_rows, ticks_per_coin, LineScore};
use lifescape_planner::training::{missing_inputs, priced_at_resolve, TrainPick};
use lifescape_planner::{Life, Node, NodeKind};

use crate::guides_store::GuideSet;

/// The line a bot trains with and why.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GuidePick {
    pub pick: TrainPick,
    pub supply: Supply,
    pub score: LineScore,
    /// Runnable lines scored.
    pub scored: usize,
    /// The bot's D17 price (best accessible money line per 6000).
    pub best_profit: Option<f64>,
}

/// True for a guide line a `skill` level climb can launch.
fn launchable(method: &str, skill: &str) -> bool {
    if method.starts_with("quest:") {
        return false;
    }
    let probe = Node::new("probe", NodeKind::ReachLevel { skill: skill.to_string(), level: 99 }, Vec::new(), 0);
    lifescape_planner::map_method(method, &probe, None).is_some()
}

/// The pick of a `skill` climb from `level` to `target` for `life`, read
/// in `set` (module doc). `held`: free units the bot holds.
pub(crate) fn pick_from_guide(set: &GuideSet, skill: &str, level: u8, target: u8, life: &Life, quests_done: &[String], held: &dyn Fn(&str) -> i64) -> Result<GuidePick, String> {
    let filter = GuideFilter { skill: Some(skill.to_string()), level: Some(level), quests_done: quests_done.to_vec(), ..GuideFilter::default() };
    let (rows, _) = set.training(&filter);
    let lines: Vec<_> = rows.into_iter().filter(|row| row.skill == skill && launchable(&row.method, skill)).collect();
    let bot = GuideFilter { level: Some(level), quests_done: quests_done.to_vec(), ..GuideFilter::default() };
    let best_profit = lifescape_planner::guide_read::best_profit_in(set.money_view(Some(level)), &bot);
    let reach = |row: &lifescape_planner::guide_model::TrainingRow| lifescape_planner::guide_feasible::with_filter_bot(&bot, |judge| lifescape_planner::guide_feasible::feasible_for(row, judge).within_reach());
    let ranked = rank_rows(&lines, &life.profile, ticks_per_coin(best_profit), &reach);
    let Some(row) = choose(&ranked) else {
        return Err(format!("no runnable {skill} line at {level} in the training guide ({} lines read)", lines.len()));
    };
    let score = ranked.iter().find(|(_, line)| std::ptr::eq(*line, row)).map(|(score, _)| score.clone()).ok_or("chosen line lost its score")?;
    let pick = TrainPick {
        method: row.method.clone(),
        batch: 0,
        missing: missing_inputs(&row.method, level, target, held),
        cost_per_xp: row.coins_per_xp,
        xp_per_tick: row.effective_xp_6000 / lifescape_planner::guide_model::WINDOW_TICKS,
        profit_per_tick: row.coins_6000 / lifescape_planner::guide_model::WINDOW_TICKS,
    };
    Ok(GuidePick { pick, supply: row.supply, score, scored: ranked.len(), best_profit })
}

/// Fills `ctx.training` for a priced climb about to resolve (module doc),
/// logging the pick with its numbers.
pub(crate) fn fill_ctx(ctx: &mut ReadyCtx, node: &Node, life: &Life, live: &Value) {
    let NodeKind::ReachLevel { skill, level: target } = &node.kind else { return };
    if node.method.is_some() {
        return;
    }
    let level = crate::brain_succession::live_level(live, skill);
    if !priced_at_resolve(skill, level) || level >= *target {
        return;
    }
    let Some(set) = crate::guides_store::current() else {
        crate::logln!("brain {}: {} {skill} {level}->{target}: no training guide published yet, static tranche", life.account, node.id);
        return;
    };
    let locks = crate::brain_locks::locked_items(&life.account);
    let held = |item: &str| crate::brain_locks::free_count(crate::brain_ambition::live_count(live, item), crate::brain_locks::locked_in(&locks, item));
    match pick_from_guide(&set, skill, level, *target, life, &ctx.quests_done, &held) {
        Ok(found) => {
            crate::logln!(
                "brain {}: {} {skill} {level}->{target} trains by {} ({}, score {:.3} = {:.3} ticks/XP + {:.3} coin ticks/XP, thrift {:.2}, affinity {:.2}, {:.1} XP/tick, {:+.2} coins/tick, {} lines, guide {} s old{})",
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
                found.scored,
                set.age().as_secs(),
                if found.pick.missing.is_empty() { String::new() } else { format!(", obtains {}", found.pick.missing.join(",")) }
            );
            ctx.training = Some(found.pick);
        }
        Err(error) => crate::logln!("brain {}: {} {skill} {level}->{target}: {error}, static tranche", life.account, node.id),
    }
}

#[cfg(test)]
#[path = "brain_train_tests.rs"]
mod tests;
