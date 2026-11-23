#![forbid(unsafe_code)]
//! Test-only reference of the launch decision (lot 3 of the launch
//! refactor): the node and dispatch steps as they were before the
//! decision moved into the planner, copied step by step over a recording
//! fake of the effects (`RecFx`). `brain_launch_equiv_tests` runs it and
//! the real path (`brain_launch_drive::drive` over the same fake) on the
//! same scenarios and compares what they decide, perform, log and write.
//! The pure leaves they share (the pick note, the cooldown wait, the
//! counting rules) keep their own unit tests; what this reference proves
//! is the composition: the order, the early stops and the mutations.

use std::cell::RefCell;

use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::launch_apply::{block_outcome, on_resolve_wait, push_history, settle_market_buy, PlanSource};
use lifescape_planner::launch_decide::{CleanupFacts, Emit, LootFacts, Rules, ShopTrip, Site};
use lifescape_planner::launch_leaf::{coin_need_lift, coin_need_met, fight_spot, held_verdict, loot_skip, merge_keep, skip_args, HeldVerdict};
use lifescape_planner::trade_craft::TradeLot;
use lifescape_planner::{draw_engagement_ticks, map_method, resolve_ready, Engagement, LaunchInputs, Life, Node, NodeKind, NodeStatus};

use crate::brain_launch_fx::{live_rules, Fx};
use crate::brain_launch_pass::Save;

/// What the fake answers where the real world would decide.
#[derive(Debug, Clone, Default)]
pub(crate) struct Knobs {
    /// A deal in flight keeps the lock after the listing is withdrawn.
    pub stubborn: bool,
    /// The market refuses the buy.
    pub refuse_buy: bool,
    /// The food the wallet affords is another one.
    pub retarget_food: bool,
    /// 0 none, 1 trimmed (changed), 2 trimmed (same), 3 dropped.
    pub shop: u8,
    /// The coin rate of the fighter.
    pub rate_tenths: i64,
}

/// One observable event of a pass: an effect or a log line.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Ev {
    Log(String),
    Release(Vec<String>),
    CancelBuys(String, String),
    GearWithdrawn(String, String),
    ClaimLot(String, String),
    ReleaseRun(String, String),
    Food(String, Vec<String>),
    Loot(String),
    Cleanup(String),
    MarketBuy { node: String, held: i64, share: Option<i64> },
    Crowd(Option<Site>, String),
    Bid { node: String, share: Option<i64>, held: i64 },
    ClaimLaunch { node: String, method: String, budget: Option<(u64, u64)> },
    NoteLaunch,
    Save(Save),
    RunJob { script: String, args: Vec<String>, node: String },
}

/// The recording fake: a world (market state), canned answers, a trace.
pub(crate) struct RecFx {
    pub world: RefCell<LaunchInputs>,
    pub trace: RefCell<Vec<Ev>>,
    pub knobs: Knobs,
}

struct PriorPlan;

impl PlanSource for PriorPlan {
    fn with_read(&self, _account: &str, _profile: &lifescape_planner::Profile, _quests: &[String], body: &mut dyn FnMut(&lifescape_planner::plan_read::PlanRead)) {
        body(&lifescape_planner::plan_read::PlanRead::prior());
    }
}

static PRIOR: PriorPlan = PriorPlan;

impl RecFx {
    pub(crate) fn new(world: LaunchInputs, knobs: Knobs) -> Self {
        RecFx { world: RefCell::new(world), trace: RefCell::new(Vec::new()), knobs }
    }

    fn push(&self, event: Ev) {
        self.trace.borrow_mut().push(event);
    }

    pub(crate) fn log_ref(&self, line: String) {
        self.push(Ev::Log(line));
    }

    pub(crate) fn refresh_ref(&self, inputs: &mut LaunchInputs) {
        let world = self.world.borrow();
        inputs.inv = world.inv.clone();
        inputs.bank = world.bank.clone();
        inputs.wearing = world.wearing.clone();
        inputs.locks = world.locks.clone();
        inputs.listed = world.listed.clone();
        inputs.commit_rows = world.commit_rows.clone();
        inputs.commit_flat = world.commit_flat;
        inputs.crowd = world.crowd.clone();
        inputs.seats = world.seats.clone();
    }

    /// Withdraws the listings of every member of each item's family.
    pub(crate) fn release_ref(&self, items: &[String]) -> usize {
        self.push(Ev::Release(items.to_vec()));
        let mut closed = 0;
        let mut world = self.world.borrow_mut();
        for item in items {
            for name in lifescape_planner::tools::tool_family(item) {
                let Some(id) = lifescape_gamedata::objs_by_name(&name).into_iter().next().map(|obj| obj.obj_id as i64) else { continue };
                let units = world.listed.get(&id).copied().unwrap_or(0);
                if units <= 0 {
                    continue;
                }
                closed += 1;
                world.listed.insert(id, 0);
                if !self.knobs.stubborn {
                    let base = lifescape_planner::base_of(id);
                    let left = (world.locks.get(&base).copied().unwrap_or(0) - units).max(0);
                    world.locks.insert(base, left);
                }
            }
        }
        closed
    }

    pub(crate) fn food_ref(&self, method: &str, needed: Vec<String>) -> Vec<String> {
        self.push(Ev::Food(method.to_string(), needed.clone()));
        if self.knobs.retarget_food {
            needed.into_iter().map(|need| format!("{need}_x")).collect()
        } else {
            needed
        }
    }

    pub(crate) fn loot_ref(&self, spot: &str) -> LootFacts {
        self.push(Ev::Loot(spot.to_string()));
        LootFacts {
            rate: self.knobs.rate_tenths as f64 / 10.0,
            values: lifescape_planner::launch_leaf::loot_candidates(spot).into_iter().map(|name| { let value = (name.len() as i64 * 7) % 90; (name, value) }).collect(),
        }
    }

    pub(crate) fn cleanup_ref(&self, snapshot: &Node) -> CleanupFacts {
        self.push(Ev::Cleanup(snapshot.id.clone()));
        let shop = match self.knobs.shop {
            1 => ShopTrip::Trimmed { lines: 2, args: vec!["--shopsell".to_string(), "coal".to_string()], changed: true },
            2 => ShopTrip::Trimmed { lines: 3, args: vec!["--shopsell".to_string(), "ore".to_string()], changed: false },
            3 => ShopTrip::Dropped,
            _ => ShopTrip::None,
        };
        CleanupFacts { keep: vec!["coal".to_string(), "Bronze_axe".to_string()], shop }
    }

    pub(crate) fn buy_ref(&self, snapshot: &Node, ctx: &ReadyCtx) -> Result<(), String> {
        self.push(Ev::MarketBuy { node: format!("{snapshot:?}"), held: ctx.held, share: ctx.coin_share_pct });
        if self.knobs.refuse_buy {
            Err("refused by the book".to_string())
        } else {
            Ok(())
        }
    }

    pub(crate) fn bid_ref(&self, snapshot: &Node, ctx: &ReadyCtx) {
        self.push(Ev::Bid { node: format!("{snapshot:?}"), share: ctx.coin_share_pct, held: ctx.held });
    }
}

impl Fx for RecFx {
    fn rules(&self) -> Rules {
        live_rules()
    }
    fn profile(&self, _label: &str, body: &mut dyn FnMut()) {
        body();
    }
    fn log(&mut self, line: String) {
        self.log_ref(line);
    }
    fn refresh_market(&mut self, inputs: &mut LaunchInputs) {
        self.refresh_ref(inputs);
    }
    fn release_listings(&mut self, items: &[String]) -> usize {
        self.release_ref(items)
    }
    fn cancel_open_buys(&mut self, account: &str, item: &str) {
        self.push(Ev::CancelBuys(account.to_string(), item.to_string()));
    }
    fn note_gear_withdrawn(&mut self, account: &str, item: &str) {
        self.push(Ev::GearWithdrawn(account.to_string(), item.to_string()));
    }
    fn claim_lot(&mut self, life: &Life, lot: &TradeLot) {
        self.push(Ev::ClaimLot(life.account.clone(), format!("{lot:?}")));
    }
    fn release_run(&mut self, account: &str, node: &str) {
        self.push(Ev::ReleaseRun(account.to_string(), node.to_string()));
    }
    fn afford_food(&mut self, method: &str, needed: Vec<String>) -> Vec<String> {
        self.food_ref(method, needed)
    }
    fn loot_facts(&mut self, spot: &str) -> LootFacts {
        self.loot_ref(spot)
    }
    fn cleanup_facts(&mut self, _life: &Life, _inputs: &LaunchInputs, snapshot: &Node) -> CleanupFacts {
        self.cleanup_ref(snapshot)
    }
    fn market_buy(&mut self, _life: &Life, _position: usize, snapshot: &Node, ctx: &ReadyCtx) -> Result<(), String> {
        self.buy_ref(snapshot, ctx)
    }
    fn crowd_note(&mut self, site: Option<Site>, method: &str) {
        self.push(Ev::Crowd(site, method.to_string()));
    }
    fn standing_bid(&mut self, _life: &Life, _position: usize, snapshot: &Node, ctx: &ReadyCtx) {
        self.bid_ref(snapshot, ctx);
    }
    fn claim_launch(&mut self, snapshot: &Node, method: &str, budget: Option<(u64, u64)>) {
        self.push(Ev::ClaimLaunch { node: format!("{snapshot:?}"), method: method.to_string(), budget });
    }
    fn note_launch(&mut self) {
        self.push(Ev::NoteLaunch);
    }
    fn save(&mut self, _life: &Life, save: Save) {
        self.push(Ev::Save(save));
    }
    fn run_job(&mut self, script: &str, args: &[String], node_id: &str) {
        self.push(Ev::RunJob { script: script.to_string(), args: args.to_vec(), node: node_id.to_string() });
    }
    fn plan_source(&self) -> &dyn PlanSource {
        &PRIOR
    }
}

fn run_emits(fx: &RecFx, life: &Life, emits: Vec<Emit>) {
    for emit in emits {
        match emit {
            Emit::Log(line) => fx.log_ref(line),
            Emit::ClaimLot(lot) => fx.push(Ev::ClaimLot(life.account.clone(), format!("{lot:?}"))),
            other => panic!("unexpected emit {other:?}"),
        }
    }
}

fn status_of(life: &Life, id: &str) -> String {
    life.nodes.iter().find(|node| node.id == id).map_or_else(|| "dropped".to_string(), |node| node.status.as_str())
}

/// File a wait block the way every blocking step did.
fn wait_block(fx: &RecFx, life: &mut Life, inputs: &LaunchInputs, ctx: &ReadyCtx, position: usize, missing: &[String], tail: String) {
    let id = life.nodes[position].id.clone();
    let level_of = |skill: &str| inputs.level_of(skill);
    let profile = life.profile.clone();
    let account = life.account.clone();
    let mut fold = None;
    PRIOR.with_read(&account, &profile, &ctx.quests_done, &mut |read| fold = Some(lifescape_planner::record_wait_block(life, position, missing, &level_of, read)));
    let fold = fold.expect("read");
    push_history(life, &id, block_outcome(&fold), inputs.now);
    if let lifescape_planner::FailureFold::CycleBroken(line) = &fold {
        fx.log_ref(format!("brain {account}: {line}"));
    }
    let status = status_of(life, &id);
    fx.log_ref(format!("brain {account}: {id} {tail} -> {status}"));
    lifescape_planner::normalize(life);
}

/// The old node steps and dispatch steps, in their old order, over the fake.
pub(crate) fn reference_pass(fx: &RecFx, life: &mut Life, inputs: &mut LaunchInputs, ctx: &mut ReadyCtx, position: usize, committed: i64) -> Save {
    let account = life.account.clone();
    let now = inputs.now;
    let rules = live_rules();
    // already_held
    if let NodeKind::ObtainItems { item, count } = life.nodes[position].kind.clone() {
        let exact = lifescape_planner::gear_ladder::is_gear_obtain(life, position) || matches!(&life.goal, lifescape_planner::GoalKind::Produce { item: made, .. } if *made == item);
        let mut free = if ctx.held < count {
            0
        } else {
            fx.refresh_ref(inputs);
            inputs.free_now(&item, exact)
        };
        if held_verdict(count, ctx.held, free) == HeldVerdict::WithdrawListings {
            let withdrawn = fx.release_ref(std::slice::from_ref(&item));
            fx.refresh_ref(inputs);
            free = inputs.free_now(&item, exact);
            fx.log_ref(format!("brain {account}: {} holds {} {item} but its listings locked them: {withdrawn} listing(s) withdrawn, {free} free now", life.nodes[position].id, ctx.held));
        }
        if held_verdict(count, ctx.held, free) == HeldVerdict::Done {
            let id = life.nodes[position].id.clone();
            fx.push(Ev::CancelBuys(account.clone(), item.clone()));
            lifescape_planner::complete_node(life, position, ctx.held);
            push_history(life, &id, "already_held", now);
            fx.log_ref(format!("brain {account}: {id} holds {} {item}, done without launch", ctx.held));
            lifescape_planner::normalize(life);
            return Save::Always;
        }
    }
    // gear_withdraw
    if lifescape_planner::gear_ladder::is_gear_obtain(life, position) {
        let gear_item = match &life.nodes[position].kind {
            NodeKind::ObtainItems { item, .. } => Some(item.clone()),
            _ => None,
        };
        ctx.coin_share_pct = gear_item.as_deref().map_or(Some(lifescape_planner::GEAR_SHARE_PCT), lifescape_planner::tool_kit::step_coin_share);
        let shop_piece = gear_item.as_deref().is_some_and(lifescape_gamedata::obtain::kit_shop_route);
        let made_piece = life.nodes[position].alternatives.iter().any(|alt| !alt.method.starts_with("market:") && !alt.method.starts_with("shop:"));
        if !shop_piece && !made_piece && lifescape_planner::gear_ladder::withdraw_due(ctx) {
            if let Some(item) = &gear_item {
                fx.push(Ev::GearWithdrawn(account.clone(), item.clone()));
            }
            let removed = lifescape_planner::withdraw_gear_step(life, position);
            fx.log_ref(format!("brain {account}: gear step {} withdrawn: no seller, the standing bid waits", removed.join(", ")));
            for id in &removed {
                push_history(life, id, "gear_withdrawn", now);
            }
            lifescape_planner::normalize(life);
            return Save::Always;
        }
    }
    // coin_target
    if let NodeKind::HaveCoins { amount } = life.nodes[position].kind {
        if ctx.coins >= amount {
            let id = life.nodes[position].id.clone();
            lifescape_planner::complete_node(life, position, ctx.coins);
            push_history(life, &id, "goal_reached", now);
            fx.log_ref(format!("brain {account}: {id} holds {} coins >= {amount}, done", ctx.coins));
            lifescape_planner::normalize(life);
            return Save::Always;
        }
    }
    // coin_cycle
    let regress = lifescape_planner::lifecycle_cycle::coin_regress(life, position);
    if !regress.is_empty() {
        let id = life.nodes[position].id.clone();
        if let lifescape_planner::FailureFold::CycleBroken(line) = lifescape_planner::lifecycle_cycle::break_cycle(life, position, "coins", &regress) {
            fx.log_ref(format!("brain {account}: {line}"));
            push_history(life, &id, "cycle_broken", now);
        }
        lifescape_planner::normalize(life);
        return Save::Always;
    }
    // resolve
    {
        let node = &mut life.nodes[position];
        resolve_ready(node, ctx);
        fx.log_ref(format!("brain {account}: resolve {} -> {:?} (ctx coins={})", node.id, node.method, ctx.coins));
    }
    // steer_off_bans
    if matches!(life.nodes[position].kind, NodeKind::MakeMoney | NodeKind::HaveCoins { .. }) {
        let banned = lifescape_planner::banned_skills(&life.history, &|stamp: i64| inputs.ticks_of(now - stamp));
        let money_id = life.nodes[position].id.clone();
        match lifescape_planner::steer_for_work(&mut life.nodes[position], ctx, &banned, &[]) {
            Ok(lifescape_planner::WorkSteer::OffBans(true)) => fx.log_ref(format!("brain {account}: {money_id} steered off banned {} -> {:?}", banned.join(","), life.nodes[position].method)),
            Ok(lifescape_planner::WorkSteer::OffBans(false)) => {}
            Ok(lifescape_planner::WorkSteer::ThroughBan(skill)) => fx.log_ref(format!(
                "brain {account}: {money_id} works through the {skill} ban (every workable money method is banned) -> {:?}",
                life.nodes[position].method
            )),
            Err(reason) => {
                fx.log_ref(format!("brain {account}: {money_id} waits: {reason}"));
                lifescape_planner::retry_uncounted(life, position);
                return Save::Always;
            }
        }
    }
    // picks_and_trade_note
    {
        let node = life.nodes[position].clone();
        let mut emits = Vec::new();
        lifescape_planner::launch_note::record_pick(&mut life.memory, &account, &node, ctx, now, &mut emits);
        lifescape_planner::launch_note::note_lot(&mut life.memory, &account, &node, ctx, now, &mut emits);
        run_emits(fx, life, emits);
    }
    // blocked_at_resolve
    if let NodeStatus::Blocked { reason } = life.nodes[position].status.clone() {
        let missing: Vec<String> = reason.strip_prefix("missing:").map(|needs| needs.split(',').filter(|need| !coin_need_met(need, ctx.coins)).map(|need| coin_need_lift(need, committed)).collect()).unwrap_or_default();
        let id = life.nodes[position].id.clone();
        let mut out = Vec::new();
        let mut waited = false;
        if lifescape_planner::cooldown::resolve_wait(&reason).is_some() {
            on_resolve_wait(life, position, &reason, inputs, &mut out);
            waited = true;
        }
        if waited {
            run_emits(fx, life, out);
            return Save::Always;
        }
        if missing.is_empty() {
            lifescape_planner::record_contract_error(life, position, &format!("resolve blocked: {reason}"));
            push_history(life, &id, "abandoned", now);
            let status = status_of(life, &id);
            fx.log_ref(format!("brain {account}: {id} blocked at resolve ({reason}) -> {status}"));
            lifescape_planner::normalize(life);
        } else {
            wait_block(fx, life, inputs, ctx, position, &missing, format!("blocked at resolve ({reason})"));
        }
        return Save::Always;
    }
    // method_resolved
    let method = life.nodes[position].method.clone().unwrap_or_default();
    if method.is_empty() {
        let id = life.nodes[position].id.clone();
        fx.log_ref(format!("brain {account}: no method resolvable for {id}, abandoning"));
        lifescape_planner::record_contract_error(life, position, "no method resolvable (no workable source for this node)");
        push_history(life, &id, "abandoned", now);
        lifescape_planner::normalize(life);
        return Save::Always;
    }
    // iron_guard
    if !lifescape_planner::launch_allowed(&method, life.profile.ironman) {
        fx.log_ref(format!("brain {account}: refusing {method} for Ironman"));
        lifescape_planner::retry_uncounted(life, position);
        return Save::Silent;
    }
    // input_gate: the counts are live reads of the world, the release a closure over it.
    let gate = {
        let free = |name: &str| fx.world.borrow().held(name);
        let listed = |name: &str| fx.world.borrow().listed_units(name);
        let total = |name: &str| fx.world.borrow().raw_units(name);
        let _ = rules;
        crate::brain_input_gate::verdict(&method, ctx.bank_known, &free, &total, &listed, &mut |items| fx.release_ref(items))
    };
    match gate {
        crate::brain_input_gate::Gate::Clear => {}
        crate::brain_input_gate::Gate::Released(items) => {
            fx.log_ref(format!("brain {account}: {} inputs listed on the market ({}): listings withdrawn, launching", life.nodes[position].id, items.join(", ")));
        }
        crate::brain_input_gate::Gate::Short(missing) => {
            wait_block(fx, life, inputs, ctx, position, &missing.clone(), format!("inputs short before launch ({})", missing.join(", ")));
            return Save::Always;
        }
    }
    // food_gate
    let needed: Vec<String> = lifescape_planner::food_gate::food_short(&method, ctx).into_iter().collect();
    if !needed.is_empty() {
        let missing = fx.food_ref(&method, needed);
        wait_block(fx, life, inputs, ctx, position, &missing.clone(), format!("foodless before launch ({})", missing.join(", ")));
        return Save::Always;
    }
    dispatch(fx, life, inputs, ctx, position, method)
}

fn dispatch(fx: &RecFx, life: &mut Life, inputs: &LaunchInputs, ctx: &ReadyCtx, launch: usize, method: String) -> Save {
    let account = life.account.clone();
    let now = inputs.now;
    // open
    let budget: Option<(u64, u64)> = match &life.nodes[launch].kind {
        NodeKind::MakeMoney if lifescape_planner::is_alcher(life) => {
            let window = inputs.filler_ticks;
            life.engagement = Some(Engagement { node: life.nodes[launch].id.clone(), min_ticks: window, max_ticks: window, chosen: window });
            Some((window, window))
        }
        NodeKind::MakeMoney => {
            let chosen = draw_engagement_ticks(life.seed, life.history.len() as u64);
            life.engagement = Some(Engagement { node: life.nodes[launch].id.clone(), min_ticks: 6000, max_ticks: chosen.max(6000), chosen: chosen.max(6000) });
            let max = life.engagement.as_ref().map(|engaged| engaged.max_ticks).unwrap_or(6000);
            Some((6000, max))
        }
        NodeKind::ReachLevel { .. } | NodeKind::ReachCombat { .. } => Some((0, 18000)),
        NodeKind::ObtainItems { .. } if !method.starts_with("market:") && !method.starts_with("shop:") => Some((6000, 6000)),
        NodeKind::HaveCoins { .. } => Some((6000, 6000)),
        NodeKind::SpaceCleanup => Some((0, 6000)),
        _ => None,
    };
    if matches!(life.nodes[launch].kind, NodeKind::ObtainItems { .. }) {
        lifescape_planner::record_progress(life, launch, ctx.held);
    }
    let snapshot = life.nodes[launch].clone();
    // market_buy
    if method == "market:buy" {
        let id = snapshot.id.clone();
        let placed = fx.buy_ref(&snapshot, ctx);
        settle_market_buy(life, launch, ctx.held, placed, now);
        let _ = id;
        return Save::Always;
    }
    // map_script
    let Some((script, mut args)) = map_method(&method, &snapshot, budget) else {
        let id = life.nodes[launch].id.clone();
        fx.log_ref(format!("brain {account}: no script for {method}, abandoning {id}"));
        fx.push(Ev::ReleaseRun(account.clone(), id.clone()));
        lifescape_planner::record_contract_error(life, launch, &format!("no-script: {method}"));
        push_history(life, &id, "abandoned", now);
        return Save::Silent;
    };
    // crowd_and_standing_bid
    fx.push(Ev::Crowd(snapshot.site, method.clone()));
    if matches!(snapshot.kind, NodeKind::ObtainItems { .. }) && !method.starts_with("shop:") {
        fx.bid_ref(&snapshot, ctx);
    }
    // loot_skip
    if let Some(spot) = fight_spot(&method) {
        if inputs.market_up && !life.profile.ironman {
            let facts = fx.loot_ref(spot);
            let skip = loot_skip(spot, &|name| facts.values.iter().find(|(known, _)| known == name).map_or(0, |(_, value)| *value), facts.rate);
            if !skip.is_empty() {
                fx.log_ref(format!("brain {account}: leaving {} lying at {spot} (under {} ticks at {:.2} coins/tick)", skip.join(","), lifescape_planner::launch_leaf::PICKUP_TICKS, facts.rate));
            }
            args.extend(skip_args(&skip));
        }
    }
    // cleanup_trip
    if method == "space:cleanup" {
        let facts = fx.cleanup_ref(&snapshot);
        for kept in merge_keep(facts.keep, inputs.worn.clone()) {
            args.push("--keep".to_string());
            args.push(kept);
        }
        if life.profile.ironman {
            args.push("--iron".to_string());
        } else {
            match facts.shop {
                ShopTrip::None => {}
                ShopTrip::Trimmed { lines, args: trip, changed } => {
                    if changed {
                        fx.log_ref(format!("brain {account}: shop trip trimmed to {lines} lot(s): the graph needs the others"));
                    }
                    args.extend(trip);
                }
                ShopTrip::Dropped => fx.log_ref(format!("brain {account}: shop trip dropped: the graph needs every lot")),
            }
        }
    }
    // script_known
    if !crate::registry::script_known(&script) {
        let id = life.nodes[launch].id.clone();
        fx.log_ref(format!("brain {account}: no script for {method} -> {script}, abandoning {id}"));
        fx.push(Ev::ReleaseRun(account.clone(), id.clone()));
        lifescape_planner::record_contract_error(life, launch, &format!("no-script: {method} -> {script}"));
        push_history(life, &id, "abandoned", now);
        return Save::Silent;
    }
    // run_job
    fx.push(Ev::ClaimLaunch { node: format!("{snapshot:?}"), method: method.clone(), budget });
    args.push(crate::report_meta::METHOD_FLAG.to_string());
    args.push(method.clone());
    fx.log_ref(format!("brain {account}: launching {script} {}", args.join(" ")));
    lifescape_planner::mark_launched(life, launch);
    let launched_id = life.nodes[launch].id.clone();
    push_history(life, &launched_id, "launched", now);
    fx.push(Ev::NoteLaunch);
    fx.push(Ev::Save(Save::Always));
    fx.push(Ev::RunJob { script, args, node: launched_id });
    Save::None
}
