//! The planner's shop for a `shop:buy` leg (audit 2026-11-13 item B3/C10:
//! the script picked the nearest shop itself, toll and underground
//! included). The brain picks the shop and its keeper's
//! site from the obtain index (`Way::Shop`: shop key, quest, NPC spawns),
//! the script walks there and buys.
//!
//! Rules: a shop whose quest the bot has not finished is out (Oziach
//! before Dragon Slayer); a keeper underground or behind a gate the bot
//! does not pass is out (`gamedata::access`), except the underground
//! floors the nav routes (`shop_floor_routed`: the Dwarven Mine, Nurmof's
//! pickaxes, 2026-11-14); a keeper whose tile a hunter covers at the
//! bot's combat level is out; the Wilderness is open (user 2026-11-16);
//! across the Al Kharid fence or on Karamja (user 2026-11-16: "le planner
//! doit pouvoir aller à Karamja") only with the fares covered, the launch
//! then carries `toll` / `boat`. A keeper reached without a fare first,
//! then the nearest.

use crate::combat::{Fare, Pass};
use crate::expand::ReadyCtx;
use lifescape_gamedata::obtain::{ways, WayKind};

/// The shop (key) and keeper site a bot buys `item` from now, if any.
/// A keeper site this bot cooled (`shop:buy@<site>`: an empty live stock,
/// `contended`, live 2026-11-17 Zezima relaunched the same empty gem shop
/// 59 times in 20 min) is out until its cooldown ends.
pub fn pick_shop(item: &str, ctx: &ReadyCtx) -> Option<(String, (i64, i64, i64))> {
    let cooled = crate::cooldown::cooled_sites(&ctx.cooled, "shop:buy");
    pick_shop_among(item, ctx, &|site| cooled.contains(site))
}

/// True when a keeper would sell `item` to this bot but every such site
/// is cooled for it: the node waits (`cooled:shop:buy`), never relaunches.
pub fn every_shop_cooled(item: &str, ctx: &ReadyCtx) -> bool {
    pick_shop(item, ctx).is_none() && pick_shop_among(item, ctx, &|_| false).is_some()
}

fn pick_shop_among(item: &str, ctx: &ReadyCtx, skipped: &dyn Fn(&(i64, i64, i64)) -> bool) -> Option<(String, (i64, i64, i64))> {
    let pass = Pass::of(ctx);
    let fare = Fare::of(ctx);
    let distance = |site: &(i64, i64, i64)| match ctx.position {
        Some((x, z, level)) => (site.0 - x).abs() + (site.1 - z).abs() + if site.2 == level { 0 } else { 1000 },
        None => 0,
    };
    let mut best: Option<((bool, i64), String, (i64, i64, i64))> = None;
    for way in ways(item).into_iter().filter(|way| way.kind == WayKind::Shop) {
        if let Some(quest) = &way.quest {
            let key = quest.strip_prefix("quest_").unwrap_or(quest);
            if !ctx.quests_done.iter().any(|done| done == key) {
                continue;
            }
        }
        for site in way.sites.iter().filter(|site| !skipped(site)) {
            let (x, z, level) = *site;
            // An underground keeper the nav routes (the Dwarven Mine:
            // Nurmof's pickaxes) is a walk, not a refusal.
            let routed = lifescape_gamedata::access::shop_floor_routed(x, z, level);
            let boat = lifescape_gamedata::access::behind_boat(x, z, level);
            let passes = |gate: &lifescape_gamedata::access::Gate| {
                pass.opens(gate)
                    || (routed && *gate == lifescape_gamedata::access::Gate::Underground)
                    || (boat && matches!(gate, lifescape_gamedata::access::Gate::Boat { area: "karamja", .. }) && !lifescape_gamedata::access::impassable(gate))
            };
            if !lifescape_gamedata::access::gates_at(x, z, level).iter().all(passes) {
                continue;
            }
            if lifescape_gamedata::danger::required_combat_at(x, z, level) > ctx.combat {
                continue;
            }
            let need = crate::combat::fare_need(*site, ctx.position, fare);
            if need > fare.wallet {
                continue;
            }
            // A paid trip only when no free keeper sells (the boat costs a
            // round trip): rank (paid, distance).
            let rank = (need > 0, distance(site));
            if best.as_ref().is_none_or(|(known, _, _)| rank < *known) {
                best = Some((rank, way.detail.clone(), *site));
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

    /// Shears from Lumbridge: a nearby general store, never a gated
    /// keeper; the mapping finds the same shop from the site.
    #[test]
    fn shears_come_from_a_reachable_shop() {
        let mut ctx = ReadyCtx::blind(0.5);
        ctx.combat = 3;
        ctx.position = Some((3222, 3218, 0));
        let (shop, site) = pick_shop("shears", &ctx).expect("a shop sells shears");
        assert!(lifescape_gamedata::access::gates_at(site.0, site.1, site.2).is_empty(), "{shop} at {site:?}");
        assert_eq!(shop_at("shears", site).as_deref(), Some(shop.as_str()));
        assert!(pick_shop("no_such_item", &ctx).is_none());
    }

    /// A keeper site cooled for this bot (empty stock, `contended`) is
    /// skipped: the next keeper sells, and when every keeper is cooled the
    /// node waits (`every_shop_cooled`), never relaunching the same shop.
    #[test]
    fn cooled_keepers_are_skipped() {
        let mut ctx = ReadyCtx::blind(0.5);
        ctx.combat = 3;
        ctx.position = Some((3222, 3218, 0));
        let (_, first) = pick_shop("shears", &ctx).expect("a shop sells shears");
        ctx.cooled = vec![crate::cooldown::cool_key("shop:buy", Some(first))];
        let next = pick_shop("shears", &ctx).map(|(_, site)| site);
        assert!(next.is_some_and(|site| site != first), "another keeper: {next:?}");
        assert!(!super::every_shop_cooled("shears", &ctx));
        let mut every = ctx.clone();
        while let Some((_, site)) = pick_shop("shears", &every) {
            every.cooled.push(crate::cooldown::cool_key("shop:buy", Some(site)));
        }
        assert!(super::every_shop_cooled("shears", &every), "every keeper cooled: the node waits");
    }

    /// User 2026-11-14: upgrade pickaxes are Nurmof's alone (Dwarven
    /// Mine, under Falador's trapdoor): a fresh bot from Falador buys
    /// there, the mapping names `pickaxeshop` back from the site.
    #[test]
    fn pickaxes_come_from_nurmof() {
        let mut ctx = ReadyCtx::blind(0.5);
        ctx.combat = 3;
        ctx.position = Some((2965, 3380, 0));
        for pickaxe in ["iron_pickaxe", "steel_pickaxe", "mithril_pickaxe", "adamant_pickaxe", "rune_pickaxe"] {
            let (shop, site) = pick_shop(pickaxe, &ctx).expect("Nurmof sells it");
            assert_eq!(shop, "pickaxeshop", "{pickaxe} at {site:?}");
            assert_eq!(shop_at(pickaxe, site).as_deref(), Some("pickaxeshop"));
        }
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

    /// User 2026-11-16: "le planner doit pouvoir aller à Karamja". The rum
    /// sells only there: a bot with the boat's round trip buys it on the
    /// island and the launch sails (`boat`); without the fare, no shop.
    #[test]
    fn karamja_keepers_sell_with_the_boat_fare() {
        let mut ctx = ReadyCtx::blind(0.5);
        ctx.combat = 20;
        ctx.position = Some((3092, 3245, 0));
        ctx.coins = 500;
        let (shop, site) = pick_shop("karamja_rum", &ctx).expect("the Karamja pub sells rum");
        assert!(lifescape_gamedata::access::behind_boat(site.0, site.1, site.2), "{shop} at {site:?}");
        let mut node = crate::Node::new("rum", crate::NodeKind::ObtainItems { item: "karamja_rum".to_string(), count: 1 }, Vec::new(), 1);
        node.site = Some(site);
        node.boat = true;
        let (script, args) = crate::mapping::map_method("shop:buy", &node, None).expect("shop maps");
        assert_eq!(script, "market");
        assert!(args.contains(&"boat".to_string()), "the launch sails: {args:?}");
        ctx.coins = 0;
        assert!(pick_shop("karamja_rum", &ctx).is_none(), "no fare, no boat");
    }

    /// Every F2P quest key (39 quest points).
    fn every_quest() -> Vec<String> {
        lifescape_gamedata::quest_requirements::all_f2p_quests().into_iter().map(|quest| quest.key).collect()
    }

    /// User 2026-11-16 (Q7): Champions' Guild purchases for every bot that
    /// holds 32 quest points. Scavvo stands upstairs (3191,3351,1): the
    /// launch names his shop and his floor (`market shopbuy` walks the
    /// level, the nav opens the door on `qp:32`).
    #[test]
    fn champions_guild_keepers_sell_past_32_quest_points() {
        let mut ctx = ReadyCtx::blind(0.5);
        ctx.combat = 40;
        ctx.position = Some((3212, 3424, 0));
        assert!(pick_shop("rune_chainbody", &ctx).is_none(), "no quest points");
        ctx.quests_done = vec!["runemysteries".to_string(), "cook".to_string()];
        assert!(pick_shop("rune_chainbody", &ctx).is_none(), "2 points");
        ctx.quests_done = every_quest();
        let (shop, site) = pick_shop("rune_chainbody", &ctx).expect("Scavvo sells it");
        assert_eq!((shop.as_str(), site), ("runiteshop", (3191, 3351, 1)));
        let (shop, site) = pick_shop("adamant_platebody", &ctx).expect("Valaine sells it");
        assert_eq!((shop.as_str(), site), ("championshop", (3193, 3361, 1)));
        let mut node = crate::Node::new("chain", crate::NodeKind::ObtainItems { item: "rune_chainbody".to_string(), count: 1 }, Vec::new(), 1);
        node.site = Some((3191, 3351, 1));
        let (script, args) = crate::mapping::map_method("shop:buy", &node, None).expect("shop maps");
        assert_eq!(script, "market");
        assert!(args.windows(2).any(|pair| pair[0] == "--shop" && pair[1] == "runiteshop"), "{args:?}");
        assert!(args.windows(2).any(|pair| pair[0] == "--site" && pair[1] == "3191,3351,1"), "his floor: {args:?}");
    }

    /// The resolver takes the guild keeper for a bot with the points and
    /// the coins; an Iron without them (no market; the coif is Scavvo's
    /// alone, nobody makes it) blocks on the door, which the quest-point
    /// ladder opens (`missing:access:champions_guild`). A piece a skill
    /// also makes (rune chainbody, smithing 96) keeps the level gap first,
    /// like a quest-gated shop.
    #[test]
    fn the_guild_door_is_a_gap_the_ladder_fills() {
        let item = "coif";
        let alternatives = crate::obtain_legs::obtain_alternatives(item, true);
        assert!(alternatives.iter().any(|alt| alt.method == "shop:buy"), "{alternatives:?}");
        let mut node = crate::Node::new("chain", crate::NodeKind::ObtainItems { item: item.to_string(), count: 1 }, Vec::new(), 1);
        node.alternatives = alternatives;
        let mut ctx = ReadyCtx::blind(0.5);
        ctx.ironman = true;
        ctx.combat = 40;
        ctx.position = Some((3212, 3424, 0));
        ctx.levels = vec![("smithing".to_string(), 30)];
        ctx.coins = 2_000;
        ctx.shop_price = 200;
        let mut shut = node.clone();
        crate::expand::resolve_ready(&mut shut, &ctx);
        assert_eq!(shut.method, None);
        assert_eq!(shut.status, crate::NodeStatus::Blocked { reason: "missing:access:champions_guild".to_string() });
        ctx.quests_done = every_quest();
        let mut open = node.clone();
        crate::expand::resolve_ready(&mut open, &ctx);
        assert_eq!(open.method.as_deref(), Some("shop:buy"), "{:?}", open.status);
        assert_eq!(open.site, Some((3191, 3351, 1)), "Scavvo's floor");
    }
}
