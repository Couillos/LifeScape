#![forbid(unsafe_code)]
//! Food gate of the fights, asked by the launch pass before a worker
//! starts (the twin of `brain_input_gate` for production inputs). The
//! melee scripts stop `blocked` on food a biting spot needs, but only
//! after a worker was spawned and an error row written; the brain knows
//! the holdings (`food_gate::food_short`: pack, bank, worn), so a foodless
//! fight files its food sub-task at once, no launch.
//! Live 2026-11-22: wolfe_carl, 34 sibling `farm:uncut_sapphire` nodes
//! launched one after the other, each blocked 11 ticks on swordfish: 136
//! `melee blocked` errors in 50 minutes, half of the fleet's errors.

use lifescape_json::Value;
use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::Life;

use crate::jobs::Job;

/// The food `method` lacks before its first action (empty: not a fight,
/// fed, a harmless spot, bank or levels unknown).
pub(crate) fn foodless_need(method: &str, ctx: &ReadyCtx) -> Vec<String> {
    lifescape_planner::food_gate::food_short(method, ctx).into_iter().collect()
}

/// A fight the bot cannot feed files its food obtain (retargeted to what
/// it buys or makes, full load) and the node waits on it: true when the
/// node was blocked here (the caller saves and ends the pass).
pub(crate) fn block_if_foodless(job: &Job, life: &mut Life, launch: usize, method: &str, ctx: &ReadyCtx, live: &Value) -> bool {
    let needed = foodless_need(method, ctx);
    if needed.is_empty() {
        return false;
    }
    let missing = crate::brain_food::afford_food_needs(job, method, needed);
    let id = life.nodes[launch].id.clone();
    let level_of = |skill: &str| crate::brain_succession::live_level(live, skill);
    let view = crate::brain_plan_read::BotView::of(life);
    let fold = crate::brain_plan_read::with_plan_read(&view, live, |read| lifescape_planner::record_wait_block(life, launch, &missing, &level_of, read));
    crate::brain::push_history(life, &id, crate::brain_launch::block_outcome(&fold));
    if let lifescape_planner::FailureFold::CycleBroken(line) = &fold {
        crate::logln!("brain {}: {line}", job.account);
    }
    let status = life.nodes.iter().find(|node| node.id == id).map_or_else(|| "dropped".to_string(), |node| node.status.as_str());
    crate::logln!("brain {}: {id} foodless before launch ({}) -> {status}", job.account, missing.join(", "));
    lifescape_planner::normalize(life);
    true
}

#[cfg(test)]
mod tests {
    use super::foodless_need;
    use lifescape_planner::expand::ReadyCtx;

    fn fighter(tools: &[&str]) -> ReadyCtx {
        let mut ctx = ReadyCtx::blind(0.5);
        ctx.bank_known = true;
        ctx.levels = vec![("hitpoints".to_string(), 10)];
        ctx.tools = tools.iter().map(|name| name.to_string()).collect();
        ctx
    }

    /// A 10-HP bot at the men with no food names the fish to obtain; fed,
    /// on a harmless spot, or off a fight it names nothing.
    #[test]
    fn a_foodless_fight_names_its_food_before_launch() {
        assert_eq!(foodless_need("fight:melee/men", &fighter(&[])), vec!["herring".to_string()]);
        assert!(foodless_need("fight:melee/men", &fighter(&["shrimp"])).is_empty(), "fed");
        assert!(foodless_need("fight:melee/chickens", &fighter(&[])).is_empty(), "harmless spot");
        assert!(foodless_need("smelt:iron_bar", &fighter(&[])).is_empty(), "not a fight");
        let mut unseen = fighter(&[]);
        unseen.bank_known = false;
        assert!(foodless_need("fight:melee/men", &unseen).is_empty(), "bank unseen: no judgment");
    }
}
