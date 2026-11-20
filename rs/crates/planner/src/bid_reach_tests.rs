use super::{beyond_reach_answer, planned_levels, BeyondReach};
use crate::ambition::{new_ambition, BotNow, Milestone, MilestoneStatus, Stamp, Target};
use crate::model::{Archetype, GoalKind, Life, Profile};

fn stone(id: &str, target: Target, status: MilestoneStatus) -> Milestone {
    Milestone { id: id.to_string(), label: id.to_string(), target, status, progress: (0, 1), blocked_count: 0, origin: "ladder".to_string() }
}

/// alice_knight live 2026-11-20 05:09 UTC: a smith at smithing 73, its
/// open milestones adamant daggers and smithing 80, answering 28 adamant
/// platebodies (smithing 88).
fn smith_answering(item: &str) -> Life {
    let now = BotNow { level_of: &|_| 73, coins: 500, held: &|_| 0 };
    let mut ambition = new_ambition("smith", "need", &now, 7, &[], Stamp { tick: 0, secs: 0 }).expect("a smith");
    ambition.milestones = vec![
        stone("m1", Target::Level { skill: "smithing".to_string(), level: 60 }, MilestoneStatus::Done),
        stone("m2", Target::Produce { item: "adamant_dagger".to_string(), count: 200, skill: "smithing".to_string() }, MilestoneStatus::Pending),
        stone("m3", Target::Level { skill: "smithing".to_string(), level: 80 }, MilestoneStatus::Pending),
    ];
    Life {
        schema: 1,
        account: "alice_knight".to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(false, 0.69, 0.5, 7, Archetype::Producer),
        goal: GoalKind::Produce { item: item.to_string(), count: 28 },
        nodes: Vec::new(),
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: Some(ambition),
        goal_origin: Some(format!("bid:{item}")),
        memory: Default::default(),
    }
}

/// The ambition climbs its OPEN level milestones now, never its ladder's
/// top (rungs to 90): smithing 80 for this smith, the done 60 ignored.
#[test]
fn the_ambition_plans_its_open_level_milestones_only() {
    let life = smith_answering("adamant_platebody");
    assert_eq!(planned_levels(&life), vec![("smithing".to_string(), 80)]);
    assert!(crate::ambition_catalog::ladder_level("smith", "smithing") >= 88, "the ladder climbs past the platebody's level: the old rule's free training");
    let mut bare = life.clone();
    bare.ambition = None;
    assert!(planned_levels(&bare).is_empty());
}

/// An answer whose object needs a level above the bot's and above its
/// open milestones is given back (persisted under the old rule); one its
/// level or its milestone reaches stays; a milestone goal is never one.
#[test]
fn an_answer_beyond_the_climb_of_now_is_given_back() {
    let at = |level: u8| move |skill: &str| if skill == "smithing" { level } else { 1 };
    let platebody = smith_answering("adamant_platebody");
    let found = beyond_reach_answer(&platebody, &at(73)).expect("smithing 88 is beyond 73 and the milestone 80");
    assert_eq!(found, BeyondReach { origin: "bid:adamant_platebody".to_string(), why: "smithing 88 needed, the bot at 73 and its milestones at 80".to_string() });
    assert_eq!(beyond_reach_answer(&platebody, &at(88)), None, "its own level makes it");
    assert_eq!(beyond_reach_answer(&smith_answering("adamant_dagger"), &at(73)), None, "smithing 70: made now");
    assert_eq!(beyond_reach_answer(&smith_answering("adamant_battleaxe"), &at(73)), None, "smithing 80: the open milestone climbs there anyway");
    let mut milestone_goal = smith_answering("adamant_platebody");
    milestone_goal.goal_origin = Some("milestone:m2".to_string());
    assert_eq!(beyond_reach_answer(&milestone_goal, &at(73)), None, "only bid answers");
    let mut closed = smith_answering("adamant_platebody");
    closed.history.push(crate::model::HistoryEntry { node: "goal".to_string(), started: 0, ended: 0, outcome: format!("{}x", crate::goals::DONE_TAG) });
    assert_eq!(beyond_reach_answer(&closed, &at(73)), None, "a closed goal is the draw's");
    // Drawn under the new rule (its training priced at a quarter): kept.
    let mut priced = smith_answering("adamant_platebody");
    let entry = |outcome: &str| crate::model::HistoryEntry { node: "goal".to_string(), started: 0, ended: 0, outcome: outcome.to_string() };
    priced.history.push(entry(&format!("{}produce adamant_platebody 28 (bid:adamant_platebody)", crate::ambition_review::NEXT_TAG)));
    priced.history.push(entry(super::PLANNED_RULE_TAG));
    assert!(super::drawn_under_rule(&priced.history));
    assert_eq!(beyond_reach_answer(&priced, &at(73)), None, "an answer drawn under the rule is never given back");
    priced.history.push(entry(&format!("{}produce adamant_platebody 28 (bid:adamant_platebody)", crate::ambition_review::NEXT_TAG)));
    assert!(beyond_reach_answer(&priced, &at(73)).is_some(), "a later draw without the mark is an old-rule answer");
}

/// bellarhodes live 2026-11-20 (n3350): an old-rule answer within reach of
/// its milestones still climbing on a line chosen under the old training
/// score (bronze bars, 40 060 copper ore) is given back too; once the
/// climb is over, nothing.
#[test]
fn an_old_rule_answer_still_climbing_is_given_back() {
    let at = |skill: &str| if skill == "smithing" { 73 } else { 1 };
    let mut life = smith_answering("adamant_battleaxe");
    assert_eq!(beyond_reach_answer(&life, &at), None, "nothing to climb: kept");
    let mut climb = crate::model::Node::new("n3350", crate::model::NodeKind::ReachLevel { skill: "smithing".to_string(), level: 80 }, Vec::new(), 0);
    climb.blocked_by = Some("smelt:bronze_bar".to_string());
    life.nodes.push(climb);
    let found = beyond_reach_answer(&life, &at).expect("an old-rule climb");
    assert_eq!(found.why, "n3350 still climbs smithing to 80 on a line chosen under the old training score");
    life.nodes[0].status = crate::model::NodeStatus::Done;
    assert_eq!(beyond_reach_answer(&life, &at), None, "the climb is over");
}
