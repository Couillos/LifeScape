//! How a bot chooses among the lines of a precomputed guide (plan
//! rendements R7, D10, D14): ONE rule for every choice read from the
//! guides — money (make_money, have_coins, a tool's financing, D17's
//! coins) and training (every level climb: harvest, combat, recipes,
//! spells) —, pure, deterministic for a given seed.
//!
//! The caller hands the candidates of ONE class (the bot's judgement,
//! `guide_feasible::feasible_for` on its live state: the lines it works
//! now first, then those it works after a prerequisite it obtains on its
//! own — a tool, a lot's coins; the class order is the caller's, see
//! `class_order`), each with:
//! - `score`, higher is better (money: the guide's profit per 6000 ticks;
//!   training: minus `train_score`'s weighted ticks per XP);
//! - `weight`, what the draw weighs it by (the score times the profile:
//!   money `merit_money`, training `merit_training`), > 0 for a line
//!   worth taking;
//! - `runs`, the measured runs behind its yield (`Confidence::runs`).
//!
//! The draw (D10): the BAND is every candidate whose score is within
//! `BAND_SHARE` (15 %) of the best (`score >= best - 15 % × |best|`); one
//! is drawn with a probability proportional to its weight (all weights
//! nil: uniformly).
//!
//! Exploration (D14): the candidates OUTSIDE the band whose yield rests on
//! fewer than `MEASURED_RUNS` (3) runs and whose weight is positive (a
//! line worth taking, only not measured) share `EXPLORE_SHARE` (5 %) of
//! the draws, uniformly, until measured; with none, every draw goes to
//! the band. The share is the whole slice, never 5 % per line (at the
//! start of the measures nearly every line is unmeasured: 5 % each would
//! be the whole draw).
//!
//! The seed is the bot's (`Profile::seed`) mixed with the node and the
//! bot's draw counter (`seed_for`): the same bot, node and counter always
//! draw the same line (testable, replayable), another bot or the next
//! draw another one.

/// Share of the best score a line may lose and stay in the band (D10).
pub const BAND_SHARE: f64 = 0.15;
/// Share of the draws the unmeasured lines outside the band receive (D14).
pub const EXPLORE_SHARE: f64 = 0.05;
/// Runs from which a line's yield counts as measured (the estimator's
/// threshold, `rates_service::MIN_MEASURED_REPORTS`).
pub const MEASURED_RUNS: u64 = 3;
/// Salt of the draw seeds (the profile draws use 101 to 103).
pub const DRAW_SALT: u64 = 104;

/// One line a bot may take.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// Higher is better (module doc).
    pub score: f64,
    /// Weight of the draw (score × profile), > 0 to be worth taking.
    pub weight: f64,
    /// Measured runs behind the line's yield.
    pub runs: u64,
}

/// What a draw chose and among how many.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drawn {
    /// Index of the chosen candidate.
    pub index: usize,
    /// Candidates in the band (D10).
    pub band: usize,
    /// Candidates offered.
    pub candidates: usize,
    /// True when the exploration slice chose it (D14).
    pub explored: bool,
}

/// One money line of the guide a bot may take (R7), as the brain hands
/// it to the resolver (`ReadyCtx::money_guide`, `money_pick`).
#[derive(Debug, Clone, PartialEq)]
pub struct GuideLine {
    /// The planner method it launches (`trade:craft/<recipe>` for a
    /// market row).
    pub method: String,
    /// The row's family (`farm`, `recipe`, `market`), for the logs.
    pub family: String,
    /// The guide's profit per 6000 ticks (the score).
    pub profit_6000: f64,
    /// The draw's weight (`merit_money`).
    pub weight: f64,
    /// Measured runs behind the row's yield.
    pub runs: u64,
    /// The bot's judgement (`guide_feasible::feasible_for` on its live
    /// state): feasible now, else within reach once a tool is obtained
    /// or a lot's coins earned.
    pub feasible: bool,
    /// A recipe on the bot's own stock (`stock_lot`): the actions its free
    /// stock covers, the lot the node runs (`node.batch`, `--use`); 0 for
    /// every other line.
    pub lot: i64,
}

/// What one resolve chose from a guide (logged and recorded by the
/// brain: `LifeMemory::picks`, `orchestrator economy`).
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    /// `money` or `training`.
    pub kind: &'static str,
    pub method: String,
    /// The line's score (money: profit per 6000; training: weighted
    /// ticks per XP).
    pub score: f64,
    /// The class drawn in (`class_order`).
    pub class: u8,
    pub drawn: Drawn,
    /// The node's previous pick taken again (it blocked on a tool now
    /// obtained): no draw.
    pub sticky: bool,
}

/// One splitmix64 step.
fn mix(word: u64) -> u64 {
    let mut word = word.wrapping_add(0x9E37_79B9_7F4A_7C15);
    word = (word ^ (word >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    word = (word ^ (word >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    word ^ (word >> 31)
}

/// A uniform number in [0, 1) of `seed` and a `slot` (one per decision of
/// the draw).
fn unit(seed: u64, slot: u64) -> f64 {
    (mix(seed ^ mix(slot.wrapping_add(DRAW_SALT))) >> 11) as f64 / (1u64 << 53) as f64
}

/// The seed of one draw: the bot's seed, the node it chooses for, and
/// its draw counter (`LifeMemory::draws`).
pub fn seed_for(bot_seed: u64, node: &str, counter: u64) -> u64 {
    let mut hash = 0xCBF2_9CE4_8422_2325u64;
    for byte in node.bytes() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01B3);
    }
    mix(bot_seed ^ mix(hash ^ mix(counter ^ DRAW_SALT)))
}

/// True when `score` is within `BAND_SHARE` of `best`.
pub fn in_band(score: f64, best: f64) -> bool {
    score >= best - BAND_SHARE * best.abs() - 1e-9
}

/// Index of a weighted pick of `pool` (indices into `candidates`) at `u`
/// in [0, 1); uniform when no weight is positive.
fn weighted(candidates: &[Candidate], pool: &[usize], at: f64) -> usize {
    let weight = |index: &usize| candidates[*index].weight.max(0.0);
    let total: f64 = pool.iter().map(weight).filter(|value| value.is_finite()).sum();
    if !(total > 0.0) {
        let slot = ((at * pool.len() as f64) as usize).min(pool.len() - 1);
        return pool[slot];
    }
    let mut left = at * total;
    for index in pool {
        let value = weight(index);
        if !value.is_finite() {
            continue;
        }
        if left < value {
            return *index;
        }
        left -= value;
    }
    *pool.last().unwrap_or(&0)
}

/// The draw of D10 with the exploration of D14 (module doc); `None` for
/// no candidate.
pub fn draw(candidates: &[Candidate], seed: u64) -> Option<Drawn> {
    let best = candidates.iter().map(|candidate| candidate.score).filter(|score| score.is_finite()).fold(None, |top: Option<f64>, score| Some(top.map_or(score, |known| known.max(score))))?;
    let band: Vec<usize> = (0..candidates.len()).filter(|index| in_band(candidates[*index].score, best)).collect();
    let explore: Vec<usize> = (0..candidates.len())
        .filter(|index| !band.contains(index) && candidates[*index].runs < MEASURED_RUNS && candidates[*index].weight > 0.0 && candidates[*index].score.is_finite())
        .collect();
    if !explore.is_empty() && unit(seed, 1) < EXPLORE_SHARE {
        let slot = ((unit(seed, 2) * explore.len() as f64) as usize).min(explore.len() - 1);
        return Some(Drawn { index: explore[slot], band: band.len(), candidates: candidates.len(), explored: true });
    }
    let index = weighted(candidates, &band, unit(seed, 3));
    Some(Drawn { index, band: band.len(), candidates: candidates.len(), explored: false })
}

/// The draw's weight of a money line (D10, D13): its profit per 6000
/// ticks, a MARKET line (inputs bought) weighed by market_affinity as
/// `cost::choose_market_or_farm` weighs a purchase (÷ (1.5 − affinity):
/// ×2 for a market bot, ×0.67 for an autarkic one). The thrift trait
/// weighs the coins a TRAINING line spends against its XP; a money line
/// earns, it is not read here.
pub fn merit_money(profit_6000: f64, market: bool, market_affinity: f64) -> f64 {
    if market {
        profit_6000 / crate::train_score::market_weight(market_affinity)
    } else {
        profit_6000
    }
}

/// The draw's weight of a training line from its `train_score` score
/// (weighted ticks per XP, lower is better, the profile already in it):
/// weighted XP per tick (1 / score) for a line that costs time, the coin
/// ticks it earns per XP (−score) for one that earns more than its time.
pub fn merit_training(score: f64) -> f64 {
    if score > 0.0 {
        1.0 / score
    } else {
        -score
    }
}

/// Order of the classes a caller draws in: the first non-empty wins.
/// 0 works now; 1 works after a prerequisite it gets on its own (food in
/// hand, coins for a lot); 2 needs a tool obtained first (the node blocks
/// on it); anything else never draws.
pub fn class_order() -> [u8; 3] {
    [0, 1, 2]
}

/// The first non-empty class of `classes` (one per candidate) and the
/// indices of its members: what the draw reads.
pub fn first_class(classes: &[u8]) -> (Option<u8>, Vec<usize>) {
    for class in class_order() {
        let members: Vec<usize> = (0..classes.len()).filter(|index| classes[*index] == class).collect();
        if !members.is_empty() {
            return (Some(class), members);
        }
    }
    (None, Vec::new())
}

#[cfg(test)]
#[path = "guide_pick_tests.rs"]
mod tests;
