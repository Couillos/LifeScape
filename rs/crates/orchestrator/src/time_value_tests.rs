//! D1: a bot's time is worth what its work really earned per tick
//! (flows valued at the current price over its work ticks), the fleet's
//! measured mean without a measure of its own, never under the floor.

use lifescape_planner::market_fit::MIN_COIN_RATE;
use lifescape_store::AccountWork;

use super::{rate_of_source, Measured, RateSource, TimeValues, MIN_MEASURED_WORK_TICKS};

fn work(account: &str, work_ticks: i64, flows: &[(&str, i64)]) -> AccountWork {
    AccountWork { account: account.to_string(), work_ticks, flows: flows.iter().map(|(item, quantity)| (item.to_string(), *quantity)).collect() }
}

/// Current prices of the test: logs 2, oak logs 10, a bronze axe 30,
/// coins 1.
fn price(item: &str) -> f64 {
    match item {
        "Coins" => 1.0,
        "Logs" => 2.0,
        "Oak logs" => 10.0,
        "Bronze axe" => 30.0,
        _ => 0.0,
    }
}

fn values(works: &[AccountWork]) -> TimeValues {
    let fleet_member = |account: &str| !account.eq_ignore_ascii_case("admin");
    TimeValues::from_work(works, &mut |item: &str| price(item), &fleet_member)
}

/// A measured bot reads its own gain per tick of work: products and
/// coins in at the current price, what it sold or spent out.
#[test]
fn a_measured_bot_reads_its_own_gain_per_work_tick() {
    // 3000 logs (6000) + 200 oak logs (2000) + 4000 coins of shop sales
    // - 1000 logs sold (2000) - one axe bought for 30 coins + the axe
    // (30) = 10 000 coins over 10 000 work ticks: 1 coin per tick.
    let found = values(&[work("ann", 10_000, &[("Logs", 2000), ("Oak logs", 200), ("Coins", 3970), ("Bronze axe", 1)])]);
    assert_eq!(found.source("ann"), RateSource::Own(Measured { value: 10_000.0, work_ticks: 10_000 }));
    assert!((found.rate("ann") - 1.0).abs() < 1e-12, "{}", found.rate("ann"));
    assert!((found.rate("ANN") - 1.0).abs() < 1e-12, "account names ignore case");
}

/// A bot with no measure (absent, or under the minimum work) reads the
/// fleet's measured mean: the fleet's value over the fleet's work ticks,
/// staff left out.
#[test]
fn a_bot_without_a_measure_reads_the_fleet_mean() {
    let found = values(&[
        work("ann", 10_000, &[("Coins", 6000)]),
        work("bob", 20_000, &[("Logs", 3000)]),
        work("admin", 50_000, &[("Coins", 1_000_000)]),
        work("cid", MIN_MEASURED_WORK_TICKS - 1, &[("Coins", 50_000)]),
    ]);
    // (6000 + 6000 + 50 000) / (10 000 + 20 000 + 5999): cid's work
    // counts in the fleet, admin's never.
    let fleet = Measured { value: 62_000.0, work_ticks: 30_000 + MIN_MEASURED_WORK_TICKS - 1 };
    assert_eq!(found.source("dan"), RateSource::Fleet(fleet), "absent");
    assert_eq!(found.source("cid"), RateSource::Fleet(fleet), "under the minimum work");
    assert!((found.rate("dan") - fleet.per_tick()).abs() < 1e-12);
    assert!((found.rate("bob") - 0.3).abs() < 1e-12, "bob is measured: 6000 / 20 000");
    assert_eq!(found.source("admin"), RateSource::Own(Measured { value: 1_000_000.0, work_ticks: 50_000 }), "staff read their own measure");
}

/// Offline read of the time values of a COPY of the live store (never the
/// live file: opening migrates). Ignored by default; run with
/// `LIFESCAPE_CHECK_DB=<copy of run/lifescape.db>
/// LIFESCAPE_CHECK_ACCOUNTS=<a,b,c> cargo test --release -p
/// lifescape-orchestrator time_value_of_a_copy -- --ignored --nocapture`.
#[test]
#[ignore]
fn time_value_of_a_copy() {
    let Ok(db) = std::env::var("LIFESCAPE_CHECK_DB") else {
        eprintln!("time_value_of_a_copy: set LIFESCAPE_CHECK_DB");
        return;
    };
    let store = lifescape_store::Store::open(std::path::Path::new(&db)).expect("db copy opens");
    let started = std::time::Instant::now();
    // The window ends at the copy's newest report: an old copy is never
    // read empty (every bot at the floor).
    let copy_end = store.newest_report_clock().expect("newest report").expect("a report in the copy");
    let values = super::measure_in(&store, copy_end).expect("measure");
    eprintln!("fleet {:?} ({} accounts measured, {} ms)", values.fleet().map(|fleet| fleet.per_tick()), values.measured_accounts(), started.elapsed().as_millis());
    for account in std::env::var("LIFESCAPE_CHECK_ACCOUNTS").unwrap_or_default().split(',').filter(|name| !name.is_empty()) {
        eprintln!("{account}: {:.3} coins/tick ({:?})", values.rate(account), values.source(account));
    }
}

/// A tick is never free: a loss, a gain under the floor, or no measure
/// anywhere reads `MIN_COIN_RATE`.
#[test]
fn the_rate_never_falls_under_the_floor() {
    let found = values(&[work("ann", 10_000, &[("Logs", -500)]), work("bob", 10_000, &[("Coins", 100)])]);
    assert_eq!(found.rate("ann"), MIN_COIN_RATE, "a loss");
    assert_eq!(found.rate("bob"), MIN_COIN_RATE, "0.01 per tick");
    assert_eq!(found.rate("dan"), MIN_COIN_RATE, "the fleet mean itself under the floor");
    let empty = values(&[]);
    assert_eq!(empty.source("ann"), RateSource::Floor);
    assert_eq!(empty.rate("ann"), MIN_COIN_RATE, "nothing measured anywhere");
    assert_eq!(rate_of_source(RateSource::Own(Measured { value: 5000.0, work_ticks: 10_000 })), 0.5);
}
