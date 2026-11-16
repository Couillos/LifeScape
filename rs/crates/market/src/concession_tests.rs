//! Concession curve (plan marché M3/M4, Q2/Q4).

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
        public_price: None,
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
    // Stepping windows: same monotony, stays inside the bounds.
    let shape = curve(100, 40, 1.0);
    let mut current = 100;
    for index in 0..30 {
        if let Step::Amend(next) = step(&window(true, current, shape, index * 2000)) {
            assert!(next <= current && next >= 40, "{current} -> {next}");
            current = next;
        }
    }
    assert_eq!(current, 40);
}

#[test]
fn the_limit_is_reached_at_the_horizon() {
    for beta in [MIN_BETA, 0.5, 1.0, 2.0, MAX_BETA] {
        assert_eq!(curve(100, 40, beta).price_at(36_000), 40);
        assert_eq!(curve(40, 100, beta).price_at(36_000), 100);
        assert_eq!(curve(40, 100, beta).price_at(90_000), 100);
        assert_eq!(curve(40, 100, beta).price_at(0), 40, "opens at the opening");
    }
    // One coin from the limit at the horizon: the last move lands even
    // under the 1 % threshold.
    assert_eq!(step(&window(false, 99, curve(40, 100, 1.0), 36_000)), Step::Amend(100));
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
    // A curve from 1000 to 900 over 36 000 ticks moves ~2.8 coins per
    // 1000 ticks: under 1 % (10 coins) it holds.
    let shape = curve(1000, 900, 1.0);
    assert_eq!(step(&window(true, 1000, shape, 1000)), Step::Hold);
    assert_eq!(step(&window(true, 1000, shape, 4000)), Step::Amend(989));
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
    seen.public_price = Some(90);
    assert_eq!(step(&seen), Step::Amend(70), "others paid 90: halfway");
    seen.public_price = Some(200);
    assert_ne!(step(&seen), Step::Amend(125), "never past the limit");
    seen.public_price = Some(30);
    assert!(matches!(step(&seen), Step::Hold | Step::Amend(_)), "a cheaper print never lowers a bid");
    if let Step::Amend(price) = step(&seen) {
        assert!(price >= 50);
    }
}

#[test]
fn pressed_orders_cross_inside_their_limit() {
    let mut buyer = window(false, 60, curve(60, 120, 2.0), 500);
    buyer.best_opposite = Some(110);
    assert!(matches!(step(&buyer), Step::Amend(price) if price < 110), "a normal buyer walks its curve: {:?}", step(&buyer));
    buyer.pressed = true;
    assert_eq!(step(&buyer), Step::Amend(110), "a pressed buyer takes the ask");
    buyer.best_opposite = Some(130);
    assert_ne!(step(&buyer), Step::Amend(130), "never over its ceiling");
    let mut seller = window(true, 100, curve(100, 50, 1.0), 500);
    seller.best_opposite = Some(70);
    seller.pressed = true;
    assert_eq!(step(&seller), Step::Amend(70), "a full bank hits the bid");
    seller.best_opposite = Some(45);
    assert_ne!(step(&seller), Step::Amend(45), "never under its floor");
    // A patient order takes the opposite its curve reaches next window.
    let mut close = window(true, 60, curve(100, 50, 1.0), 34_500);
    close.best_opposite = Some(51);
    assert_eq!(step(&close), Step::Amend(51));
}

#[test]
fn q4_sellers_withdraw_at_the_horizon_buyers_stay() {
    assert_eq!(step(&window(true, 40, curve(100, 40, 1.0), 36_000)), Step::Withdraw);
    assert_eq!(step(&window(true, 40, curve(100, 40, 1.0), 20_000)), Step::Hold, "before H it rests");
    assert_eq!(step(&window(false, 100, curve(40, 100, 1.0), 90_000)), Step::Hold, "a buyer stays at its limit");
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
