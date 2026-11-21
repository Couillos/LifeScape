#![forbid(unsafe_code)]
//! A demand milestone is kept only while serving it pays for the bot's
//! time (2026-11-21). Live: crafters carried `demand:diamond`-like
//! milestones ahead of their ladder, never checked for where the inputs
//! come from, and farmed uncut gems by melee drops for 120k-232k ticks.
//! The value comparison is `lifescape_planner::serve_value` (bid against
//! the inputs bought at a real ask or made at the bot's own ticks, per
//! tick of the bot, against `brain_bids::bot_rate`). No price floor, no
//! item list, no ban.
//!
//! - `set_aside_unpaying`: the persisted milestones that do not pay are
//!   set aside (A2: kept, resumed when nothing else is drawable); an open
//!   goal of one waits for its node to end, then is given back;
//! - `paying_items`: the demand items adoption may still draw from.
//!
//! No data (no price, no rate), no decision. Protected accounts and
//! ironmen are left alone (no market).

use lifescape_json::Value;
use lifescape_planner::serve_value::{ServeContext, ServeVerdict};
use lifescape_planner::Life;

/// Last error of the nodes of a goal given back as unpaying.
pub const UNPAID_MILESTONE_CAUSE: &str = "demand milestone set aside: serving it no longer pays for the bot's time";

/// Judges `item` sold at the bid the fleet shows (the market reference
/// when none rests) for this bot; `use_verdict` receives the judge.
fn with_judge<R>(life: &Life, live: &Value, use_verdict: impl FnOnce(&dyn Fn(&str) -> ServeVerdict) -> R) -> R {
    let signals = crate::market_demand::signals_for(&life.account);
    let reader = crate::brain_bids::MarketReader::live(&life.account);
    let time_value = crate::brain_bids::bot_rate(life);
    crate::brain_bids::with_bot_state(life, live, |state| {
        let ask_of = |item: &str| reader.view(item).and_then(|view| view.best_ask()).map(|price| price as f64);
        let context = ServeContext { units: state.units, ask_of: &ask_of, time_value };
        let judge = |item: &str| match crate::market_demand::serving_price(item, &signals) {
            Some(price) => context.verdict(item, price),
            None => ServeVerdict::NoData,
        };
        use_verdict(&judge)
    })
}

fn exempt(life: &Life) -> bool {
    life.profile.ironman || lifescape_planner::protected(&life.account)
}

/// Demand items (of `items`) whose serving is not judged unpaying: the
/// ones a new demand milestone may be adopted for. Only computed while no
/// demand milestone is open (adoption adds one at a time).
pub(crate) fn paying_items(life: &Life, live: &Value, items: Vec<lifescape_planner::DemandItem>) -> Vec<lifescape_planner::DemandItem> {
    let Some(ambition) = life.ambition.as_ref() else { return items };
    let open = ambition.milestones.iter().any(|stone| lifescape_planner::drawable(stone) && stone.origin.starts_with("demand:"));
    if open || exempt(life) {
        return items;
    }
    with_judge(life, live, |judge| items.into_iter().filter(|wanted| !matches!(judge(&wanted.item), ServeVerdict::Unpaying { .. })).collect())
}

/// Sets aside the demand milestones that do not pay; true when the life
/// changed (save it).
pub(crate) fn set_aside_unpaying(life: &mut Life, live: &Value) -> bool {
    if exempt(life) || !has_open_demand(life) {
        return false;
    }
    let judged = with_judge(life, live, |judge| judge_all(life, judge));
    apply(life, live, judged)
}

fn has_open_demand(life: &Life) -> bool {
    life.ambition.as_ref().is_some_and(|ambition| ambition.milestones.iter().any(|stone| lifescape_planner::drawable(stone) && stone.origin.starts_with("demand:")))
}

/// Verdict of each open demand milestone (by id).
fn judge_all(life: &Life, judge: &dyn Fn(&str) -> ServeVerdict) -> Vec<(String, ServeVerdict)> {
    let Some(ambition) = life.ambition.as_ref() else { return Vec::new() };
    ambition
        .milestones
        .iter()
        .filter(|stone| lifescape_planner::drawable(stone))
        .filter_map(|stone| stone.origin.strip_prefix("demand:").map(|item| (stone.id.clone(), judge(item))))
        .collect()
}

/// Sets aside the milestones judged unpaying and gives back the goal of
/// one (`judged`: verdict per open demand milestone id).
fn apply(life: &mut Life, live: &Value, judged: Vec<(String, ServeVerdict)>) -> bool {
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
        crate::logln!("brain {account}: milestone {id} (demand:{item}) set aside, ambition {kind} kept: {per_tick:.4} coins a tick of the bot < a tick worth {time_value:.2}, inputs bought at a real ask or made at its own ticks, no ban");
    }
    let running_aside = running.as_ref().is_some_and(|origin| aside.iter().any(|entry| &entry.0 == origin));
    if running_aside {
        let origin = running.unwrap_or_default();
        let abandoned = lifescape_planner::abandon_goal_work(life, UNPAID_MILESTONE_CAUSE);
        lifescape_planner::normalize(life);
        crate::logln!("brain {account}: goal of {origin} given back, {} nodes abandoned: {}", abandoned.len(), abandoned.join(","));
        crate::brain::push_history(life, "goal", &format!("{}{origin}", crate::brain_serve_value::UNPAID_TAG));
        // The milestone is set aside: the close folds into nothing (it
        // would otherwise mark the quota done).
        life.goal_origin = None;
        match crate::brain_succession::succeed_goal(life, live, false) {
            Some(label) => crate::logln!("brain {account}: next goal {label} (after setting {origin} aside)"),
            None => crate::logln!("brain {account}: nothing drawable after setting {origin} aside (see the reason above)"),
        }
        lifescape_planner::normalize(life);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{apply, ServeVerdict};
    use lifescape_json::object;
    use lifescape_planner::{new_ambition, Archetype, BotNow, GoalKind, Life, Milestone, MilestoneStatus, Profile, Stamp, Target};

    fn crafter() -> Life {
        let now = BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 };
        let mut ambition = new_ambition("crafter", "random", &now, 3, &[], Stamp { tick: 0, secs: 0 }).expect("crafter");
        ambition.milestones.retain(|stone| !stone.origin.starts_with("demand:"));
        ambition.milestones.insert(
            0,
            Milestone {
                id: "m90".to_string(),
                label: "produce diamond".to_string(),
                target: Target::Produce { item: "diamond".to_string(), count: 100, skill: "crafting".to_string() },
                status: MilestoneStatus::Pending,
                progress: (0, 100),
                blocked_count: 0,
                origin: "demand:diamond".to_string(),
            },
        );
        Life {
            schema: 1,
            account: "b0".to_string(),
            created_at: 0,
            seed: 0,
            profile: Profile::new(false, 0.5, 0.5, 0, Archetype::Producer),
            goal: GoalKind::Produce { item: "diamond".to_string(), count: 100 },
            nodes: Vec::new(),
            engagement: None,
            active: None,
            history: Vec::new(),
            ambition: Some(ambition),
            goal_origin: Some("m90".to_string()),
            memory: Default::default(),
        }
    }

    fn unpaying() -> Vec<(String, ServeVerdict)> {
        vec![("m90".to_string(), ServeVerdict::Unpaying { per_tick: 0.01, time_value: 0.5 })]
    }

    /// An unpaying milestone is set aside, kept, never marked done; its
    /// goal is given back; the ladder's next milestone is drawn.
    #[test]
    fn the_running_unpaying_milestone_is_set_aside_not_done() {
        let mut life = crafter();
        assert!(apply(&mut life, &object(vec![]), unpaying()));
        let stone = life.ambition.as_ref().and_then(|ambition| ambition.milestones.iter().find(|stone| stone.id == "m90")).expect("kept");
        assert!(lifescape_planner::set_aside(stone), "{stone:?}");
        assert_ne!(stone.status, MilestoneStatus::Done);
        assert_ne!(life.goal_origin.as_deref(), Some("m90"));
    }

    /// A paying milestone, or one with no data, decides nothing.
    #[test]
    fn paying_or_unread_milestones_stay() {
        for verdict in [ServeVerdict::Pays { per_tick: 5.0 }, ServeVerdict::NoData] {
            let mut life = crafter();
            assert!(!apply(&mut life, &object(vec![]), vec![("m90".to_string(), verdict)]));
            assert_eq!(life.goal_origin.as_deref(), Some("m90"));
            assert!(life.ambition.as_ref().is_some_and(|ambition| ambition.milestones.iter().any(|stone| stone.id == "m90" && lifescape_planner::drawable(stone))));
        }
    }

    /// A node running holds the milestone until it ends.
    #[test]
    fn a_running_node_waits() {
        let mut life = crafter();
        life.active = Some("n1".to_string());
        assert!(!apply(&mut life, &object(vec![]), unpaying()));
    }
}
