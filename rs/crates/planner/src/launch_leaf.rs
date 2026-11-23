#![forbid(unsafe_code)]
//! Pure leaves of the launch decision (lot 3 of the launch refactor),
//! moved from the orchestrator (`brain_tools`, `brain_coin_need`,
//! `brain_loot`, `brain_plan`) so the decision (`launch_decide`) and the
//! brain share ONE copy of each.

/// Verdict of an obtain on its holdings: `free` counts what the bot's own
/// listings leave free, `held` everything physically held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeldVerdict {
    /// Enough free copies: the node is done.
    Done,
    /// Enough held, locked by the bot's own listings: withdraw them.
    WithdrawListings,
    /// Not enough held: the obtain runs.
    Launch,
}

/// Pure verdict over the family counts: `held` (pack, bank, worn) and
/// `free` (held minus what the bot's resting sells lock).
pub fn held_verdict(count: i64, held: i64, free: i64) -> HeldVerdict {
    if free >= count {
        HeldVerdict::Done
    } else if held >= count {
        HeldVerdict::WithdrawListings
    } else {
        HeldVerdict::Launch
    }
}

/// A `coins:N` need computed on the free wallet: the wallet to earn adds
/// back the coins the bot's other resting buys commit, so the
/// `have_coins` target it files really frees `N` (other needs unchanged).
pub fn coin_need_lift(need: &str, committed: i64) -> String {
    match need.strip_prefix("coins:").and_then(|amount| amount.parse::<i64>().ok()) {
        Some(amount) => format!("coins:{}", amount.saturating_add(committed.max(0))),
        None => need.to_string(),
    }
}

/// A `coins:N` need the free wallet already covers is no shortage: its
/// `have_coins` closes "done" at once and the node re-resolves forever
/// (live 2026-11-23, david_clay). Dropped, the block falls to the typed
/// contract error instead of a loop.
pub fn coin_need_met(need: &str, free_coins: i64) -> bool {
    need.strip_prefix("coins:").and_then(|amount| amount.parse::<i64>().ok()).is_some_and(|amount| amount <= free_coins)
}

/// Ticks one pickup costs: the walk and the click (loot.rs: 98.9 % of
/// the takes land within 20 ticks).
pub const PICKUP_TICKS: f64 = 20.0;

/// The spot key of a fight method (`fight:melee/<spot>`,
/// `fight:ranged/<spot>`, `fight:magic/<spell>/<spot>`).
pub fn fight_spot(method: &str) -> Option<&str> {
    let tail = method.strip_prefix("fight:")?;
    tail.rsplit('/').next().filter(|spot| !spot.is_empty() && *spot != tail)
}

/// Every drop name `loot_skip` prices for `spot` (coins, bones and
/// zero-rate drops never), in the order it asks for them.
pub fn loot_candidates(spot: &str) -> Vec<String> {
    let Some(entry) = lifescape_gamedata::combat_spots::by_key(spot).or_else(|| lifescape_gamedata::combat_spots::catalogue_spots().into_iter().find(|candidate| candidate.key == spot)) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for npc in entry.npc_names.iter().flat_map(|name| lifescape_gamedata::npcs_by_name(name)) {
        for drop in lifescape_gamedata::drops::drops_for_npc(npc.npc_id) {
            if drop.obj == "coins" || drop.obj.ends_with("bones") || drop.rate <= 0.0 {
                continue;
            }
            names.push(drop.obj.clone());
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Drops of `spot`'s monsters not worth their pickup (pure): `value_of`
/// gives the public value of one unit, `coins_per_tick` the bot's rate.
pub fn loot_skip(spot: &str, value_of: &dyn Fn(&str) -> i64, coins_per_tick: f64) -> Vec<String> {
    let Some(entry) = lifescape_gamedata::combat_spots::by_key(spot).or_else(|| lifescape_gamedata::combat_spots::catalogue_spots().into_iter().find(|candidate| candidate.key == spot)) else {
        return Vec::new();
    };
    let pickup_worth = PICKUP_TICKS * coins_per_tick.max(0.0);
    let mut skip: Vec<String> = Vec::new();
    for npc in entry.npc_names.iter().flat_map(|name| lifescape_gamedata::npcs_by_name(name)) {
        for drop in lifescape_gamedata::drops::drops_for_npc(npc.npc_id) {
            if drop.obj == "coins" || drop.obj.ends_with("bones") || drop.rate <= 0.0 {
                continue;
            }
            let mean = if drop.qty_min >= 1 && drop.qty_max >= drop.qty_min { (drop.qty_min + drop.qty_max) as f64 / 2.0 } else { 1.0 };
            if (value_of(&drop.obj) as f64) * mean < pickup_worth {
                skip.push(drop.obj.clone());
            }
        }
    }
    skip.sort();
    skip.dedup();
    skip
}

/// Launch arguments of the filter.
pub fn skip_args(skip: &[String]) -> Vec<String> {
    skip.iter().flat_map(|name| ["--skip-loot".to_string(), name.clone()]).collect()
}

/// The keep list of a cleanup: the graph's names and the worn names,
/// case-insensitively unique, sorted.
pub fn merge_keep(graph: Vec<String>, worn: Vec<String>) -> Vec<String> {
    let mut keep = graph;
    keep.extend(worn);
    keep.sort_by_key(|name| name.to_lowercase());
    keep.dedup_by(|second, first| second.eq_ignore_ascii_case(first));
    keep
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_coin_need_adds_the_committed_coins() {
        assert_eq!(coin_need_lift("coins:120", 40), "coins:160");
        assert_eq!(coin_need_lift("coins:120", 0), "coins:120");
        assert_eq!(coin_need_lift("tool:pickaxe", 40), "tool:pickaxe");
        assert_eq!(coin_need_lift("coins:many", 40), "coins:many");
    }

    #[test]
    fn a_need_the_free_wallet_covers_is_met() {
        assert!(coin_need_met("coins:120", 120));
        assert!(!coin_need_met("coins:121", 120));
        assert!(!coin_need_met("level:fishing:5", 1000));
    }

    #[test]
    fn the_held_verdict_reads_free_then_held() {
        assert_eq!(held_verdict(1, 1, 0), HeldVerdict::WithdrawListings);
        assert_eq!(held_verdict(1, 1, 1), HeldVerdict::Done);
        assert_eq!(held_verdict(1, 0, 0), HeldVerdict::Launch);
        assert_eq!(held_verdict(28, 30, 20), HeldVerdict::WithdrawListings, "a quota counts free units only");
    }

    #[test]
    fn fight_methods_name_their_spot() {
        assert_eq!(fight_spot("fight:melee/chickens"), Some("chickens"));
        assert_eq!(fight_spot("fight:ranged/cows"), Some("cows"));
        assert_eq!(fight_spot("fight:magic/wind_strike/goblins"), Some("goblins"));
        assert_eq!(fight_spot("smelt:iron_bar"), None);
    }

    #[test]
    fn the_candidates_are_what_the_skip_prices() {
        let priced = std::cell::RefCell::new(Vec::new());
        let _ = loot_skip("chickens", &|name| { priced.borrow_mut().push(name.to_string()); 0 }, 1.0);
        let mut asked = priced.into_inner();
        asked.sort();
        asked.dedup();
        assert_eq!(asked, loot_candidates("chickens"));
    }

    #[test]
    fn the_keep_list_merges_case_insensitively() {
        assert_eq!(merge_keep(vec!["feather".to_string()], vec!["Iron_Sword".to_string(), "feather".to_string()]), vec!["feather".to_string(), "Iron_Sword".to_string()]);
    }
}
