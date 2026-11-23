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
//! - the persisted milestones that do not pay are set aside (A2: kept,
//!   resumed when nothing else is drawable); an open goal of one waits for
//!   its node to end, then is given back: the rule is the planner's
//!   (`upkeep::milestone_query`, `upkeep::milestone_aside`), driven by
//!   `brain_upkeep_drive::set_aside_unpaying`; `judge_items` reads the
//!   verdicts;
//! - `paying_items`: the demand items adoption may still draw from.
//!
//! No data (no price, no rate), no decision. Protected accounts and
//! ironmen are left alone (no market).

use lifescape_json::Value;
use lifescape_planner::serve_value::{ServeContext, ServeVerdict};
use lifescape_planner::Life;

/// Last error of the nodes of a goal given back as unpaying.
pub const UNPAID_MILESTONE_CAUSE: &str = lifescape_planner::upkeep::UNPAID_MILESTONE_CAUSE;

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

/// The verdict of each open demand milestone `(id, item)` (the planner's
/// `upkeep::milestone_aside` sets the unpaying ones aside).
pub(crate) fn judge_items(life: &Life, live: &Value, items: &[(String, String)]) -> Vec<(String, ServeVerdict)> {
    with_judge(life, live, |judge| items.iter().map(|(id, item)| (id.clone(), judge(item))).collect())
}

#[cfg(test)]
mod tests {
    use super::ServeVerdict;
    use lifescape_json::{object, Value};

    fn apply(life: &mut Life, live: &Value, judged: Vec<(String, ServeVerdict)>) -> bool {
        crate::brain_upkeep_drive::aside_judged(&crate::brain_upkeep_fx::LiveUpkeepFx::bare(live), life, &judged)
    }
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

    /// The set-aside cools, so the resume (cooldown over, nothing else
    /// drawable) does not bring it back on the next pass.
    #[test]
    fn the_unpaying_set_aside_is_cooled_and_not_resumed_at_once() {
        let mut life = crafter();
        assert!(apply(&mut life, &object(vec![]), unpaying()));
        let cooled = lifescape_planner::cooldown::cooled_keys(&life.history, &|_| 0);
        let ambition = life.ambition.as_mut().expect("ambition");
        for stone in ambition.milestones.iter_mut().filter(|stone| stone.id != "m90") {
            stone.status = MilestoneStatus::Done;
        }
        assert!(lifescape_planner::resume_set_aside(ambition, &cooled).is_empty(), "cooled: {cooled:?}");
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
