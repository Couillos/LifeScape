//! The seller of a `dialogue:buy` leg (user rule 2026-11-13: an NPC sells
//! to a bot only what F2P gives no other way, `gamedata::dialogue_shops`):
//! the nearest F2P seller whose tile no hunter covers at the bot's
//! combat level; across the Al Kharid fence (Karim, the silk trader) only
//! with the toll budgeted on top of the purchase.

use crate::expand::ReadyCtx;

/// Where a dialogue purchase happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialoguePick {
    /// The seller's spawn; `toll` when the walk crosses the fence.
    At { site: (i64, i64, i64), toll: bool },
    /// The wallet lacks purchase plus toll: these coins are the need.
    Coins(i64),
    /// No seller is safe or reachable for this bot.
    None,
}

/// Coins `units` purchases of `item` cost at its cheapest allowed seller.
pub fn purchase_cost(item: &str, units: i64) -> Option<i64> {
    lifescape_gamedata::dialogue_shops::allowed(item).iter().map(|shop| shop.cost).min().map(|cost| cost * units.max(1))
}

/// The seller for `units` of `item` from the ready context.
pub fn pick_seller(item: &str, units: i64, ctx: &ReadyCtx) -> DialoguePick {
    let distance = |site: &(i64, i64, i64)| match ctx.position {
        Some((x, z, level)) => (site.0 - x).abs() + (site.1 - z).abs() + if site.2 == level { 0 } else { 1000 },
        None => 0,
    };
    let free_pass = ctx.quests_done.iter().any(|quest| quest == lifescape_gamedata::toll::FREE_PASS_QUEST);
    let mut best: Option<(i64, i64, (i64, i64, i64), bool)> = None;
    for shop in lifescape_gamedata::dialogue_shops::allowed(item) {
        for site in lifescape_gamedata::dialogue_shops::sites(shop) {
            if lifescape_gamedata::danger::required_combat_at(site.0, site.1, site.2) > ctx.combat || !lifescape_gamedata::access::gates_at(site.0, site.1, site.2).is_empty() {
                continue;
            }
            let across = ctx.position.is_some_and(|from| lifescape_gamedata::toll::crossing(from, site));
            let toll = if across && !free_pass { lifescape_gamedata::toll::FARE } else { 0 };
            let need = shop.cost * units.max(1) + toll;
            let key = (need, distance(&site));
            if best.as_ref().is_none_or(|(known_need, known_far, _, _)| key < (*known_need, *known_far)) {
                best = Some((need, key.1, site, across));
            }
        }
    }
    match best {
        Some((need, _, _, _)) if ctx.coins < need => DialoguePick::Coins(need),
        Some((_, _, site, across)) => DialoguePick::At { site, toll: across },
        None => DialoguePick::None,
    }
}

#[cfg(test)]
mod tests {
    use super::{pick_seller, purchase_cost, DialoguePick};
    use crate::expand::ReadyCtx;

    /// Karim's kebabs from Lumbridge: across the toll fence, so the
    /// purchase plus the 10-coin gate; broke, the coins are the need.
    #[test]
    fn kebabs_cross_the_toll_with_the_fare() {
        let mut ctx = ReadyCtx::blind(0.5);
        ctx.combat = 20;
        ctx.position = Some((3222, 3218, 0));
        ctx.coins = 0;
        assert_eq!(purchase_cost("kebab", 5), Some(5));
        assert_eq!(pick_seller("kebab", 5, &ctx), DialoguePick::Coins(15));
        ctx.coins = 15;
        match pick_seller("kebab", 5, &ctx) {
            DialoguePick::At { site, toll: true } => assert!(site.0 > 3268, "Karim is in Al Kharid: {site:?}"),
            other => panic!("15 coins cover kebabs and the gate, got {other:?}"),
        }
        ctx.quests_done = vec!["prince".to_string()];
        ctx.coins = 5;
        assert!(matches!(pick_seller("kebab", 5, &ctx), DialoguePick::At { .. }), "the gate is free after Prince Ali");
        assert_eq!(pick_seller("bronze_bar", 1, &ctx), DialoguePick::None, "a producible object has no seller");
    }
}
