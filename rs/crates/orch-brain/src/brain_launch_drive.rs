#![forbid(unsafe_code)]
//! Node and dispatch phase of the launch pass (lot 3 of the launch
//! refactor): decide -> effects -> finish. The decision itself is the
//! planner's (`launch_decide`), the life changes too (`launch_apply`);
//! this loop gathers what a decision asks for (fresh market counts,
//! listings withdrawn, food, loot values, the cleanup keep sources),
//! performs the effects the decision emits in order, and ends with the
//! save the outcome asks for. Generic over `Fx`: the real effects in
//! production, a recording fake in the equivalence tests.

use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::launch_apply::{apply_pre, apply_to_life, settle_market_buy, ApplyEnv};
use lifescape_planner::launch_decide::{decide, Decide, Decided, Emit, GateRelease, Known, LaunchDecision, LaunchSave, Need};
use lifescape_planner::{LaunchInputs, Life};

use crate::brain_launch_fx::Fx;
use crate::brain_launch_pass::Save;

/// What the drive works on: the life, the typed inputs (re-read when a
/// decision asks), the resolve context, the node and its committed coins.
pub(crate) struct Drive<'a> {
    pub life: &'a mut Life,
    pub inputs: &'a mut LaunchInputs,
    pub ctx: &'a mut ReadyCtx,
    pub position: usize,
    pub committed: i64,
}

fn save_of(save: LaunchSave) -> Save {
    match save {
        LaunchSave::None => Save::None,
        LaunchSave::Always => Save::Always,
        LaunchSave::Silent => Save::Silent,
    }
}

/// Performs one emitted effect (or log), in the place the decision put it.
fn run_emit<F: Fx>(fx: &mut F, life: &Life, position: usize, ctx: &ReadyCtx, decided: &Decided, emit: &Emit) {
    match emit {
        Emit::Log(line) => fx.log(line.clone()),
        Emit::CancelOpenBuys(item) => fx.cancel_open_buys(&life.account, item),
        Emit::NoteGearWithdrawn(item) => fx.note_gear_withdrawn(&life.account, item),
        Emit::ClaimLot(lot) => fx.claim_lot(life, lot),
        Emit::ReleaseRun(node) => fx.release_run(&life.account, node),
        Emit::CrowdNote { site, method } => fx.crowd_note(*site, method),
        Emit::StandingBid => {
            if let Some(plan) = &decided.plan {
                fx.standing_bid(life, position, &plan.snapshot, ctx);
            }
        }
    }
}

/// Decides, performs, applies; returns how the pass saves the life (a
/// launch saves itself before the worker call and returns `Save::None`).
pub(crate) fn drive<F: Fx>(fx: &mut F, work: Drive<'_>) -> Save {
    let Drive { life, inputs, ctx, position, committed } = work;
    let rules = fx.rules();
    let mut known = Known::default();
    let mut printed = 0usize;
    // A decision asks at most a handful of times (fresh counts, listings,
    // food, loot, keep sources); more is a bug, never an endless pass.
    for round in 0..16 {
        // The old pass read the clock at each use: a fresh read per round.
        fx.refresh_clock(inputs);
        let decided = {
            let timed = |label: &str, body: &mut dyn FnMut()| fx.profile(label, body);
            decide(&Decide { life: &*life, position, ctx: &*ctx, committed, inputs: &*inputs, rules: &rules, known: &known, timed: &timed })
        };
        // The gear step's coin share rides on the context the effects read.
        if let Some(share) = decided.coin_share {
            ctx.coin_share_pct = share;
        }
        // The resolve prelude and the dispatch plan are in the life before
        // any effect that reads it (the standing bid, the keep sources).
        apply_pre(life, &decided);
        for emit in &decided.emits[printed.min(decided.emits.len())..] {
            run_emit(fx, life, position, ctx, &decided, emit);
        }
        printed = decided.emits.len();
        if decided.asks() && round < 15 {
            known.resumed = decided.boundary.clone();
            answer(fx, life, inputs, &decided, &mut known);
            continue;
        }
        assert!(!decided.asks(), "the launch decision keeps asking: {:?}", decided.decision);
        return finish(fx, life, inputs, ctx, decided);
    }
    Save::None
}

/// Reads what the decision asked for and records the answer.
fn answer<F: Fx>(fx: &mut F, life: &Life, inputs: &mut LaunchInputs, decided: &Decided, known: &mut Known) {
    let method = decided.boundary.as_ref().and_then(|boundary| boundary.node.method.clone()).unwrap_or_default();
    match &decided.decision {
        LaunchDecision::Need(Need::Recount) => {
            fx.refresh_market(inputs);
            known.recounted = true;
        }
        LaunchDecision::Need(Need::Gate) => {
            fx.refresh_market(inputs);
            known.gate_fresh = true;
        }
        LaunchDecision::Need(Need::Food(needed)) => known.food = Some(fx.afford_food(&method, needed.clone())),
        LaunchDecision::Need(Need::Loot(spot)) => known.loot = Some(fx.loot_facts(spot)),
        LaunchDecision::Need(Need::Cleanup) => {
            if let Some(plan) = &decided.plan {
                known.cleanup = Some(fx.cleanup_facts(life, inputs, &plan.snapshot));
            }
        }
        LaunchDecision::WithdrawOwnListings { items } => {
            let withdrawn = fx.release_listings(items);
            // lot 3: `release_own_locks` just withdrew listings and cancelled
            // locks; the free count is read again from the market.
            fx.refresh_market(inputs);
            known.released = Some(withdrawn);
        }
        LaunchDecision::WithdrawInputListings { items } => {
            let withdrawn = fx.release_listings(items);
            fx.refresh_market(inputs);
            known.gate_released = Some(GateRelease { withdrawn, items: items.clone() });
        }
        _ => {}
    }
}

/// The outcome: its life changes and effects, then the save.
fn finish<F: Fx>(fx: &mut F, life: &mut Life, inputs: &LaunchInputs, ctx: &ReadyCtx, decided: Decided) -> Save {
    let position = decided.position;
    let now = inputs.now;
    match &decided.decision {
        LaunchDecision::PlaceMarketBuy { held } => {
            let Some(plan) = &decided.plan else { return Save::Always };
            let placed = fx.market_buy(life, position, &plan.snapshot, ctx);
            settle_market_buy(life, position, *held, placed, now);
            Save::Always
        }
        LaunchDecision::RunScript { method, script, args } => {
            let Some(plan) = &decided.plan else { return Save::Always };
            fx.claim_launch(&plan.snapshot, method, plan.budget);
            // The planner's fine method rides to the task report (session-only
            // flag, stripped before the script parses, `report_meta`).
            let mut args = args.clone();
            args.push(crate::report_meta::METHOD_FLAG.to_string());
            args.push(method.clone());
            fx.log(format!("brain {}: launching {script} {}", life.account, args.join(" ")));
            // Queued, not run: the launcher thread owns the blocking worker RPC.
            // Mark Active now so the next pass does not double-queue; a refused
            // launch re-queues via crash recovery (no report ever arrives).
            let env = ApplyEnv { inputs, quests_done: &ctx.quests_done, reader: fx.plan_source() };
            let _ = apply_to_life(life, &decided, &env);
            let launched_id = life.nodes[position].id.clone();
            fx.note_launch();
            // Saved before the blocking RPC: a report racing the reply must find
            // the node active.
            fx.save(life, Save::Always);
            fx.run_job(script, &args, &launched_id);
            Save::None
        }
        _ => {
            let env = ApplyEnv { inputs, quests_done: &ctx.quests_done, reader: fx.plan_source() };
            let emits = apply_to_life(life, &decided, &env);
            for emit in &emits {
                run_emit(fx, life, position, ctx, &decided, emit);
            }
            save_of(decided.save())
        }
    }
}
