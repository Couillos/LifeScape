use super::*;

fn contract(method: &str, work_ticks: i64, units: Vec<(&str, &str, i64)>) -> ParsedContract {
    ParsedContract {
        planner_method: method.to_string(),
        method_source: "node".to_string(),
        site: "3087,3227,0".to_string(),
        target: String::new(),
        tool: "small_fishing_net".to_string(),
        combat_style: String::new(),
        work_ticks,
        pause_ticks: 0,
        levels: vec![("fishing".to_string(), 12), ("magic".to_string(), 1), ("attack".to_string(), 7), ("strength".to_string(), 23)],
        flows: units.iter().filter(|(kind, _, _)| *kind == "product").map(|(_, name, qty)| (name.to_string(), "product".to_string(), *qty)).collect(),
        units: units.iter().map(|(kind, name, qty)| (kind.to_string(), name.to_string(), *qty)).collect(),
        alerts: Vec::new(),
    }
}

/// The D2 key: planner method, style, site, tool, band of the method's
/// skill at the start (fishing 12 -> band 11).
#[test]
fn key_is_method_site_tool_band() {
    let report = contract("gather:fishing/raw_shrimp", 1200, vec![("product", "Raw shrimps", 100)]);
    let key = key_of(&report).expect("key");
    assert_eq!(key, YieldKey { method: "gather:fishing/raw_shrimp".to_string(), style: String::new(), site: "3087,3227,0".to_string(), tool: "small_fishing_net".to_string(), band: 11 });
    let mut fight = contract("fight:melee/cows", 1200, vec![]);
    fight.combat_style = "melee:strength".to_string();
    assert_eq!(key_of(&fight).map(|key| key.band), Ok(21), "melee bands on the trained skill");
    assert_eq!(band_for("milk:bucket_milk", "", 40), 0, "no skill, no band");
    fight.combat_style = "melee:controlled".to_string();
    assert_eq!(key_of(&fight).map(|key| key.band), Ok(6), "controlled bands on attack (7)");
    assert!(key_of(&contract("", 600, vec![])).is_err(), "no planner method, never guessed");
    assert!(key_of(&contract("market:buy", 600, vec![])).is_err(), "trading never rates");
    assert!(key_of(&contract("quest:cooks_assistant", 600, vec![])).is_err());
}

/// Failed and blocked runs never rate; a run above the server's ceiling
/// is rejected; a normal run is rated.
#[test]
fn admission_rejects_the_impossible() {
    let normal = contract("gather:fishing/raw_shrimp", 1200, vec![("product", "Raw shrimps", 100)]);
    assert!(matches!(admit(&normal, "budget_exhausted", 5), Admission::Rated(_, _)));
    assert!(matches!(admit(&normal, "failed", 5), Admission::Failed(_, 1200)));
    assert!(matches!(admit(&normal, "blocked", 5), Admission::Failed(_, 1200)));
    // A roll every 6 ticks: 1200 ticks hold 200 shrimps at most.
    let impossible = contract("gather:fishing/raw_shrimp", 1200, vec![("product", "Raw shrimps", 400)]);
    assert!(matches!(admit(&impossible, "goal_reached", 5), Admission::Rejected(_, _)), "{:?}", admit(&impossible, "goal_reached", 5));
    let edge = contract("gather:fishing/raw_shrimp", 1200, vec![("product", "Raw shrimps", 201)]);
    assert!(matches!(admit(&edge, "goal_reached", 5), Admission::Rated(_, _)), "one roll of slack");
    let none = contract("", 1200, vec![]);
    assert!(matches!(admit(&none, "goal_reached", 5), Admission::Skipped(_)));
}

/// D4: derived XP from the units against the observed XP; a gap above
/// 5 % is reported, a match is not.
#[test]
fn xp_coherence_flags_gaps_above_five_percent() {
    let report = contract("gather:fishing/raw_shrimp", 1200, vec![("product", "Raw shrimps", 30)]);
    assert!(xp_gaps(&report, &[("fishing".to_string(), 300)]).is_empty());
    let gaps = xp_gaps(&report, &[("fishing".to_string(), 400), ("attack".to_string(), 50)]);
    assert_eq!(gaps.len(), 2, "{gaps:?}");
    assert_eq!(gaps[0], XpGap { skill: "attack".to_string(), derived: 0.0, observed: 50.0 });
    let mut fight = contract("fight:melee/cows", 1200, vec![("damage", "cow", 100), ("kill", "cow", 12)]);
    fight.combat_style = "melee:strength".to_string();
    assert!(xp_gaps(&fight, &[("strength".to_string(), 400), ("hitpoints".to_string(), 133)]).is_empty());
}

/// R2.1: a drop farm lands on the fight it ran, whatever object the
/// planner wanted: feathers and arrows farmed on goblins in the same
/// style are one bucket.
#[test]
fn a_drop_farm_keys_the_fight_it_ran() {
    let mut arrows = contract("farm:bronze_arrow", 1200, vec![("damage", "goblin", 40), ("kill", "goblin", 8)]);
    arrows.combat_style = "melee:attack".to_string();
    arrows.target = "goblins".to_string();
    let mut runes = arrows.clone();
    runes.planner_method = "farm:naturerune".to_string();
    let key = key_of(&arrows).expect("fight key");
    assert_eq!(key.method, "fight:melee/goblins");
    assert_eq!(key_of(&runes), Ok(key.clone()), "same fight, same bucket");
    assert_eq!(key.band, 6, "bands on the trained skill (attack 7)");
    let mut salmon = contract("farm:raw_salmon", 1200, vec![("product", "Raw salmon", 10)]);
    salmon.levels.push(("fishing".to_string(), 31));
    assert_eq!(key_of(&salmon).map(|key| key.method), Ok("gather:fishing/raw_trout".to_string()), "one spot option");
}

/// R2.1: market trades, shop legs and unattributed flows never enter a
/// sample; every yield cause does.
#[test]
fn only_yield_causes_enter_a_sample() {
    let mut report = contract("gather:fishing/raw_shrimp", 1200, vec![("product", "Raw shrimps", 100)]);
    for cause in ["traded", "shop_bought", "shop_sold", "unattributed", "loot", "food", "fee", "death"] {
        report.flows.push(("Coins".to_string(), cause.to_string(), -5));
    }
    let Admission::Rated(_, sample) = admit(&report, "budget_exhausted", 5) else { panic!("rated") };
    let causes: Vec<&str> = sample.flows.iter().map(|(_, cause, _)| cause.as_str()).collect();
    assert_eq!(causes, vec!["product", "loot", "food", "fee", "death"]);
    for excluded in ["traded", "shop_bought", "shop_sold", "unattributed"] {
        assert!(!rated_cause(excluded), "{excluded}");
    }
}
