//! The brain's other site for a resource whose nodes deplete and respawn
//! (rocks of every ore, trees). User decision 2026-11-20 (« pour
//! l'adamantite, ça dépend du temps de marche pour accéder à un autre
//! site, si c'est plus long que l'attente, alors on attend »): the launch
//! carries the site the brain would pick next (`--alt-site`), the script
//! weighs the walk there against the respawn wait when every node of its
//! site is depleted (`scripts::kit::deplete`), and reports `depleted:`
//! when the other site is sooner; the brain then cools the site for the
//! wait and its next resolve picks exactly this other site. Pure.

use crate::combat::{method_sites, pick_site_weighed, same_resource_seats, Fare, Pass, SitePick};

/// True when `method` gathers a resource whose nodes deplete and respawn:
/// a rock with a server respawn (`rock_respawnrate`; rune essence never
/// depletes), a tree (`woodcutting_trees:respawnrate`).
pub fn depletes(method: &str) -> bool {
    match method_sites(method) {
        Some(("mining", ore)) => lifescape_gamedata::action_odds::rock_respawn(&ore).is_some_and(|ticks| ticks > 0),
        Some(("woodcut", logs)) => lifescape_gamedata::action_odds::tree_respawn_max(&logs).is_some(),
        _ => false,
    }
}

/// The site the brain would send the bot to from `site` once `site`
/// cools: the same pick (`pick_site_weighed`: safe for its combat level,
/// gates it passes, not full, not cooled for it), read from `site` with
/// `site` avoided: an open site, across the Al Kharid toll or on Karamja
/// only with the fare covered (`Tolled`, `Boat`: the next launch carries
/// `toll`/`boat`). `None` for a resource that never depletes, or no
/// such site (a fare short, a closed gate, every site cooled or full).
pub fn alternative_site(method: &str, ctx: &crate::expand::ReadyCtx, loads: i64, site: (i64, i64, i64)) -> Option<(i64, i64, i64)> {
    if !depletes(method) {
        return None;
    }
    let mut avoid = crate::cooldown::cooled_sites(&ctx.cooled, method);
    avoid.push(site);
    let seats = same_resource_seats(method, &ctx.seats);
    match pick_site_weighed(method, ctx.combat, Some(site), Fare::of(ctx), &Pass::of(ctx), &ctx.crowd, &seats, &avoid, loads) {
        SitePick::At(other) | SitePick::Tolled(other) | SitePick::Boat { site: other, .. } => Some(other),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{alternative_site, depletes};
    use crate::expand::ReadyCtx;
    use crate::model::{Alternative, Node, NodeKind, NodeStatus};

    fn miner() -> ReadyCtx {
        let mut ctx = ReadyCtx::blind(0.5);
        ctx.combat = 99;
        ctx.position = Some((3253, 3420, 0));
        ctx.coins = 1_000;
        ctx.levels = vec![("mining".to_string(), 99), ("woodcutting".to_string(), 99)];
        ctx.tools = vec!["rune_pickaxe".to_string(), "rune_axe".to_string()];
        ctx
    }

    /// Rocks and trees deplete; fishing water, rune essence and fights
    /// never do.
    #[test]
    fn rocks_and_trees_deplete() {
        assert!(depletes("gather:mining/adamantite_ore"));
        assert!(depletes("farm:coal"));
        assert!(depletes("gather:woodcut/yew_logs"));
        assert!(depletes("gather:woodcut/logs"));
        assert!(!depletes("gather:fishing/raw_shrimp"));
        assert!(!depletes("farm:blankrune"));
        assert!(!depletes("fight:melee/cows"));
    }

    /// Adamantite (live 2026-11-20: 3242,3165 and 3300,3318, either side
    /// of the Al Kharid toll): the node placed on one site carries the
    /// other one (fare covered), never a second rock of the same site;
    /// with every other site cooled for the bot, or the fare short, none.
    #[test]
    fn an_adamantite_site_carries_the_other_one() {
        let method = "gather:mining/adamantite_ore";
        let mut ctx = miner();
        let mut node = Node::new("n1", NodeKind::ReachLevel { skill: "mining".to_string(), level: 99 }, Vec::new(), 99);
        node.status = NodeStatus::Ready;
        node.method = Some(method.to_string());
        node.alternatives = vec![Alternative { method: method.to_string(), est_ticks: 6000, est_coins: 0 }];
        crate::expand::place_on_site(&mut node, &ctx);
        let site = node.site.expect("an adamantite site");
        let other = node.alt_site.expect("another adamantite site");
        assert!(!crate::cooldown::same_area(site, other), "{site:?} vs {other:?}");
        let (_, args) = crate::mapping::map_method(method, &node, None).expect("mining maps");
        assert!(args.windows(2).any(|pair| pair[0] == "--alt-site" && pair[1] == format!("{},{},{}", other.0, other.1, other.2)), "{args:?}");
        let table = crate::combat::method_site_table(method).expect("adamantite sites");
        ctx.cooled = table.iter().filter(|row| !crate::cooldown::same_area(row.site, site)).map(|row| crate::cooldown::cool_key(method, Some(row.site))).collect();
        assert_eq!(alternative_site(method, &ctx, 20, site), None, "every other site cooled");
        ctx.cooled.clear();
        assert!(alternative_site(method, &ctx, 20, site).is_some());
        ctx.coins = 0;
        if lifescape_gamedata::toll::crossing(site, other) {
            assert_eq!(alternative_site(method, &ctx, 20, site), None, "the toll short: no other site");
        }
    }

    /// Fishing water never depletes: no other site rides the launch.
    #[test]
    fn water_carries_no_other_site() {
        let mut ctx = miner();
        ctx.position = Some((3092, 3243, 0));
        assert_eq!(alternative_site("gather:fishing/raw_shrimp", &ctx, 20, (3085, 3230, 0)), None);
    }
}
