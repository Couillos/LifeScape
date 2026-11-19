#![forbid(unsafe_code)]
//! Stock this bot's open sell orders and in-flight deals lock (live
//! 2026-11-13, b3gybcev: a listed hammer read as held, the smith script
//! could not take it out, smithing got banned). The locks are the market
//! audit's (`MarketService::locks_for`, the ones pushed to the workers):
//! what the brain counts as held for `already_held`, the tools a method
//! needs, quota baselines and fill checks is held minus locked. Coins
//! are not reduced here: their commitments ride in
//! `brain_gear::free_coins`.

use std::collections::BTreeMap;

/// Base object id → units locked by this account's resting sells and
/// seller deals (empty while the market service is down: nothing rests).
pub(crate) fn locked_items(account: &str) -> BTreeMap<i64, i64> {
    let Some(service) = crate::market_service::shared() else { return BTreeMap::new() };
    service.locks_for(&[account.to_string()]).remove(account).map(|(_, items)| items).unwrap_or_default()
}

/// Units of `item` (debug name) locked in `locks`.
pub(crate) fn locked_in(locks: &BTreeMap<i64, i64>, item: &str) -> i64 {
    lifescape_gamedata::objs_by_name(item)
        .into_iter()
        .next()
        .and_then(|obj| locks.get(&crate::market_audit::base_of(obj.obj_id as i64)).copied())
        .unwrap_or(0)
}

/// Objects `account` sold and has not handed over yet (deals still open,
/// neither settled nor failed), by base debug name, sorted: the bank
/// janitor keeps them (user 2026-11-19, point 2: ida_reed's sold big
/// bones were relisted, refused "seller holds 0", marked unsellable and
/// droppable on a full bank, the handover then short).
pub(crate) fn promised_names(account: &str, deals: &[lifescape_store::Deal]) -> Vec<String> {
    use lifescape_store::DealStatus;
    let mut names: Vec<String> = deals
        .iter()
        .filter(|deal| deal.seller == account && !matches!(deal.status, DealStatus::Settled | DealStatus::Failed))
        .filter_map(|deal| usize::try_from(crate::market_audit::base_of(deal.item_id)).ok())
        .filter_map(|id| lifescape_gamedata::obj_by_id(id).and_then(|obj| obj.name))
        .map(str::to_string)
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Held units the bot may use.
pub fn free_count(held: i64, locked: i64) -> i64 {
    (held - locked.max(0)).max(0)
}

/// Tool names (one per name, first-seen order) of live entries `(name,
/// count)` that still have a unit free of `locked`.
pub fn free_tool_names(entries: &[(String, i64)], locked: &dyn Fn(&str) -> i64) -> Vec<String> {
    let mut totals: Vec<(String, i64)> = Vec::new();
    for (name, count) in entries {
        match totals.iter_mut().find(|(known, _)| known.eq_ignore_ascii_case(name)) {
            Some(slot) => slot.1 = slot.1.saturating_add((*count).max(1)),
            None => totals.push((name.clone(), (*count).max(1))),
        }
    }
    totals.into_iter().filter(|(name, total)| free_count(*total, locked(name)) > 0).map(|(name, _)| name).collect()
}

#[cfg(test)]
#[path = "brain_locks_tests.rs"]
mod tests;
