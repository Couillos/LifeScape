use super::{ServeContext, ServeVerdict};

/// Cut diamond: cutting is fast, the uncut gem only comes from slow drops.
fn rates(method: &str, _item: &str) -> f64 {
    match method {
        "cut:diamond" => 1200.0,
        "farm:uncut_diamond" => 3.0,
        _ => 0.0,
    }
}

fn verdict(bid: f64, ask: Option<f64>, units: &dyn Fn(&str, &str) -> f64, time_value: f64) -> ServeVerdict {
    let ask_of = move |item: &str| if item == "uncut_diamond" { ask } else { None };
    ServeContext { units, ask_of: &ask_of, time_value }.verdict("diamond", bid)
}

/// Inputs only a slow drop gives (2000 ticks a gem): today's bid does not
/// pay the bot's time, a high bid does.
#[test]
fn slow_drops_do_not_pay_at_today_price_and_pay_at_a_high_bid() {
    assert!(matches!(verdict(100.0, None, &rates, 0.5), ServeVerdict::Unpaying { .. }));
    assert!(matches!(verdict(5000.0, None, &rates, 0.5), ServeVerdict::Pays { .. }));
}

/// A real ask below the value of the bot's own ticks is the input cost.
#[test]
fn a_cheap_ask_beats_making_the_input() {
    assert!(matches!(verdict(100.0, Some(20.0), &rates, 0.5), ServeVerdict::Pays { .. }));
    assert!(matches!(verdict(100.0, Some(99.0), &rates, 0.5), ServeVerdict::Unpaying { .. }));
}

/// No rate, a zero rate or a NaN decides nothing.
#[test]
fn missing_or_zero_data_decides_nothing() {
    let zero = |_: &str, _: &str| 0.0;
    let nan = |_: &str, _: &str| f64::NAN;
    assert_eq!(verdict(100.0, None, &zero, 0.5), ServeVerdict::NoData);
    assert_eq!(verdict(100.0, None, &nan, 0.5), ServeVerdict::NoData);
    // The input has no rate and no ask: the unit cannot be costed.
    let cut_only = |method: &str, _: &str| if method == "cut:diamond" { 1200.0 } else { 0.0 };
    assert_eq!(verdict(100.0, None, &cut_only, 0.5), ServeVerdict::NoData);
    assert_eq!(verdict(f64::NAN, None, &rates, 0.5), ServeVerdict::NoData);
}

fn crafter_with_demand() -> crate::ambition::Ambition {
    let now = crate::ambition::BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 };
    let mut ambition = crate::ambition::new_ambition("crafter", "random", &now, 3, &[], crate::ambition::Stamp { tick: 0, secs: 0 }).expect("crafter");
    ambition.milestones.retain(|stone| !stone.origin.starts_with("demand:"));
    ambition.milestones.insert(
        0,
        crate::ambition::Milestone {
            id: "m90".to_string(),
            label: "produce diamond".to_string(),
            target: crate::ambition::Target::Produce { item: "diamond".to_string(), count: 100, skill: "crafting".to_string() },
            status: crate::ambition::MilestoneStatus::Pending,
            progress: (0, 100),
            blocked_count: 0,
            origin: "demand:diamond".to_string(),
        },
    );
    ambition
}

/// An unpaying demand milestone is set aside (kept, never drawn); the
/// next milestone is drawn; a paying or undecided one stays drawable.
#[test]
fn an_unpaying_milestone_is_set_aside_and_the_ladder_goes_on() {
    let unpaying = |_: &str, _: &crate::ambition::Milestone| ServeVerdict::Unpaying { per_tick: 0.01, time_value: 0.5 };
    let mut ambition = crafter_with_demand();
    let aside = super::set_aside_unpaying(&mut ambition, &unpaying, None, true);
    assert_eq!(aside.iter().map(|entry| entry.0.as_str()).collect::<Vec<_>>(), ["m90"]);
    let stone = ambition.milestones.iter().find(|stone| stone.id == "m90").expect("kept");
    assert!(crate::ambition::set_aside(stone));
    assert_ne!(crate::ambition::next_milestone(&ambition, &[]).map(|next| next.id.as_str()), Some("m90"));
    for kept in [ServeVerdict::Pays { per_tick: 9.0 }, ServeVerdict::NoData] {
        let mut ambition = crafter_with_demand();
        let judge = move |_: &str, _: &crate::ambition::Milestone| kept;
        assert!(super::set_aside_unpaying(&mut ambition, &judge, None, true).is_empty());
        assert!(ambition.milestones.iter().any(|stone| stone.id == "m90" && crate::ambition::drawable(stone)));
    }
}

/// The milestone a node is running waits for its end.
#[test]
fn the_running_milestone_waits() {
    let unpaying = |_: &str, _: &crate::ambition::Milestone| ServeVerdict::Unpaying { per_tick: 0.01, time_value: 0.5 };
    let mut ambition = crafter_with_demand();
    assert!(super::set_aside_unpaying(&mut ambition, &unpaying, Some("m90"), false).is_empty());
    assert_eq!(super::set_aside_unpaying(&mut ambition, &unpaying, Some("m90"), true).len(), 1);
}
