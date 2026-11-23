#![forbid(unsafe_code)]
//! Test-only reference of the life upkeep and the graph state (lot 4 of
//! the launch refactor): the rules as they were before their decisions
//! moved into the planner (`upkeep*`), copied rule by rule over a recording
//! fake of the world (`RecUpkeep`, which also implements `UpkeepFx`, so the
//! real drive runs on the same fake). `brain_upkeep_equiv_tests` runs both
//! on the same scenarios and compares what they decide, perform, print
//! and write. The only edits of the copy: the clock and every external
//! call go through the fake (`fx.clock()`, `fx.…`), the history entries are
//! stamped with that clock, and the bodies of `spell`, `goal_met` and
//! `stuck_summary` (now planner leaves) are kept here as they were.

use std::any::Any;
use std::cell::RefCell;

use lifescape_planner::ambition::{Ambition, BotNow, Stamp};
use lifescape_planner::launch_apply::push_history;
use lifescape_planner::serve_value::ServeVerdict;
use lifescape_planner::upkeep::{Clock, ServeFacts, ServeQuery};
use lifescape_planner::upkeep_ambition::RebalanceRow;
use lifescape_planner::upkeep_graph::{RescueReader, RouteFacts};
use lifescape_planner::upkeep_role::OrderFact;
use lifescape_planner::{GoalKind, GraphState, HistoryEntry, Life, Node, NodeKind, MAX_REVIVES, REVIVE_COOLDOWN_TICKS};

use crate::brain_launch_pass::{Flow, Save};
use crate::brain_upkeep_fx::UpkeepFx;

/// What the fake answers where the real world would read or decide.
#[derive(Debug, Clone)]
pub(crate) struct World {
    pub clock: Clock,
    pub levels: Vec<(String, u8)>,
    pub coins: i64,
    pub held: Vec<(String, i64)>,
    /// The operator's filed kind, taken once.
    pub pending: Option<String>,
    pub stamp: Stamp,
    /// The ambition giving steps succeed.
    pub assign_ok: bool,
    /// The goal draws find a goal.
    pub draw_ok: bool,
    /// A retried draw that finds nothing still changed the life.
    pub redraw_changed: bool,
    pub orders: Option<Vec<OrderFact>>,
    pub cancel_ok: bool,
    pub slice: Vec<i64>,
    /// The ambition `ensure` gives to a life without one.
    pub ambition: Option<Ambition>,
    pub rebalance: Option<RebalanceRow>,
    pub surplus: Vec<(String, f64)>,
    pub serve: Option<ServeFacts>,
    /// Verdict by demand item.
    pub verdicts: Vec<(String, ServeVerdict)>,
    pub reengage: bool,
    /// Market facts by node id (default: no route).
    pub routes: Vec<(String, RouteFacts)>,
}

/// One observable event: a line, a phase, a read, an effect.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Ev {
    Log(String),
    Timed(&'static str),
    Read(&'static str),
    Fx(String),
}

pub(crate) struct RecUpkeep {
    pub world: RefCell<World>,
    pub trace: RefCell<Vec<Ev>>,
}

impl RecUpkeep {
    pub(crate) fn new(world: World) -> Self {
        RecUpkeep { world: RefCell::new(world), trace: RefCell::new(Vec::new()) }
    }

    fn push(&self, event: Ev) {
        self.trace.borrow_mut().push(event);
    }

    fn fx(&self, text: String) {
        self.push(Ev::Fx(text));
    }

    fn drawn(&self, life: &mut Life) -> Option<String> {
        let now = self.world.borrow().clock.now;
        if self.world.borrow().draw_ok {
            push_history(life, "goal", "next:fake goal", now);
            Some("fake goal".to_string())
        } else {
            None
        }
    }
}

impl RescueReader for RecUpkeep {
    fn wallet(&self) -> i64 {
        self.push(Ev::Read("wallet"));
        self.world.borrow().coins
    }

    fn facts(&self, _life: &Life, node: &Node) -> RouteFacts {
        self.push(Ev::Read("route facts"));
        self.world.borrow().routes.iter().find(|(id, _)| *id == node.id).map(|(_, facts)| *facts).unwrap_or(RouteFacts { best_ask: None, price: 1, held: 0, committed: 0 })
    }
}

impl UpkeepFx for RecUpkeep {
    fn log(&self, line: &str) {
        self.push(Ev::Log(line.to_string()));
    }

    fn timed<R>(&self, label: &'static str, body: impl FnOnce() -> R) -> R {
        self.push(Ev::Timed(label));
        body()
    }

    fn clock(&self) -> Clock {
        self.world.borrow().clock
    }

    fn level_of(&self, skill: &str) -> u8 {
        self.world.borrow().levels.iter().find(|(name, _)| name == skill).map(|(_, level)| *level).unwrap_or(1)
    }

    fn with_now(&self, body: &mut dyn FnMut(&BotNow)) {
        let world = self.world.borrow().clone();
        let level_of = |skill: &str| world.levels.iter().find(|(name, _)| name == skill).map(|(_, level)| *level).unwrap_or(1);
        let held = |item: &str| world.held.iter().find(|(name, _)| name == item).map(|(_, count)| *count).unwrap_or(0);
        body(&BotNow { level_of: &level_of, coins: world.coins, held: &held });
    }

    fn stamp(&self) -> Stamp {
        self.world.borrow().stamp
    }

    fn take_pending(&self, account: &str) -> Option<String> {
        let kind = self.world.borrow_mut().pending.take();
        self.fx(format!("take pending {account}: {kind:?}"));
        kind
    }

    fn assign_natural(&self, draft: &mut Life) -> bool {
        let ok = self.world.borrow().assign_ok;
        self.fx(format!("assign natural {ok}"));
        if ok {
            let now = self.world.borrow().clock.now;
            push_history(draft, "goal", "fake:assigned natural", now);
            if let Some(held) = draft.ambition.as_mut() {
                held.why = "fake natural".to_string();
                // A real draw marks the ambition it gives (`mark_rule`, lot 5).
                held.draw_rule = "need".to_string();
            }
        }
        ok
    }

    fn assign_kind(&self, draft: &mut Life, kind: &str, why: &str) -> bool {
        let ok = self.world.borrow().assign_ok;
        self.fx(format!("assign kind {kind} {why} {ok}"));
        if ok {
            let now = self.world.borrow().clock.now;
            push_history(draft, "goal", &format!("fake:assigned {kind}"), now);
            if let Some(held) = draft.ambition.as_mut() {
                held.why = why.to_string();
                held.draw_rule = "operator".to_string();
            }
        }
        ok
    }

    fn succeed_goal(&self, life: &mut Life, stuck: bool) -> Option<String> {
        self.fx(format!("succeed goal stuck={stuck}"));
        let now = self.world.borrow().clock.now;
        push_history(life, "goal", &format!("done:fake {stuck}"), now);
        self.drawn(life)
    }

    fn close_goal(&self, life: &mut Life, stuck: bool) {
        self.fx(format!("close goal stuck={stuck}"));
        let now = self.world.borrow().clock.now;
        push_history(life, "goal", &format!("done:fake {stuck}"), now);
    }

    fn draw_next(&self, life: &mut Life) -> Option<String> {
        self.fx("draw next".to_string());
        self.drawn(life)
    }

    fn redraw(&self, life: &mut Life) -> (Option<String>, bool) {
        self.fx("redraw".to_string());
        if !lifescape_planner::goal_closed(&life.history) {
            return (None, false);
        }
        let before = life.history.len();
        let drawn = self.drawn(life);
        let changed = drawn.is_some() || life.history.len() != before || self.world.borrow().redraw_changed;
        (drawn, changed)
    }

    fn redraw_neediest(&self, life: &mut Life, why: &str) {
        self.fx(format!("redraw neediest {why}"));
        let now = self.world.borrow().clock.now;
        push_history(life, "goal", &format!("fake:neediest {why}"), now);
    }

    fn coins(&self) -> i64 {
        self.push(Ev::Read("coins"));
        self.world.borrow().coins
    }

    fn orders(&self) -> Option<Vec<OrderFact>> {
        self.push(Ev::Read("orders"));
        self.world.borrow().orders.clone()
    }

    fn slice_items(&self, slice: u32) -> Vec<i64> {
        self.push(Ev::Read("slice"));
        let _ = slice;
        self.world.borrow().slice.clone()
    }

    fn cancel(&self, order: i64, reason: &str) -> bool {
        let ok = self.world.borrow().cancel_ok;
        self.fx(format!("cancel {order} {reason} {ok}"));
        ok
    }

    fn free_board(&self, account: &str) {
        self.fx(format!("free board {account}"));
    }

    fn ensure(&self, life: &mut Life) -> bool {
        self.fx("ensure".to_string());
        if life.ambition.is_some() {
            return false;
        }
        let given = self.world.borrow().ambition.clone();
        life.ambition = given;
        life.ambition.is_some()
    }

    fn coverage(&self, life: &Life) {
        self.fx(format!("coverage {}", life.ambition.is_some()));
    }

    fn fleet_surplus(&self) -> Vec<(String, f64)> {
        self.push(Ev::Read("surplus"));
        self.world.borrow().surplus.clone()
    }

    fn serve_facts(&self, _life: &Life, query: &ServeQuery) -> Option<ServeFacts> {
        self.push(Ev::Read("serve facts"));
        let _ = query;
        self.world.borrow().serve
    }

    fn judge(&self, _life: &Life, items: &[(String, String)]) -> Vec<(String, ServeVerdict)> {
        self.push(Ev::Read("judge"));
        let world = self.world.borrow();
        items.iter().map(|(id, item)| (id.clone(), world.verdicts.iter().find(|(name, _)| name == item).map(|(_, verdict)| *verdict).unwrap_or(ServeVerdict::NoData))).collect()
    }

    fn reengage(&self, life: &Life) -> bool {
        self.fx("reengage".to_string());
        let wants = lifescape_planner::upkeep_ambition::reengage_candidate(life);
        wants && self.world.borrow().reengage
    }
}

// ---------------------------------------------------------------------
// The rules as they were.
// ---------------------------------------------------------------------

fn now(fx: &RecUpkeep) -> i64 {
    fx.clock().now
}

fn ticks_since(fx: &RecUpkeep, stamp: i64) -> u64 {
    fx.clock().ticks_since(stamp)
}

fn ref_lift_role_bans(fx: &RecUpkeep, life: &mut Life) -> bool {
    let skills = lifescape_planner::role_lifted_bans(&life.history, &|stamp| ticks_since(fx, stamp));
    for skill in &skills {
        push_history(life, "goal", &lifescape_planner::unban_entry(skill), now(fx));
        fx.log(&format!("brain {}: unban:{skill} (an operator role switch caused it)", life.account));
    }
    !skills.is_empty()
}

fn drawable_trade(kind: &str) -> bool {
    lifescape_planner::ambition_catalog::specs().iter().any(|entry| entry.kind == kind) && !lifescape_planner::ambition::ladderless(kind) && kind != lifescape_planner::MARKET_MAKER_KIND
}

pub(crate) fn ref_apply_pending(fx: &RecUpkeep, life: &mut Life) -> bool {
    let lifted = ref_lift_role_bans(fx, life);
    let Some(kind) = fx.take_pending(&life.account) else { return lifted };
    if lifescape_planner::protected(&life.account) {
        fx.log(&format!("brain {}: operator ambition {kind} dropped (protected account, overall for good)", life.account));
        return lifted;
    }
    if kind == "natural" {
        return ref_apply_natural(fx, life) || lifted;
    }
    if kind != lifescape_planner::ALCHER {
        if !drawable_trade(&kind) {
            return lifted;
        }
        let mut draft = life.clone();
        draft.goal_origin = None;
        if !fx.assign_kind(&mut draft, &kind, &format!("operator:{kind}")) {
            fx.log(&format!("brain {}: operator ambition {kind} not given (nothing buildable or refused), life untouched", life.account));
            return lifted;
        }
        *life = draft;
        let abandoned = lifescape_planner::abandon_for_role(life, lifescape_planner::OPERATOR_ROLE_CAUSE);
        push_history(life, "goal", &format!("role:{kind} (operator)"), now(fx));
        fx.log(&format!("brain {}: ambition -> {kind} (operator), {} nodes abandoned", life.account, abandoned.len()));
        if let Some(label) = fx.draw_next(life) {
            fx.log(&format!("brain {}: {kind}'s first goal {label}", life.account));
        }
        lifescape_planner::normalize(life);
        return true;
    }
    let stamp = fx.stamp();
    let mut switched = None;
    fx.with_now(&mut |bot| switched = Some(lifescape_planner::become_alcher(life, bot, stamp)));
    match switched.expect("with_now ran") {
        Ok(abandoned) => {
            push_history(life, "goal", "role:alcher (operator)", now(fx));
            fx.log(&format!("brain {}: ambition -> alcher (operator), {} nodes abandoned: {}", life.account, abandoned.len(), abandoned.join(",")));
            match fx.draw_next(life) {
                Some(label) => fx.log(&format!("brain {}: alcher's first goal {label}", life.account)),
                None => fx.log(&format!("brain {}: alcher drew no goal yet (see the reason above)", life.account)),
            }
            lifescape_planner::normalize(life);
            true
        }
        Err(error) => {
            fx.log(&format!("brain {}: operator ambition alcher refused: {error:?}", life.account));
            lifted
        }
    }
}

fn ref_apply_natural(fx: &RecUpkeep, life: &mut Life) -> bool {
    if lifescape_planner::protected(&life.account) || lifescape_planner::maker_role(life).is_some() {
        fx.log(&format!("brain {}: operator ambition natural refused (protected account or market maker)", life.account));
        return false;
    }
    let mut draft = life.clone();
    if let Some(held) = draft.ambition.as_mut() {
        held.pin = None;
        held.draw_rule.clear();
    }
    draft.goal_origin = None;
    if !fx.assign_natural(&mut draft) {
        fx.log(&format!("brain {}: natural balance not given (no draw or change refused), life untouched", life.account));
        return false;
    }
    *life = draft;
    let abandoned = lifescape_planner::abandon_for_role(life, lifescape_planner::OPERATOR_ROLE_CAUSE);
    push_history(life, "goal", "role:natural (operator)", now(fx));
    fx.log(&format!("brain {}: ambition -> natural balance (operator), {} nodes abandoned", life.account, abandoned.len()));
    if let Some(label) = fx.draw_next(life) {
        fx.log(&format!("brain {}: first goal {label}", life.account));
    }
    lifescape_planner::normalize(life);
    true
}

pub(crate) fn ref_retire(fx: &RecUpkeep, life: &mut Life) -> bool {
    if lifescape_planner::MAKERS_ACTIVE || lifescape_planner::maker_role(life).is_none() {
        return false;
    }
    let mut cancelled = 0;
    if let Some(rows) = fx.orders() {
        for row in rows.iter().filter(|row| row.market_make && row.live) {
            cancelled += usize::from(fx.cancel(row.order_id, crate::market_maker::RETIRED_REASON));
        }
    }
    life.profile.archetype = lifescape_planner::Archetype::Producer;
    let abandoned = lifescape_planner::abandon_for_role(life, lifescape_planner::MAKER_RETIRED_CAUSE);
    life.goal_origin = None;
    fx.free_board(&life.account);
    push_history(life, "goal", "role:market_maker retired", now(fx));
    fx.log(&format!("brain {}: market maker retired ({cancelled} quotes cancelled, {} nodes abandoned), drawing the neediest trade", life.account, abandoned.len()));
    fx.redraw_neediest(life, "market maker retired");
    lifescape_planner::normalize(life);
    true
}

pub(crate) fn ref_maybe_switch(fx: &RecUpkeep, life: &mut Life) -> bool {
    let Some(role) = lifescape_planner::maker_role(life) else { return false };
    let coins = fx.coins();
    if !lifescape_planner::switch_due(&role, coins) {
        return false;
    }
    let slice = fx.slice_items(role.slice);
    if let Some(rows) = fx.orders() {
        for row in rows.iter().filter(|row| row.live) {
            if row.buy || slice.contains(&row.book_item) {
                fx.cancel(row.order_id, crate::market_maker::ROLE_REASON);
            }
        }
    }
    let abandoned = lifescape_planner::become_quoting(life, coins, now(fx));
    fx.free_board(&life.account);
    push_history(life, "goal", &format!("role:market_maker quoting slice {}/{} ({coins} coins >= {})", role.slice, role.slices, role.capital_target), now(fx));
    fx.log(&format!(
        "brain {}: market maker slice {}/{}: capital {coins} >= target {}, quoting for good ({} nodes abandoned)",
        life.account,
        role.slice,
        role.slices,
        role.capital_target,
        abandoned.len()
    ));
    true
}

fn ref_draw_after_give(fx: &RecUpkeep, life: &mut Life, origin: &str) {
    match fx.succeed_goal(life, false) {
        Some(label) => fx.log(&format!("brain {}: next goal {label} (after giving back {origin})", life.account)),
        None => fx.log(&format!("brain {}: nothing drawable after giving back {origin} (see the reason above)", life.account)),
    }
    lifescape_planner::normalize(life);
}

pub(crate) fn ref_release_blocked_bid(fx: &RecUpkeep, life: &mut Life) -> bool {
    let bid = life.goal_origin.as_deref().is_some_and(|origin| origin.starts_with(lifescape_planner::bid_response::BID_PREFIX));
    if !bid || life.active.is_some() {
        return false;
    }
    let cooled = lifescape_planner::cooldown::cooled_keys(&life.history, &|stamp| ticks_since(fx, stamp));
    let Some(blocked) = lifescape_planner::bid_release::blocked_bid(life, &cooled, &|stamp| fx.clock().awake_ticks_since(stamp)) else { return false };
    let abandoned = lifescape_planner::abandon_goal_work(life, crate::brain_bid_release::RELEASE_CAUSE);
    lifescape_planner::normalize(life);
    fx.log(&format!(
        "brain {}: {} given back: {} waited {} ticks on refused sites (>= {}), no ban; {} nodes abandoned: {}",
        life.account,
        blocked.origin,
        blocked.node,
        blocked.waited_ticks,
        lifescape_planner::bid_release::BID_WAIT_RELEASE_TICKS,
        abandoned.len(),
        abandoned.join(",")
    ));
    push_history(life, "goal", &format!("{}{}|{}|{}", lifescape_planner::bid_release::RELEASED_TAG, blocked.origin, blocked.node, blocked.waited_ticks), now(fx));
    ref_draw_after_give(fx, life, &blocked.origin);
    true
}

pub(crate) fn ref_release_off_trade(fx: &RecUpkeep, life: &mut Life) -> bool {
    if life.active.is_some() {
        return false;
    }
    let Some(origin) = lifescape_planner::bid_trade::off_trade_answer(life) else { return false };
    let kind = life.ambition.as_ref().map(|held| held.kind.clone()).unwrap_or_default();
    let abandoned = lifescape_planner::abandon_goal_work(life, crate::brain_bid_release::OFF_TRADE_CAUSE);
    lifescape_planner::normalize(life);
    fx.log(&format!("brain {}: {origin} given back: off the {kind}'s trade, no ban; {} nodes abandoned: {}", life.account, abandoned.len(), abandoned.join(",")));
    push_history(life, "goal", &format!("{}{origin}", crate::brain_bid_release::OFF_TRADE_TAG), now(fx));
    ref_draw_after_give(fx, life, &origin);
    true
}

pub(crate) fn ref_release_beyond_reach(fx: &RecUpkeep, life: &mut Life) -> bool {
    if life.active.is_some() {
        return false;
    }
    let level_of = |skill: &str| fx.level_of(skill);
    let Some(found) = lifescape_planner::bid_reach::beyond_reach_answer(life, &level_of) else { return false };
    let abandoned = lifescape_planner::abandon_goal_work(life, crate::brain_bid_release::BEYOND_REACH_CAUSE);
    lifescape_planner::normalize(life);
    fx.log(&format!(
        "brain {}: {} given back (drawn under the old rule): {}; no ban; {} nodes abandoned: {}",
        life.account,
        found.origin,
        found.why,
        abandoned.len(),
        abandoned.join(",")
    ));
    push_history(life, "goal", &format!("{}{}|{}", crate::brain_bid_release::BEYOND_REACH_TAG, found.origin, found.why), now(fx));
    ref_draw_after_give(fx, life, &found.origin);
    true
}

fn served_item(life: &Life) -> Option<(String, String)> {
    let origin = life.goal_origin.as_deref()?;
    let item = origin.strip_prefix(lifescape_planner::SIDE_PREFIX).and_then(|rest| rest.strip_prefix("demand:"))?;
    match &life.goal {
        GoalKind::Produce { item: goal_item, .. } if goal_item == item => Some((origin.to_string(), item.to_string())),
        _ => None,
    }
}

pub(crate) fn ref_release_unpaying(fx: &RecUpkeep, life: &mut Life) -> bool {
    if life.active.is_some() || life.profile.ironman || lifescape_planner::goal_closed(&life.history) {
        return false;
    }
    let Some((origin, item)) = served_item(life) else { return false };
    let query = ServeQuery { origin: origin.clone(), item };
    let Some(facts) = fx.serve_facts(life, &query) else { return false };
    let ServeFacts { margin, per_tick, time_value } = facts;
    if per_tick <= 0.0 || lifescape_planner::pays_its_way(margin, per_tick, time_value) {
        return false;
    }
    let abandoned = lifescape_planner::abandon_goal_work(life, crate::brain_serve_value::UNPAID_CAUSE);
    lifescape_planner::normalize(life);
    fx.log(&format!(
        "brain {}: {origin} given back: {margin:.0} coins a unit x {per_tick:.5} units a tick < a tick worth {time_value:.2}, no ban; {} nodes abandoned: {}",
        life.account,
        abandoned.len(),
        abandoned.join(",")
    ));
    push_history(life, "goal", &format!("{}{origin}", crate::brain_serve_value::UNPAID_TAG), now(fx));
    ref_draw_after_give(fx, life, &origin);
    true
}

fn exempt(life: &Life) -> bool {
    life.profile.ironman || lifescape_planner::protected(&life.account)
}

fn has_open_demand(life: &Life) -> bool {
    life.ambition.as_ref().is_some_and(|ambition| ambition.milestones.iter().any(|stone| lifescape_planner::drawable(stone) && stone.origin.starts_with("demand:")))
}

pub(crate) fn ref_set_aside_unpaying(fx: &RecUpkeep, life: &mut Life) -> bool {
    if exempt(life) || !has_open_demand(life) {
        return false;
    }
    let items: Vec<(String, String)> = life
        .ambition
        .as_ref()
        .map(|ambition| ambition.milestones.iter().filter(|stone| lifescape_planner::drawable(stone)).filter_map(|stone| stone.origin.strip_prefix("demand:").map(|item| (stone.id.clone(), item.to_string()))).collect())
        .unwrap_or_default();
    let judged = fx.judge(life, &items);
    let account = life.account.clone();
    let running = life.goal_origin.clone().filter(|_| !lifescape_planner::goal_closed(&life.history));
    let may_end_running = life.active.is_none();
    let Some(ambition) = life.ambition.as_mut() else { return false };
    let verdict = |_: &str, stone: &lifescape_planner::Milestone| judged.iter().find(|(id, _)| *id == stone.id).map(|(_, verdict)| *verdict).unwrap_or(ServeVerdict::NoData);
    let aside = lifescape_planner::serve_value::set_aside_unpaying(ambition, &verdict, running.as_deref(), may_end_running);
    let kind = ambition.kind.clone();
    if aside.is_empty() {
        return false;
    }
    for (id, item, per_tick, time_value) in &aside {
        let key = lifescape_planner::milestone_cool_key(&kind, id);
        push_history(life, lifescape_planner::cooldown::COOL_NODE, &lifescape_planner::cooldown::cool_entry(&key, lifescape_planner::AMBITION_CHANGE_TICKS, "set aside, unpaying"), now(fx));
        fx.log(&format!("brain {account}: milestone {id} (demand:{item}) set aside, ambition {kind} kept: {per_tick:.4} coins a tick of the bot < a tick worth {time_value:.2}, inputs bought at a real ask or made at its own ticks, no ban"));
    }
    let running_aside = running.as_ref().is_some_and(|origin| aside.iter().any(|entry| &entry.0 == origin));
    if running_aside {
        let origin = running.unwrap_or_default();
        let abandoned = lifescape_planner::abandon_goal_work(life, crate::brain_milestone_value::UNPAID_MILESTONE_CAUSE);
        lifescape_planner::normalize(life);
        fx.log(&format!("brain {account}: goal of {origin} given back, {} nodes abandoned: {}", abandoned.len(), abandoned.join(",")));
        push_history(life, "goal", &format!("{}{origin}", crate::brain_serve_value::UNPAID_TAG), now(fx));
        life.goal_origin = None;
        match fx.succeed_goal(life, false) {
            Some(label) => fx.log(&format!("brain {account}: next goal {label} (after setting {origin} aside)")),
            None => fx.log(&format!("brain {account}: nothing drawable after setting {origin} aside (see the reason above)")),
        }
        lifescape_planner::normalize(life);
    }
    true
}

/// The A4 rebalance's world, as it was a method of the effects trait (the
/// rule left the loop in lot 5, the reference keeps it).
impl RecUpkeep {
    pub(crate) fn rebalance_row(&self, _life: &Life) -> Option<(RebalanceRow, Box<dyn Any>)> {
        self.fx("rebalance row".to_string());
        self.world.borrow().rebalance.clone().map(|row| (row, Box::new(()) as Box<dyn Any>))
    }

    pub(crate) fn assign_rebalanced(&self, life: &mut Life, row: &RebalanceRow) {
        let ok = self.world.borrow().assign_ok;
        self.fx(format!("assign rebalanced {} {ok}", row.kind));
        if ok {
            if let Some(held) = life.ambition.as_mut() {
                held.kind = row.kind.clone();
            }
        }
    }
}

pub(crate) fn ref_review(fx: &RecUpkeep, life: &mut Life) -> bool {
    let account = life.account.clone();
    let Some(ambition) = life.ambition.as_mut() else { return false };
    if lifescape_planner::adopt_need_mark(ambition) {
        fx.log(&format!("brain {account}: ambition {} ({}) drawn by need before the mark, marked need (D3)", ambition.kind, ambition.why));
        return true;
    }
    false
}

pub(crate) fn ref_replace_stale_goal(fx: &RecUpkeep, life: &mut Life) -> bool {
    if life.active.is_some() || !lifescape_planner::goal_from_earlier_ambition(life) {
        return false;
    }
    let old = life.history.iter().rev().find(|entry| entry.node == "goal" && entry.outcome.starts_with(lifescape_planner::ambition_review::NEXT_TAG)).map(|entry| entry.outcome.clone()).unwrap_or_default();
    let kind = life.ambition.as_ref().map(|ambition| ambition.kind.clone()).unwrap_or_default();
    let abandoned = lifescape_planner::abandon_goal_work(life, lifescape_planner::STALE_GOAL_CAUSE);
    lifescape_planner::normalize(life);
    fx.log(&format!("brain {}: goal ({old}) drawn by an earlier ambition, replaced for the {kind} ambition; {} nodes abandoned: {}", life.account, abandoned.len(), abandoned.join(",")));
    match fx.succeed_goal(life, false) {
        Some(label) => fx.log(&format!("brain {}: next goal {label} ({kind})", life.account)),
        None => fx.log(&format!("brain {}: nothing drawable for the {kind} ambition yet (see the reason above)", life.account)),
    }
    lifescape_planner::normalize(life);
    true
}

pub(crate) fn ref_sync_catalog(fx: &RecUpkeep, life: &mut Life) -> bool {
    let Some(ambition) = life.ambition.as_mut() else { return false };
    let mut dropped = lifescape_planner::drop_stale_products(ambition);
    if !dropped.is_empty() {
        fx.log(&format!("brain {}: milestones {} left the {} ladder (catalogue revised)", life.account, dropped.join(", "), ambition.kind));
    }
    let surplus = fx.fleet_surplus();
    let running = life.goal_origin.clone().filter(|_| !lifescape_planner::goal_closed(&life.history));
    let seed = life.seed;
    let mut revised = Vec::new();
    fx.with_now(&mut |bot| revised = lifescape_planner::ambition_supply::revise_supplied(ambition, bot, seed, &surplus, running.as_deref()));
    if !revised.is_empty() {
        fx.log(&format!(
            "brain {}: milestones {} re-aimed by the fleet's supply (piles to eat, tools someone can get): {}",
            life.account,
            revised.join(", "),
            lifescape_planner::describe_ambition(ambition)
        ));
        dropped.extend(revised);
    }
    if dropped.is_empty() {
        return false;
    }
    let running = life.goal_origin.as_ref().is_some_and(|origin| dropped.contains(origin));
    if running && !lifescape_planner::goal_closed(&life.history) {
        match fx.succeed_goal(life, false) {
            Some(label) => {
                fx.log(&format!("brain {}: goal of a dropped milestone closed, next goal {label}", life.account));
                lifescape_planner::normalize(life);
            }
            None => fx.log(&format!("brain {}: goal of a dropped milestone closed, nothing drawable yet", life.account)),
        }
    }
    true
}

pub(crate) fn ref_rebalance(fx: &RecUpkeep, life: &mut Life) -> bool {
    if life.ambition.is_none() {
        return false;
    }
    let Some((row, _guard)) = fx.rebalance_row(life) else { return false };
    let account = life.account.clone();
    let Some(held) = life.ambition.as_ref() else { return false };
    if lifescape_planner::pinned(held) || lifescape_planner::maker_role(life).is_some() || lifescape_planner::protected(&account) {
        fx.log(&format!("brain rebalance {account}: kept {} (operator or protected, never touched)", held.kind));
        return false;
    }
    if held.kind != row.from {
        fx.log(&format!("brain rebalance {account}: row stale (planned from {:?}, the bot holds {}), nothing applied", row.from, held.kind));
        return false;
    }
    if held.kind == row.kind {
        fx.log(&format!("brain rebalance {account}: kept {} ({})", held.kind, row.numbers));
        return false;
    }
    fx.assign_rebalanced(life, &row);
    life.ambition.as_ref().is_some_and(|now| now.kind == row.kind)
}

pub(crate) fn ref_operator_and_maker(fx: &RecUpkeep, life: &mut Life, dirty: &mut bool) -> bool {
    if ref_apply_pending(fx, life) {
        *dirty = true;
    }
    if ref_retire(fx, life) {
        *dirty = true;
    }
    if ref_maybe_switch(fx, life) {
        *dirty = true;
    }
    lifescape_planner::quoting(life)
}

pub(crate) fn ref_ambition_upkeep(fx: &RecUpkeep, life: &mut Life, dirty: &mut bool, quoting: bool) {
    if !quoting && fx.timed("ambition.ensure", || fx.ensure(life)) {
        *dirty = true;
    }
    if !quoting {
        fx.timed("ambition.coverage", || fx.coverage(life));
    }
    if !quoting && fx.timed("ambition.review", || ref_review(fx, life)) {
        *dirty = true;
    }
    if !quoting && fx.timed("ambition.rebalance", || ref_rebalance(fx, life)) {
        *dirty = true;
    }
    if !quoting && ref_sync_catalog(fx, life) {
        *dirty = true;
    }
    if !quoting && ref_replace_stale_goal(fx, life) {
        *dirty = true;
    }
    if !quoting && ref_release_blocked_bid(fx, life) {
        *dirty = true;
    }
    if !quoting && ref_release_off_trade(fx, life) {
        *dirty = true;
    }
    if !quoting && ref_release_beyond_reach(fx, life) {
        *dirty = true;
    }
    if !quoting && ref_release_unpaying(fx, life) {
        *dirty = true;
    }
    if !quoting && ref_set_aside_unpaying(fx, life) {
        *dirty = true;
    }
    if !quoting {
        let stale = lifescape_planner::abandon_stale_goal_work(life, "work of an earlier goal, no current node waits on it");
        if !stale.is_empty() {
            fx.log(&format!("brain {}: {} open nodes of earlier goals abandoned: {}", life.account, stale.len(), stale.join(",")));
            *dirty = true;
        }
    }
    if !quoting {
        fx.reengage(life);
    }
}

// --- graph state -------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Spell {
    Fresh,
    Waiting(i64),
    Final,
}

fn spell(history: &[HistoryEntry]) -> Spell {
    match history.iter().rev().find(|entry| entry.node != "goal") {
        Some(entry) if entry.node == "graph" && entry.outcome == "stuck" => Spell::Waiting(entry.ended),
        Some(entry) if entry.node == "graph" && entry.outcome == "final" => Spell::Final,
        _ => Spell::Fresh,
    }
}

fn goal_met(goal: &GoalKind, fx: &RecUpkeep) -> bool {
    match goal {
        GoalKind::MaxSkills { skills, level } => !skills.is_empty() && skills.iter().all(|skill| fx.level_of(skill) >= *level),
        _ => false,
    }
}

fn goal_reached(life: &Life, fx: &RecUpkeep) -> bool {
    goal_met(&life.goal, fx) || lifescape_planner::goal_root_done(life)
}

fn effective_state(life: &Life, fx: &RecUpkeep) -> GraphState {
    match lifescape_planner::graph_state(life) {
        GraphState::Stuck { .. } if goal_reached(life, fx) => GraphState::Finished,
        state => state,
    }
}

fn stuck_summary(life: &Life, abandoned: &[String]) -> String {
    let nodes: Vec<&Node> = life.nodes.iter().filter(|node| abandoned.contains(&node.id)).collect();
    let cascade = nodes.iter().filter(|node| node.last_error.starts_with("dependency ")).count();
    let roots: Vec<String> = nodes
        .iter()
        .filter(|node| !node.last_error.starts_with("dependency "))
        .map(|node| {
            let error: String = node.last_error.replace(['\n', '\r'], " ").chars().take(160).collect();
            format!("{}={}", node.id, if error.is_empty() { "?".to_string() } else { error })
        })
        .collect();
    let shown = roots.iter().take(3).cloned().collect::<Vec<_>>().join(" | ");
    let more = roots.len().saturating_sub(3);
    let more = if more > 0 { format!(" (+{more} more roots)") } else { String::new() };
    format!("{} nodes ({cascade} by cascade); roots: {}{more}", nodes.len(), if shown.is_empty() { "?" } else { shown.as_str() })
}

fn ref_retry_draw(fx: &RecUpkeep, life: &mut Life) -> (bool, bool) {
    let (drawn, changed) = fx.redraw(life);
    match drawn {
        Some(label) => {
            fx.log(&format!("brain {}: stuck spell over, next goal {label}", life.account));
            lifescape_planner::normalize(life);
            (true, true)
        }
        None => (false, changed),
    }
}

fn ref_on_stuck(fx: &RecUpkeep, life: &mut Life, abandoned: &[String]) -> bool {
    match spell(&life.history) {
        Spell::Final => ref_retry_draw(fx, life).1,
        Spell::Fresh => {
            fx.log(&format!("brain {}: graph stuck, nothing left to run; abandoned: {}", life.account, stuck_summary(life, abandoned)));
            push_history(life, "graph", "stuck", now(fx));
            let stuck = !goal_reached(life, fx);
            let drawn = if lifescape_planner::goal_closed(&life.history) { fx.redraw(life).0 } else { fx.succeed_goal(life, stuck) };
            match drawn {
                Some(label) => {
                    fx.log(&format!("brain {}: goal {} -> next goal {label}", life.account, if stuck { "stuck" } else { "done" }));
                    lifescape_planner::normalize(life);
                }
                None => fx.log(&format!(
                    "brain {}: no goal left to draw (see the reason above); the draw retries as bans expire, abandoned nodes retry after {REVIVE_COOLDOWN_TICKS} ticks (at most {MAX_REVIVES} times)",
                    life.account
                )),
            }
            true
        }
        Spell::Waiting(since) => {
            let (drawn, changed) = ref_retry_draw(fx, life);
            if drawn {
                return true;
            }
            let waited = fx.clock().ticks_since(since);
            if waited < REVIVE_COOLDOWN_TICKS {
                return changed;
            }
            let changes = lifescape_planner::revive_stuck(life, waited);
            if changes.is_empty() {
                fx.log(&format!("brain {}: revive cap reached ({MAX_REVIVES} per node), standing by (operator decides)", life.account));
                push_history(life, "graph", "final", now(fx));
            } else {
                for change in &changes {
                    fx.log(&format!("brain {}: revive {change}", life.account));
                }
                push_history(life, "graph", "revived", now(fx));
            }
            true
        }
    }
}

fn ref_market_rescue(fx: &RecUpkeep, life: &mut Life) -> bool {
    if !matches!(lifescape_planner::graph_state(life), GraphState::Stuck { .. }) || spell(&life.history) != Spell::Fresh {
        return false;
    }
    let Some(cause) = lifescape_planner::stuck_cause(life) else { return false };
    let wallet = fx.wallet();
    for position in lifescape_planner::rescue_candidates(life, &cause.node) {
        let node = life.nodes[position].clone();
        let NodeKind::ObtainItems { item, count } = &node.kind else { continue };
        if lifescape_planner::market_failed(&node) {
            let changes = lifescape_planner::retarget_produce(life, position);
            if changes.is_empty() {
                fx.log(&format!("brain {}: {item} unfilled on the market and never produced, no rescue", life.account));
                continue;
            }
            for change in changes {
                fx.log(&format!("brain {}: {change}", life.account));
            }
            fx.log(&format!("brain {}: {item} produced instead of bought again (root {}: {})", life.account, cause.node, cause.last_error));
            push_history(life, &node.id, lifescape_planner::PRODUCE_RESCUE_TAG, now(fx));
            return true;
        }
        let facts = fx.facts(life, &node);
        let coins = (wallet - facts.committed).max(0);
        let need = (*count - facts.held).max(1);
        if !lifescape_planner::market_route(need, coins, facts.best_ask, facts.price) {
            fx.log(&format!("brain {}: no market route for {item} x{need} (ask {:?}, reference {}, free coins {coins})", life.account, facts.best_ask, facts.price));
            continue;
        }
        for change in lifescape_planner::retarget_market(life, position) {
            fx.log(&format!("brain {}: {change}", life.account));
        }
        fx.log(&format!("brain {}: {item} bought on the market instead of banning (root {}: {})", life.account, cause.node, cause.last_error));
        push_history(life, &node.id, lifescape_planner::MARKET_RESCUE_TAG, now(fx));
        return true;
    }
    false
}

pub(crate) fn ref_graph_state(fx: &RecUpkeep, life: &mut Life, dirty: bool) -> Flow {
    match fx.timed("stuck.effective_state", || effective_state(life, fx)) {
        GraphState::Launchable => Flow::Continue,
        GraphState::Running(_) => Flow::Stop(Save::IfDirty),
        GraphState::Stuck { abandoned } => {
            if fx.timed("rescue.market", || ref_market_rescue(fx, life)) {
                return Flow::Stop(Save::Always);
            }
            if fx.timed("stuck.on_stuck", || ref_on_stuck(fx, life, &abandoned)) || dirty {
                return Flow::Stop(Save::Always);
            }
            Flow::Stop(Save::None)
        }
        GraphState::Finished => {
            let closing = !lifescape_planner::goal_closed(&life.history);
            if closing {
                fx.close_goal(life, false);
            }
            let (drawn, changed) = fx.timed("succession.redraw", || fx.redraw(life));
            match drawn {
                Some(label) => {
                    fx.log(&format!("brain {}: goal done -> next goal {label}", life.account));
                    lifescape_planner::normalize(life);
                }
                None if closing => fx.log(&format!("brain {}: no goal left to draw (see the reason above), retrying every pass", life.account)),
                None => {}
            }
            if closing || changed || dirty {
                return Flow::Stop(Save::Always);
            }
            Flow::Stop(Save::None)
        }
    }
}

