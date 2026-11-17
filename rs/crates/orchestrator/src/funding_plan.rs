#![forbid(unsafe_code)]
//! Funding plan of the market makers (user 2026-11-17: « force les users
//! les plus riches à donner de l'argent aux market makers pour qu'ils
//! atteignent leur target »). Pure: the caller reads the live purses and
//! the in-flight gifts, this module decides who gives what to whom.
//!
//! - A maker still EARNING lacks `target - capital - pending_in`, where
//!   capital is its live coins, pack plus bank (the same read as the
//!   switch, `market_maker::maker_capital`) and `pending_in` the gifts to
//!   it still in flight. A quoting, offline or unread maker gets nothing
//!   (named in `skipped`). A shortfall under `MIN_GIFT` is left to the
//!   maker's own play: no square trip for a few hundred coins.
//! - A donor is a fleet account (never staff admin/admin2, never bite,
//!   never a maker, never an Iron: an Iron never trades) whose bank is
//!   streamed. It keeps a named reserve for its own plan: the coins its
//!   resting bids and buyer deals commit, its next gear and kit purchase,
//!   and a margin (`margin`: 10 % of its coins, at least `MIN_MARGIN`).
//!   What it gives is `coins - pending_out - reserve`, the gifts it
//!   already owes counted out.
//! - `--force` (user 2026-11-17, « force les plus riches »): the donor's
//!   next gear and kit purchase is not reserved and its margin is the
//!   floor `MIN_MARGIN`; only the coins its resting bids and buyer deals
//!   commit stay untouched (a filled bid must still be paid).
//! - A maker is funded to its target plus `SWITCH_BUFFER`: it keeps
//!   playing until its next launch pass reads its purse, and a purchase
//!   meanwhile must not drop it back under the target.
//! - Greedy: the makers by decreasing shortfall (name breaks ties), each
//!   served by the richest donor left, then the next, until covered or
//!   the donors run dry (`uncovered`).
//! - Idempotent: the gifts in flight count on both sides, so a plan
//!   computed again before they settle gives nothing more; a settled
//!   gift is in the maker's purse and out of the donor's.

use std::collections::BTreeMap;

/// Floor of a donor's margin (coins).
pub const MIN_MARGIN: i64 = 5_000;
/// Margin share of a donor's coins (percent).
pub const MARGIN_PCT: i64 = 10;
/// Smallest gift worth a square trip (coins).
pub const MIN_GIFT: i64 = 1_000;
/// Coins a funded maker gets over its target (spent before its switch).
pub const SWITCH_BUFFER: i64 = 2_000;

/// One maker as the plan reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MakerNeed {
    pub account: String,
    /// Live coins, pack plus bank; `None`: offline or bank not streamed.
    pub capital: Option<i64>,
    pub target: i64,
    pub quoting: bool,
    /// Gifts to this maker still in flight.
    pub pending_in: i64,
}

/// One candidate donor as the plan reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DonorPurse {
    pub account: String,
    /// Live coins, pack plus bank; `None`: bank not streamed.
    pub coins: Option<i64>,
    /// Coins its resting bids and in-flight buyer deals commit.
    pub committed: i64,
    /// Its next gear and kit purchase (coins).
    pub next_purchase: i64,
    /// Gifts from this donor still in flight.
    pub pending_out: i64,
    pub ironman: bool,
    /// Staff (admin, admin2), the owner's private account, the persisted
    /// exclusion list.
    pub excluded: bool,
    pub maker: bool,
}

/// One planned gift.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gift {
    pub donor: String,
    pub maker: String,
    pub amount: i64,
}

/// A donor's named reserve and what it may give.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DonorReserve {
    pub account: String,
    pub coins: i64,
    pub committed: i64,
    pub next_purchase: i64,
    pub margin: i64,
    pub pending_out: i64,
    pub free: i64,
}

/// One maker's line of the plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MakerLine {
    pub account: String,
    pub capital: i64,
    pub target: i64,
    pub pending_in: i64,
    pub shortfall: i64,
    pub planned: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FundingPlan {
    pub gifts: Vec<Gift>,
    pub makers: Vec<MakerLine>,
    pub reserves: Vec<DonorReserve>,
    /// (account, why it is neither funded nor a donor).
    pub skipped: Vec<(String, String)>,
    /// Shortfall no donor could cover.
    pub uncovered: i64,
}

/// The margin a donor keeps over its committed coins and next purchase.
pub fn margin(coins: i64) -> i64 {
    (coins.max(0).saturating_mul(MARGIN_PCT) / 100).max(MIN_MARGIN)
}

/// The donor's reserve and free coins (`None`: never a donor, with why).
pub fn reserve_of(purse: &DonorPurse, force: bool) -> Result<DonorReserve, String> {
    if purse.excluded {
        return Err("staff or private account, never a donor".to_string());
    }
    if purse.maker {
        return Err("market maker, never a donor".to_string());
    }
    if purse.ironman {
        return Err("Iron account, never trades".to_string());
    }
    let Some(coins) = purse.coins else { return Err("bank not streamed, coins unread".to_string()) };
    let margin = if force { MIN_MARGIN } else { margin(coins) };
    let next_purchase = if force { 0 } else { purse.next_purchase.max(0) };
    let held = purse.committed.max(0) + next_purchase + margin + purse.pending_out.max(0);
    Ok(DonorReserve {
        account: purse.account.clone(),
        coins,
        committed: purse.committed.max(0),
        next_purchase,
        margin,
        pending_out: purse.pending_out.max(0),
        free: (coins - held).max(0),
    })
}

/// The plan: who gives what to which maker (`force`: see the module).
pub fn plan(makers: &[MakerNeed], donors: &[DonorPurse], force: bool) -> FundingPlan {
    let mut out = FundingPlan::default();
    let mut needs: Vec<MakerLine> = Vec::new();
    for maker in makers {
        if maker.quoting {
            out.skipped.push((maker.account.clone(), "already quoting".to_string()));
            continue;
        }
        let Some(capital) = maker.capital else {
            out.skipped.push((maker.account.clone(), "offline or bank not streamed, capital unread".to_string()));
            continue;
        };
        let shortfall = (maker.target + SWITCH_BUFFER - capital - maker.pending_in.max(0)).max(0);
        needs.push(MakerLine { account: maker.account.clone(), capital, target: maker.target, pending_in: maker.pending_in.max(0), shortfall, planned: 0 });
    }
    for purse in donors {
        match reserve_of(purse, force) {
            Ok(reserve) => out.reserves.push(reserve),
            Err(why) => out.skipped.push((purse.account.clone(), why)),
        }
    }
    let mut free: BTreeMap<String, i64> = out.reserves.iter().map(|reserve| (reserve.account.clone(), reserve.free)).collect();
    needs.sort_by(|left, right| right.shortfall.cmp(&left.shortfall).then_with(|| left.account.cmp(&right.account)));
    for need in &mut needs {
        let mut left = need.shortfall;
        while left >= MIN_GIFT {
            // The richest donor left (name breaks ties: deterministic).
            let Some((donor, purse)) = free.iter().max_by(|left, right| left.1.cmp(right.1).then_with(|| right.0.cmp(left.0))).map(|(name, coins)| (name.clone(), *coins)) else { break };
            if purse < MIN_GIFT {
                break;
            }
            let amount = left.min(purse);
            free.insert(donor.clone(), purse - amount);
            out.gifts.push(Gift { donor, maker: need.account.clone(), amount });
            need.planned += amount;
            left -= amount;
        }
        if left >= MIN_GIFT {
            out.uncovered += left;
        }
    }
    needs.sort_by(|left, right| left.account.cmp(&right.account));
    out.makers = needs;
    out.reserves.sort_by(|left, right| right.free.cmp(&left.free).then_with(|| left.account.cmp(&right.account)));
    out
}

/// The plan as operator lines (`fund-makers`, dry run or not).
pub fn plan_lines(plan: &FundingPlan) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!("makers (earning): {}", plan.makers.len()));
    for maker in &plan.makers {
        lines.push(format!(
            "  {:<14} capital {:>8} / target {:>8}, in flight {:>7}, shortfall {:>8}, planned {:>8}",
            maker.account, maker.capital, maker.target, maker.pending_in, maker.shortfall, maker.planned
        ));
    }
    lines.push(format!("gifts: {} ({} coins)", plan.gifts.len(), plan.gifts.iter().map(|gift| gift.amount).sum::<i64>()));
    for gift in &plan.gifts {
        lines.push(format!("  {} -> {} : {}", gift.donor, gift.maker, gift.amount));
    }
    if plan.uncovered > 0 {
        lines.push(format!("uncovered: {} coins (donors' free coins exhausted)", plan.uncovered));
    }
    let givers: Vec<&DonorReserve> = plan.reserves.iter().filter(|reserve| reserve.free >= MIN_GIFT).collect();
    lines.push(format!("donors with free coins: {} of {}", givers.len(), plan.reserves.len()));
    for reserve in givers {
        lines.push(format!(
            "  {:<14} coins {:>8} = reserve (bids {} + next purchase {} + margin {}) + owed {} + free {}",
            reserve.account,
            reserve.coins,
            reserve.committed,
            reserve.next_purchase,
            reserve.margin,
            reserve.pending_out,
            reserve.free
        ));
    }
    for (account, why) in &plan.skipped {
        lines.push(format!("  skipped {account}: {why}"));
    }
    lines
}

#[cfg(test)]
#[path = "funding_plan_tests.rs"]
mod tests;
