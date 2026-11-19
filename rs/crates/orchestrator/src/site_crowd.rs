//! Fleet site registry: the site each account's latest launch works
//! (`node.site`) and its method, read back as the crowd every other
//! account's site pick weighs (`planner::combat::pick_site_crowded`:
//! every bot lengthens a site's distance) and as the seats taken on a
//! resource (`ReadyCtx::seats`: only the bots on the same resource fill
//! a site's capacity, review lot 3). A launch without a site clears the
//! entry; nothing persists across a daemon restart (the next launches
//! refill it).

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

/// Sites and methods the other accounts work, with their head count.
pub fn seats(account: &str) -> Vec<(Site, String, u32)> {
    let sites = registry().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    count_seats(&sites, account)
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

#[cfg(test)]
mod tests {
    use super::{count_others, count_seats};
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
}
