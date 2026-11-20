#![forbid(unsafe_code)]
//! Public order board claims (2026-11-14, user requirement: account for
//! the supply already in flight from other producers answering the same
//! object). A producer posts what it will bring, like a crafting order
//! taken in a real game: the board is PUBLIC (every bot reads the claimed
//! units per object, never who nor anything else of the claimer), so D22
//! holds — no hidden memory, the same fact a player reads on the order
//! board. Claims live in memory (a restart forgets them; the books stay)
//! and expire after twice the estimated duration (`claim_secs`).
//!
//! Plan équilibre step 2 (2026-11-19): EVERY production the brain
//! launches posts its claim, not only the bid answers:
//! - a bid answer (`bid:<item>` goal, `claim`): its lot, until the bot
//!   draws its next goal (`release`);
//! - a run (`claim_run`, keyed by its node): a money node's method or a
//!   filler window (units per 6000 ticks measured for the bot × the
//!   run's budget, `run_units`; a stock recipe its lot), a 100 % market
//!   lot (its products, from the lock through the purchases to the
//!   craft); released when that node's run reports (`release_run`), when
//!   the bot draws its next goal, or at expiry.
//! The guides' build reads every claim (`claimed_total`), a bot's pick
//! the claims of the others (`claimed_by_others`): both net them from
//! the demand an activity is valued on (`guide_engine::sale_steps`).
//!
//! Purchases (2026-11-20, live 22:20-23:20: 14 bots answered 51 amulet of
//! power bids with the same 6 uncut diamonds on sale): a bid answer also
//! posts the units its plan BUYS from the resting asks
//! (`claim_purchases`, `Opportunity::purchases`); another producer reads
//! them (`purchases_by_others`) and its view of those asks shrinks by
//! them, cheapest first (`market_fit::BookView::without_bought`). A fill
//! that lands shrinks them (`purchase_landed`), the need met ends them
//! (`purchase_done`, from `brain_market::cancel_open_buys`): the bought
//! asks are gone from the book, never counted twice. Otherwise the next
//! goal draw or expiry ends them.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Shortest claim (wall seconds).
pub const MIN_CLAIM_SECS: i64 = 600;
/// Longest claim (wall seconds).
pub const MAX_CLAIM_SECS: i64 = 6 * 3600;
/// Key of a bid answer's claim (one per account, the goal's lot).
const GOAL_KEY: &str = "goal";
/// Key of a bid answer's purchases (one set per account).
const PURCHASE_KEY: &str = "goal-buy";

#[derive(Debug, Clone, PartialEq)]
struct Claim {
    account: String,
    /// `GOAL_KEY`, or the node id of the run that brings the units.
    key: String,
    item: String,
    units: i64,
    expires_secs: i64,
    /// A purchase from the resting asks, never supply on its way.
    buy: bool,
}

fn board() -> &'static Mutex<Vec<Claim>> {
    static BOARD: OnceLock<Mutex<Vec<Claim>>> = OnceLock::new();
    BOARD.get_or_init(|| Mutex::new(Vec::new()))
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
}

/// Wall seconds a claim of a lot estimated at `ticks` lasts.
pub fn claim_secs(ticks: f64, tick_ms: u64) -> i64 {
    let secs = (ticks.max(0.0) * 2.0 * tick_ms.max(1) as f64 / 1000.0).ceil() as i64;
    secs.clamp(MIN_CLAIM_SECS, MAX_CLAIM_SECS)
}

/// Units a run brings: `per_6000` units a window (measured for the bot)
/// over its `budget_ticks`, whole units; 0 without a finite rate.
pub fn run_units(per_6000: f64, budget_ticks: f64) -> i64 {
    if !per_6000.is_finite() || per_6000 <= 0.0 || budget_ticks <= 0.0 {
        return 0;
    }
    (per_6000 * budget_ticks / 6000.0).floor() as i64
}

/// `account` takes `units` of `item` for a bid answer (its previous bid
/// answer's claim ends; its runs' claims stay).
pub fn claim(account: &str, item: &str, units: i64, secs: i64) {
    claim_at(account, GOAL_KEY, item, units, now_secs() + secs);
}

/// The run of `node` (its id) brings `units` of `item` (the node's
/// previous claim ends).
pub fn claim_run(account: &str, node: &str, item: &str, units: i64, secs: i64) {
    claim_at(account, node, item, units, now_secs() + secs);
}

fn claim_at(account: &str, key: &str, item: &str, units: i64, expires_secs: i64) {
    let mut table = board().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    table.retain(|claim| !(claim.account.eq_ignore_ascii_case(account) && claim.key == key));
    if units > 0 {
        table.push(Claim { account: account.to_string(), key: key.to_string(), item: item.to_string(), units, expires_secs, buy: false });
    }
}

/// `account`'s bid answer buys `purchases` (object, units) from the
/// resting asks (its previous answer's purchases end).
pub fn claim_purchases(account: &str, purchases: &[(String, i64)], secs: i64) {
    purchases_at(account, purchases, now_secs() + secs);
}

fn purchases_at(account: &str, purchases: &[(String, i64)], expires_secs: i64) {
    let mut table = board().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    table.retain(|claim| !(claim.account.eq_ignore_ascii_case(account) && claim.key == PURCHASE_KEY));
    for (item, units) in purchases.iter().filter(|(_, units)| *units > 0) {
        table.push(Claim { account: account.to_string(), key: PURCHASE_KEY.to_string(), item: item.clone(), units: *units, expires_secs, buy: true });
    }
}

/// `account` bought `units` of `item` (a partial fill landed): its
/// reservation of `item` shrinks by them, never below zero.
pub fn purchase_landed(account: &str, item: &str, units: i64) {
    let mut table = board().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    for claim in table.iter_mut().filter(|claim| claim.buy && claim.item == item && claim.account.eq_ignore_ascii_case(account)) {
        claim.units -= units.max(0);
    }
    table.retain(|claim| !(claim.buy && claim.units <= 0));
}

/// `account`'s need of `item` is met (its buy node done, its open buys
/// closed): its reservation of `item` ends, the asks it took are gone
/// from the book already (review 2026-11-20: never counted twice).
pub fn purchase_done(account: &str, item: &str) {
    board().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).retain(|claim| !(claim.buy && claim.item == item && claim.account.eq_ignore_ascii_case(account)));
}

/// Units per object the bid answers of every other account buy from the
/// resting asks (live ones only).
pub fn purchases_by_others(account: &str) -> HashMap<String, i64> {
    bought_at(account, now_secs())
}

fn bought_at(account: &str, now: i64) -> HashMap<String, i64> {
    let mut table = board().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    table.retain(|claim| claim.expires_secs > now);
    let mut out: HashMap<String, i64> = HashMap::new();
    for claim in table.iter().filter(|claim| claim.buy && !claim.account.eq_ignore_ascii_case(account)) {
        *out.entry(claim.item.clone()).or_default() += claim.units;
    }
    out
}

/// Every claim of `account` ends (it draws its next goal).
pub fn release(account: &str) {
    board().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).retain(|claim| !claim.account.eq_ignore_ascii_case(account));
}

/// The claim of the run of `node` ends (its report came back).
pub fn release_run(account: &str, node: &str) {
    board().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).retain(|claim| !(claim.account.eq_ignore_ascii_case(account) && claim.key == node));
}

/// Units per object claimed by every other account (live ones only).
pub fn claimed_by_others(account: &str) -> HashMap<String, i64> {
    claimed_at(Some(account), now_secs())
}

/// Units per object claimed by every account (the guides' build: a
/// shared view, every producer's supply on its way).
pub fn claimed_total() -> HashMap<String, i64> {
    claimed_at(None, now_secs())
}

fn claimed_at(except: Option<&str>, now: i64) -> HashMap<String, i64> {
    let mut table = board().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    table.retain(|claim| claim.expires_secs > now);
    let mut out: HashMap<String, i64> = HashMap::new();
    for claim in table.iter().filter(|claim| !claim.buy && except.is_none_or(|account| !claim.account.eq_ignore_ascii_case(account))) {
        *out.entry(claim.item.clone()).or_default() += claim.units;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{bought_at, claim_at, claim_secs, claimed_at, purchase_done, purchase_landed, purchases_at, release, release_run, run_units, GOAL_KEY, MAX_CLAIM_SECS, MIN_CLAIM_SECS};

    /// Another producer's lot is supply in flight; one's own is not; a
    /// new claim replaces the old, release and expiry end it.
    #[test]
    fn claims_are_public_supply_in_flight() {
        claim_at("claims_smith_a", GOAL_KEY, "claims_iron_full_helm", 2, 1_000);
        claim_at("claims_smith_b", GOAL_KEY, "claims_iron_full_helm", 3, 2_000);
        assert_eq!(claimed_at(Some("claims_smith_a"), 500).get("claims_iron_full_helm"), Some(&3));
        assert_eq!(claimed_at(Some("claims_other"), 500).get("claims_iron_full_helm"), Some(&5));
        assert_eq!(claimed_at(Some("claims_other"), 1_500).get("claims_iron_full_helm"), Some(&3), "the first expired");
        claim_at("claims_smith_b", GOAL_KEY, "claims_iron_dagger", 4, 2_000);
        assert_eq!(claimed_at(Some("claims_other"), 1_500).get("claims_iron_full_helm"), None, "replaced");
        release("claims_smith_b");
        assert_eq!(claimed_at(Some("claims_other"), 1_500).get("claims_iron_dagger"), None, "released");
        assert_eq!(claim_secs(100.0, 60), MIN_CLAIM_SECS);
        assert_eq!(claim_secs(1e9, 600), MAX_CLAIM_SECS);
        assert_eq!(claim_secs(2_000.0, 600), 2_400);
    }

    /// Plan équilibre step 2: a run's claim lives beside the bot's bid
    /// answer, the guides' build counts every claim, a bot the others';
    /// the run's report releases that run only, the next goal everything.
    #[test]
    fn a_run_claim_is_released_at_the_end_of_its_run() {
        claim_at("runs_smelter_a", "n7", "runs_mithril_bar", 40, 5_000);
        claim_at("runs_smelter_a", GOAL_KEY, "runs_iron_bar", 10, 5_000);
        claim_at("runs_smelter_b", "fill-n3", "runs_mithril_bar", 25, 5_000);
        assert_eq!(claimed_at(None, 100).get("runs_mithril_bar"), Some(&65), "the guides' build: every producer");
        assert_eq!(claimed_at(Some("runs_smelter_b"), 100).get("runs_mithril_bar"), Some(&40), "a pick: the others only");
        release_run("runs_smelter_a", "n7");
        assert_eq!(claimed_at(None, 100).get("runs_mithril_bar"), Some(&25), "n7 reported: its bars are no longer on their way");
        assert_eq!(claimed_at(None, 100).get("runs_iron_bar"), Some(&10), "the bid answer stays");
        release_run("runs_smelter_b", "fill-n3");
        release("runs_smelter_a");
        assert!(claimed_at(None, 100).keys().all(|item| !item.starts_with("runs_")), "all released");
        assert_eq!(run_units(120.0, 18_000.0), 360, "a run of 18 000 ticks at 120 a window");
        assert_eq!(run_units(f64::NAN, 6_000.0), 0, "no rate, no claim");
    }

    /// 2026-11-20: a bid answer's purchases are no supply on its way; the
    /// others read them, the claimer does not; a new answer replaces
    /// them, the next goal draw and expiry end them.
    #[test]
    fn purchases_reserve_the_asks_for_the_others_only() {
        purchases_at("buys_crafter_a", &[("buys_uncut_diamond".to_string(), 6), ("buys_gold_bar".to_string(), 6)], 1_000);
        claim_at("buys_crafter_a", GOAL_KEY, "buys_amulet", 6, 1_000);
        assert_eq!(bought_at("buys_crafter_b", 500).get("buys_uncut_diamond"), Some(&6));
        assert_eq!(bought_at("buys_crafter_a", 500).get("buys_uncut_diamond"), None, "its own purchases are its own");
        assert_eq!(claimed_at(Some("buys_crafter_b"), 500).get("buys_uncut_diamond"), None, "a purchase is no supply");
        assert_eq!(claimed_at(Some("buys_crafter_b"), 500).get("buys_amulet"), Some(&6));
        purchases_at("buys_crafter_a", &[("buys_uncut_diamond".to_string(), 2)], 1_000);
        assert_eq!(bought_at("buys_crafter_b", 500).get("buys_uncut_diamond"), Some(&2), "replaced");
        assert_eq!(bought_at("buys_crafter_b", 500).get("buys_gold_bar"), None, "replaced");
        assert_eq!(bought_at("buys_crafter_b", 1_500).get("buys_uncut_diamond"), None, "expired");
        purchases_at("buys_crafter_a", &[("buys_uncut_diamond".to_string(), 2)], 1_000);
        release("buys_crafter_a");
        assert_eq!(bought_at("buys_crafter_b", 500).get("buys_uncut_diamond"), None, "released with the goal");
    }

    /// Review 2026-11-20: once the reserved units are bought, the asks are
    /// gone from the book; the reservation shrinks with each fill and ends
    /// with the need, so the next asks are never hidden twice.
    #[test]
    fn a_bought_reservation_shrinks_then_ends() {
        purchases_at("landed_crafter", &[("landed_diamond".to_string(), 6), ("landed_wool".to_string(), 6)], 1_000);
        purchase_landed("landed_crafter", "landed_diamond", 4);
        assert_eq!(bought_at("landed_other", 500).get("landed_diamond"), Some(&2), "4 of 6 landed");
        purchase_landed("landed_crafter", "landed_diamond", 5);
        assert_eq!(bought_at("landed_other", 500).get("landed_diamond"), None, "all landed");
        purchase_done("landed_crafter", "landed_wool");
        assert_eq!(bought_at("landed_other", 500).get("landed_wool"), None, "need met");
    }
}
