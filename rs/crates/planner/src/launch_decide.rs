#![forbid(unsafe_code)]
//! The launch decision (lot 3 of the launch refactor): given the life, the
//! picked node, its resolve context and the typed inputs, WHAT the brain
//! does with the node, as data. Pure: no I/O, no global, no mutation of
//! its arguments (the resolve runs on a copy of the node and of the
//! context). The orchestrator gathers the inputs, calls `decide`, performs
//! the external effects the decision asks for (market calls, listings
//! withdrawn, the worker run, logs, the save) and `launch_apply` writes the
//! life changes.
//!
//! The decision is a sequence of stages, the order the node steps and the
//! dispatch steps always had:
//! 1. already held, gear step withdrawn, coin target, coin cycle;
//! 2. resolve, steer off bans, the guide pick and the trade lot recorded;
//! 3. blocked at resolve, no method, Iron, input gate, food gate;
//! 4. dispatch: budget, market buy, script, crowd note and standing bid,
//!    loot left lying, cleanup keep list, script registered.
//!
//! A fact only the orchestrator can read (listings to withdraw, the food a
//! wallet affords, loot values, the cleanup keep sources, fresh market
//! counts) is asked as a `Need` or a `Withdraw*` decision: the caller
//! answers in `Known` and calls `decide` again. The decision is
//! deterministic, so the second call retraces the first; `Known::resumed`
//! carries the resolve across the calls (it is never run twice) and every
//! `Decided::emits` is a prefix of the next one (the caller prints only
//! what it has not printed yet).

use std::borrow::Cow;

use crate::expand::ReadyCtx;
use crate::launch_inputs::LaunchInputs;
use crate::launch_leaf::{coin_need_lift, coin_need_met, fight_spot, held_verdict, loot_skip, merge_keep, skip_args, HeldVerdict};
use crate::trade_craft::TradeLot;
use crate::{Engagement, GoalKind, Life, LifeMemory, Node, NodeKind, NodeStatus};

/// A site: `(x, z, level)`.
pub type Site = (i64, i64, i64);

/// Pure rules that live above the planner (the scripts crate, the script
/// registry): plain function pointers, the orchestrator hands the real
/// ones, a test its own.
#[derive(Clone, Copy)]
pub struct Rules {
    /// Inputs of one action of `method` the holdings do not cover
    /// (`brain_input_gate::short_inputs`).
    pub short_inputs: fn(&str, bool, &dyn Fn(&str) -> i64) -> Vec<String>,
    /// Units of `name` one action of `method` consumes (`>= 1`).
    pub input_each: fn(&str, &str) -> i64,
    /// The script is registered (`registry::script_known`).
    pub script_known: fn(&str) -> bool,
}

/// Something that happens at a point of the decision, in order: a log
/// line, or an external effect the orchestrator performs there.
#[derive(Debug, Clone, PartialEq)]
pub enum Emit {
    Log(String),
    /// Cancel the bot's resting buys of an object (`cancel_open_buys`).
    CancelOpenBuys(String),
    /// Note a gear step withdrawn (`brain_gear::note_withdrawn`).
    NoteGearWithdrawn(String),
    /// The resolve locked a 100 % market lot: post its products on the
    /// order board (`market_claims::claim_run`).
    ClaimLot(TradeLot),
    /// Nothing of the lot a node locked is on its way any more.
    ReleaseRun(String),
    /// The fleet learns the bot works this site by this method.
    CrowdNote { site: Option<Site>, method: String },
    /// Place the obtain's standing bid (`place_standing_bid`).
    StandingBid,
}

/// What `decide` asks the orchestrator to read.
#[derive(Debug, Clone, PartialEq)]
pub enum Need {
    /// The market state (locks, listings, committed coins) read again.
    Recount,
    /// Fresh counts for the input gate.
    Gate,
    /// The food the wallet affords for these needs (`afford_food_needs`).
    Food(Vec<String>),
    /// Reference values of the spot's drops and the bot's coin rate.
    Loot(String),
    /// The cleanup keep sources and the shop trip plan.
    Cleanup,
}

/// How the shop trip of a cleanup rides.
#[derive(Debug, Clone, PartialEq)]
pub enum ShopTrip {
    /// No lots planned (or Iron: no trip).
    None,
    /// The plan, trimmed to what the graph does not need.
    Trimmed { lines: usize, args: Vec<String>, changed: bool },
    /// Every lot is needed by the graph.
    Dropped,
}

/// The facts a cleanup launch reads (`Need::Cleanup`).
#[derive(Debug, Clone, PartialEq)]
pub struct CleanupFacts {
    /// The graph, tool, ladder, alchemy, maker slice and promised names
    /// (merged with the worn names by `merge_keep`).
    pub keep: Vec<String>,
    pub shop: ShopTrip,
}

/// The facts a fight launch reads (`Need::Loot`).
#[derive(Debug, Clone, PartialEq)]
pub struct LootFacts {
    /// The bot's coin rate (`brain_d17::coin_rate`).
    pub rate: f64,
    /// Public value of each drop of the spot (`loot_candidates`).
    pub values: Vec<(String, i64)>,
}

impl LootFacts {
    fn value_of(&self, name: &str) -> i64 {
        self.values.iter().find(|(known, _)| known == name).map_or(0, |(_, value)| *value)
    }
}

/// The resolve prelude and what it carries across calls.
#[derive(Debug, Clone, PartialEq)]
pub struct Boundary {
    /// The node after the resolve and the ban steering.
    pub node: Node,
    /// The memory after the pick and the lot were recorded (`None`: unchanged).
    pub memory: Option<LifeMemory>,
    /// Everything emitted up to here.
    pub emits: Vec<Emit>,
    /// The gear step's coin share set on the context (`None`: untouched).
    pub coin_share: Option<Option<i64>>,
}

/// The input gate withdrew listings: how many closed, of which inputs.
#[derive(Debug, Clone, PartialEq)]
pub struct GateRelease {
    pub withdrawn: usize,
    pub items: Vec<String>,
}

/// What the caller already answered.
#[derive(Debug, Clone, Default)]
pub struct Known {
    pub recounted: bool,
    /// Listings withdrawn for the held obtain (the count).
    pub released: Option<usize>,
    pub gate_fresh: bool,
    /// Listings withdrawn by the input gate.
    pub gate_released: Option<GateRelease>,
    pub food: Option<Vec<String>>,
    pub loot: Option<LootFacts>,
    pub cleanup: Option<CleanupFacts>,
    pub resumed: Option<Boundary>,
}

/// Profiling hook (`cpu_profile::timed` on the orchestrator side).
pub type Timed<'a> = &'a dyn Fn(&str, &mut dyn FnMut());

/// Everything `decide` reads.
pub struct Decide<'a> {
    pub life: &'a Life,
    /// Index of the picked node in `life.nodes`.
    pub position: usize,
    /// The resolve context the brain built for the node.
    pub ctx: &'a ReadyCtx,
    /// Coins the bot's other resting buys commit (obtain nodes only).
    pub committed: i64,
    pub inputs: &'a LaunchInputs,
    pub rules: &'a Rules,
    pub known: &'a Known,
    pub timed: Timed<'a>,
}

/// Why a node is abandoned here.
#[derive(Debug, Clone, PartialEq)]
pub enum AbandonKind {
    /// The resolve blocked on nothing the graph can file.
    ResolveBlocked(String),
    /// The resolve left no method.
    NoMethod,
}

/// Where a wait block comes from (its log line).
#[derive(Debug, Clone, PartialEq)]
pub enum BlockStage {
    Resolve(String),
    Inputs,
    Food,
}

/// Every outcome of the node and dispatch steps.
#[derive(Debug, Clone, PartialEq)]
pub enum LaunchDecision {
    /// Read this, then decide again.
    Need(Need),
    /// Withdraw the bot's own listings of these items (held obtain), then decide again.
    WithdrawOwnListings { items: Vec<String> },
    /// Withdraw the listings of these production inputs, then decide again.
    WithdrawInputListings { items: Vec<String> },
    /// The holdings already meet the obtain.
    AlreadyHeld { item: String, held: i64 },
    /// A ladder purchase with no seller left: the step is withdrawn.
    GearStepWithdrawn { item: Option<String> },
    CoinTargetMet { amount: i64, coins: i64 },
    /// A coin node under a blocked one: a cycle through the coins.
    CoinCycle { regress: Vec<usize> },
    /// The steering found no workable method: the node waits, uncounted.
    WaitUncounted { reason: String },
    /// Every candidate method of the resolve is cooled or full.
    CooledWait { reason: String },
    Abandon(AbandonKind),
    /// File the missing needs as sub-tasks.
    Blocked { stage: BlockStage, missing: Vec<String> },
    RefuseIron { method: String },
    /// A market buy: the brain places the bid, the node waits for the fill.
    PlaceMarketBuy { held: i64 },
    /// No executable for the method (`script`: mapped, not registered).
    NoScript { method: String, script: Option<String> },
    /// Queue the script on the worker.
    RunScript { method: String, script: String, args: Vec<String> },
}

/// The dispatch plan of a resolved node (engagement, progress, what the
/// script is launched on).
#[derive(Debug, Clone, PartialEq)]
pub struct DispatchPlan {
    pub engagement: Option<Engagement>,
    /// The held units ride as an obtain's progress.
    pub progress: Option<i64>,
    /// The node as the launch sees it (after the progress).
    pub snapshot: Node,
    pub budget: Option<(u64, u64)>,
}

/// How the life is saved once the decision is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchSave {
    None,
    Always,
    /// Saved, an error never logged.
    Silent,
}

/// The decision with what leads to it.
#[derive(Debug, Clone, PartialEq)]
pub struct Decided {
    pub position: usize,
    /// The gear step's coin share, to set on the context for the effects.
    pub coin_share: Option<Option<i64>>,
    /// The resolve prelude (`None` before the resolve).
    pub boundary: Option<Boundary>,
    /// The dispatch plan (`None` before the dispatch).
    pub plan: Option<DispatchPlan>,
    /// Logs and effects before the outcome, in order (a prefix of the
    /// next call's).
    pub emits: Vec<Emit>,
    pub decision: LaunchDecision,
}

impl Decided {
    /// True when the caller has to read something and decide again.
    pub fn asks(&self) -> bool {
        matches!(self.decision, LaunchDecision::Need(_) | LaunchDecision::WithdrawOwnListings { .. } | LaunchDecision::WithdrawInputListings { .. })
    }

    /// How the life is saved after the outcome (a launch is saved by the
    /// caller itself, before the worker call).
    pub fn save(&self) -> LaunchSave {
        match &self.decision {
            LaunchDecision::Need(_) | LaunchDecision::WithdrawOwnListings { .. } | LaunchDecision::WithdrawInputListings { .. } => LaunchSave::None,
            LaunchDecision::RefuseIron { .. } | LaunchDecision::NoScript { .. } => LaunchSave::Silent,
            _ => LaunchSave::Always,
        }
    }
}

struct Draft {
    position: usize,
    coin_share: Option<Option<i64>>,
    boundary: Option<Boundary>,
    plan: Option<DispatchPlan>,
    emits: Vec<Emit>,
}

impl Draft {
    fn end(self, decision: LaunchDecision) -> Decided {
        Decided { position: self.position, coin_share: self.coin_share, boundary: self.boundary, plan: self.plan, emits: self.emits, decision }
    }
}

/// The context the effects and the later stages see: the original, or a
/// copy carrying the gear step's coin share.
pub fn patched_ctx(ctx: &ReadyCtx, coin_share: Option<Option<i64>>) -> Cow<'_, ReadyCtx> {
    match coin_share {
        Some(share) => {
            let mut copy = ctx.clone();
            copy.coin_share_pct = share;
            Cow::Owned(copy)
        }
        None => Cow::Borrowed(ctx),
    }
}

/// Decides what the brain does with the picked node (module doc).
pub fn decide(d: &Decide<'_>) -> Decided {
    let mut draft = Draft { position: d.position, coin_share: None, boundary: None, plan: None, emits: Vec::new() };
    if let Some(boundary) = &d.known.resumed {
        draft.coin_share = boundary.coin_share;
        draft.emits = boundary.emits.clone();
        draft.boundary = Some(boundary.clone());
        let ctx = patched_ctx(d.ctx, boundary.coin_share);
        let node = boundary.node.clone();
        return after_resolve(d, draft, &ctx, node);
    }
    before_resolve(d, draft)
}

fn before_resolve(d: &Decide<'_>, mut draft: Draft) -> Decided {
    let (life, position, ctx, inputs) = (d.life, d.position, d.ctx, d.inputs);
    let account = &life.account;
    // An obtain the holdings already meet closes without a launch
    // (positive proof: pack, bank, worn), like a met coin target below.
    if let NodeKind::ObtainItems { item, count } = &life.nodes[position].kind {
        // Only the copies free of the bot's own listings count (P2-7).
        let exact = crate::gear_ladder::is_gear_obtain(life, position) || matches!(&life.goal, GoalKind::Produce { item: made, .. } if made == item);
        let mut free = if ctx.held < *count {
            0
        } else {
            // lot 3: the publishes before may have moved the market state.
            if !d.known.recounted {
                return draft.end(LaunchDecision::Need(Need::Recount));
            }
            inputs.free_now(item, exact)
        };
        // Once the listings were withdrawn, the first verdict (on the counts
        // read before) was `WithdrawListings`, whatever the fresh counts say.
        let mut verdict = if d.known.released.is_some() { HeldVerdict::WithdrawListings } else { held_verdict(*count, ctx.held, free) };
        if verdict == HeldVerdict::WithdrawListings {
            let Some(withdrawn) = d.known.released else {
                return draft.end(LaunchDecision::WithdrawOwnListings { items: vec![item.clone()] });
            };
            // A copy still in a deal stays locked: the obtain runs.
            // lot 3: `release_own_locks` just withdrew listings and cancelled
            // locks; the free count is read again from the market.
            free = inputs.free_now(item, exact);
            draft.emits.push(Emit::Log(format!(
                "brain {account}: {} holds {} {item} but its listings locked them: {withdrawn} listing(s) withdrawn, {free} free now",
                life.nodes[position].id, ctx.held
            )));
            verdict = held_verdict(*count, ctx.held, free);
        }
        if verdict == HeldVerdict::Done {
            return draft.end(LaunchDecision::AlreadyHeld { item: item.clone(), held: ctx.held });
        }
    }
    // A ladder purchase spends its wallet share only; with no seller
    // left and a wallet that can bid, the step withdraws and the standing
    // bid waits for a producer (never a worker held on an empty book).
    let mut patched: Option<ReadyCtx> = None;
    if crate::gear_ladder::is_gear_obtain(life, position) {
        let gear_item = match &life.nodes[position].kind {
            NodeKind::ObtainItems { item, .. } => Some(item.clone()),
            _ => None,
        };
        let mut gear_ctx = ctx.clone();
        // A kit tool spends the free wallet its rule checked (user
        // 2026-11-17: a funded axe earns its price, never twice it); other
        // pieces the gear plan's half.
        gear_ctx.coin_share_pct = gear_item.as_deref().map_or(Some(crate::GEAR_SHARE_PCT), crate::tool_kit::step_coin_share);
        draft.coin_share = Some(gear_ctx.coin_share_pct);
        // A shop-only piece (D-A) never has a market seller: the step walks
        // to the shop, it is never withdrawn for an empty book; nor is a
        // kit tool the bot makes itself (its production leg is listed).
        let shop_piece = gear_item.as_deref().is_some_and(lifescape_gamedata::obtain::kit_shop_route);
        let made_piece = life.nodes[position].alternatives.iter().any(|alt| !alt.method.starts_with("market:") && !alt.method.starts_with("shop:"));
        if !shop_piece && !made_piece && crate::gear_ladder::withdraw_due(&gear_ctx) {
            return draft.end(LaunchDecision::GearStepWithdrawn { item: gear_item });
        }
        patched = Some(gear_ctx);
    }
    let ctx: &ReadyCtx = patched.as_ref().unwrap_or(ctx);
    // A coin target already met closes without a launch (PRD3 lot 4).
    if let NodeKind::HaveCoins { amount } = life.nodes[position].kind {
        if ctx.coins >= amount {
            return draft.end(LaunchDecision::CoinTargetMet { amount, coins: ctx.coins });
        }
    }
    // Earning coins never waits on earning coins: a coin node under a
    // blocked one is a cycle through the coins (graphs unrolled before
    // the breaker, live jim_ellis): the outermost takes over, logged.
    let regress = crate::lifecycle_cycle::coin_regress(life, position);
    if !regress.is_empty() {
        return draft.end(LaunchDecision::CoinCycle { regress });
    }
    resolve(d, draft, ctx)
}

/// Resolve the node's method against the context, steer a money node off
/// the bans, record the guide pick and the trade lot.
fn resolve(d: &Decide<'_>, mut draft: Draft, ctx: &ReadyCtx) -> Decided {
    let (life, position, inputs) = (d.life, d.position, d.inputs);
    let account = &life.account;
    // The resolve runs on copies: `drawn` is the only thing it sets on the
    // context, the node is the only thing it changes in the life.
    let rctx = ctx.clone();
    let mut node = life.nodes[position].clone();
    (d.timed)("resolve", &mut || crate::resolve_ready(&mut node, &rctx));
    draft.emits.push(Emit::Log(format!("brain {account}: resolve {} -> {:?} (ctx coins={})", node.id, node.method, ctx.coins)));
    // A money or coin node never works a banned skill (D-SIDE: a side
    // money goal, a coin target): the next ranked method off the bans.
    if matches!(node.kind, NodeKind::MakeMoney | NodeKind::HaveCoins { .. }) {
        let banned = crate::banned_skills(&life.history, &|stamp: i64| inputs.ticks_of(inputs.now - stamp));
        let money_id = node.id.clone();
        // Every workable method banned: the best ranked one works through
        // its ban, never an idle bot (2026-11-14).
        match crate::steer_for_work(&mut node, &rctx, &banned, &[]) {
            Ok(crate::WorkSteer::OffBans(true)) => {
                draft.emits.push(Emit::Log(format!("brain {account}: {money_id} steered off banned {} -> {:?}", banned.join(","), node.method)));
            }
            Ok(crate::WorkSteer::OffBans(false)) => {}
            Ok(crate::WorkSteer::ThroughBan(skill)) => draft.emits.push(Emit::Log(format!(
                "brain {account}: {money_id} works through the {skill} ban (every workable money method is banned) -> {:?}",
                node.method
            ))),
            Err(reason) => {
                draft.emits.push(Emit::Log(format!("brain {account}: {money_id} waits: {reason}")));
                draft.boundary = Some(Boundary { node, memory: None, emits: Vec::new(), coin_share: draft.coin_share });
                return draft.end(LaunchDecision::WaitUncounted { reason });
            }
        }
    }
    // The choice drawn from a guide (R7), logged and kept in memory; a
    // 100 % market lot locked by the resolve is recorded (R5): its
    // purchases, cost and predicted profit, before the block buys them.
    let mut memory = life.memory.clone();
    let mut changed = crate::launch_note::record_pick(&mut memory, account, &node, &rctx, inputs.now, &mut draft.emits);
    changed |= crate::launch_note::note_lot(&mut memory, account, &node, &rctx, inputs.now, &mut draft.emits);
    let boundary = Boundary { node: node.clone(), memory: changed.then_some(memory), emits: draft.emits.clone(), coin_share: draft.coin_share };
    draft.boundary = Some(boundary);
    after_resolve(d, draft, ctx, node)
}

/// The checks after the resolve, then the dispatch.
fn after_resolve(d: &Decide<'_>, mut draft: Draft, ctx: &ReadyCtx, node: Node) -> Decided {
    let (life, inputs, rules) = (d.life, d.inputs, d.rules);
    let account = &life.account;
    let id = node.id.clone();
    // The planner blocks typed when no method is workable (tool to
    // fund): the block becomes an obtain sub-task now, never a node that
    // flips Ready/Blocked forever without launching.
    if let NodeStatus::Blocked { reason } = &node.status {
        // A priced training pick blocks on everything it lacks at once
        // (`missing:hammer,bronze_bar`).
        let missing: Vec<String> = reason
            .strip_prefix("missing:")
            .map(|needs| needs.split(',').filter(|need| !coin_need_met(need, ctx.coins)).map(|need| coin_need_lift(need, d.committed)).collect())
            .unwrap_or_default();
        // Every site of every candidate full or cooled (tour 3 point 1):
        // the method cools, the node stays open (next method, D6).
        if crate::cooldown::resolve_wait(reason).is_some() {
            return draft.end(LaunchDecision::CooledWait { reason: reason.clone() });
        }
        return if missing.is_empty() {
            draft.end(LaunchDecision::Abandon(AbandonKind::ResolveBlocked(reason.clone())))
        } else {
            draft.end(LaunchDecision::Blocked { stage: BlockStage::Resolve(reason.clone()), missing })
        };
    }
    // The resolved method; none abandons the node (a redirect target
    // without a method waits a pass: the gate re-carries it).
    let method = node.method.clone().unwrap_or_default();
    if method.is_empty() {
        return draft.end(LaunchDecision::Abandon(AbandonKind::NoMethod));
    }
    // Combat requirements are graph dependencies now: the site pick in
    // `resolve_ready` blocked typed (`missing:combat:N`) above, and the
    // block filed a `reach_combat` node the node depends on.
    // Iron never trades (D15): a stale market lock from an old file resets
    // instead of launching; the planner never locks one on fresh resolves.
    if !crate::launch_allowed(&method, life.profile.ironman) {
        draft.emits.push(Emit::Log(format!("brain {account}: refusing {method} for Ironman")));
        return draft.end(LaunchDecision::RefuseIron { method });
    }
    // A production method whose inputs the holdings do not cover files
    // its obtain sub-tasks now: no worker, no 3-tick blocked run.
    // lot 3: the gate withdraws listings between its reads and counts
    // again after; its counts are the fresh market state, never the one
    // the pass started with.
    let free = |name: &str| inputs.held(name);
    let listed = |name: &str| inputs.listed_units(name);
    let total = |name: &str| inputs.raw_units(name);
    // The gate reads fresh counts (`Need::Gate`) whenever the method is
    // gated at all: nothing held would be short.
    let gated = !(rules.short_inputs)(&method, ctx.bank_known, &|_| 0).is_empty();
    if gated && !d.known.gate_fresh {
        return draft.end(LaunchDecision::Need(Need::Gate));
    }
    if let Some(release) = &d.known.gate_released {
        // The listings of these inputs were withdrawn (`release.withdrawn`
        // closed). A released input launches only when, once the listings
        // are gone, it reads free: a lock a deal in flight keeps (or units
        // sold away) files the plain need.
        let still_short = if release.withdrawn == 0 { release.items.clone() } else { (rules.short_inputs)(&method, ctx.bank_known, &free) };
        if !still_short.is_empty() {
            return draft.end(LaunchDecision::Blocked { stage: BlockStage::Inputs, missing: still_short });
        }
        draft.emits.push(Emit::Log(format!("brain {account}: {id} inputs listed on the market ({}): listings withdrawn, launching", release.items.join(", "))));
    } else {
        let short = if gated { (rules.short_inputs)(&method, ctx.bank_known, &free) } else { Vec::new() };
        if !short.is_empty() {
            let each_of = |name: &str| (rules.input_each)(&method, name).max(1);
            let (listed_short, plain): (Vec<String>, Vec<String>) = short.into_iter().partition(|name| listed(name) > 0 && total(name) >= each_of(name));
            if !plain.is_empty() {
                return draft.end(LaunchDecision::Blocked { stage: BlockStage::Inputs, missing: plain });
            }
            return draft.end(LaunchDecision::WithdrawInputListings { items: listed_short });
        }
    }
    // A fight the bot cannot feed files its food obtain now (one load
    // shared by every sibling node), no worker, no 11-tick blocked run.
    let needed: Vec<String> = crate::food_gate::food_short(&method, ctx).into_iter().collect();
    if !needed.is_empty() {
        let Some(missing) = d.known.food.clone() else { return draft.end(LaunchDecision::Need(Need::Food(needed))) };
        return draft.end(LaunchDecision::Blocked { stage: BlockStage::Food, missing });
    }
    dispatch(d, draft, ctx, node, method)
}

/// The dispatch of the resolved node: budget, market buy, the script.
fn dispatch(d: &Decide<'_>, mut draft: Draft, ctx: &ReadyCtx, node: Node, method: String) -> Decided {
    let (life, inputs, rules) = (d.life, d.inputs, d.rules);
    // Engagement: money draws [6000, 18000] at the activity start;
    // reach boxes at one window and retries; farm obtains work one
    // window then recheck holdings; cleanup works one window per pass.
    // The held units ride as the obtain's progress.
    let mut engagement = None;
    let budget: Option<(u64, u64)> = match &node.kind {
        // The alchemist's money funds its buys between alchemies: one
        // filler window, then the draw looks at the stock again.
        NodeKind::MakeMoney if crate::is_alcher(life) => {
            let window = inputs.filler_ticks;
            engagement = Some(Engagement { node: node.id.clone(), min_ticks: window, max_ticks: window, chosen: window });
            Some((window, window))
        }
        NodeKind::MakeMoney => {
            let chosen = crate::draw_engagement_ticks(life.seed, life.history.len() as u64);
            let engaged = Engagement { node: node.id.clone(), min_ticks: 6000, max_ticks: chosen.max(6000), chosen: chosen.max(6000) };
            let max = engaged.max_ticks;
            engagement = Some(engaged);
            Some((6000, max))
        }
        NodeKind::ReachLevel { .. } | NodeKind::ReachCombat { .. } => Some((0, 18000)),
        // Every production leg of an obtain works one window, then the
        // holdings recheck (PRD3: smith, spin, mill… not only farm).
        NodeKind::ObtainItems { .. } if !method.starts_with("market:") && !method.starts_with("shop:") => Some((6000, 6000)),
        NodeKind::HaveCoins { .. } => Some((6000, 6000)),
        NodeKind::SpaceCleanup => Some((0, 6000)),
        _ => None,
    };
    let mut snapshot = node;
    let progress = matches!(snapshot.kind, NodeKind::ObtainItems { .. }).then_some(ctx.held);
    if let Some(held) = progress {
        crate::lifecycle::note_progress(&mut snapshot, held);
    }
    draft.plan = Some(DispatchPlan { engagement, progress, snapshot: snapshot.clone(), budget });
    // A market buy needs no worker (PRD3 lot 4): the brain places the
    // bid on the daemon market and the node waits for the fill.
    if method == "market:buy" {
        return draft.end(LaunchDecision::PlaceMarketBuy { held: ctx.held });
    }
    let Some((script, mut args)) = crate::map_method(&method, &snapshot, budget) else {
        // No executable for this method: typed failure, never a silent
        // spin — the dashboard shows it and the 3-strike rule can
        // abandon it (replan on new scripts, never ghost-retry).
        return draft.end(LaunchDecision::NoScript { method, script: None });
    };
    // The site's crowd note and the obtain's standing bid.
    draft.emits.push(Emit::CrowdNote { site: snapshot.site, method: method.clone() });
    if matches!(snapshot.kind, NodeKind::ObtainItems { .. }) && !method.starts_with("shop:") {
        draft.emits.push(Emit::StandingBid);
    }
    // M5: a fighter leaves lying the drops not worth their pickup ticks
    // at its own coin rate (public values: traded price or shop value).
    if let Some(spot) = fight_spot(&method) {
        if inputs.market_up && !life.profile.ironman {
            let Some(facts) = d.known.loot.clone() else { return draft.end(LaunchDecision::Need(Need::Loot(spot.to_string()))) };
            // The bot's time value (D1): what its work really earns.
            let skip = loot_skip(spot, &|name| facts.value_of(name), facts.rate);
            if !skip.is_empty() {
                draft.emits.push(Emit::Log(format!(
                    "brain {}: leaving {} lying at {spot} (under {} ticks at {:.2} coins/tick)",
                    life.account,
                    skip.join(","),
                    crate::launch_leaf::PICKUP_TICKS,
                    facts.rate
                )));
            }
            args.extend(skip_args(&skip));
        }
    }
    // Cleanup carries its keep-list (graph obtains, quest checks, the
    // permanent tool kit and the kept combat set: never sold nor dropped,
    // 2026-11-14) and the Iron flag (D2 direct, no carnet) to the worker;
    // coins, food and best tools resolve worker-side.
    if method == "space:cleanup" {
        let Some(facts) = d.known.cleanup.clone() else { return draft.end(LaunchDecision::Need(Need::Cleanup)) };
        for kept in merge_keep(facts.keep, inputs.worn.clone()) {
            args.push("--keep".to_string());
            args.push(kept);
        }
        if life.profile.ironman {
            args.push("--iron".to_string());
        } else {
            // Q3: the shop trip first (never a unit under the market). The
            // lots were planned at the listing pass: what the graph needs
            // at launch (a node drawn since) leaves the trip (live
            // 2026-11-22, owen_wood: 7 680 iron ore offered, its smelt
            // blocked `missing: iron_ore`).
            match facts.shop {
                ShopTrip::None => {}
                ShopTrip::Trimmed { lines, args: trip, changed } => {
                    if changed {
                        draft.emits.push(Emit::Log(format!("brain {}: shop trip trimmed to {lines} lot(s): the graph needs the others", life.account)));
                    }
                    args.extend(trip);
                }
                ShopTrip::Dropped => draft.emits.push(Emit::Log(format!("brain {}: shop trip dropped: the graph needs every lot", life.account))),
            }
        }
    }
    // The script is registered: else the same typed failure, the script
    // name riding in the error.
    if !(rules.script_known)(&script) {
        // The method is wired but its P3 script has not landed yet:
        // same typed failure, the script name rides in the error.
        return draft.end(LaunchDecision::NoScript { method, script: Some(script) });
    }
    draft.end(LaunchDecision::RunScript { method, script, args })
}

#[cfg(test)]
#[path = "launch_decide_tests.rs"]
mod tests;
