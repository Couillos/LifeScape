//! `coins:N` needs of a resolve block: lifted by the coins other resting
//! buys commit, dropped when the free wallet already covers them.

/// A `coins:N` need computed on the free wallet: the wallet to earn adds
/// back the coins the bot's other resting buys commit, so the
/// `have_coins` target it files really frees `N` (other needs unchanged).
pub(crate) fn lift(need: &str, committed: i64) -> String {
    match need.strip_prefix("coins:").and_then(|amount| amount.parse::<i64>().ok()) {
        Some(amount) => format!("coins:{}", amount.saturating_add(committed.max(0))),
        None => need.to_string(),
    }
}

/// A `coins:N` need the free wallet already covers is no shortage: its
/// `have_coins` closes "done" at once and the node re-resolves forever
/// (live 2026-11-23, david_clay). Dropped, the block falls to the typed
/// contract error instead of a loop.
pub(crate) fn met(need: &str, free_coins: i64) -> bool {
    need.strip_prefix("coins:").and_then(|amount| amount.parse::<i64>().ok()).is_some_and(|amount| amount <= free_coins)
}

#[cfg(test)]
mod tests {
    use super::{lift, met};

    /// Review 2026-11-13 (finding 6): a graph buy bids on the coins the
    /// bot's standing bids leave free; the wallet it must earn adds those
    /// commitments back.
    #[test]
    fn a_coin_need_adds_the_committed_coins() {
        assert_eq!(lift("coins:120", 40), "coins:160");
        assert_eq!(lift("coins:120", 0), "coins:120");
        assert_eq!(lift("tool:pickaxe", 40), "tool:pickaxe");
        assert_eq!(lift("coins:many", 40), "coins:many");
    }

    #[test]
    fn a_need_the_free_wallet_covers_is_met() {
        assert!(met("coins:120", 120));
        assert!(!met("coins:121", 120));
        assert!(!met("level:fishing:5", 1000));
    }
}
