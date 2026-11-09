#![forbid(unsafe_code)]
//! Public trend signals. Inputs are strictly what any player could see:
//! resting book depth plus settled executions. No bot internals, no
//! positions, no cross-bot state ever enter here.

/// What any market watcher can see for one item and window.
#[derive(Clone, Debug, PartialEq)]
pub struct TrendInput {
    /// Resting buy quantity across the visible depth.
    pub buy_depth: i64,
    /// Resting sell quantity across the visible depth.
    pub sell_depth: i64,
    /// First execution price in the window, when any trade settled.
    pub first_price: Option<i64>,
    /// Last execution price in the window, when any trade settled.
    pub last_price: Option<i64>,
    /// Previous smoothed score (for continuity), 0 when unscored.
    pub previous_score: f64,
}

/// Shortage, momentum and the smoothed score, all in [-1, 1].
#[derive(Clone, Debug, PartialEq)]
pub struct TrendScores {
    pub shortage: f64,
    pub momentum: f64,
    pub score: f64,
}

fn clamp(unit: f64) -> f64 {
    if unit > 1.0 {
        1.0
    } else if unit < -1.0 {
        -1.0
    } else {
        unit
    }
}

/// Scores one item for one window from public data only. The score
/// blends shortage and momentum equally, then smooths against the
/// previous value so one print cannot whip the bots around.
pub fn compute_trend(input: &TrendInput) -> TrendScores {
    let total = input.buy_depth.max(0) + input.sell_depth.max(0);
    let shortage = if total == 0 {
        0.0
    } else {
        (input.buy_depth.max(0) - input.sell_depth.max(0)) as f64 / total as f64
    };
    let momentum = match (input.first_price, input.last_price) {
        (Some(first), Some(last)) if first >= 1 && last >= 1 => {
            (last - first) as f64 / first as f64
        }
        _ => 0.0,
    };
    let raw = (clamp(shortage) + clamp(momentum)) / 2.0;
    let score = clamp(0.7 * clamp(input.previous_score) + 0.3 * raw);
    TrendScores { shortage: clamp(shortage), momentum: clamp(momentum), score }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(buy: i64, sell: i64, first: Option<i64>, last: Option<i64>) -> TrendInput {
        TrendInput { buy_depth: buy, sell_depth: sell, first_price: first, last_price: last, previous_score: 0.0 }
    }

    #[test]
    fn empty_book_and_no_trades_score_zero() {
        let scores = compute_trend(&input(0, 0, None, None));
        assert_eq!(scores, TrendScores { shortage: 0.0, momentum: 0.0, score: 0.0 });
    }

    #[test]
    fn buy_pressure_and_rising_prices_push_up() {
        let scores = compute_trend(&input(300, 100, Some(10), Some(12)));
        assert!((scores.shortage - 0.5).abs() < 1e-9);
        assert!((scores.momentum - 0.2).abs() < 1e-9);
        assert!(scores.score > 0.0 && scores.score <= 0.35 + 1e-9);
    }

    #[test]
    fn smoothing_caps_single_print_jumps() {
        let steady = TrendInput { previous_score: 1.0, ..input(0, 100, Some(10), Some(1)) };
        let scores = compute_trend(&steady);
        assert!(scores.score > -1.0 && scores.score < 1.0);
        assert!(scores.score > -0.4, "smoothing resists the cliff, got {}", scores.score);
    }

    #[test]
    fn scores_stay_bounded() {
        let scores = compute_trend(&input(1_000_000, 0, Some(1), Some(1_000_000)));
        assert!((scores.shortage - 1.0).abs() < 1e-9);
        assert!((scores.momentum - 1.0).abs() < 1e-9);
        assert!((scores.score - 0.3).abs() < 1e-9);
    }
}
