//! Offline CPU bench of the bots' choices from the guides (plan
//! rendements R7): a guide set built from the game priors (in-memory
//! store, empty snapshot: every view of a real cycle, ~the live size),
//! fabricated bots at every band, the cost of one decision before R7
//! (R6: the training read of a priced climb and its best score; the
//! static money ranking) and after (the draw of a climb of any skill, the
//! money lines read from the guide and drawn, the skeleton's oracle on a
//! whole goal). Ignored (it builds a full set, seconds):
//! `cargo test --release -p lifescape-orchestrator --lib brain_choice_bench -- --ignored --nocapture`

use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::guide_feasible::BotState;
use lifescape_planner::guide_model::GuideFilter;
use lifescape_planner::{Archetype, GoalKind, Life, Node, NodeKind, Profile};

use crate::cpu_profile::Probe;

fn life_at(seed: u64) -> Life {
    Life {
        schema: 1,
        account: format!("bench{seed}"),
        created_at: 0,
        seed,
        profile: Profile::new(false, (seed % 10) as f64 / 10.0, 0.5, seed, Archetype::Producer),
        goal: GoalKind::MaxSkills { skills: vec!["smithing".to_string()], level: 99 },
        nodes: Vec::new(),
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
        memory: Default::default(),
    }
}

/// CPU milliseconds per call of `work` over `rounds` calls (one untimed
/// warm-up: lazy indexes).
fn per_call<R>(label: &str, rounds: u32, mut work: impl FnMut() -> R) -> f64 {
    let _ = work();
    let probe = Probe::start();
    for _ in 0..rounds {
        let _ = work();
    }
    let (_, cpu) = probe.elapsed();
    let per = cpu as f64 / 1e6 / f64::from(rounds);
    println!("{label:<52} {per:>9.3} ms cpu/call");
    per
}

#[test]
#[ignore]
fn brain_choice_bench() {
    let store = lifescape_store::Store::open_in_memory().expect("store");
    let probe = Probe::start();
    let set = crate::guides_store::build_set(&store, &crate::rates_service::Snapshot::new(), &crate::web_guides::StoreWaits::default());
    println!("guide set built from the priors in {:.0} ms cpu", probe.elapsed().1 as f64 / 1e6);
    let levels = [5u8, 20, 35, 50, 65, 80, 95];
    let rate = |method: &str, level: u8| lifescape_planner::work_rate::prior_rate(method, level);
    let none = |_: &str| 0.0;
    let asks = |item: &str| crate::brain_trade::cycle_asks(&set, item);
    let never = |_: &str| false;
    let (mut before_train, mut after_train, mut before_money, mut after_money, mut oracle) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (index, level) in levels.iter().enumerate() {
        let life = life_at(index as u64 + 1);
        let level_of = |_: &str| *level;
        let bot = BotState { level_of: &level_of, combat: (*level).max(3), quests_done: &[], held: &none, coins: Some(5000.0), ironman: false, asks: &asks };
        println!("--- bot at {level} in every skill");
        // Before (R6): the training read of a priced climb, its best score.
        before_train += per_call("R6 smithing climb: read + rank + best", 20, || {
            let filter = GuideFilter { skill: Some("smithing".to_string()), level: Some(*level), ..GuideFilter::default() };
            let (rows, _) = set.training(&filter);
            let profit = lifescape_planner::guide_read::best_profit_in(set.money_view(Some(*level)), &GuideFilter { level: Some(*level), ..GuideFilter::default() });
            let reach = |row: &lifescape_planner::guide_model::TrainingRow| lifescape_planner::guide_feasible::feasible_for(row, &bot).within_reach();
            let ranked = lifescape_planner::train_score::rank_rows(&rows, &life.profile, lifescape_planner::train_score::ticks_per_coin(profit), &reach);
            lifescape_planner::train_score::best(&ranked).map(|row| row.method.clone())
        });
        // After (R7): the draw of a climb, any skill.
        for skill in ["smithing", "woodcutting", "attack", "magic"] {
            let climb = crate::brain_train::Climb { skill, level: *level, target: level.saturating_add(5).min(99), previous: None, food: None };
            let cost = per_call(&format!("R7 {skill} climb: pick_from_guide (draw)"), 20, || crate::brain_train::pick_from_guide(&set, &climb, &life, &bot, &[], 7, &never, &rate).map(|found| found.pick.method));
            if skill == "smithing" {
                after_train += cost;
            }
        }
        // Money: the static ranking before R7, the guide's lines and draw after.
        let mut ctx = ReadyCtx::blind(life.profile.market_affinity);
        ctx.levels = lifescape_planner::guide_methods::SKILLS.iter().map(|skill| (skill.to_string(), *level)).collect();
        ctx.combat = (*level).max(3);
        before_money += per_call("before R7 money: static ranking (resolve_ready)", 20, || {
            let mut node = Node::new("n1", NodeKind::MakeMoney, Vec::new(), 0);
            lifescape_planner::resolve_ready(&mut node, &ctx);
            node.method
        });
        per_call("  of which bot_rows", 20, || crate::brain_money::bot_rows(&set, &level_of, &[]).rows.len());
        let read = crate::brain_money::bot_rows(&set, &level_of, &[]);
        let rows = read.all();
        println!("  {} rows read, {} priced and earning", rows.len(), rows.iter().filter(|row| row.priced && row.profit_6000 > 0.0).count());
        per_call("  of which lines_for", 20, || crate::brain_money::lines_for(&rows, &ctx, &bot, &[], 0.5).len());
        after_money += per_call("R7 money: bot_rows + lines_for + draw", 20, || {
            let read = crate::brain_money::bot_rows(&set, &level_of, &[]);
            let rows = read.all();
            let mut drawn = ctx.clone();
            drawn.money_guide = crate::brain_money::lines_for(&rows, &drawn, &bot, &[], life.profile.market_affinity);
            drawn.draw_seed = 7;
            let mut node = Node::new("n1", NodeKind::MakeMoney, Vec::new(), 0);
            lifescape_planner::resolve_ready(&mut node, &drawn);
            node.method
        });
        oracle += per_call("R7 skeleton oracle: guide_covers, 15 skills x 1 tranche", 5, || {
            lifescape_planner::guide_methods::SKILLS.iter().filter(|skill| crate::brain_train::guide_covers(&set, skill, *level, level.saturating_add(5).min(99), &life.profile, &bot)).count()
        });
    }
    let count = levels.len() as f64;
    println!("=== mean per decision over {} bots", levels.len());
    println!("training: R6 {:.3} ms -> R7 {:.3} ms", before_train / count, after_train / count);
    println!("money:    before {:.3} ms -> R7 {:.3} ms", before_money / count, after_money / count);
    println!("skeleton oracle (15 tranches): {:.3} ms", oracle / count);
}
