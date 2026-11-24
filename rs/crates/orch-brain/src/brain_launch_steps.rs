#![forbid(unsafe_code)]
//! Pass-level steps of the launch pass (lot 2 of the launch refactor),
//! in the order `maybe_launch` runs them: guards, life upkeep, bank
//! space and NPC sale, the running node, the tutorial gate, the side
//! market passes, the graph state. Each step keeps the dated rule it
//! carries; a stopping step names how the life is saved (`Save`).

use std::sync::Arc;

use lifescape_planner::NodeKind;

use crate::brain::{load_life, maybe_insert_cleanup, push_history};
use crate::brain_launch::{fail_no_script, note_launch, refused_launch, seconds_since_history};
use crate::brain_launch_pass::{Flow, LaunchPass, Save};
use crate::brain_upkeep_drive as upkeep;
use crate::brain_upkeep_fx::LiveUpkeepFx;
use crate::brain_tutorial::{tutorial_gate, TutorialGate};
use crate::fleet::Fleet;
use crate::jobs::Job;

/// Guards: an idle worker, a life on disk, a live snapshot, a bot that
/// is not stranded. `None`: the pass ends here, nothing saved.
pub(crate) fn open<'a>(orch: &'a Arc<dyn Fleet>, job: &'a Arc<Job>) -> Option<LaunchPass<'a>> {
    if !crate::brain_launch::job_idle(job) {
        return None;
    }
    let life = load_life(&job.account)?;
    // Never decide on a missing snapshot (bt7ktb8c: an empty live read as
    // "not on the island" launched fishing there). A contended lock is
    // the same: next pass.
    let live = match job.inner.try_lock() {
        Ok(inner) => inner.live_shared(),
        Err(_) => return None,
    };
    let live = live?;
    let deps = crate::brain_deps::BrainDeps::live();
    let inputs = crate::brain_launch_inputs::gather(job, &live, &deps);
    // A bot no route leaves (P2-1a): no launch while it stands there,
    // its orders retired once; the operator decides (rule 7, D5).
    if let Some((tile, first)) = crate::brain_stranded::gate(&job.account, inputs.position) {
        if first {
            crate::logln!("{}", crate::brain_stranded::verdict_line(&job.account, tile));
            if let Some(service) = crate::market_service::shared() {
                let (failed, cancelled) = service.retire_account(&job.account, crate::brain_stranded::STRANDED_REASON);
                crate::logln!("market: {} stranded, deals {failed:?} failed no_show, {cancelled} order(s) retired", job.account);
            }
        }
        return None;
    }
    Some(LaunchPass { orch, job, life, live, inputs, deps, dirty: false, quoting: false, waiting: Vec::new() })
}

/// Life upkeep, first half: the operator's switch and the market maker
/// (`brain_upkeep_drive::operator_and_maker`, decisions in the planner's
/// `upkeep_role`).
pub(crate) fn operator_and_maker(pass: &mut LaunchPass) -> Flow {
    let fx = LiveUpkeepFx::new(pass.orch, pass.job, &pass.live, pass.deps.clone());
    // A quoting maker has no ambition pass, no gear, tool or wish bid, no
    // alchemy: its post, its quotes, its deals, its bank (Q6).
    pass.quoting = upkeep::operator_and_maker(&fx, &mut pass.life, &mut pass.dirty);
    Flow::Continue
}

/// Life upkeep, second half: ambition (ensure, coverage, review,
/// rebalance, catalogue sync, stale goal), bid releases, unpaying
/// releases, stale goal work, re-engage after a restart
/// (`brain_upkeep_drive::ambition_upkeep`).
pub(crate) fn ambition_upkeep(pass: &mut LaunchPass) -> Flow {
    let fx = LiveUpkeepFx::new(pass.orch, pass.job, &pass.live, pass.deps.clone());
    upkeep::ambition_upkeep(&fx, &mut pass.life, &mut pass.dirty, pass.quoting);
    Flow::Continue
}

/// Bank space (L8), the pending NPC shop sale (Q3), the invariants.
pub(crate) fn bank_and_normalize(pass: &mut LaunchPass) -> Flow {
    let job = pass.job;
    // Space pressure (L8) precedes the terminal check: a full bank needs
    // its janitor even when the graph is done.
    let bank_types = pass.inputs.bank_types;
    if crate::cpu_profile::timed("cleanup_check", || maybe_insert_cleanup(&mut pass.life, bank_types)) {
        let inserted = pass.life.nodes.last().map(|node| node.id.clone()).unwrap_or_default();
        crate::logln!("brain {}: bank past {} types, inserting {}", job.account, lifescape_planner::BANK_TYPE_LIMIT, inserted);
        pass.dirty = true;
    }
    // Q3: a pending NPC shop sale rides the janitor node (`brain_npc_sale`).
    let now_secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0);
    if crate::brain_npc_sale::maybe_insert(&mut pass.life, now_secs) {
        let inserted = pass.life.nodes.last().map(|node| node.id.clone()).unwrap_or_default();
        crate::logln!("brain {}: npc shop sale pending, inserting {}", job.account, inserted);
        pass.dirty = true;
    }
    for change in crate::cpu_profile::timed("normalize", || lifescape_planner::normalize(&mut pass.life)) {
        crate::logln!("brain {}: normalize {change}", job.account);
        pass.dirty = true;
    }
    Flow::Continue
}

/// The running node with an idle worker: either a market order
/// awaiting its fill (bounded), or a run whose report never came
/// (worker crash, daemon restart): counted as a failure, so a script
/// that kills its worker cannot relaunch forever.
pub(crate) fn running_node(pass: &mut LaunchPass) -> Flow {
    let job = pass.job;
    let Some(active_id) = pass.life.active.clone() else { return Flow::Continue };
    // lot 3: the grace mark and the report flags are other threads' state
    // (the folding inbox), read where the idle detector reads them.
    crate::brain_launch_inputs::refresh_launch_mark(job, &mut pass.inputs);
    if pass.inputs.recent_launch {
        return Flow::Stop(Save::IfDirty);
    }
    let Some(position) = pass.life.nodes.iter().position(|node| node.id == active_id) else { return Flow::Continue };
    let awaiting = seconds_since_history(&pass.life, &active_id, "awaiting_fill");
    match (&pass.life.nodes[position].kind.clone(), awaiting) {
        // A wait locked by an older daemon (or a worker-side buy)
        // parks: the bid rests, the worker works on (no idle bot).
        (NodeKind::ObtainItems { item, count }, Some(waited)) => {
            if crate::brain_await::settle_fill(job, &mut pass.life, position, item, *count, waited) {
                crate::logln!("brain {}: {active_id} parked on the book, the worker works on", job.account);
                // Its last entry stays `awaiting_fill`: the deadline
                // keeps counting from the placement.
                lifescape_planner::park_for_fill(&mut pass.life, position);
            }
        }
        // Right after a daemon restart the restored workers read
        // idle before their scripts report: an operator restart,
        // never a crash (live 2026-11-12: every restart cost each
        // active node one failure). Same for an operator `unload`,
        // `close` or `open` (redeploy). Retry uncounted, like `stopped`.
        // A report received and not folded yet (inbox, or kept
        // pending on a full inbox) is the run's verdict (P10): wait.
        _ if {
            // lot 3: read only when this arm is reached (after the fill wait).
            crate::brain_launch_inputs::refresh_report_flags(job, &mut pass.inputs);
            !crate::brain_inflight::missing_report(pass.inputs.report_in_flight, pass.inputs.report_pending)
        } =>
        {
            crate::logln!("brain {}: {active_id} idle with its report in flight, waiting for the fold", job.account);
            return Flow::Stop(Save::IfDirty);
        }
        // The stop mark is TAKEN here, never peeked (`take_operator_stop`,
        // `first_idle_since_start` consume): the calls stay where they were.
        _ if crate::brain_grace::take_operator_stop(&job.account) || crate::brain_grace::first_idle_since_start(&job.account) => {
            crate::logln!("brain {}: {active_id} idle after a daemon restart or an operator stop, retrying uncounted", job.account);
            crate::brain_inflight::fold_stopped(&mut pass.life, position);
        }
        _ => {
            let text = "no report: worker went idle without reporting (crash or restart)";
            crate::logln!("brain {}: {active_id} {text}", job.account);
            crate::brain_inflight::fold_lost_report(&mut pass.life, position, text);
        }
    }
    lifescape_planner::normalize(&mut pass.life);
    pass.dirty = true;
    Flow::Continue
}

/// Tutorial gate: on the island only the tutorial runs; no position
/// yet means no launch at all.
pub(crate) fn tutorial(pass: &mut LaunchPass) -> Flow {
    let (orch, job) = (pass.orch, pass.job);
    match crate::cpu_profile::timed("tutorial_gate", || tutorial_gate(&mut pass.life, &pass.live)) {
        TutorialGate::Launch(at) => {
            if !crate::registry::script_known("tutorial") {
                fail_no_script(&mut pass.life, at, "tutorial");
                return Flow::Stop(Save::Always);
            }
            let id = pass.life.nodes[at].id.clone();
            crate::logln!("brain {}: launching tutorial --node {id}", job.account);
            lifescape_planner::mark_launched(&mut pass.life, at);
            push_history(&mut pass.life, &id, "launched");
            note_launch(&job.account);
            pass.finish(Save::Always);
            let args = vec![lifescape_json::str_value("--node"), lifescape_json::str_value(&id)];
            let reply = crate::cpu_profile::timed("launch.run_job", || orch.run_job(&job.account, "tutorial", &args));
            if reply.get("ok").as_bool() != Some(true) {
                let error = reply.get("error").as_str().unwrap_or("?");
                crate::logln!("brain {}: launch refused: {error}", job.account);
                if crate::brain_inflight::refusal_never_ran(error) {
                    refused_launch(&job.account, &id);
                }
            }
            Flow::Stop(Save::None)
        }
        TutorialGate::Hold(reason) => {
            if pass.life.history.last().is_none_or(|entry| entry.outcome != "hold") {
                crate::logln!("brain {}: holding launches: {reason}", job.account);
            }
            Flow::Stop(Save::IfDirty)
        }
        TutorialGate::Closed(id) => {
            crate::logln!("brain {}: seen off the island, closing tutorial {id}", job.account);
            push_history(&mut pass.life, &id, "tutorial_done");
            lifescape_planner::normalize(&mut pass.life);
            pass.dirty = true;
            Flow::Continue
        }
        TutorialGate::Clear => Flow::Continue,
    }
}

/// Graph tasks read the pack (tools, coins): none before it streamed.
/// The tutorial above needs only the position (the island hides the
/// pack until its own tab opens).
pub(crate) fn pack_streamed(pass: &mut LaunchPass) -> Flow {
    if !pass.inputs.inv_streamed {
        return Flow::Stop(Save::IfDirty);
    }
    Flow::Continue
}

/// The side market passes (`brain_commerce::COMMERCE_STEPS`, lot 5): quest
/// reserve publish, surplus listing, the maker's quotes or the wishlist,
/// alchemy, arbitrage, stock, tool shop and gear. Commerce never preempts
/// the ambition (user rule 2026-11-23).
pub(crate) fn market_passes(pass: &mut LaunchPass) -> Flow {
    crate::brain_commerce::market_passes(pass)
}

/// Orphan needs, parked settles, the quest hold and cooled methods: what
/// waits like a parked buy.
pub(crate) fn waits(pass: &mut LaunchPass) -> Flow {
    let job = pass.job;
    // Persistent needs (plan marché M2): a need's bid outlives its fill
    // wait; it closes here once no open node wants its object.
    crate::cpu_profile::timed("market.orphan_needs", || crate::brain_market::release_orphan_needs(job, &pass.life));
    // Market buys parked on the book settle every pass; the worker never
    // waits for them (no idle bot, 2026-11-13).
    let (settled, waiting) = crate::cpu_profile::timed("filler.settle_parked", || crate::brain_filler::settle_parked(job, &mut pass.life));
    pass.waiting = waiting;
    if settled {
        pass.dirty = true;
    }
    // Quest gate (2026-11-14): a ready quest launches only with its
    // objects at hand (missing ones obtained first, listed ones
    // released); a cooled quest waits like a parked buy.
    let (gated, quest_waits) = crate::cpu_profile::timed("quest_gate.hold_pass", || crate::brain_quest_gate::hold_pass(job, &mut pass.life, &pass.live));
    if gated {
        pass.dirty = true;
    }
    pass.waiting.extend(quest_waits);
    // Nodes waiting out a cooled method (every candidate full or cooled)
    // wait like a parked buy: a filler works meanwhile.
    pass.waiting.extend(crate::brain_cool::waiting(&pass.life));
    Flow::Continue
}

/// A reached goal over older goals' abandoned nodes is Finished, never
/// a stuck spell (`brain_upkeep_drive::graph_state`, decisions in the
/// planner's `upkeep_graph`).
pub(crate) fn graph_state(pass: &mut LaunchPass) -> Flow {
    let fx = LiveUpkeepFx::new(pass.orch, pass.job, &pass.live, pass.deps.clone());
    upkeep::graph_state(&fx, &mut pass.life, pass.dirty)
}
