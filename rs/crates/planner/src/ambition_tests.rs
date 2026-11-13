//! Ambition tests (pure): catalogue, fleet coverage, change throttle,
//! blocked redraw, melee as one group, existing levels, life file.

use super::{
    achieved, goal_for_milestone, mark_drawn, may_change, new_ambition, next_milestone, on_goal_closed, refresh, revise,
    BotNow, CloseOutcome, MilestoneStatus, Stamp, Target, AMBITION_CHANGE_TICKS, GENERALIST, MILESTONE_MAX_BLOCKS,
};
use crate::ambition_catalog::{fit, kind_weights, products, spec, DemandItem, Then, GROUPS, MERCHANT, SPECS};
use crate::ambition_fleet::{assign_fleet, coverage_gaps, draw_kind, optional_change, FleetBot};
use crate::model::{Archetype, GoalKind, Life, Profile};

const ZERO: Stamp = Stamp { tick: 0, secs: 0 };

fn bot(account: &str, seed: u64, levels: &[(&str, u8)]) -> FleetBot {
    FleetBot {
        account: account.to_string(),
        levels: levels.iter().map(|(skill, level)| (skill.to_string(), *level)).collect(),
        ironman: false,
        merchant_archetype: false,
        market_affinity: 0.5,
        seed,
        kind: None,
    }
}

fn levels<'pairs>(pairs: &'pairs [(&'pairs str, u8)]) -> impl Fn(&str) -> u8 + 'pairs {
    move |skill| pairs.iter().find(|(name, _)| *name == skill).map(|(_, level)| *level).unwrap_or(1)
}

fn life_with(kind: &str) -> Life {
    Life {
        schema: 1,
        account: "b0".to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer),
        goal: GoalKind::MakeMoney,
        nodes: Vec::new(),
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: new_ambition(kind, "random", &BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 }, 7, &[], ZERO).ok(),
        goal_origin: None,
    }
}

/// 25 fresh bots: the 12 groups and the merchant are covered, each by a
/// dedicated bot, and the 12 others draw random ambitions.
#[test]
fn coverage_for_25_bots_covers_every_group_and_a_merchant() {
    let bots: Vec<FleetBot> = (0..25).map(|index| bot(&format!("b{index:02}"), 1000 + index as u64 * 7919, &[])).collect();
    let assigned = assign_fleet(&bots, &[]);
    assert_eq!(assigned.len(), 25, "every bot gets one");
    let kinds: Vec<&str> = assigned.iter().map(|row| row.kind.as_str()).collect();
    assert!(coverage_gaps(&kinds).is_empty(), "no gap left: {kinds:?}");
    let coverage = assigned.iter().filter(|row| row.why.starts_with("coverage:")).count();
    assert_eq!(coverage, GROUPS.len() + 1, "12 groups + merchant by coverage");
    let random: Vec<&str> = assigned.iter().filter(|row| !row.why.starts_with("coverage:")).map(|row| row.why.as_str()).collect();
    assert_eq!(random.len(), 12);
    assert!(random.iter().all(|why| *why == "random"), "no demand: plain random, got {random:?}");
    assert!(!kinds.contains(&GENERALIST), "the generalist is never drawn");
    let distinct: std::collections::BTreeSet<&str> = kinds.iter().copied().collect();
    assert!(distinct.len() >= GROUPS.len() + 1, "varied: {distinct:?}");
}

/// Beyond coverage, the draw is random (seeded), tilted by demand.
#[test]
fn beyond_coverage_draws_vary_and_follow_demand() {
    let mut seen = std::collections::BTreeSet::new();
    for seed in 0..200u64 {
        seen.insert(draw_kind(&bot("x", seed * 104_729 + 3, &[]), &[], &[], &[]).0);
    }
    assert!(seen.len() >= 10, "random spread over the catalogue, got {seen:?}");
    let demand = kind_weights(&[DemandItem { item: "bronze_platebody".to_string(), weight: 5000.0, unmet: 20, margin: Some(10.0) }], &[]);
    let smith = demand.iter().find(|row| row.kind == "smith").expect("smith weight");
    assert!(smith.weight >= 2.0 && smith.top_item.as_deref() == Some("bronze_platebody"), "{smith:?}");
    let smiths = (0..400u64).filter(|seed| draw_kind(&bot("x", seed * 7_919 + 1, &[]), &[], &demand, &[]).0 == "smith").count();
    let plain = (0..400u64).filter(|seed| draw_kind(&bot("x", seed * 7_919 + 1, &[]), &[], &[], &[]).0 == "smith").count();
    assert!(smiths > plain, "open bids tilt the draw: {smiths} vs {plain}");
    let (_, why) = (0..400u64).map(|seed| draw_kind(&bot("x", seed * 7_919 + 1, &[]), &[], &demand, &[])).find(|(kind, _)| kind == "smith").expect("a smith");
    assert_eq!(why, "demand:bronze_platebody");
}

/// Melee is ONE group: one warrior covers attack, strength and defence,
/// and no ambition is ever keyed on a single melee stat.
#[test]
fn melee_is_one_group() {
    assert_eq!(GROUPS.len(), 12);
    for stat in ["attack", "strength", "defence", "hitpoints"] {
        assert!(!GROUPS.contains(&stat), "{stat} is not a group");
        assert_eq!(crate::ambition_catalog::group_of_skill(stat), Some("melee"));
    }
    let warriors = SPECS.iter().filter(|entry| entry.groups.first() == Some(&"melee")).count();
    assert_eq!(warriors, 1, "one melee ambition");
    assert!(coverage_gaps(&["warrior"]).iter().all(|gap| gap != "melee"));
    let warrior = spec("warrior").expect("warrior");
    let skills: std::collections::BTreeSet<&str> = warrior.ladder.iter().map(|rung| rung.skill).collect();
    assert!(skills.contains("attack") && skills.contains("strength") && skills.contains("defence"), "{skills:?}");
}

/// Past coverage the draw is random (user decision 2026-11-13: "au-delà
/// aléatoire"), tilted toward a bot's strong group but never fixed by it.
#[test]
fn existing_levels_tilt_but_never_fix_the_draw() {
    let frank = bot("frank_lane", 11, &[("woodcutting", 64), ("fishing", 12), ("attack", 10), ("mining", 8)]);
    assert!(fit("woodcutter", &|skill| frank.level(skill)) > 50.0);
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for seed in 0..200u64 {
        let mut again = frank.clone();
        again.seed = seed;
        let (kind, why) = draw_kind(&again, &[], &[], &[]);
        assert!(!why.starts_with("levels:"), "no deterministic specialist branch: {why}");
        *counts.entry(kind).or_default() += 1;
    }
    let woodcutter = counts.get("woodcutter").copied().unwrap_or(0);
    assert!(counts.values().all(|count| *count <= woodcutter), "strong group stays the most likely: {counts:?}");
    assert!(woodcutter < 150, "but other kinds keep a real share: {counts:?}");
    assert!(counts.len() >= 6, "variety across seeds: {counts:?}");
    // Excluded (blocked) kind: the next best fit or a random draw, never it.
    assert_ne!(draw_kind(&frank, &[], &[], &["woodcutter"]).0, "woodcutter");
    // In a fleet pass, the best-fitting bot takes the matching gap.
    let mut fleet: Vec<FleetBot> = (0..14).map(|index| bot(&format!("b{index:02}"), index as u64 + 1, &[])).collect();
    fleet.push(frank.clone());
    fleet.push(bot("smithy", 99, &[("smithing", 40), ("mining", 35)]));
    let assigned = assign_fleet(&fleet, &[]);
    let assigned_to = |account: &str| assigned.iter().find(|row| row.account == account).map(|row| (row.kind.clone(), row.why.clone()));
    assert_eq!(assigned_to("frank_lane"), Some(("woodcutter".to_string(), "coverage:woodcutting".to_string())));
    assert_eq!(assigned_to("smithy").map(|(kind, _)| kind), Some("smith".to_string()));
    // Milestones skip the rungs a high-level bot is far past.
    let ambition = new_ambition("woodcutter", "coverage:woodcutting", &BotNow { level_of: &|skill| frank.level(skill), coins: 0, held: &|_| 0 }, 11, &[], ZERO).expect("woodcutter");
    let first = &ambition.milestones[0];
    assert!(matches!(&first.target, Target::Produce { item, .. } if item == "maple_logs" || item == "willow_logs"), "{first:?}");
}

/// Held ambitions count as coverage; Iron never merchants.
#[test]
fn held_ambitions_cover_and_iron_never_merchants() {
    let mut fleet: Vec<FleetBot> = GROUPS.iter().enumerate().map(|(index, _)| bot(&format!("h{index}"), index as u64, &[])).collect();
    for (row, group) in fleet.iter_mut().zip(GROUPS.iter()) {
        row.kind = crate::ambition_catalog::kind_for_group(group).map(str::to_string);
    }
    let mut iron = bot("Ironabc", 3, &[]);
    iron.ironman = true;
    fleet.push(iron);
    let mut trader = bot("trader", 4, &[]);
    trader.merchant_archetype = true;
    fleet.push(trader);
    let assigned = assign_fleet(&fleet, &[]);
    assert_eq!(assigned.len(), 2, "held ambitions are never reassigned");
    let trader_row = assigned.iter().find(|row| row.account == "trader").expect("trader");
    assert_eq!((trader_row.kind.as_str(), trader_row.why.as_str()), (MERCHANT, "coverage:merchant"));
    for seed in 0..100u64 {
        let mut iron = bot("Iron", seed, &[]);
        iron.ironman = true;
        assert_ne!(draw_kind(&iron, &["merchant".to_string()], &[], &[]).0, MERCHANT);
    }
}

/// Optional change: at most once every 72 000 ticks, never for the sole
/// holder of a group; a blocked or achieved redraw is not optional.
#[test]
fn change_throttle_holds_72000_ticks() {
    let mut life = life_with("fisher");
    let ambition = life.ambition.clone().expect("fisher");
    let myself = bot("b0", 7, &[]);
    let early = |_: i64| AMBITION_CHANGE_TICKS - 1;
    let late = |_: i64| AMBITION_CHANGE_TICKS;
    assert!(!may_change(&ambition, &early));
    assert!(may_change(&ambition, &late));
    let gaps = vec!["smithing".to_string()];
    assert_eq!(optional_change(&ambition, &myself, &gaps, true, &[], &early), None, "throttled");
    assert_eq!(optional_change(&ambition, &myself, &gaps, false, &[], &late), None, "sole holder keeps its group");
    let (kind, why) = optional_change(&ambition, &myself, &gaps, true, &[], &late).expect("a redundant fisher fills the gap");
    assert_eq!((kind.as_str(), why.as_str()), ("smith", "coverage:smithing"));
    let next = new_ambition(&kind, &why, &BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 }, 7, &[], Stamp { tick: 80_000, secs: 5_000 }).expect("smith");
    revise(&mut life, next, "change:coverage:smithing", Stamp { tick: 80_000, secs: 5_000 });
    let revised = life.ambition.as_ref().expect("revised");
    assert_eq!(revised.revisions.len(), 1);
    assert_eq!((revised.revisions[0].from.as_str(), revised.revisions[0].to.as_str()), ("fisher", "smith"));
    let since_revision = |at: i64| if at == 5_000 { 10 } else { AMBITION_CHANGE_TICKS };
    assert!(!may_change(revised, &since_revision), "the clock restarts at the revision");
}

/// A milestone blocked 3 times asks for a redraw; done quotas close.
#[test]
fn blocked_milestone_redraws_after_three_blocks() {
    let mut life = life_with("smith");
    let first = next_milestone(life.ambition.as_ref().expect("smith"), &[]).expect("a milestone").id.clone();
    for round in 1..=MILESTONE_MAX_BLOCKS {
        mark_drawn(&mut life, Some(&first));
        assert_eq!(life.goal_origin.as_deref(), Some(first.as_str()));
        let outcome = on_goal_closed(&mut life, true);
        if round < MILESTONE_MAX_BLOCKS {
            assert_eq!(outcome, CloseOutcome::Blocked { id: first.clone(), count: round });
        } else {
            assert_eq!(outcome, CloseOutcome::RedrawNeeded { id: first.clone(), count: round });
        }
    }
    let stone = life.ambition.as_ref().and_then(|ambition| ambition.milestones.iter().find(|stone| stone.id == first)).expect("stone");
    assert_eq!((stone.status, stone.blocked_count), (MilestoneStatus::Blocked, 3));
    let myself = bot("b0", 7, &[]);
    let (kind, _) = draw_kind(&myself, &["smithing".to_string()], &[], &["smith"]);
    assert_ne!(kind, "smith", "the blocked kind is excluded from the redraw");
}

/// Bans are respected: a banned skill's milestones are skipped.
#[test]
fn banned_skills_are_skipped_and_goals_follow_milestones() {
    let mut life = life_with("smith");
    let ambition = life.ambition.as_ref().expect("smith");
    let first = next_milestone(ambition, &[]).expect("open");
    assert!(matches!(&first.target, Target::Level { skill, level: 10 } if skill == "mining"), "{first:?}");
    let banned = next_milestone(ambition, &["mining".to_string()]).expect("a smithing one");
    assert!(banned.target.skills().iter().all(|skill| skill != "mining"));
    assert!(next_milestone(ambition, &["mining".to_string(), "smithing".to_string()]).is_none());
    let drawn = crate::goals::draw_goal_origin(&life, &|_| 1, &|_| 0).expect("a goal");
    assert!(matches!(drawn.0, GoalKind::MaxSkills { ref skills, level: 10 } if skills == &vec!["mining".to_string()]));
    let produce = ambition.milestones.iter().find(|stone| stone.target.kind() == "produce").expect("a quota");
    assert!(matches!(goal_for_milestone(produce), GoalKind::Produce { .. }));
    life.ambition = None;
    assert_eq!(crate::goals::draw_goal_origin(&life, &|_| 1, &|_| 0), Err(crate::goals::NoGoal::NoAmbition), "never a silent fallback");
}

/// Refresh closes met levels and keeps three milestones open; a ladder
/// run to its end is achieved; merchants chase coins.
#[test]
fn refresh_materializes_and_achieves() {
    let mut ambition = new_ambition("miner", "random", &BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 }, 3, &[], ZERO).expect("miner");
    // Whole rungs materialize (level + quota): at least three open.
    assert!(ambition.milestones.iter().filter(|stone| stone.status != MilestoneStatus::Done).count() >= 3);
    let maxed = |_: &str| 99u8;
    for stone in &mut ambition.milestones {
        stone.status = MilestoneStatus::Done;
    }
    for _ in 0..20 {
        refresh(&mut ambition, &BotNow { level_of: &maxed, coins: 0, held: &|_| 0 }, 3, &[]);
        for stone in &mut ambition.milestones {
            stone.status = MilestoneStatus::Done;
        }
    }
    assert!(achieved(&ambition));
    let mut merchant = new_ambition(MERCHANT, "coverage:merchant", &BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 }, 1, &[], ZERO).expect("merchant");
    assert!(matches!(merchant.milestones[0].target, Target::Coins { amount: 5_000 }));
    assert!(matches!(goal_for_milestone(&merchant.milestones[0]), GoalKind::GetRich { merchant: Some(true) }));
    refresh(&mut merchant, &BotNow { level_of: &|_| 1, coins: 30_000, held: &|_| 0 }, 1, &[]);
    assert_eq!(merchant.milestones[0].status, MilestoneStatus::Done);
    assert_eq!(merchant.milestones[1].progress, (20_000, 20_000));
}

/// Buy orders motivate production: an open bid the ambition can serve
/// becomes a produce milestone ahead of the ladder, sized by the unmet
/// quantity, even when the bot must train first.
#[test]
fn open_bids_become_demand_milestones() {
    let demand = vec![DemandItem { item: "bronze_platebody".to_string(), weight: 900.0, unmet: 12, margin: Some(10.0) }];
    let mut ambition = new_ambition("smith", "random", &BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 }, 5, &[], ZERO).expect("smith");
    refresh(&mut ambition, &BotNow { level_of: &levels(&[("smithing", 9)]), coins: 0, held: &|_| 0 }, 5, &demand);
    let next = next_milestone(&ambition, &[]).expect("open");
    assert_eq!(next.origin, "demand:bronze_platebody");
    assert!(matches!(&next.target, Target::Produce { item, count: 12, skill } if item == "bronze_platebody" && skill == "smithing"));
    refresh(&mut ambition, &BotNow { level_of: &levels(&[("smithing", 9)]), coins: 0, held: &|_| 0 }, 5, &demand);
    let demanded = ambition.milestones.iter().filter(|stone| stone.origin.starts_with("demand:")).count();
    assert_eq!(demanded, 1, "one open demand milestone at a time");
}

/// Every catalogue product maps to a production leg (made, never bought)
/// and every rung's product is within its rung level.
#[test]
fn catalogue_products_are_producible() {
    for entry in SPECS {
        for rung in entry.ladder {
            if let Then::Produce(items, count, _) = rung.then {
                assert!(count > 0);
                for item in items {
                    let legs = crate::obtain_legs::produce_alternatives(item);
                    assert!(!legs.is_empty(), "{}: {item} has no production leg", entry.kind);
                    assert!(legs.iter().all(|leg| !leg.method.starts_with("market:") && !leg.method.starts_with("shop:")));
                    for leg in crate::obtain_legs::production_legs(item) {
                        if let Some((_, needed)) = leg.skill {
                            assert!(rung.level == 0 || needed <= i64::from(rung.level), "{}: {item} needs {needed} > rung {}", entry.kind, rung.level);
                        }
                    }
                }
            }
        }
    }
    assert!(products("crafter").len() >= 8, "varied crafting products");
}

/// The produce goal expands to production legs only.
#[test]
fn produce_goal_never_buys_its_product() {
    let profile = Profile::new(false, 0.5, 0.5, 1, Archetype::Producer);
    let nodes = crate::expand::expand_goal(&GoalKind::Produce { item: "bronze_platebody".to_string(), count: 20 }, &profile);
    assert_eq!(nodes.len(), 1);
    let methods: Vec<&str> = nodes[0].alternatives.iter().map(|alt| alt.method.as_str()).collect();
    assert_eq!(methods, vec!["smith:bronze_platebody"]);
    assert_eq!(crate::goals::goal_skills(&GoalKind::Produce { item: "bronze_platebody".to_string(), count: 20 }), vec!["smithing".to_string()]);
}

/// Old files (no `ambition`) load; the section round-trips exactly.
#[test]
fn life_file_keeps_the_ambition() {
    let mut life = life_with("crafter");
    mark_drawn(&mut life, Some("m1"));
    let value = crate::model::life_to_value(&life);
    let back = crate::model::life_from_value(&value).expect("parses");
    assert_eq!(back, life);
    let section = value.get("ambition");
    for key in ["kind", "label", "groups", "why", "milestones", "drawn_at_tick", "revisions"] {
        assert!(!matches!(section.get(key), lifescape_json::Value::Null), "contract key {key}");
    }
    let stone = section.get("milestones").as_arr().and_then(|rows| rows.first().cloned()).expect("a milestone");
    for key in ["id", "label", "kind", "target", "status", "progress", "blocked_count"] {
        assert!(!matches!(stone.get(key), lifescape_json::Value::Null), "milestone key {key}");
    }
    assert_eq!(value.get("goal_origin").as_str(), Some("m1"));
    let mut old = value.clone();
    if let lifescape_json::Value::Obj(map) = &mut old {
        map.remove("ambition");
        map.remove("goal_origin");
    }
    let legacy = crate::model::life_from_value(&old).expect("old files load");
    assert!(legacy.ambition.is_none() && legacy.goal_origin.is_none());
}
