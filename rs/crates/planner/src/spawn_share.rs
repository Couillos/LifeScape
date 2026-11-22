//! A ground spawn's output is shared (live 2026-11-22: ~10 bots on the
//! waterrune spawn at 3298,3316, 517 filler runs a day on it): the items
//! respawn once per tile, whoever picks first. A bot adding itself to a
//! spawn gets about its rate over (bots there + 1); it works the spawn
//! the fewest bots work (the site pick weighs the crowd).

use crate::method_way::{method_way, spawn_workable};

type Site = (i64, i64, i64);

/// Share of a `ground:<item>` line's rate one more bot gets, given the
/// fleet's seats (site, method, bots): 1 for any other method.
pub fn crowd_factor(method: &str, seats: &[(Site, String, u32)]) -> f64 {
    if !method.starts_with("ground:") {
        return 1.0;
    }
    let Some((_, way)) = method_way(method) else { return 1.0 };
    let bots_at = |site: &Site| -> u32 { seats.iter().filter(|(seat, other, _)| seat == site && other == method).map(|(_, _, bots)| *bots).sum() };
    let Some(fewest) = way.sites.iter().filter(|site| spawn_workable(site)).map(bots_at).min() else { return 1.0 };
    1.0 / (f64::from(fewest) + 1.0)
}

#[cfg(test)]
mod tests {
    use super::crowd_factor;
    use crate::method_way::{method_way, spawn_workable};

    fn workable_sites(method: &str) -> Vec<(i64, i64, i64)> {
        method_way(method).expect("way").1.sites.into_iter().filter(spawn_workable).collect()
    }

    /// Ten bots on every tile leave a newcomer a eleventh of the rate;
    /// an empty tile or another method is untouched.
    #[test]
    fn a_crowded_spawn_is_shared() {
        let method = "ground:waterrune";
        let sites = workable_sites(method);
        assert!(!sites.is_empty());
        let full: Vec<_> = sites.iter().map(|site| (*site, method.to_string(), 10)).collect();
        assert!((crowd_factor(method, &full) - 1.0 / 11.0).abs() < 1e-9);
        assert_eq!(crowd_factor(method, &[]), 1.0);
        let other: Vec<_> = sites.iter().map(|site| (*site, "ground:earthrune".to_string(), 10)).collect();
        assert_eq!(crowd_factor(method, &other), 1.0, "other methods' bots take nothing");
        assert_eq!(crowd_factor("farm:waterrune", &full), 1.0);
        if sites.len() > 1 {
            let one = vec![(sites[0], method.to_string(), 10)];
            assert_eq!(crowd_factor(method, &one), 1.0, "a free tile remains");
        }
    }
}
