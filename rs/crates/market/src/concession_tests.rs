//! Concession curve (plan marché M3/M4, Q2/Q4) and the 10 % walk of
//! every resting order (user 2026-11-19).

use super::*;

fn curve(opening: i64, limit: i64, beta: f64) -> Concession {
    Concession { opening, limit, horizon_ticks: 36_000, beta }
}

fn window(sell: bool, current: i64, curve: Concession, elapsed: u64) -> Window {
    Window {
        sell,
        current,
        curve,
        elapsed_ticks: elapsed,
        window_ticks: window_ticks(curve.horizon_ticks),
        filled: false,
        anchor_price: None,
        own_fill: None,
        best_opposite: None,
        pressed: false,
    }
}

#[test]
fn prices_never_leave_opening_and_limit() {
    for (opening, limit) in [(100, 40), (40, 100), (7, 7), (1, 3), (3, 1)] {
        for beta in [MIN_BETA, 0.5, 1.0, 2.0, MAX_BETA, f64::NAN] {
            let shape = curve(opening, limit, beta);
            for elapsed in (0..=40_000).step_by(500) {
                let price = shape.price_at(elapsed);
                assert!(price >= opening.min(limit) && price <= opening.max(limit), "{opening}->{limit} beta {beta} t {elapsed}: {price}");
            }
        }
    }
}

#[test]
fn without_a_fill_the_price_only_moves_toward_the_limit() {
    for (opening, limit) in [(100, 40), (40, 100)] {
        let shape = curve(opening, limit, 1.3);
        let mut last = shape.price_at(0);
        for elapsed in (0..=36_000).step_by(100) {
            let price = shape.price_at(elapsed);
            if limit < opening {
                assert!(price <= last, "seller climbed at {elapsed}");
            } else {
                assert!(price >= last, "buyer fell at {elapsed}");
            }
            last = price;
        }
    }
}

#[test]
fn the_limit_is_reached_at_the_horizon() {
    for beta in [MIN_BETA, 0.5, 1.0, 2.0, MAX_BETA] {
        assert_eq!(curve(100, 40, beta).price_at(36_000), 40);
        assert_eq!(curve(40, 100, beta).price_at(36_000), 100);
        assert_eq!(curve(40, 100, beta).price_at(90_000), 100);
        assert_eq!(curve(40, 100, beta).price_at(0), 40, "opens at the opening");
    }
}

#[test]
fn patient_bots_concede_late_pressed_ones_early() {
    let patient = concession_beta(0.95, Urgency::Normal);
    let hasty = concession_beta(0.05, Urgency::Normal);
    assert!(patient < 1.0 && hasty > 1.0, "{patient} {hasty}");
    assert!((concession_beta(0.5, Urgency::Normal) - 1.0).abs() < 1e-9, "linear at mid patience");
    assert!(concession_beta(0.5, Urgency::Pressed) > concession_beta(0.5, Urgency::Normal));
    assert!(concession_beta(0.5, Urgency::Patient) < concession_beta(0.5, Urgency::Normal));
    assert!(concession_beta(1.0, Urgency::Patient) >= MIN_BETA && concession_beta(0.0, Urgency::Pressed) <= MAX_BETA);
    // Halfway through the horizon: beta < 1 has conceded less than
    // linear, beta > 1 more.
    let boulware = curve(40, 100, 0.5).price_at(18_000);
    let linear = curve(40, 100, 1.0).price_at(18_000);
    let conceder = curve(40, 100, 2.0).price_at(18_000);
    assert!(boulware < linear && linear < conceder, "{boulware} {linear} {conceder}");
}

#[test]
fn small_moves_wait_for_the_amend_threshold() {
    assert_eq!(amend_threshold(50), 1);
    assert_eq!(amend_threshold(1000), 10);
    assert_eq!(amend_threshold(1001), 11);
}

/// User 2026-11-19 (« la règle des 10 % vaut pour tout le monde en bid et
/// ask, sinon les prix ne sont pas réels »): live yew logs, carl_miller
/// listed at 68 over its own cost 34, walked down to 34 and sat there,
/// withdrawn and relisted at 68 eight times in a row, nothing bought for
/// 7 h. An unsold ask drops 10 % per window (at least a coin), under its
/// own cost, down to one coin; it never withdraws.
#[test]
fn an_unsold_ask_drops_ten_percent_per_window_down_to_one() {
    let own_cost = curve(68, 34, 1.0);
    assert_eq!(step(&window(true, 68, own_cost, 2000)), Step::Amend(61));
    assert_eq!(step(&window(true, 34, own_cost, 36_000)), Step::Amend(30), "under its own cost");
    assert_eq!(step(&window(true, 5, own_cost, 90_000)), Step::Amend(4), "at least a coin");
    assert_eq!(step(&window(true, 1, own_cost, 90_000)), Step::Hold, "one coin rests, never withdrawn");
    let mut current = 68;
    let mut windows = 0;
    while let Step::Amend(next) = step(&window(true, current, own_cost, windows * 2000)) {
        assert!(next < current, "{current} -> {next}");
        current = next;
        windows += 1;
    }
    assert_eq!(current, 1);
    assert!(windows <= 40, "68 to 1 in {windows} windows");
}

/// The same rule for bids: an unfilled bid climbs 10 % per window (at
/// least a coin), up to what the buyer can pay.
#[test]
fn an_unfilled_bid_climbs_ten_percent_per_window_up_to_its_ceiling() {
    let ceiling = curve(50, 120, 1.0);
    assert_eq!(step(&window(false, 50, ceiling, 500)), Step::Amend(55));
    assert_eq!(step(&window(false, 5, ceiling, 500)), Step::Amend(6), "at least a coin");
    assert_eq!(step(&window(false, 115, ceiling, 500)), Step::Amend(120), "never over its ceiling");
    assert_eq!(step(&window(false, 120, ceiling, 90_000)), Step::Hold, "a buyer stays at its ceiling");
}

#[test]
fn an_own_fill_backs_off_and_restarts() {
    let mut seen = window(true, 100, curve(120, 40, 1.0), 6000);
    seen.filled = true;
    assert_eq!(step(&seen), Step::Restart { opening: 105 }, "a seller that sold was cheap");
    let mut bought = window(false, 100, curve(80, 150, 1.0), 6000);
    bought.filled = true;
    assert_eq!(step(&bought), Step::Restart { opening: 95 }, "a buyer that bought paid too much");
    assert_eq!(zip_backoff(false, 1, 1), 1, "never under 1");
}

#[test]
fn public_prints_pull_the_quote_halfway() {
    let mut seen = window(false, 50, curve(50, 120, 0.5), 2000);
    seen.anchor_price = Some(90);
    assert_eq!(step(&seen), Step::Amend(70), "others paid 90: halfway, past the 10 % walk");
    seen.anchor_price = Some(200);
    assert_eq!(step(&seen), Step::Amend(55), "a print over the ceiling never pulls: the walk goes on");
    seen.anchor_price = Some(30);
    assert_eq!(step(&seen), Step::Amend(55), "a cheaper print never lowers a bid: the walk goes on");
}

#[test]
fn pressed_orders_cross_and_any_order_takes_what_its_next_walk_reaches() {
    let mut buyer = window(false, 60, curve(60, 120, 2.0), 500);
    buyer.best_opposite = Some(110);
    assert_eq!(step(&buyer), Step::Amend(66), "a normal buyer walks 10 %");
    buyer.pressed = true;
    assert_eq!(step(&buyer), Step::Amend(110), "a pressed buyer takes the ask");
    buyer.best_opposite = Some(130);
    assert_eq!(step(&buyer), Step::Amend(66), "never over its ceiling");
    let mut seller = window(true, 100, curve(100, 50, 1.0), 500);
    seller.best_opposite = Some(45);
    seller.pressed = true;
    assert_eq!(step(&seller), Step::Amend(45), "a full bank hits the bid, no floor");
    let mut close = window(true, 60, curve(100, 50, 1.0), 500);
    close.best_opposite = Some(55);
    assert_eq!(step(&close), Step::Amend(55), "its next walk (54) passes the bid: it takes it");
}

/// B5 (lot 3 marché): near the volume-weighted mean price of the recent
/// trades the walk shrinks, proportionally to the relative gap, inside
/// [1 %, 10 %]; without a mean price it is the plain 10 %.
#[test]
fn the_step_shrinks_near_the_mean_price() {
    assert_eq!(step_pct(100, None), WALK_PCT, "no mean price known: 10 %");
    assert_eq!(step_pct(100, Some(100)), MIN_WALK_PCT, "on it: 1 %");
    assert_eq!(step_pct(103, Some(100)), 3, "3 % away: 3 %");
    assert_eq!(step_pct(90, Some(100)), 10);
    assert_eq!(step_pct(500, Some(100)), WALK_PCT, "far: never over 10 %");
    assert_eq!(step_pct(40, Some(100)), WALK_PCT);
    assert_eq!(step_pct(100, Some(0)), WALK_PCT, "a nonsense mean never shrinks the step");
}

#[test]
fn a_walk_close_to_the_mean_moves_a_percent_not_ten() {
    let shape = curve(1000, 1, 1.0);
    let mut near = window(true, 1000, shape, 500);
    near.anchor_price = Some(1000);
    assert_eq!(step(&near), Step::Amend(990), "an ask on the mean price: 1 %");
    near.anchor_price = Some(950);
    assert_eq!(step(&near), Step::Amend(940), "5.3 % over the mean: a 6 % step (the pull would only reach 975)");
    near.anchor_price = None;
    assert_eq!(step(&near), Step::Amend(900), "no mean: 10 %");
    let bid_curve = curve(1000, 5000, 1.0);
    let mut buyer = window(false, 1000, bid_curve, 500);
    buyer.anchor_price = Some(1000);
    assert_eq!(step(&buyer), Step::Amend(1010), "a bid on the mean climbs 1 %");
    assert_eq!(step(&window(true, 5, curve(5, 1, 1.0), 500)), Step::Amend(4), "still at least a coin");
}

#[test]
fn windows_are_spread_per_order() {
    assert_eq!(window_ticks(6000), MIN_WINDOW_TICKS);
    assert_eq!(window_ticks(36_000), MAX_WINDOW_TICKS);
    let offsets: std::collections::HashSet<u64> = (0..20).map(|index| window_offset(&format!("bot{index}"), 314, 2000)).collect();
    assert!(offsets.len() >= 15, "spread: {offsets:?}");
    assert!(offsets.iter().all(|offset| *offset < 2000));
    assert_eq!(window_offset("bot1", 314, 2000), window_offset("bot1", 314, 2000), "stable");
}
