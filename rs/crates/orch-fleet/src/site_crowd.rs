//! Fleet site registry: the site each account's latest launch works
//! (`node.site`) and its method, read back as the crowd every other
//! account's site pick weighs (`planner::combat::pick_site_crowded`:
//! every bot lengthens a site's distance) and as the seats taken on a
//! resource (`ReadyCtx::seats`: only the bots on the same resource fill
//! a site's capacity, review lot 3). A launch without a site clears the
//! entry; nothing persists across a daemon restart (the next launches
//! refill it).
//!
//! Engaged seats (2026-11-20): a bid answer whose plan gathers on a site
//! of limited capacity (`bid_response::UnitCost::seats`) holds that seat
//! from its draw (`engage`) until its next goal draw (`disengage`) or
//! expiry, even while it buys or fills elsewhere: the other bots' bid
//! answers and site picks read it taken (`seats`). A bot whose latest
//! launch already works the same resource counts once (its launch).

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

type Site = (i64, i64, i64);

fn registry() -> &'static Mutex<HashMap<String, (Site, String)>> {
    static SITES: OnceLock<Mutex<HashMap<String, (Site, String)>>> = OnceLock::new();
    SITES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Record (or clear) the site `account` now works with `method`.
pub fn note(account: &str, site: Option<Site>, method: &str) {
    let mut sites = registry().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    match site {
        Some(site) => {
            sites.insert(account.to_string(), (site, method.to_string()));
        }
        None => {
            sites.remove(account);
        }
    }
}

/// Sites the other accounts work, with their head count.
pub fn others(account: &str) -> Vec<(Site, u32)> {
    let sites = registry().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    count_others(&sites, account)
}

/// Engaged seats per account: (site, method) and the expiry (unix secs).
fn engaged() -> &'static Mutex<HashMap<String, (Vec<(Site, String)>, i64)>> {
    static ENGAGED: OnceLock<Mutex<HashMap<String, (Vec<(Site, String)>, i64)>>> = OnceLock::new();
    ENGAGED.get_or_init(|| Mutex::new(HashMap::new()))
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
}

/// `account`'s bid answer holds `seats` (method, site) for `secs` wall
/// seconds at most (its previous engagement ends).
pub fn engage(account: &str, seats: &[(String, Site)], secs: i64) {
    let mut table = engaged().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if seats.is_empty() {
        table.remove(account);
        return;
    }
    table.insert(account.to_string(), (seats.iter().map(|(method, site)| (*site, method.clone())).collect(), now_secs() + secs));
}

/// `account`'s engaged seats end (it draws its next goal).
pub fn disengage(account: &str) {
    engaged().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(account);
}

/// True while `account` holds engaged seats (not expired).
pub fn is_engaged(account: &str) -> bool {
    let now = now_secs();
    engaged().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(account).is_some_and(|(_, expires)| *expires > now)
}

/// Sites and methods the other accounts work or are engaged on, with
/// their head count.
pub fn seats(account: &str) -> Vec<(Site, String, u32)> {
    let sites = registry().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    let mut table = engaged().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = now_secs();
    table.retain(|_, (_, expires)| *expires > now);
    count_seats_engaged(&sites, &table, account)
}

fn count_others(sites: &HashMap<String, (Site, String)>, account: &str) -> Vec<(Site, u32)> {
    let mut counts: HashMap<Site, u32> = HashMap::new();
    for (owner, (site, _)) in sites {
        if owner != account {
            *counts.entry(*site).or_insert(0) += 1;
        }
    }
    let mut crowd: Vec<(Site, u32)> = counts.into_iter().collect();
    crowd.sort();
    crowd
}

fn count_seats(sites: &HashMap<String, (Site, String)>, account: &str) -> Vec<(Site, String, u32)> {
    let mut counts: HashMap<(Site, String), u32> = HashMap::new();
    for (owner, (site, method)) in sites {
        if owner != account {
            *counts.entry((*site, method.clone())).or_insert(0) += 1;
        }
    }
    let mut seats: Vec<(Site, String, u32)> = counts.into_iter().map(|((site, method), count)| (site, method, count)).collect();
    seats.sort();
    seats
}

/// `count_seats` plus the engaged seats of the other accounts, an
/// engagement on the resource an account's latest launch already works
/// left out (one bot, one seat).
fn count_seats_engaged(sites: &HashMap<String, (Site, String)>, engaged: &HashMap<String, (Vec<(Site, String)>, i64)>, account: &str) -> Vec<(Site, String, u32)> {
    let mut seats = count_seats(sites, account);
    for (owner, (held, _)) in engaged.iter().filter(|(owner, _)| owner.as_str() != account) {
        for (site, method) in held {
            let working = sites.get(owner).is_some_and(|(_, launched)| lifescape_planner::combat::same_resource(launched, method));
            if working {
                continue;
            }
            match seats.iter_mut().find(|(known, known_method, _)| known == site && known_method == method) {
                Some(entry) => entry.2 += 1,
                None => seats.push((*site, method.clone(), 1)),
            }
        }
    }
    seats.sort();
    seats
}

#[cfg(test)]
mod tests {
    use super::{count_others, count_seats, count_seats_engaged};
    use std::collections::HashMap;

    #[test]
    fn the_crowd_counts_everyone_but_the_asker() {
        let mut sites = HashMap::new();
        sites.insert("ana".to_string(), ((3082, 3422, 0), "gather:mining/coal".to_string()));
        sites.insert("bob".to_string(), ((3082, 3422, 0), "fight:melee/barbarians".to_string()));
        sites.insert("cal".to_string(), ((3018, 9739, 0), "gather:mining/coal".to_string()));
        assert_eq!(count_others(&sites, "ana"), vec![((3018, 9739, 0), 1), ((3082, 3422, 0), 1)]);
        assert_eq!(count_others(&sites, "dan"), vec![((3018, 9739, 0), 1), ((3082, 3422, 0), 2)]);
        assert_eq!(
            count_seats(&sites, "dan"),
            vec![((3018, 9739, 0), "gather:mining/coal".to_string(), 1), ((3082, 3422, 0), "fight:melee/barbarians".to_string(), 1), ((3082, 3422, 0), "gather:mining/coal".to_string(), 1)]
        );
    }

    /// 2026-11-20: a bid answer's seat counts from its draw, while the bot
    /// still buys or fills elsewhere; once its launch works the same
    /// resource, the launch alone counts.
    #[test]
    fn an_engaged_seat_counts_once() {
        let mut sites = HashMap::new();
        sites.insert("lily".to_string(), ((3044, 9737, 0), "gather:mining/coal".to_string()));
        sites.insert("oscar".to_string(), ((3046, 9756, 0), "farm:mithril_ore".to_string()));
        let mut engaged = HashMap::new();
        engaged.insert("lily".to_string(), (vec![((3046, 9756, 0), "farm:mithril_ore".to_string())], i64::MAX));
        engaged.insert("oscar".to_string(), (vec![((3046, 9756, 0), "farm:mithril_ore".to_string())], i64::MAX));
        assert_eq!(
            count_seats_engaged(&sites, &engaged, "daisy"),
            vec![((3044, 9737, 0), "gather:mining/coal".to_string(), 1), ((3046, 9756, 0), "farm:mithril_ore".to_string(), 2)],
            "lily fills elsewhere and holds her mithril seat; oscar mines there already"
        );
        assert_eq!(count_seats_engaged(&sites, &engaged, "lily"), vec![((3046, 9756, 0), "farm:mithril_ore".to_string(), 1)], "its own seat is not the others'");
    }
}
