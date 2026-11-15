//! Fleet site registry: the site each account's latest launch works
//! (`node.site`), read back as the crowd every other account's site pick
//! weighs (`planner::combat::pick_site_crowded`). A launch without a site
//! clears the entry; nothing persists across a daemon restart (the next
//! launches refill it).

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

type Site = (i64, i64, i64);

fn registry() -> &'static Mutex<HashMap<String, Site>> {
    static SITES: OnceLock<Mutex<HashMap<String, Site>>> = OnceLock::new();
    SITES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Record (or clear) the site `account` now works.
pub fn note(account: &str, site: Option<Site>) {
    let mut sites = registry().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    match site {
        Some(site) => {
            sites.insert(account.to_string(), site);
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

fn count_others(sites: &HashMap<String, Site>, account: &str) -> Vec<(Site, u32)> {
    let mut counts: HashMap<Site, u32> = HashMap::new();
    for (owner, site) in sites {
        if owner != account {
            *counts.entry(*site).or_insert(0) += 1;
        }
    }
    let mut crowd: Vec<(Site, u32)> = counts.into_iter().collect();
    crowd.sort();
    crowd
}

#[cfg(test)]
mod tests {
    use super::count_others;
    use std::collections::HashMap;

    #[test]
    fn the_crowd_counts_everyone_but_the_asker() {
        let mut sites = HashMap::new();
        sites.insert("ana".to_string(), (3082, 3422, 0));
        sites.insert("bob".to_string(), (3082, 3422, 0));
        sites.insert("cal".to_string(), (3018, 9739, 0));
        assert_eq!(count_others(&sites, "ana"), vec![((3018, 9739, 0), 1), ((3082, 3422, 0), 1)]);
        assert_eq!(count_others(&sites, "dan"), vec![((3018, 9739, 0), 1), ((3082, 3422, 0), 2)]);
    }
}
