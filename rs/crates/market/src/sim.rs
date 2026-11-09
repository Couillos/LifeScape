#![forbid(unsafe_code)]
//! Fixed-seed offline simulator: synthetic ZIP agents trade one item, so
//! pacing, patience and bands get tuned without live bots. Deterministic
//! on the seed: the same catalogue and seed always print the same fills.
//! Convergence bar: last-trade price within 10% of equilibrium in under
//! N matches; a supply shock must settle, not oscillate.

use crate::book::{Book, Side};
use crate::error::Error;

/// Deterministic 64-bit xorshift: no external RNG, same seed same run.
#[derive(Clone, Debug)]
pub struct FixedRng {
    state: u64,
}

impl FixedRng {
    /// Non-zero seed; zero falls back to a fixed non-zero constant.
    pub fn new(seed: u64) -> Self {
        FixedRng { state: if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed } }
    }

    fn next(&mut self) -> u64 {
        let mut word = self.state;
        word ^= word << 13;
        word ^= word >> 7;
        word ^= word << 17;
        self.state = word;
        word
    }

    /// Uniform value in [low, high], inclusive.
    pub fn between(&mut self, low: i64, high: i64) -> i64 {
        if low >= high {
            return low;
        }
        low + (self.next() % ((high - low + 1) as u64)) as i64
    }
}

/// One synthetic trader: a private value, a ZIP-like margin that adapts
/// toward the last print, and a fixed side for the scenario.
#[derive(Clone, Debug)]
pub struct SimAgent {
    pub name: String,
    pub side: Side,
    pub value: i64,
    pub margin: f64,
    pub stock: i64,
    pub coins: i64,
    /// Fills this agent has taken (for the adaptive step).
    pub fills: u64,
}

impl SimAgent {
    /// Current limit quote from value and margin.
    pub fn quote(&self) -> i64 {
        let raw = match self.side {
            Side::Buy => self.value as f64 * (1.0 - self.margin),
            Side::Sell => self.value as f64 * (1.0 + self.margin),
        };
        raw.round() as i64
    }

    /// ZIP core in one line: nudge the margin toward the last print so
    /// the next quote is more competitive after a miss, less generous
    /// after a favorable fill.
    pub fn adapt(&mut self, last_price: Option<i64>) {
        let Some(print) = last_price else {
            return;
        };
        let target = match self.side {
            Side::Buy => (self.value - print) as f64 / self.value.max(1) as f64,
            Side::Sell => (print - self.value) as f64 / self.value.max(1) as f64,
        };
        self.margin += 0.05 * (target - self.margin);
        if self.margin < -0.5 {
            self.margin = -0.5;
        }
        if self.margin > 0.5 {
            self.margin = 0.5;
        }
    }

    /// Round outcome: a fill means surplus taken (shade more next time);
    /// a missed print at or through our quote means we were too passive.
    pub fn learn(&mut self, filled: bool, through_us: bool) {
        if filled {
            self.margin = (self.margin + 0.01).min(0.5);
        } else if through_us {
            self.margin = (self.margin - 0.03).max(-0.05);
        }
    }
}

/// Scenario outcome: enough to judge convergence at a glance.
#[derive(Clone, Debug, PartialEq)]
pub struct SimReport {
    pub matches: usize,
    pub traded_qty: i64,
    pub last_price: Option<i64>,
    pub equilibrium: i64,
    /// Last print within 10% of equilibrium.
    pub converged: bool,
    /// First match index (1-based) printing inside the 10% band, if any.
    pub converged_at: Option<usize>,
    /// Sign flips of the price path (oscillation gauge).
    pub direction_flips: usize,
}

fn within_band(price: i64, equilibrium: i64) -> bool {
    if equilibrium < 1 || price < 1 {
        return false;
    }
    if equilibrium < 10 {
        return price == equilibrium;
    }
    (price - equilibrium).abs() * 10 <= equilibrium
}

/// Builds a symmetric book of buyers above and sellers below an anchor.
pub fn scenario_agents(
    seed: u64,
    buyers: usize,
    sellers: usize,
    anchor: i64,
    spread: i64,
) -> Vec<SimAgent> {
    let mut rng = FixedRng::new(seed);
    let mut agents = Vec::new();
    for index in 0..buyers {
        agents.push(SimAgent {
            name: format!("sim-buyer-{index}"),
            side: Side::Buy,
            value: rng.between(anchor, anchor + spread),
            margin: 0.05,
            stock: 0,
            coins: 1_000_000,
            fills: 0,
        });
    }
    for index in 0..sellers {
        agents.push(SimAgent {
            name: format!("sim-seller-{index}"),
            side: Side::Sell,
            value: rng.between((anchor - spread).max(1), anchor),
            margin: 0.05,
            stock: 1_000,
            coins: 0,
            fills: 0,
        });
    }
    agents
}

/// Runs one item until `max_matches` fills, `max_rounds` quoting rounds,
/// or the book goes quiet. Returns the report plus the print path.
pub fn run_scenario(
    item: i64,
    agents: &mut [SimAgent],
    max_matches: usize,
    start_order_id: u64,
    max_rounds: usize,
) -> Result<(SimReport, Vec<i64>), Error> {
    if agents.is_empty() {
        return Err(Error::Invalid("no agents".to_string()));
    }
    let equilibrium = agents.iter().map(|agent| agent.value).sum::<i64>() / agents.len().max(1) as i64;
    let mut book = Book::new();
    let mut order_id = start_order_id.max(1);
    let mut matches = 0usize;
    let mut traded_qty = 0i64;
    let mut last_price: Option<i64> = None;
    let mut path: Vec<i64> = Vec::new();
    let mut filled_flag = vec![false; agents.len()];
    let name_index = |agents: &[SimAgent], name: &str| agents.iter().position(|agent| agent.name == name);
    // Seed both sides before any crossing, cheapest deterministic order.
    let mut roster: Vec<usize> = (0..agents.len()).collect();
    roster.sort_by(|&left, &right| {
        agents[left]
            .quote()
            .cmp(&agents[right].quote())
            .then_with(|| left.cmp(&right))
    });
    for position in roster {
        let agent = &agents[position];
        let quote = agent.quote().max(1);
        let qty = 10;
        let (fills, _) = book.place(order_id, &agent.name, item, agent.side, qty, quote)?;
        order_id += 1;
        for fill in &fills {
            matches += 1;
            traded_qty += fill.quantity;
            last_price = Some(fill.price);
            path.push(fill.price);
            if let Some(index) = name_index(agents, &fill.buyer) {
                filled_flag[index] = true;
            }
            if let Some(index) = name_index(agents, &fill.seller) {
                filled_flag[index] = true;
            }
        }
        if matches >= max_matches {
            break;
        }
    }
    // Quoting rounds: each agent requotes from its adapted margin; filled
    // agents shade more, agents priced through shade less.
    let mut round = 0;
    while matches < max_matches && round < max_rounds {
        round += 1;
        let mut progress = false;
        for position in 0..agents.len() {
            let quote = agents[position].quote().max(1);
            let (side, name) = (agents[position].side, agents[position].name.clone());
            let (fills, _) = book.place(order_id, &name, item, side, 5, quote)?;
            order_id += 1;
            if fills.is_empty() {
                let through = match (side, last_price) {
                    (Side::Buy, Some(print)) => quote >= print,
                    (Side::Sell, Some(print)) => quote <= print,
                    _ => false,
                };
                agents[position].learn(false, through);
            } else {
                for fill in &fills {
                    matches += 1;
                    traded_qty += fill.quantity;
                    last_price = Some(fill.price);
                    path.push(fill.price);
                    progress = true;
                    if let Some(index) = name_index(agents, &fill.buyer) {
                        filled_flag[index] = true;
                    }
                    if let Some(index) = name_index(agents, &fill.seller) {
                        filled_flag[index] = true;
                    }
                }
                agents[position].learn(true, false);
            }
            if matches >= max_matches {
                break;
            }
        }
        if !progress {
            break;
        }
        // Resting agents priced through the last print were too passive.
        for position in 0..agents.len() {
            if filled_flag[position] {
                continue;
            }
            let quote = agents[position].quote().max(1);
            let through = match (agents[position].side, last_price) {
                (Side::Buy, Some(print)) => quote >= print,
                (Side::Sell, Some(print)) => quote <= print,
                _ => false,
            };
            agents[position].learn(false, through);
        }
        filled_flag.fill(false);
    }
    let equilibrium = equilibrium.max(1);
    let converged = last_price.is_some_and(|price| within_band(price, equilibrium));
    let mut converged_at = None;
    for (index, price) in path.iter().enumerate() {
        if within_band(*price, equilibrium) {
            converged_at = Some(index + 1);
            break;
        }
    }
    let mut direction_flips = 0usize;
    let mut last_sign = 0i64;
    for pair in path.windows(2) {
        let sign = (pair[1] - pair[0]).signum();
        if sign != 0 && last_sign != 0 && sign != last_sign {
            direction_flips += 1;
        }
        if sign != 0 {
            last_sign = sign;
        }
    }
    Ok((
        SimReport { matches, traded_qty, last_price, equilibrium, converged, converged_at, direction_flips },
        path,
    ))
}

/// Supply shock: extra sellers at a discount join mid-run. Returns the
/// post-shock report; callers assert it settles (few flips, converges).
pub fn run_shock(
    item: i64,
    seed: u64,
    anchor: i64,
    extra_sellers: usize,
) -> Result<(SimReport, Vec<i64>), Error> {
    let mut agents = scenario_agents(seed, 12, 12, anchor, 8);
    let (_warming, mut path) = run_scenario(item, &mut agents, 60, 1, 200)?;
    let mut shock = Vec::new();
    let mut rng = FixedRng::new(seed ^ 0x5BD1_E995);
    for index in 0..extra_sellers {
        shock.push(SimAgent {
            name: format!("shock-seller-{index}"),
            side: Side::Sell,
            value: rng.between((anchor - 20).max(1), anchor - 5),
            margin: 0.02,
            stock: 1_000,
            coins: 0,
            fills: 0,
        });
    }
    agents.extend(shock);
    let (after, mut tail) = run_scenario(item, &mut agents, 120, 10_000, 200)?;
    path.append(&mut tail);
    let report = SimReport { matches: after.matches, traded_qty: after.traded_qty, ..after };
    Ok((report, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    
    #[test]
    fn rng_repeats_on_the_seed() {
        let mut first = FixedRng::new(42);
        let mut second = FixedRng::new(42);
        for _ in 0..20 {
            assert_eq!(first.between(1, 1000), second.between(1, 1000));
        }
    }

    #[test]
    fn symmetric_book_converges_within_budget() {
        let mut agents = scenario_agents(7, 12, 12, 50, 8);
        let (report, _path) = run_scenario(314, &mut agents, 500, 1, 200).expect("runs");
        assert!(report.matches > 0, "agents actually meet");
        assert!(report.converged, "last {} vs equilibrium {}", report.last_price.unwrap_or(-1), report.equilibrium);
        let reached = report.converged_at.expect("converges at some match");
        assert!(reached < 200, "converges in under N matches, got {reached}");
    }

    #[test]
    fn supply_shock_settles_without_sustained_oscillation() {
        let (report, _path) = run_shock(314, 11, 50, 6).expect("runs");
        assert!(report.matches > 0);
        // Post-shock prints lean cheaper but stay in one regime: flips
        // stay far below one per match (no ring-around-the-anchor).
        assert!(
            report.direction_flips * 4 < report.matches.max(1),
            "oscillates: {} flips over {} matches",
            report.direction_flips,
            report.matches
        );
    }

    #[test]
    fn merchants_widen_liquidity() {
        // Sparse book, fixed quoting budget: two tight market-makers at
        // the anchor must print clearly more matches than the bare book.
        let mut plain = scenario_agents(21, 3, 3, 40, 20);
        let (bare, _) = run_scenario(314, &mut plain, 500, 1, 30).expect("runs");
        let mut with_merchants = scenario_agents(21, 3, 3, 40, 20);
        for index in 0..2 {
            with_merchants.push(SimAgent {
                name: format!("merchant-{index}"),
                side: if index % 2 == 0 { Side::Buy } else { Side::Sell },
                value: 40,
                margin: 0.01,
                stock: 1_000,
                coins: 1_000_000,
                fills: 0,
            });
        }
        let (merched, _) = run_scenario(314, &mut with_merchants, 500, 1, 30).expect("runs");
        assert!(merched.converged, "merchant book still converges");
        assert!(
            merched.matches > bare.matches,
            "merchants add matches: {} vs {}",
            merched.matches,
            bare.matches
        );
    }
}
