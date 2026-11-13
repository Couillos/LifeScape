//! Per-bot long-term ambitions (user decision 2026-11-13). Every bot has
//! one ambition (`smith`, `fisher`, `merchant`…) whose milestones drive
//! its goal draws: level tranches, production quotas (made, never bought,
//! then listed by the default selling), equipment tiers, coin targets.
//! Pure: live levels, coins, demand and ticks come in as arguments; only
//! this module changes a milestone status (the node statuses stay with
//! `lifecycle`).
//!
//! Rules: an optional change at most once every `AMBITION_CHANGE_TICKS`;
//! a milestone blocked `MILESTONE_MAX_BLOCKS` times redraws the ambition
//! at once (approved policy, logged by the brain); banned skills are
//! skipped (`next_milestone`), never trained through a ban. The catalogue
//! lives in `ambition_catalog`, the fleet coverage in `ambition_fleet`.

use lifescape_json::{object, str_value, Value};

use crate::ambition_catalog::{pick_product, spec, DemandItem, Then};
use crate::model::{GoalKind, Life};

/// Ticks between two optional ambition changes (72 min at 60 ms dev, 12 h
/// at 600 ms prod). Blocked and achieved redraws are not optional.
pub const AMBITION_CHANGE_TICKS: u64 = 72_000;
/// Stuck closes of one milestone before the ambition is redrawn.
pub const MILESTONE_MAX_BLOCKS: u32 = 3;
/// Open (not done) milestones kept materialized ahead.
pub const OPEN_MILESTONES: usize = 3;
/// Done milestones kept in the file (the oldest go first; ids are never
/// reused): a bot serving the market for months never grows its life.
pub const MAX_DONE_KEPT: usize = 40;
/// Levels an extension rung climbs past the live level (or its rung).
pub const EXTEND_LEVELS: u8 = 5;
/// Last catalogue rungs an extension cycles through.
pub const EXTEND_LAP: usize = 3;
/// Origin of the milestones materialized past the catalogue ladder.
pub const EXTEND_ORIGIN: &str = "extend";
/// Explicit ambition that keeps the plain lowest-skill draw; never drawn
/// by the fleet, only set by an operator.
pub const GENERALIST: &str = "generalist";

/// What a milestone asks for (`kind` + `target` in the file).
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Level { skill: String, level: u8 },
    /// `skill` is the skill the production trains (bans read it).
    Produce { item: String, count: i64, skill: String },
    Equip { style: String },
    Coins { amount: i64 },
}

impl Target {
    pub fn kind(&self) -> &'static str {
        match self {
            Target::Level { .. } => "level",
            Target::Produce { .. } => "produce",
            Target::Equip { .. } => "equip",
            Target::Coins { .. } => "coins",
        }
    }

    /// Skills a ban can hold back (empty: never banned).
    pub fn skills(&self) -> Vec<String> {
        match self {
            Target::Level { skill, .. } | Target::Produce { skill, .. } if !skill.is_empty() => vec![skill.clone()],
            _ => Vec::new(),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Target::Level { skill, level } => format!("{skill} {level}"),
            Target::Produce { item, count, .. } => format!("produire {count} {item} (mis en vente)"),
            Target::Equip { style } => format!("équiper le meilleur {style}"),
            Target::Coins { amount } => format!("détenir {amount} pièces"),
        }
    }

    fn to_value(&self) -> Value {
        match self {
            Target::Level { skill, level } => object(vec![("skill", str_value(skill)), ("level", Value::Int(i64::from(*level)))]),
            Target::Produce { item, count, skill } => {
                object(vec![("item", str_value(item)), ("count", Value::Int(*count)), ("skill", str_value(skill))])
            }
            Target::Equip { style } => object(vec![("style", str_value(style))]),
            Target::Coins { amount } => object(vec![("amount", Value::Int(*amount))]),
        }
    }

    fn from_value(kind: &str, value: &Value) -> Option<Target> {
        let text = |key: &str| value.get(key).as_str().unwrap_or("").to_string();
        match kind {
            "level" => Some(Target::Level { skill: text("skill"), level: value.get("level").as_i64()?.clamp(2, 99) as u8 }),
            "produce" => Some(Target::Produce { item: text("item"), count: value.get("count").as_i64()?.max(1), skill: text("skill") }),
            "equip" => Some(Target::Equip { style: text("style") }),
            "coins" => Some(Target::Coins { amount: value.get("amount").as_i64()?.max(1) }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MilestoneStatus {
    Pending,
    Active,
    Done,
    Blocked,
}

impl MilestoneStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            MilestoneStatus::Pending => "pending",
            MilestoneStatus::Active => "active",
            MilestoneStatus::Done => "done",
            MilestoneStatus::Blocked => "blocked",
        }
    }

    fn parse(text: &str) -> MilestoneStatus {
        match text {
            "active" => MilestoneStatus::Active,
            "done" => MilestoneStatus::Done,
            "blocked" => MilestoneStatus::Blocked,
            _ => MilestoneStatus::Pending,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Milestone {
    pub id: String,
    pub label: String,
    pub target: Target,
    pub status: MilestoneStatus,
    /// (current, target): live level, coins held, units made.
    pub progress: (i64, i64),
    pub blocked_count: u32,
    /// `ladder` (catalogue rung) or `demand:<item>` (open bids).
    pub origin: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Revision {
    pub tick: u64,
    /// Unix seconds of the change (elapsed ticks are measured from it).
    /// File key `at_secs` (files before 2026-11-13 wrote `at`).
    pub at_secs: i64,
    pub from: String,
    pub to: String,
    pub why: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ambition {
    pub kind: String,
    pub label: String,
    pub groups: Vec<String>,
    /// `coverage:<group>` | `random` | `demand:<item>` (or `operator`);
    /// `levels:<group>` only in lives drawn before 2026-11-13.
    pub why: String,
    pub milestones: Vec<Milestone>,
    pub drawn_at_tick: u64,
    pub drawn_at: i64,
    /// Next catalogue rung to materialize.
    pub rung: usize,
    pub revisions: Vec<Revision>,
}

/// When a draw happens: daemon tick clock and unix seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub tick: u64,
    pub secs: i64,
}

/// What an ambition reads of its bot now (live snapshot).
pub struct BotNow<'view> {
    pub level_of: &'view dyn Fn(&str) -> u8,
    /// Coins held (pack + bank + worn).
    pub coins: i64,
    /// Units of an object held (pack, bank, worn): listed stock stays on
    /// the bot until its deal is delivered, so it counts here too.
    pub held: &'view dyn Fn(&str) -> i64,
}

/// Why no ambition of a kind could be made (typed, logged by the brain).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AmbitionError {
    UnknownKind(String),
    /// The ladder gives this bot no open milestone (it is far past every
    /// rung): never an ambition achieved at birth, redrawn every pass.
    NoMilestone(String),
}

impl std::fmt::Display for AmbitionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AmbitionError::UnknownKind(kind) => write!(formatter, "ambition {kind} unknown to the catalogue"),
            AmbitionError::NoMilestone(kind) => write!(formatter, "ambition {kind} gives no open milestone at these levels"),
        }
    }
}

/// A fresh ambition of `kind` for a bot at these levels and coins: its
/// first milestones materialized from the catalogue (rungs the bot is far
/// past are skipped, coin targets it holds too). A ladder that leaves no
/// open milestone is refused (`NoMilestone`), except the generalist's.
pub fn new_ambition(kind: &str, why: &str, now: &BotNow, seed: u64, demand: &[DemandItem], stamp: Stamp) -> Result<Ambition, AmbitionError> {
    let ambition = fresh_ambition(kind, why, now, seed, demand, stamp)?;
    if ambition.kind != GENERALIST && open_count(&ambition) == 0 {
        return Err(AmbitionError::NoMilestone(kind.to_string()));
    }
    Ok(ambition)
}

/// `new_ambition` for the sole holder of a group (coverage): a ladder the
/// bot is past extends beyond the catalogue (`extend`) instead of being
/// refused; `NoMilestone` only when even the extension is empty.
pub fn new_ambition_extended(kind: &str, why: &str, now: &BotNow, seed: u64, demand: &[DemandItem], stamp: Stamp) -> Result<Ambition, AmbitionError> {
    let mut ambition = fresh_ambition(kind, why, now, seed, demand, stamp)?;
    if ambition.kind != GENERALIST && open_count(&ambition) == 0 {
        extend(&mut ambition, now, seed);
        if open_count(&ambition) == 0 {
            return Err(AmbitionError::NoMilestone(kind.to_string()));
        }
    }
    Ok(ambition)
}

fn fresh_ambition(kind: &str, why: &str, now: &BotNow, seed: u64, demand: &[DemandItem], stamp: Stamp) -> Result<Ambition, AmbitionError> {
    let found = spec(kind).ok_or_else(|| AmbitionError::UnknownKind(kind.to_string()))?;
    let mut ambition = Ambition {
        kind: found.kind.to_string(),
        label: found.label.to_string(),
        groups: found.groups.iter().map(|group| group.to_string()).collect(),
        why: why.to_string(),
        milestones: Vec::new(),
        drawn_at_tick: stamp.tick,
        drawn_at: stamp.secs,
        rung: 0,
        revisions: Vec::new(),
    };
    refresh(&mut ambition, now, seed, demand);
    Ok(ambition)
}

fn next_milestone_id(ambition: &Ambition) -> String {
    let top = ambition.milestones.iter().filter_map(|stone| stone.id.strip_prefix('m')?.parse::<usize>().ok()).max().unwrap_or(0);
    format!("m{}", top + 1)
}

fn push_milestone(ambition: &mut Ambition, target: Target, origin: &str) {
    let goal = match &target {
        Target::Level { level, .. } => i64::from(*level),
        Target::Produce { count, .. } => *count,
        Target::Equip { .. } => 1,
        Target::Coins { amount } => *amount,
    };
    let milestone_id = next_milestone_id(ambition);
    ambition.milestones.push(Milestone {
        id: milestone_id,
        label: target.label(),
        target,
        status: MilestoneStatus::Pending,
        progress: (0, goal),
        blocked_count: 0,
        origin: origin.to_string(),
    });
}

/// Open (not done) milestones of an ambition.
pub fn open_count(ambition: &Ambition) -> usize {
    ambition.milestones.iter().filter(|stone| stone.status != MilestoneStatus::Done).count()
}

fn milestone_number(stone: &Milestone) -> usize {
    stone.id.strip_prefix('m').and_then(|raw| raw.parse::<usize>().ok()).unwrap_or(0)
}

/// Drop the oldest done milestones past `MAX_DONE_KEPT`. The highest id
/// always stays, so `next_milestone_id` never hands an id out twice.
fn prune_done(ambition: &mut Ambition) {
    let top = ambition.milestones.iter().map(milestone_number).max().unwrap_or(0);
    let done = ambition.milestones.iter().filter(|stone| stone.status == MilestoneStatus::Done).count();
    let excess = done.saturating_sub(MAX_DONE_KEPT);
    if excess == 0 {
        return;
    }
    let mut oldest: Vec<usize> = ambition
        .milestones
        .iter()
        .filter(|stone| stone.status == MilestoneStatus::Done)
        .map(milestone_number)
        .filter(|number| *number != top)
        .collect();
    oldest.sort_unstable();
    oldest.truncate(excess);
    ambition.milestones.retain(|stone| stone.status != MilestoneStatus::Done || !oldest.contains(&milestone_number(stone)));
}

/// Bring the ambition up to date: level and coin milestones the bot
/// meets are done (progress written), at most one demand milestone is
/// inserted for an open bid the ambition can serve, and catalogue rungs
/// are materialized until `OPEN_MILESTONES` are open (rungs the bot is
/// 10+ levels past are skipped). Generalists hold no milestones. Returns
/// the ids of the milestones this call closed (the brain weighs an
/// optional change on them, as on a closed goal).
pub fn refresh(ambition: &mut Ambition, now: &BotNow, seed: u64, demand: &[DemandItem]) -> Vec<String> {
    let Some(found) = spec(&ambition.kind) else { return Vec::new() };
    let level_of = now.level_of;
    let coins = now.coins;
    let mut closed = Vec::new();
    for stone in &mut ambition.milestones {
        let was_open = stone.status != MilestoneStatus::Done;
        match &stone.target {
            Target::Level { skill, level } => {
                let have = i64::from(level_of(skill).max(1));
                stone.progress = (have.min(i64::from(*level)), i64::from(*level));
                if have >= i64::from(*level) {
                    stone.status = MilestoneStatus::Done;
                }
            }
            Target::Coins { amount } => {
                stone.progress = (coins.clamp(0, *amount), *amount);
                if coins >= *amount {
                    stone.status = MilestoneStatus::Done;
                }
            }
            _ => {}
        }
        if was_open && stone.status == MilestoneStatus::Done {
            closed.push(stone.id.clone());
        }
    }
    insert_demand_milestone(ambition, now, demand);
    while open_count(ambition) < OPEN_MILESTONES && ambition.rung < found.ladder.len() {
        let index = ambition.rung;
        let rung = &found.ladder[index];
        ambition.rung += 1;
        let live = level_of(rung.skill).max(1);
        if rung.level > 0 && live >= rung.level.saturating_add(10) {
            continue;
        }
        if rung.level > 0 && live < rung.level {
            push_milestone(ambition, Target::Level { skill: rung.skill.to_string(), level: rung.level }, "ladder");
        }
        match rung.then {
            Then::Nothing => {}
            Then::Produce(items, count, skill) => {
                let item = pick_product(items, seed ^ (index as u64).wrapping_mul(0x9E37_79B9), demand);
                push_milestone(ambition, Target::Produce { item: item.to_string(), count, skill: skill.to_string() }, "ladder");
            }
            Then::Equip(style) => push_milestone(ambition, Target::Equip { style: style.to_string() }, "ladder"),
            Then::Coins(amount) => {
                if coins < amount {
                    push_milestone(ambition, Target::Coins { amount }, "ladder");
                }
            }
        }
    }
    prune_done(ambition);
    closed
}

/// Materialize milestones past the catalogue ladder (the sole holder of
/// a group at the end of its ladder, or an achieved ambition the 72 000
/// tick throttle keeps): the last `EXTEND_LAP` rungs cycle, each level
/// climbing `EXTEND_LEVELS` past the live level (capped at 99), each
/// quota and equipment tier repeated, each coin target doubling past the
/// coins held. Fills up to `OPEN_MILESTONES` open; returns how many were
/// added (0: nothing left to climb, the caller redraws).
pub fn extend(ambition: &mut Ambition, now: &BotNow, seed: u64) -> usize {
    let Some(found) = spec(&ambition.kind) else { return 0 };
    let ladder_len = found.ladder.len();
    if ladder_len == 0 {
        return 0;
    }
    let lap = EXTEND_LAP.min(ladder_len);
    ambition.rung = ambition.rung.max(ladder_len);
    let before = ambition.milestones.len();
    let mut idle = 0;
    while open_count(ambition) < OPEN_MILESTONES && idle < lap {
        let step = ambition.rung - ladder_len;
        ambition.rung += 1;
        let rung = found.ladder[ladder_len - lap + step % lap];
        let pushed = ambition.milestones.len();
        if rung.level > 0 && !rung.skill.is_empty() {
            let live = (now.level_of)(rung.skill).max(1);
            let target = rung.level.max(live).saturating_add(EXTEND_LEVELS).min(99);
            if target > live {
                push_milestone(ambition, Target::Level { skill: rung.skill.to_string(), level: target }, EXTEND_ORIGIN);
            }
        }
        match rung.then {
            Then::Nothing => {}
            Then::Produce(items, count, skill) => {
                let item = pick_product(items, seed ^ (step as u64).wrapping_mul(0x9E37_79B9), &[]);
                push_milestone(ambition, Target::Produce { item: item.to_string(), count, skill: skill.to_string() }, EXTEND_ORIGIN);
            }
            Then::Equip(style) => push_milestone(ambition, Target::Equip { style: style.to_string() }, EXTEND_ORIGIN),
            Then::Coins(amount) => {
                let highest = ambition
                    .milestones
                    .iter()
                    .filter_map(|stone| match stone.target {
                        Target::Coins { amount } => Some(amount),
                        _ => None,
                    })
                    .chain([amount, now.coins])
                    .max()
                    .unwrap_or(amount)
                    .max(1);
                push_milestone(ambition, Target::Coins { amount: highest.saturating_mul(2) }, EXTEND_ORIGIN);
            }
        }
        idle = if ambition.milestones.len() > pushed { 0 } else { idle + 1 };
    }
    ambition.milestones.len() - before
}

/// Buy orders motivate production: the heaviest open demand for a product
/// of this ambition the bot can reach within 10 levels (training first,
/// inputs bought) becomes a produce milestone, one at a time, ahead of the
/// ladder. Only a bid that pays its inputs (`DemandItem::margin` > 0, see
/// `market_demand`), and only when the bot does not already hold the
/// quantity (stock held or listed answers the bid without producing: a
/// cook holding 100 shrimp looped done/reinsert forever, review
/// 2026-11-13).
fn insert_demand_milestone(ambition: &mut Ambition, now: &BotNow, demand: &[DemandItem]) {
    let level_of = now.level_of;
    let open_demand = ambition.milestones.iter().any(|stone| stone.status != MilestoneStatus::Done && stone.origin.starts_with("demand:"));
    if open_demand {
        return;
    }
    let Some(found) = spec(&ambition.kind) else { return };
    let mut best: Option<(&DemandItem, &str, u8)> = None;
    for rung in found.ladder {
        let Then::Produce(items, _, skill) = rung.then else { continue };
        for item in items {
            let Some(wanted) = demand.iter().find(|entry| {
                entry.item == *item && entry.unmet > 0 && entry.weight > 0.0 && entry.margin.is_some_and(|margin| margin > 0.0)
            }) else {
                continue;
            };
            if (now.held)(item) >= demand_quota(wanted.unmet) {
                continue;
            }
            if rung.level > level_of(skill).max(1).saturating_add(10) {
                continue;
            }
            if best.is_none_or(|(known, _, _)| wanted.weight > known.weight) {
                best = Some((wanted, skill, rung.level));
            }
        }
    }
    let Some((wanted, skill, _)) = best else { return };
    let count = demand_quota(wanted.unmet);
    let target = Target::Produce { item: wanted.item.clone(), count, skill: skill.to_string() };
    let origin = format!("demand:{}", wanted.item);
    push_milestone(ambition, target, &origin);
    // Ahead of the ladder: move it before the first open milestone.
    if let Some(last) = ambition.milestones.pop() {
        let position = ambition.milestones.iter().position(|stone| stone.status != MilestoneStatus::Done).unwrap_or(ambition.milestones.len());
        ambition.milestones.insert(position, last);
    }
}

/// Quantity a demand milestone asks for an unmet bid quantity.
fn demand_quota(unmet: i64) -> i64 {
    unmet.clamp(5, 100)
}

/// First open milestone whose skills are not banned now (`avoid`).
pub fn next_milestone<'a>(ambition: &'a Ambition, avoid: &[String]) -> Option<&'a Milestone> {
    ambition
        .milestones
        .iter()
        .filter(|stone| stone.status != MilestoneStatus::Done)
        .find(|stone| stone.target.skills().iter().all(|skill| !avoid.contains(skill)))
}

/// Goal of one milestone. Coins targets run the merchant engagement.
pub fn goal_for_milestone(stone: &Milestone) -> GoalKind {
    match &stone.target {
        Target::Level { skill, level } => GoalKind::MaxSkills { skills: vec![skill.clone()], level: *level },
        Target::Produce { item, count, .. } => GoalKind::Produce { item: item.clone(), count: *count },
        Target::Equip { style } => GoalKind::EquipBest { style: style.clone() },
        Target::Coins { .. } => GoalKind::GetRich { merchant: Some(true) },
    }
}

/// The goal drawn from `origin` starts: that milestone turns active (any
/// other active one back to pending) and the life records the link.
pub fn mark_drawn(life: &mut Life, origin: Option<&str>) {
    life.goal_origin = origin.map(str::to_string);
    let Some(ambition) = life.ambition.as_mut() else { return };
    for stone in &mut ambition.milestones {
        if stone.status == MilestoneStatus::Active {
            stone.status = MilestoneStatus::Pending;
        }
        if Some(stone.id.as_str()) == origin && stone.status != MilestoneStatus::Done {
            stone.status = MilestoneStatus::Active;
        }
    }
}

/// What a goal close did to its milestone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseOutcome {
    /// The goal came from no milestone (operator goal, generalist).
    Unlinked,
    Done(String),
    /// Level and coin milestones wait for the next refresh to read live.
    Reopened(String),
    Blocked { id: String, count: u32 },
    /// Blocked `MILESTONE_MAX_BLOCKS` times: redraw the ambition now.
    RedrawNeeded { id: String, count: u32 },
}

/// Fold a goal close into its milestone (`life.goal_origin`).
pub fn on_goal_closed(life: &mut Life, stuck: bool) -> CloseOutcome {
    let Some(origin) = life.goal_origin.take() else { return CloseOutcome::Unlinked };
    let Some(stone) = life.ambition.as_mut().and_then(|ambition| ambition.milestones.iter_mut().find(|stone| stone.id == origin)) else {
        return CloseOutcome::Unlinked;
    };
    if stuck {
        stone.blocked_count += 1;
        stone.status = MilestoneStatus::Blocked;
        let count = stone.blocked_count;
        return if count >= MILESTONE_MAX_BLOCKS { CloseOutcome::RedrawNeeded { id: origin, count } } else { CloseOutcome::Blocked { id: origin, count } };
    }
    match stone.target {
        Target::Produce { count, .. } => {
            stone.progress = (count, count);
            stone.status = MilestoneStatus::Done;
            CloseOutcome::Done(origin)
        }
        Target::Equip { .. } => {
            stone.progress = (1, 1);
            stone.status = MilestoneStatus::Done;
            CloseOutcome::Done(origin)
        }
        _ => {
            if stone.status != MilestoneStatus::Done {
                stone.status = MilestoneStatus::Pending;
            }
            CloseOutcome::Reopened(origin)
        }
    }
}

/// Every rung materialized and every milestone done: the ambition is met.
pub fn achieved(ambition: &Ambition) -> bool {
    let Some(found) = spec(&ambition.kind) else { return false };
    ambition.kind != GENERALIST && ambition.rung >= found.ladder.len() && ambition.milestones.iter().all(|stone| stone.status == MilestoneStatus::Done)
}

/// Unix seconds of the last change (the draw, else the last revision).
pub fn last_change_secs(ambition: &Ambition) -> i64 {
    ambition.revisions.last().map(|revision| revision.at_secs).unwrap_or(ambition.drawn_at)
}

/// Optional changes are throttled: `AMBITION_CHANGE_TICKS` since the last
/// change (`ticks_since` maps unix seconds to elapsed ticks).
pub fn may_change(ambition: &Ambition, ticks_since: &dyn Fn(i64) -> u64) -> bool {
    ticks_since(last_change_secs(ambition)) >= AMBITION_CHANGE_TICKS
}

/// Replace the ambition by `next`, keeping the revision trail (one entry
/// `{tick, at, from, to, why}`) and dropping the stale goal link.
pub fn revise(life: &mut Life, mut next: Ambition, why: &str, stamp: Stamp) {
    let (from, mut revisions) = match life.ambition.take() {
        Some(old) => (old.kind, old.revisions),
        None => (String::new(), Vec::new()),
    };
    revisions.push(Revision { tick: stamp.tick, at_secs: stamp.secs, from, to: next.kind.clone(), why: why.to_string() });
    next.revisions = revisions;
    life.ambition = Some(next);
    life.goal_origin = None;
}

/// One-line summary for the daemon log.
pub fn describe(ambition: &Ambition) -> String {
    let stones: Vec<String> = ambition.milestones.iter().filter(|stone| stone.status != MilestoneStatus::Done).map(|stone| format!("{} {}", stone.id, stone.label)).collect();
    format!("{} ({}) — milestones {}", ambition.kind, ambition.why, if stones.is_empty() { "none".to_string() } else { stones.join(", ") })
}

pub fn ambition_to_value(ambition: &Ambition) -> Value {
    let milestones = ambition
        .milestones
        .iter()
        .map(|stone| {
            object(vec![
                ("id", str_value(&stone.id)),
                ("label", str_value(&stone.label)),
                ("kind", str_value(stone.target.kind())),
                ("target", stone.target.to_value()),
                ("status", str_value(stone.status.as_str())),
                ("progress", Value::Arr(vec![Value::Int(stone.progress.0), Value::Int(stone.progress.1)])),
                ("blocked_count", Value::Int(i64::from(stone.blocked_count))),
                ("origin", str_value(&stone.origin)),
            ])
        })
        .collect();
    let revisions = ambition
        .revisions
        .iter()
        .map(|revision| {
            object(vec![
                ("tick", Value::Int(revision.tick as i64)),
                ("at_secs", Value::Int(revision.at_secs)),
                ("from", str_value(&revision.from)),
                ("to", str_value(&revision.to)),
                ("why", str_value(&revision.why)),
            ])
        })
        .collect();
    object(vec![
        ("kind", str_value(&ambition.kind)),
        ("label", str_value(&ambition.label)),
        ("groups", Value::Arr(ambition.groups.iter().map(|group| str_value(group)).collect())),
        ("why", str_value(&ambition.why)),
        ("milestones", Value::Arr(milestones)),
        ("drawn_at_tick", Value::Int(ambition.drawn_at_tick as i64)),
        ("drawn_at", Value::Int(ambition.drawn_at)),
        ("rung", Value::Int(ambition.rung as i64)),
        ("revisions", Value::Arr(revisions)),
    ])
}

/// `None` for a missing section (old files) or an unreadable one.
pub fn ambition_from_value(value: &Value) -> Option<Ambition> {
    let kind = value.get("kind").as_str()?.to_string();
    let text = |row: &Value, key: &str| row.get(key).as_str().unwrap_or("").to_string();
    let milestones = value
        .get("milestones")
        .as_arr()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let target = Target::from_value(row.get("kind").as_str().unwrap_or(""), &row.get("target"))?;
                    let progress = row.get("progress");
                    let cell = |slot: usize| progress.as_arr().and_then(|cells| cells.get(slot).and_then(|cell| cell.as_i64())).unwrap_or(0);
                    Some(Milestone {
                        id: text(row, "id"),
                        label: text(row, "label"),
                        target,
                        status: MilestoneStatus::parse(row.get("status").as_str().unwrap_or("pending")),
                        progress: (cell(0), cell(1)),
                        blocked_count: row.get("blocked_count").as_i64().unwrap_or(0).clamp(0, i64::from(u32::MAX)) as u32,
                        origin: row.get("origin").as_str().unwrap_or("ladder").to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let revisions = value
        .get("revisions")
        .as_arr()
        .map(|rows| {
            rows.iter()
                .map(|row| Revision {
                    tick: row.get("tick").as_i64().unwrap_or(0).max(0) as u64,
                    at_secs: row.get("at_secs").as_i64().or_else(|| row.get("at").as_i64()).unwrap_or(0),
                    from: text(row, "from"),
                    to: text(row, "to"),
                    why: text(row, "why"),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Ambition {
        kind,
        label: text(value, "label"),
        groups: value.get("groups").as_arr().map(|rows| rows.iter().filter_map(|row| row.as_str().map(str::to_string)).collect()).unwrap_or_default(),
        why: text(value, "why"),
        milestones,
        drawn_at_tick: value.get("drawn_at_tick").as_i64().unwrap_or(0).max(0) as u64,
        drawn_at: value.get("drawn_at").as_i64().unwrap_or(0),
        rung: value.get("rung").as_i64().unwrap_or(0).max(0) as usize,
        revisions,
    })
}

#[cfg(test)]
#[path = "ambition_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "ambition_repair_tests.rs"]
mod repair_tests;
