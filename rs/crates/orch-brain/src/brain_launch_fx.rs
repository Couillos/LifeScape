#![forbid(unsafe_code)]
//! External effects of the launch decision (lot 3 of the launch
//! refactor): everything `brain_launch_drive` does that is not a pure
//! decision or a life change, behind one trait. `LiveFx` is the real
//! thing (market service, worker RPC, claims, logs, the life file); the
//! equivalence tests (`brain_launch_reference`) plug a recording fake.

use lifescape_json::Value;
use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::launch_apply::PlanSource;
use lifescape_planner::launch_decide::{CleanupFacts, LootFacts, Rules, ShopTrip, Site};
use lifescape_planner::trade_craft::TradeLot;
use lifescape_planner::{LaunchInputs, Life, Node, NodeKind};
use std::sync::Arc;

use crate::brain_launch_pass::Save;
use crate::fleet::Fleet;
use crate::jobs::Job;

/// What the launch drive asks of the outside world, in the order the
/// brain always asked it.
pub(crate) trait Fx {
    /// The pure rules that live above the planner.
    fn rules(&self) -> Rules;
    /// Runs `body` inside the named brain profile phase.
    fn profile(&self, label: &str, body: &mut dyn FnMut());
    fn log(&mut self, line: String);
    /// Reads again what the pass's own actions changed (locks, listings,
    /// committed coins, the pack, the fleet's sites).
    fn refresh_market(&mut self, inputs: &mut LaunchInputs);
    /// Reads the wall clock again before a decision (a no-op for fakes,
    /// whose clock is fixed).
    fn refresh_clock(&mut self, _inputs: &mut LaunchInputs) {}
    /// Withdraws the bot's own listings of the items; how many closed.
    fn release_listings(&mut self, items: &[String]) -> usize;
    fn cancel_open_buys(&mut self, account: &str, item: &str);
    fn note_gear_withdrawn(&mut self, account: &str, item: &str);
    fn claim_lot(&mut self, life: &Life, lot: &TradeLot);
    fn release_run(&mut self, account: &str, node: &str);
    /// The food needs retargeted to what the wallet affords.
    fn afford_food(&mut self, method: &str, needed: Vec<String>) -> Vec<String>;
    fn loot_facts(&mut self, spot: &str) -> LootFacts;
    /// The cleanup keep sources and the (consumed) shop trip plan.
    fn cleanup_facts(&mut self, life: &Life, inputs: &LaunchInputs, snapshot: &Node) -> CleanupFacts;
    /// Places the market buy; the error text of a refusal.
    fn market_buy(&mut self, life: &Life, position: usize, snapshot: &Node, ctx: &ReadyCtx) -> Result<(), String>;
    fn crowd_note(&mut self, site: Option<Site>, method: &str);
    fn standing_bid(&mut self, life: &Life, position: usize, snapshot: &Node, ctx: &ReadyCtx);
    fn claim_launch(&mut self, snapshot: &Node, method: &str, budget: Option<(u64, u64)>);
    fn note_launch(&mut self);
    /// Saves the life as asked (before the blocking worker call).
    fn save(&mut self, life: &Life, save: Save);
    /// Queues the script on the worker.
    fn run_job(&mut self, script: &str, args: &[String], node_id: &str);
    fn plan_source(&self) -> &dyn PlanSource;
}

/// The reader of the planner's rates and guides for one bot.
pub(crate) struct LivePlan<'a> {
    pub live: &'a Value,
    pub deps: crate::brain_deps::BrainDeps,
}

impl PlanSource for LivePlan<'_> {
    fn with_read(&self, account: &str, profile: &lifescape_planner::Profile, quests_done: &[String], body: &mut dyn FnMut(&lifescape_planner::plan_read::PlanRead)) {
        let deps = &self.deps;
        let view = crate::brain_plan_read::BotView { account: account.to_string(), profile: profile.clone(), quests: quests_done.to_vec() };
        crate::brain_plan_read::with_plan_read(&view, self.live, |read| body(read), deps);
    }
}

/// The real effects of one launch pass.
pub(crate) struct LiveFx<'a> {
    pub orch: &'a Arc<dyn Fleet>,
    pub job: &'a Arc<Job>,
    pub live: &'a Value,
    /// The pass's handles (market, clock, crowd).
    pub deps: crate::brain_deps::BrainDeps,
    plan: LivePlan<'a>,
}

impl<'a> LiveFx<'a> {
    pub(crate) fn new(orch: &'a Arc<dyn Fleet>, job: &'a Arc<Job>, live: &'a Value, deps: crate::brain_deps::BrainDeps) -> Self {
        LiveFx { orch, job, live, plan: LivePlan { live, deps: deps.clone() }, deps }
    }
}

/// The rules the scripts crate and the registry own.
pub(crate) fn live_rules() -> Rules {
    Rules { short_inputs: crate::brain_input_gate::short_inputs, input_each: crate::brain_input_gate::input_each, script_known: crate::registry::script_known }
}

impl Fx for LiveFx<'_> {
    fn rules(&self) -> Rules {
        live_rules()
    }

    fn profile(&self, label: &str, body: &mut dyn FnMut()) {
        let label: &'static str = match label {
            "resolve" => "resolve",
            _ => "launch.other",
        };
        crate::cpu_profile::timed(label, || body());
    }

    fn log(&mut self, line: String) {
        crate::logln!("{line}");
    }

    fn refresh_market(&mut self, inputs: &mut LaunchInputs) {
        crate::brain_launch_inputs::gather_market(self.job, inputs, &self.deps);
    }

    fn refresh_clock(&mut self, inputs: &mut LaunchInputs) {
        crate::brain_launch_inputs::refresh_clock(inputs, &self.deps);
    }

    fn release_listings(&mut self, items: &[String]) -> usize {
        let deps = &self.deps.clone();
        crate::brain_tools::release_own_locks(&**self.orch, &self.job.account, items, deps)
    }

    fn cancel_open_buys(&mut self, account: &str, item: &str) {
        let deps = &self.deps.clone();
        crate::brain_market::cancel_open_buys(account, item, deps);
    }

    fn note_gear_withdrawn(&mut self, account: &str, item: &str) {
        crate::brain_gear::note_withdrawn(account, item);
    }

    fn claim_lot(&mut self, life: &Life, lot: &TradeLot) {
        let secs = crate::market_claims::claim_secs(crate::brain_trade::lot_ticks(lot), self.deps.clock.tick_ms());
        crate::market_claims::claim_run(&life.account, &lot.node, &lot.product, lot.lot, secs);
    }

    fn release_run(&mut self, account: &str, node: &str) {
        crate::market_claims::release_run(account, node);
    }

    fn afford_food(&mut self, method: &str, needed: Vec<String>) -> Vec<String> {
        let deps = &self.deps.clone();
        crate::brain_food::afford_food_needs(self.job, method, needed, deps)
    }

    fn loot_facts(&mut self, spot: &str) -> LootFacts {
        // The bot's time value (D1): what its work really earns.
        let rate = crate::brain_d17::coin_rate(&self.job.account);
        let service = self.deps.market.clone();
        let value_of = |name: &str| {
            let traded = service
                .as_ref()
                .and_then(|service| lifescape_gamedata::objs_by_name(name).into_iter().next().map(|obj| service.reference(obj.obj_id as i64)))
                .filter(|reference| reference.traded)
                .map(|reference| reference.price)
                .unwrap_or(0);
            traded.max(lifescape_gamedata::shop_sale::npc_sale_value(name).unwrap_or(0))
        };
        LootFacts { rate, values: lifescape_planner::launch_leaf::loot_candidates(spot).into_iter().map(|name| (name.clone(), value_of(&name))).collect() }
    }

    fn cleanup_facts(&mut self, life: &Life, inputs: &LaunchInputs, snapshot: &Node) -> CleanupFacts {
        let deps = &self.deps.clone();
        let (job, live) = (self.job, self.live);
        let levels = crate::brain_tools::live_levels(live);
        let mut keep = crate::brain::cleanup_keep(life);
        keep.extend(crate::brain_tools::kept_tools(life, &inputs.stacks, &crate::brain_tools::level_refs(&levels)));
        keep.extend(crate::brain_gear::ladder_keep(job, life, live));
        // A caster's alchemy stock and runes (2026-11-15).
        keep.extend(crate::brain_alch::alch_keep(life, live, deps));
        // A quoting maker's slice is its trade (Q6): never sold nor
        // dropped by the janitor (its NPC dumps ride `--shopsell`).
        keep.extend(crate::market_maker::slice_names(life));
        // Goods sold and not handed over yet: never relisted nor dropped
        // (user 2026-11-19, point 2).
        if let Some(service) = self.deps.market.as_deref() {
            let deals = service.read_store().open_deals_for_account(&job.account).unwrap_or_default();
            keep.extend(crate::brain_locks::promised_names(&job.account, &deals));
        }
        let shop = if life.profile.ironman {
            ShopTrip::None
        } else if let Some(plan) = crate::brain_npc_sale::take(&job.account, &snapshot.id) {
            // Q3: the shop trip first (never a unit under the market). The
            // lots were planned at the listing pass: what the graph needs
            // at launch (a node drawn since) leaves the trip (live
            // 2026-11-22, owen_wood: 7 680 iron ore offered, its smelt
            // blocked `missing: iron_ore`).
            let needed = crate::brain_sales::working_keep(life, &[]);
            match plan.clone().without_kept(&needed) {
                Some(trimmed) => ShopTrip::Trimmed { lines: trimmed.lines.len(), changed: trimmed != plan, args: trimmed.args() },
                None => ShopTrip::Dropped,
            }
        } else {
            ShopTrip::None
        };
        CleanupFacts { keep, shop }
    }

    fn market_buy(&mut self, life: &Life, position: usize, snapshot: &Node, ctx: &ReadyCtx) -> Result<(), String> {
        let deps = &self.deps.clone();
        // What one unit is worth to this buyer: its own make cost.
        let worth = match &snapshot.kind {
            NodeKind::ObtainItems { item, count } => {
                let units = (*count - ctx.held).max(1);
                crate::brain_bids::production_costs_for(life, self.live, &[(item.clone(), units)], deps).get(item).map(|cost| cost.ceil() as i64)
            }
            _ => None,
        };
        // An input of a product the graph makes: never over the product.
        let reserve = crate::brain_market::derived_reserve(life, position, self.live, deps);
        crate::cpu_profile::timed("market.obtain_bid", || crate::brain_market::place_obtain_bid(self.job, snapshot, ctx, worth, reserve, deps)).map(|_| ())
    }

    fn crowd_note(&mut self, site: Option<Site>, method: &str) {
        crate::site_crowd::note(&self.job.account, site, method);
    }

    fn standing_bid(&mut self, life: &Life, position: usize, snapshot: &Node, ctx: &ReadyCtx) {
        let deps = &self.deps.clone();
        let live = self.live;
        let derived = || crate::brain_market::derived_reserve(life, position, live, deps);
        crate::cpu_profile::timed("market.standing_bid", || crate::brain_market::place_standing_bid(self.job, snapshot, ctx, &derived, deps));
    }

    fn claim_launch(&mut self, snapshot: &Node, method: &str, budget: Option<(u64, u64)>) {
        let deps = &self.deps.clone();
        // A money run posts its product on the public order board (plan
        // équilibre step 2) once it really launches, released at its report
        // (`brain_fold::reported_node`), at a lost or refused run
        // (`brain_inflight`) or at expiry.
        crate::brain_money::claim_launch(&self.job.account, snapshot, method, budget, self.live, deps);
    }

    fn note_launch(&mut self) {
        crate::brain_launch::note_launch(&self.job.account);
    }

    fn save(&mut self, life: &Life, save: Save) {
        crate::brain_launch_pass::save_as(life, save, false);
    }

    fn run_job(&mut self, script: &str, args: &[String], node_id: &str) {
        let values: Vec<Value> = args.iter().map(|arg| lifescape_json::str_value(arg)).collect();
        // Blocking worker RPC is fine here: this is the brain thread, not
        // the supervisor. A refused launch re-queues via crash recovery.
        let reply = crate::cpu_profile::timed("launch.run_job", || self.orch.run_job(&self.job.account, script, &values));
        if reply.get("ok").as_bool() != Some(true) {
            let error = reply.get("error").as_str().unwrap_or("?");
            crate::logln!("brain {}: launch refused: {error}", self.job.account);
            // Never ran (P2-5): back to ready, uncounted, relaunched once the
            // worker reads idle (no "no report" on a run that never started).
            if crate::worker_call::Refusal::parse(error).never_ran() {
                crate::brain_launch::refused_launch(&self.job.account, node_id);
            }
        }
    }

    fn plan_source(&self) -> &dyn PlanSource {
        &self.plan
    }
}
