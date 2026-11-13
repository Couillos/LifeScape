//! The planner's shop for a `shop:buy` leg (audit 2026-11-13 item B3/C10:
//! the script picked the nearest shop itself, Wilderness, toll and
//! underground included). The brain picks the shop and its keeper's
//! site from the obtain index (`Way::Shop`: shop key, quest, NPC spawns),
//! the script walks there and buys.
//!
//! Rules: a shop whose quest the bot has not finished is out (Oziach
//! before Dragon Slayer); a keeper in the Wilderness, underground or behind
//! a gate the bot does not pass is out (`gamedata::access`); a keeper whose
//! tile a hunter covers at the bot's combat level is out; across the
//! Al Kharid fence only with the fare covered. Nearest first.

use crate::combat::{Fare, Pass};
use crate::expand::ReadyCtx;
use lifescape_gamedata::obtain::{ways, WayKind};

/// The shop (key) and keeper site a bot buys `item` from now, if any.
pub fn pick_shop(item: &str, ctx: &ReadyCtx) -> Option<(String, (i64, i64, i64))> {
    let pass = Pass::of(ctx);
    let fare = Fare::of(ctx);
    let distance = |site: &(i64, i64, i64)| match ctx.position {
        Some((x, z, level)) => (site.0 - x).abs() + (site.1 - z).abs() + if site.2 == level { 0 } else { 1000 },
        None => 0,
    };
    let mut best: Option<(i64, String, (i64, i64, i64))> = None;
    for way in ways(item).into_iter().filter(|way| way.kind == WayKind::Shop) {
        if let Some(quest) = &way.quest {
            let key = quest.strip_prefix("quest_").unwrap_or(quest);
            if !ctx.quests_done.iter().any(|done| done == key) {
                continue;
            }
        }
        for site in &way.sites {
            let (x, z, level) = *site;
            if !lifescape_gamedata::access::gates_at(x, z, level).iter().all(|gate| pass.opens(gate)) {
                continue;
            }
            if lifescape_gamedata::danger::required_combat_at(x, z, level) > ctx.combat {
                continue;
            }
            let across = ctx.position.is_some_and(|from| lifescape_gamedata::toll::crossing(from, *site));
            if across && !(fare.free_pass || fare.wallet >= lifescape_gamedata::toll::FARE) {
                continue;
            }
            let far = distance(site);
            if best.as_ref().is_none_or(|(known, _, _)| far < *known) {
                best = Some((far, way.detail.clone(), *site));
            }
        }
    }
    best.map(|(_, shop, site)| (shop, site))
}

/// Shop key whose keeper stands at `site` and stocks `item` (the mapping
/// names the planner's shop from the node's site).
pub fn shop_at(item: &str, site: (i64, i64, i64)) -> Option<String> {
    ways(item).into_iter().filter(|way| way.kind == WayKind::Shop).find(|way| way.sites.contains(&site)).map(|way| way.detail)
}

#[cfg(test)]
mod tests {
    use super::{pick_shop, shop_at};
    use crate::expand::ReadyCtx;

    /// Shears from Lumbridge: a nearby general store, never a Wilderness
    /// or gated keeper; the mapping finds the same shop from the site.
    #[test]
    fn shears_come_from_a_reachable_shop() {
        let mut ctx = ReadyCtx::blind(0.5);
        ctx.combat = 3;
        ctx.position = Some((3222, 3218, 0));
        let (shop, site) = pick_shop("shears", &ctx).expect("a shop sells shears");
        assert!(lifescape_gamedata::access::gates_at(site.0, site.1, site.2).is_empty(), "{shop} at {site:?}");
        assert!(!lifescape_gamedata::danger::in_wilderness(site.0, site.1, site.2));
        assert_eq!(shop_at("shears", site).as_deref(), Some(shop.as_str()));
        assert!(pick_shop("no_such_item", &ctx).is_none());
    }

    /// Oziach sells the rune platebody only after Dragon Slayer.
    #[test]
    fn quest_shops_wait_for_their_quest() {
        let mut ctx = ReadyCtx::blind(0.5);
        ctx.position = Some((3222, 3218, 0));
        assert!(pick_shop("rune_platebody", &ctx).is_none(), "Dragon Slayer not done");
        ctx.quests_done = vec!["dragon".to_string()];
        assert!(pick_shop("rune_platebody", &ctx).is_some());
    }
}
