#![forbid(unsafe_code)]
//! Node pick of the launch pass (lot 2 of the launch refactor): pick the
//! next ready node and build its resolve context. The pre-launch checks
//! and the dispatch are the planner's decision (`launch_decide`), driven
//! by `brain_launch_drive` (lot 3).


use crate::brain::push_history;
use crate::brain_launch_pass::{Flow, LaunchPass, Picked, Save};

/// The gear sub-goal first, then the goal's next node; the tutorial
/// only ever starts through its gate. Parked market waits are skipped:
/// with nothing else ready, a short money window fills the wait.
/// `Err`: no node to launch, the pass ends with that save.
pub(crate) fn pick_next_ready(pass: &mut LaunchPass) -> Result<usize, Save> {
    let (orch, job) = (pass.orch, pass.job);
    pass.dirty |= crate::brain_plan_read::resize_session_inputs(&mut pass.life, &pass.live);
    let waiting = &pass.waiting;
    let Some(position) = crate::cpu_profile::timed("next_ready", || lifescape_planner::next_ready_except(&pass.life, waiting)) else {
        // The hybrid (2026-11-20): an ambition goal waiting on a buy past
        // one filler window sets itself aside for a market answer of any
        // trade (`brain_wait_answer`); else the filler.
        if !waiting.is_empty() && crate::cpu_profile::timed("wait_answer", || crate::brain_wait_answer::answer_while_waiting(&mut pass.life, &pass.live)) {
            return Err(Save::Always);
        }
        if !waiting.is_empty() && crate::cpu_profile::timed("filler.launch", || crate::brain_filler::launch_filler(orch, job, &mut pass.life, &pass.live, waiting)) {
            return Err(Save::None);
        }
        return Err(Save::IfDirty);
    };
    Ok(position)
}

/// A climb the live levels already meet closes without a launch (live
/// 2026-11-13: b8jxrdd2 ran "attack 10" at attack 45 for 18 015 ticks).
pub(crate) fn already_reached(pass: &mut LaunchPass, position: usize) -> Flow {
    let job = pass.job;
    let live = &pass.live;
    let level_of = |skill: &str| crate::brain_succession::live_level(live, skill);
    if let Some(reached) = lifescape_planner::reached_live(&pass.life.nodes[position], &level_of, pass.inputs.combat) {
        let id = pass.life.nodes[position].id.clone();
        crate::logln!("brain {}: {id} {:?} already reached (live {reached}), done without launch", job.account, pass.life.nodes[position].kind);
        lifescape_planner::complete_node(&mut pass.life, position, i64::from(reached));
        push_history(&mut pass.life, &id, "already_reached");
        lifescape_planner::normalize(&mut pass.life);
        return Flow::Stop(Save::Always);
    }
    Flow::Continue
}

/// Quests the bot finished (engine save + done quest nodes): a quest
/// node already complete closes, a method behind a missing quest never
/// launches (the resolver plans the quest first).
pub(crate) fn quest_done(pass: &mut LaunchPass, position: usize) -> Result<Vec<String>, Save> {
    let quests_done = crate::cpu_profile::timed("rescue.quests_done", || crate::brain_rescue::quests_done(&pass.life));
    if crate::brain_rescue::close_done_quest(&mut pass.life, position, &quests_done) {
        return Err(Save::Always);
    }
    Ok(quests_done)
}

/// The resolve context of the node: the planner's blind context, then
/// everything the brain knows about the bot (wallet, tools, site, fleet,
/// market and money guides).
pub(crate) fn build_ctx(pass: &mut LaunchPass, position: usize, quests_done: Vec<String>) -> Picked {
    let job = pass.job;
    let ctx_phase = crate::cpu_profile::phase("ctx.build");
    let mut ctx = {
        let node = pass.life.nodes[position].clone();
        crate::brain_plan::ready_ctx_in(&pass.life, &node, &pass.live, &pass.deps)
    };
    ctx.quests_done = quests_done;
    // The seed of this resolve's draw from a guide (R7): the bot, the
    // node, its draw counter.
    ctx.draw_seed = lifescape_planner::guide_pick::seed_for(pass.life.profile.seed, &pass.life.nodes[position].id, pass.life.memory.draws);
    // lot 3: the side market passes above listed, bid and withdrew: the
    // locks, listings, committed coins, the pack and the fleet's sites
    // are read here, where the wallet and the holdings are decided.
    crate::brain_launch_inputs::gather_market(job, &mut pass.inputs, &pass.deps);
    let (life, live, inputs) = (&pass.life, &pass.live, &pass.inputs);
    // Own wallet gates the market leg, a purchase bids on the free wallet,
    // the whole family counts (`launch_inputs::node_counts`). Carried tools
    // gate the gather legs the same way (t04: rod-less sardines never
    // dispatch).
    let counts = lifescape_planner::launch_inputs::node_counts(life, position, inputs);
    ctx.coins = counts.coins;
    if let Some(held) = counts.held {
        ctx.held = held;
    }
    let committed = counts.committed;
    ctx.tools = inputs.carried_tools();
    ctx.bank_known = inputs.bank_streamed;
    // The brain picks the site: the bot's combat level and position.
    ctx.combat = inputs.combat;
    ctx.position = inputs.ctx_position;
    // The fleet's sites: a crowded rock field reads farther.
    ctx.crowd = inputs.crowd.clone();
    ctx.seats = inputs.seats.clone();
    crate::cpu_profile::timed("market.fill_ctx", || crate::brain_market::fill_ctx_in(&mut ctx, &life.nodes[position], live, &pass.deps));
    // Alchemy of its own stock is a money method too (2026-11-14).
    crate::cpu_profile::timed("alch.publish", || crate::brain_alch::publish(&mut ctx, &life.nodes[position], life, live));
    // 100 % market lots read from the precomputed money guide (R5).
    crate::cpu_profile::timed("trade.publish", || crate::brain_trade::publish(&mut ctx, &life.nodes[position], life, live, job));
    // The money guide's lines the bot may take (R7), its trade offers
    // mapped to their lots.
    crate::cpu_profile::timed("money.publish", || crate::brain_money::publish(&mut ctx, &life.nodes[position], life, live, job));
    // Smithing and magic (from 21) climbs pick their method with the
    // market: what to hammer or smelt, alchemy or a combat spell.
    crate::cpu_profile::timed("train.fill_ctx", || crate::brain_train::fill_ctx(&mut ctx, &life.nodes[position], life, live));
    drop(ctx_phase);
    Picked { position, ctx, committed }
}
