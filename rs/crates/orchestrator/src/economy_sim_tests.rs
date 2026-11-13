//! End to end, offline (2026-11-13 user rules): a fighter's ladder bid
//! (wallet share ceiling, `gear_ladder`) walks up from the 1-coin
//! bootstrap on the real book (`lifescape_market::sim_demand`) until a
//! smith's `chain_profit` (real recipe, bars at market) turns positive;
//! the smith then makes and sells the piece at a profit. A wallet whose
//! share cannot cover the bars draws no production: the fighter must
//! earn coins first (`have_coins`).

use lifescape_market::sim_demand::{run_demand, Agent};
use lifescape_planner::gear_ladder::{coins_for, wallet_ceiling, GEAR_SHARE_PCT};
use lifescape_planner::profit::chain_profit;

const BAR_PRICE: f64 = 15.0;

fn smith_margin(price: i64) -> f64 {
    let prices = vec![("iron_scimitar".to_string(), price as f64), ("iron_bar".to_string(), BAR_PRICE)];
    chain_profit("iron_scimitar", &prices, 100.0).map(|verdict| verdict.margin).unwrap_or(f64::MIN)
}

fn input_cost() -> f64 {
    chain_profit("iron_scimitar", &[("iron_scimitar".to_string(), 1.0), ("iron_bar".to_string(), BAR_PRICE)], 100.0)
        .expect("iron scimitars are smithed")
        .input_cost
}

fn fighter_and_smith(wallet: i64) -> Vec<Agent> {
    vec![
        Agent::Buyer { name: "fighter".to_string(), units: 1, ceiling: wallet_ceiling(wallet, GEAR_SHARE_PCT), arrives: 0 },
        Agent::Producer { name: "smith".to_string(), every: 3 },
    ]
}

#[test]
fn a_rising_ladder_bid_makes_smithing_pay() {
    let cost = input_cost();
    assert!(cost >= BAR_PRICE, "the scimitar consumes bars: {cost}");
    let run = run_demand(&fighter_and_smith(200), &smith_margin, 300);
    eprintln!("ladder sim: prints {:?} first {:?} margins {:?}", run.prints, run.first_listing, run.producer_margins);
    assert!(smith_margin(1) < 0.0, "nothing worth smithing at the 1-coin bootstrap");
    let first = run.first_listing.expect("the bid draws the smith");
    assert!(first > 0);
    assert!(run.bid_at_first_listing.is_some_and(|bid| bid as f64 > cost), "the smith starts once the bid covers the bars");
    assert_eq!(run.served, 1, "the fighter gets its scimitar");
    assert!(run.producer_margins.iter().all(|margin| *margin > 0.0), "sold at a profit: {:?}", run.producer_margins);
    assert!(run.prints.iter().all(|(_, price, _)| *price <= wallet_ceiling(200, GEAR_SHARE_PCT)), "never above the wallet share");
}

#[test]
fn a_thin_wallet_draws_no_smith_until_it_earns() {
    let cost = input_cost();
    let thin = (cost as i64) * 2 - 10;
    let run = run_demand(&fighter_and_smith(thin), &smith_margin, 300);
    assert_eq!(run.first_listing, None, "half of {thin} coins never covers {cost} of bars");
    let enough = coins_for(cost as i64 + 1, GEAR_SHARE_PCT);
    let earned = run_demand(&fighter_and_smith(enough), &smith_margin, 300);
    assert_eq!(earned.served, 1, "a have_coins target of {enough} makes the bid draw the smith");
}
