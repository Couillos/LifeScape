#![forbid(unsafe_code)]
//! `orchestrator rares`: every object the fleet bids for while nobody
//! sells it, classed (`planner::rare_items`: producible now, F2P shop,
//! out of reach, unfindable) from the live book and the fleet's live
//! levels and quests (plan-marche §4.4, lot M9). Read-only operator
//! information: no bot reads it, no order moves (Q8: the bids stay).

use std::time::Duration;

use lifescape_planner::rare_items::{bid_only_items, classify, BotReach, RareClass, RestingOrder};

/// Longest detail shown per object (chains can name many inputs).
const DETAIL_CHARS: usize = 320;

/// Base debug name of a market id (a banknote counts as its object).
fn base_name(item_id: i64) -> Option<String> {
    let obj = lifescape_gamedata::obj_by_id(usize::try_from(item_id).ok()?)?;
    let name = obj.name?;
    Some(name.strip_prefix("cert_").unwrap_or(name).to_string())
}

/// True for an account the classification counts (staff, the owner's
/// account and the persisted exclusions out, AGENTS.md rule 11).
fn counted(account: &str, excluded: &std::collections::BTreeSet<String>) -> bool {
    !crate::economy_guard::is_staff(account) && !excluded.contains(&account.to_lowercase())
}

/// The report lines for the daemon's `rares` command. `accounts`: the
/// daemon's jobs (the fleet, before exclusions).
pub(crate) fn rares_lines(accounts: &[String]) -> Result<Vec<String>, String> {
    let service = crate::market_service::shared().ok_or_else(|| "market service down".to_string())?;
    let orders: Vec<RestingOrder> = {
        let engine = service.lock_engine();
        let mut rows = Vec::new();
        for (item_id, book) in engine.books.iter() {
            let Some(item) = base_name(*item_id) else { continue };
            for order in book.orders().into_iter().filter(|order| order.remaining > 0) {
                rows.push(RestingOrder { item: item.clone(), account: order.account.clone(), buy: order.side == lifescape_market::Side::Buy });
            }
        }
        rows
    };
    let excluded = crate::economy_guard::excluded_accounts();
    let fleet: Vec<BotReach> = accounts
        .iter()
        .filter(|account| counted(account, &excluded))
        .map(|account| BotReach {
            account: account.clone(),
            levels: crate::brain_ambition::account_levels(account, crate::jobs::live_of(account).as_ref()),
            quests_done: crate::brain::load_life(account).map(|life| crate::brain_rescue::quests_done(&life)).unwrap_or_default(),
        })
        .collect();
    let rows = bid_only_items(&orders, &|account| counted(account, &excluded));
    Ok(report(&rows, &fleet))
}

/// Pure: the classed rows, grouped by class in the class order.
pub fn report(rows: &[(String, usize, usize)], fleet: &[BotReach]) -> Vec<String> {
    let mut lines = vec![format!(
        "{} objects with bids and no ask, fleet of {} bots (staff out); operator info only, the bids stay (Q8)",
        rows.len(),
        fleet.len()
    )];
    let mut classed: Vec<(RareClass, String)> = rows
        .iter()
        .map(|(item, bids, bidders)| {
            let verdict = classify(item, fleet);
            let display = lifescape_gamedata::objs_by_name(item).into_iter().next().and_then(|obj| obj.display).unwrap_or("?");
            let mut detail = verdict.detail.clone();
            if detail.chars().count() > DETAIL_CHARS {
                detail = detail.chars().take(DETAIL_CHARS).collect::<String>() + "…";
            }
            (verdict.class, format!("  {item} ({display}): {bids} bids, {bidders} bidders — {detail}"))
        })
        .collect();
    classed.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    let mut current: Option<RareClass> = None;
    for (class, line) in classed {
        if current != Some(class) {
            lines.push(format!("{}:", class.label()));
            current = Some(class);
        }
        lines.push(line);
    }
    lines
}

/// CLI side: asks the daemon, prints its lines.
pub fn rares_cli() -> i32 {
    let request = lifescape_json::object(vec![("cmd", lifescape_json::str_value("rares"))]);
    match crate::client::send_command(&request, Duration::from_secs(60)) {
        Ok(reply) if reply.get("ok").as_bool() == Some(true) => {
            for line in reply.get("lines").as_arr().cloned().unwrap_or_default() {
                println!("{}", line.as_str().unwrap_or(""));
            }
            0
        }
        Ok(reply) => {
            eprintln!("rares: {}", lifescape_json::stringify(reply.get("error")));
            1
        }
        Err(error) => {
            eprintln!("rares: {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{counted, report};
    use lifescape_planner::rare_items::BotReach;

    /// Rows come grouped under their class label, the class order fixed.
    #[test]
    fn the_report_groups_by_class() {
        let smith = BotReach { account: "smith".to_string(), levels: vec![("smithing".to_string(), 52), ("crafting".to_string(), 20), ("mining".to_string(), 40)], quests_done: Vec::new() };
        let rows = vec![("blessedstar".to_string(), 10, 10), ("harpoon".to_string(), 1, 1), ("stringstar".to_string(), 2, 1), ("rune_full_helm".to_string(), 2, 2)];
        let lines = report(&rows, &[smith]);
        let labels: Vec<&str> = lines.iter().filter(|line| line.ends_with(':')).map(String::as_str).collect();
        assert_eq!(labels, vec!["producible now:", "F2P shop:", "out of reach:", "unfindable:"], "{lines:#?}");
        assert!(lines[0].starts_with("4 objects"), "{lines:?}");
        assert!(lines.iter().any(|line| line.contains("blessedstar (Holy symbol): 10 bids, 10 bidders")), "{lines:#?}");
    }

    #[test]
    fn staff_and_exclusions_do_not_count() {
        let excluded: std::collections::BTreeSet<String> = ["bite".to_string(), "t99".to_string()].into_iter().collect();
        assert!(!counted("admin", &excluded) && !counted("Admin2", &excluded) && !counted("T99", &excluded) && !counted("bite", &excluded));
        assert!(counted("fiona_park", &excluded));
    }
}
