//! Ready-time resolution of obtain, coin and money nodes (PRD3 lot 4).
//!
//! Obtain order:
//! 1. market buy when an ask exists the wallet covers (take it), or
//!    covers in part (take what it covers: the rest is produced or
//!    bought on the next resolve); when the bot can also produce it, the
//!    bid takes the ask while the bot produces (never idle on a handover);
//! 2. the cheapest production the bot can work now (skill level, tool),
//!    with a standing market bid beside it when the bot has coins (D-B:
//!    the first of the two to land closes the node, the brain cancels
//!    the other);
//! 3. the NPC shop, only when D-A allows it and the wallet covers it;
//! 4. a resting market bid (no production, coins enough);
//! 5. a typed block: the skill level a production needs
//!    (`missing:level:smithing:15`), its tool (`missing:bronze_axe`), or
//!    the coins a purchase needs (`missing:coins:N`, filed as a
//!    `have_coins` node that earns them), else the quest the only ways
//!    need (`missing:quest:runemysteries`, filed as the quest's nodes).
//!    A keeper behind the Champions' Guild door counts once the bot
//!    holds 32 quest points; without them, and nothing else, the door
//!    (`missing:access:champions_guild`: the quest-point ladder).
//!    Never a silent substitute. A LIGHT quest (`quests::light_quest`)
//!    comes before the resting bid when no ask is covered, and always
//!    when the quest itself hands the object over (`quests::QUEST_GRANTS`:
//!    the Duke's air talisman).
//!
//! Coin and money nodes pick the most profitable money method the bot
//! can work NOW at its levels (published rates, market prices; a hunt
//! that needs food it does not hold comes after, `food_gate`), not the
//! fastest one; coin drops only break ties for a coin target.

use crate::cost::{cost_farm, cost_market_buy};
use crate::expand::ReadyCtx;
use crate::model::{Alternative, Node, NodeStatus};
use crate::obtain_legs::{production_legs, tradeable_f2p};
use crate::quests::missing_quest;
use crate::tools::{method_feasible, required_tool};

/// Live level of `skill` (unknown = not gated: offline contexts).
fn level_of(ctx: &ReadyCtx, skill: &str) -> Option<u8> {
    ctx.levels.iter().find(|(name, _)| name == skill).map(|(_, level)| *level)
}

/// Per-unit ceiling of a bid: what the wallet covers, nothing else
/// (user decision 2026-11-14: the `min(coins/qty, max(3 x ref, 20))`
/// "fair" cap is gone; a buyer is limited only by what it can pay, the
/// market sets what the object is worth).
pub fn bid_ceiling(ctx: &ReadyCtx, count: i64) -> i64 {
    // A capped purchase (equipment step): its wallet share is the cap,
    // and it rises with the wallet (no fair-price clamp: the market sets
    // what the object is worth).
    if let Some(share) = ctx.coin_share_pct {
        return (crate::gear_ladder::wallet_ceiling(ctx.coins, share) / count.max(1)).max(1);
    }
    (ctx.coins / count.max(1)).max(1)
}

/// Public floor of a credible bid on `item` (plan marché §3.6, the
/// orchestrator's `market_lineage::credible_floor`): its low alchemy
/// value, a quarter of its current price, at least 1 coin. Public data
/// only (D22).
pub fn credible_floor(item: &str, ctx: &ReadyCtx) -> i64 {
    let alch_low = lifescape_gamedata::objs_by_name(item).into_iter().next().map(|obj| obj.alch_low).unwrap_or(0);
    alch_low.max(ctx.price / 4).max(1)
}

/// Coins the market legs may spend: the wallet, or its share when the
/// brain caps the purchase (`coin_share_pct`).
fn market_coins(ctx: &ReadyCtx) -> i64 {
    match ctx.coin_share_pct {
        Some(share) => crate::gear_ladder::wallet_ceiling(ctx.coins, share),
        None => ctx.coins,
    }
}

/// Wallet a purchase of `cost` coins needs: the cost itself, or the
/// wallet whose share covers it when the purchase is capped.
fn wallet_for(ctx: &ReadyCtx, cost: i64) -> i64 {
    match ctx.coin_share_pct {
        Some(share) => crate::gear_ladder::coins_for(cost, share),
        None => cost,
    }
}

/// Units a `market:buy` leg orders: what is still missing, capped by
/// what the wallet covers at the best ask (else the reference).
pub fn buy_quantity(ctx: &ReadyCtx, count: i64) -> i64 {
    let need = (count.max(1) - ctx.held).max(1);
    let unit = ctx.best_ask.unwrap_or(ctx.price).max(1);
    need.min(ctx.coins / unit).max(1)
}

pub(crate) fn resolve_obtain(node: &mut Node, item: &str, count: i64, ctx: &ReadyCtx) {
    let count = count.max(1);
    let need = (count - ctx.held).max(1);
    let legs = production_legs(item);
    let candidates: Vec<String> = node.alternatives.iter().map(|alt| alt.method.clone()).collect();
    let market_ok = !ctx.ironman && tradeable_f2p(item) && candidates.iter().any(|method| method == "market:buy");
    let unit = ctx.best_ask.unwrap_or(ctx.price).max(1);
    let mut priced: Vec<Alternative> = Vec::new();
    let mut workable: Vec<Alternative> = Vec::new();
    let mut level_gap: Option<(String, i64)> = None;
    let mut tool_gap: Option<String> = None;
    let mut quest_gap: Option<String> = None;
    let mut sick_gap: Option<String> = None;
    for method in &candidates {
        if method == "market:buy" || method == "shop:buy" || method == "market:bid" || method == "dialogue:buy" {
            continue;
        }
        let Some(leg) = legs.iter().find(|leg| &leg.method == method) else { continue };
        let est = cost_farm(count, ctx.yield_per_6000.max(1.0), ctx.trip_ticks);
        let alt = Alternative { method: method.clone(), est_ticks: est, est_coins: 0 };
        priced.push(alt.clone());
        // A way behind an unfinished quest never launches: the quest is
        // the dependency when nothing else works (Rune Mysteries before
        // essence).
        if let Some(quest) = missing_quest(method, Some(item), ctx) {
            quest_gap.get_or_insert(quest);
            continue;
        }
        if let Some((skill, needed)) = &leg.skill {
            if level_of(ctx, skill).is_some_and(|have| i64::from(have) < *needed) {
                if level_gap.as_ref().is_none_or(|(_, known)| needed < known) {
                    level_gap = Some((skill.clone(), *needed));
                }
                continue;
            }
        }
        if !method_feasible(method, ctx) {
            if let Some(why) = ctx.sick(method) {
                // Quarantined (`method_health`): its gear is no gap (live
                // 2026-11-17, kevin_ellis: `missing:harpoon` with the
                // harpoon held, `farm:raw_swordfish` quarantined; four
                // harpoon obtains "held, done", then abandoned and fishing
                // banned).
                sick_gap.get_or_insert(format!("{why}:{method}"));
            } else if tool_gap.is_none() {
                // The tool first; with the tool at hand, the bait it burns
                // (feathers for trout, fishing bait for pike). Gear held:
                // never named missing.
                tool_gap = required_tool(method).and_then(|need| need.missing(ctx));
            }
            continue;
        }
        workable.push(alt);
    }
    if market_ok {
        priced.push(Alternative {
            method: "market:buy".to_string(),
            est_ticks: cost_market_buy(ctx.trip_ticks, unit, need, ctx.coins_per_tick.max(0.01)),
            est_coins: unit * need,
        });
    }
    let shop_cover = ctx.shop_price.max(1) * count;
    let shop_quest = missing_quest("shop:buy", Some(item), ctx);
    // A kit axe step lists `shop:buy` itself (D-A exception,
    // `kit_shop_route`); graph obtains list it only when D-A allows. A
    // keeper behind the Champions' Guild door sells only to a bot with
    // the quest points (`shop_gate`, user 2026-11-16 Q7); without them
    // the door is the gap when nothing else serves (an Iron's rune
    // chainbody climbs the quest-point ladder, `access:champions_guild`).
    let shop_route = crate::expand::source_flags(item).1 || lifescape_gamedata::obtain::kit_shop_route(item);
    let shop_door = crate::expand::shop_gate(item, ctx).filter(|_| !lifescape_gamedata::obtain::kit_shop_route(item));
    let shop_listed = candidates.iter().any(|method| method == "shop:buy") && shop_route && shop_quest.is_none() && shop_door.is_none();
    let access_gap = shop_door.filter(|_| candidates.iter().any(|method| method == "shop:buy") && shop_quest.is_none());
    if candidates.iter().any(|method| method == "shop:buy") {
        if let Some(quest) = shop_quest {
            quest_gap.get_or_insert(quest);
        }
    }
    if shop_listed {
        priced.push(Alternative {
            method: "shop:buy".to_string(),
            est_ticks: cost_market_buy(ctx.trip_ticks, ctx.shop_price.max(1), count, ctx.coins_per_tick.max(0.01)),
            est_coins: shop_cover,
        });
    }
    // Dialogue sale (merchant rule, `gamedata::dialogue_shops`): priced
    // at the seller's own coins per unit.
    let dialogue_cover = candidates
        .iter()
        .any(|method| method == "dialogue:buy")
        .then(|| crate::dialogue_site::purchase_cost(item, need))
        .flatten();
    if let Some(cover) = dialogue_cover {
        priced.push(Alternative {
            method: "dialogue:buy".to_string(),
            est_ticks: cost_market_buy(ctx.trip_ticks, cover / need.max(1), need, ctx.coins_per_tick.max(0.01)),
            est_coins: cover,
        });
    }
    // 1. A covered ask: take it, whole or the part the wallet covers
    // (any ask the wallet pays: no fair-price cap, 2026-11-14).
    let budget = market_coins(ctx);
    let ask_covered = market_ok && ctx.best_ask.is_some_and(|ask| ask * need <= budget);
    let ask_partial = market_ok && ctx.best_ask.is_some_and(|ask| ask <= budget);
    // A production the bot works now (food in hand where its hunt
    // bites) before one whose prerequisite is still to obtain.
    let producible = workable
        .iter()
        .filter(|alt| crate::food_gate::ready_now(&alt.method, ctx))
        .min_by_key(|alt| alt.est_ticks)
        .or_else(|| workable.iter().min_by_key(|alt| alt.est_ticks))
        .cloned();
    // A light quest that unlocks a way (essence behind Rune Mysteries) or
    // hands the object over (the Duke's air talisman) comes before a
    // resting bid: without the quest nobody fills it (live 2026-11-14:
    // `market order for 17 blankrune unfilled after 6000 ticks`, then
    // runecraft and mining banned). An unlocking quest yields to a
    // covered ask; a granting one never does (the bot needs the quest
    // for the essence anyway, the object comes with it).
    let granted = crate::quests::granting_quest(item, ctx).filter(|_| need <= 1);
    if granted.is_some() {
        quest_gap = granted.clone();
    }
    let quest_first = quest_gap.as_deref().is_some_and(crate::quests::light_quest) && (granted.is_some() || !(ask_covered || ask_partial));
    // A resting bid under the public credibility floor (§3.6, wave 2
    // (c)): the coins of a credible one first, never the noise bid.
    let credible_unit = unit.max(credible_floor(item, ctx));
    let incredible = market_ok && !quest_first && bid_ceiling(ctx, need) < credible_floor(item, ctx);
    let pick: Option<String> = if granted.is_some() && quest_first {
        // The quest hands it over: no drop farm, no purchase.
        None
    } else if shop_listed && shop_cover <= ctx.coins && ctx.best_ask.is_none_or(|ask| ask >= ctx.shop_price.max(1)) {
        // 0. The keeper is the cheapest seller and the wallet pays it
        // (user 2026-11-15: every stocked object may be bought, the shop
        // price caps the market): buy there, like a player buying his
        // hammer instead of smithing one. Coins leave the world.
        Some("shop:buy".to_string())
    } else if (ask_covered || ask_partial) && producible.is_some() {
        // 1a. The bid takes the ask at once (`market:bid` at the ask, the
        // market first) and the bot produces meanwhile: whichever lands
        // first closes the node (D-B). Live 2026-11-13: a pure buy left
        // the worker idle 36–95 minutes waiting for its handover.
        priced.push(Alternative { method: "market:bid".to_string(), est_ticks: 0, est_coins: bid_ceiling(ctx, need) * need });
        producible.map(|best| best.method)
    } else if ask_covered || ask_partial {
        // 1b. Nothing the bot can make: buy and wait for the handover.
        Some("market:buy".to_string())
    } else if let Some(best) = producible {
        // 2. Produce it, with a standing bid beside it (D-B).
        if market_ok && budget >= need {
            priced.push(Alternative { method: "market:bid".to_string(), est_ticks: 0, est_coins: bid_ceiling(ctx, need) * need });
        }
        Some(best.method.clone())
    } else if shop_listed && shop_cover <= ctx.coins {
        // 3. Merchant, D-A allowed.
        Some("shop:buy".to_string())
    } else if dialogue_cover.is_some_and(|cover| cover <= ctx.coins) {
        // 3b. A dialogue seller, same rule (the seller's site may add the
        // toll: `place_on_site` budgets it).
        Some("dialogue:buy".to_string())
    } else if market_ok && budget >= unit && !quest_first && !incredible {
        // 4. Resting bid for what the wallet covers, the brain waits for
        // the fill; a ceiling under the public credibility floor is no
        // offer (§3.6): the coins first (step 5).
        Some("market:buy".to_string())
    } else {
        None
    };
    // Keep every candidate listed (travel bans prune this list); the
    // standing-bid marker rides along only when chosen above.
    let mut kept: Vec<Alternative> = priced;
    for method in &candidates {
        if !kept.iter().any(|alt| &alt.method == method) && method != "market:bid" {
            kept.push(Alternative { method: method.clone(), est_ticks: 0, est_coins: 0 });
        }
    }
    kept.sort_by_key(|alt| alt.est_ticks);
    node.alternatives = kept;
    node.method = pick.clone();
    if pick.is_none() {
        // 5. Typed block: the cheapest thing that unlocks a way.
        let reason = if let (true, Some(quest)) = (quest_first, quest_gap.as_ref()) {
            Some(format!("missing:quest:{quest}"))
        } else if incredible && budget >= unit {
            // Only the credibility held the bid back: earn its coins.
            Some(format!("missing:coins:{}", wallet_for(ctx, credible_unit * need)))
        } else if let Some((skill, level)) = level_gap {
            Some(format!("missing:level:{skill}:{level}"))
        } else if let Some(tool) = tool_gap {
            Some(format!("missing:{tool}"))
        } else if let (Some(cover), false, false) = (dialogue_cover, market_ok, shop_listed) {
            Some(format!("missing:coins:{}", wallet_for(ctx, cover)))
        } else if market_ok || shop_listed {
            // A market bid needs the coins of a CREDIBLE ceiling (§3.6).
            let price = if shop_listed && !market_ok { ctx.shop_price.max(1) } else { credible_unit };
            Some(format!("missing:coins:{}", wallet_for(ctx, price * need)))
        } else if let Some(quest) = quest_gap {
            Some(format!("missing:quest:{quest}"))
        } else {
            access_gap.map(|area| format!("missing:access:{area}")).or_else(|| sick_gap)
        };
        if let Some(reason) = reason {
            node.status = NodeStatus::Blocked { reason };
        }
    }
}

/// An obtain whose field legs (ground spawn, drop farm) are safe only
/// above the bot's combat level, while a purchase sells it, blocks on
/// the coins of the purchase, never on the combat level (live
/// 2026-11-14, ryan_mason: one 1-coin air rune blocked on `combat:43`,
/// 39 levels of training filed for it and no money method ever ran).
/// The coins are a `have_coins` node the money methods earn; the
/// cheapest priced purchase leg (market, shop, dialogue seller) sets the
/// wallet. Without a purchase leg (Iron, untradeable) the combat level
/// stays the block.
pub(crate) fn prefer_purchase_over_combat(node: &mut Node, ctx: &ReadyCtx) {
    let combat_block = matches!(&node.status, NodeStatus::Blocked { reason } if reason.starts_with("missing:combat:"));
    if !combat_block || !matches!(node.kind, crate::model::NodeKind::ObtainItems { .. }) {
        return;
    }
    let cheapest = node
        .alternatives
        .iter()
        .filter(|alt| matches!(alt.method.as_str(), "market:buy" | "shop:buy" | "dialogue:buy") && alt.est_coins > 0)
        .map(|alt| alt.est_coins)
        .min();
    if let Some(cost) = cheapest {
        node.status = NodeStatus::Blocked { reason: format!("missing:coins:{}", wallet_for(ctx, cost)) };
    }
}

/// Melee spots whose NPCs drop coins, the only coins that enter the
/// economy without a buyer (NPC shops never buy, PRD3): men 8.2 and
/// goblins 2.2 coins per kill in the F2P drop tables.
pub const COIN_SPOTS: [&str; 2] = ["men", "goblins"];
/// Obj id of coins.
const COINS_OBJ: usize = 995;

/// Money methods a bot can consider at its levels: the gather tier open
/// at each gathering level, shearing, the low-level melee spots
/// (cowhides, feathers) and the coin-dropping spots. The site pick
/// filters what is safe.
pub fn money_methods(ctx: &ReadyCtx) -> Vec<String> {
    let level = |skill: &str| level_of(ctx, skill).unwrap_or(1);
    let mut out = vec![
        crate::tranches::reach_method("fishing", level("fishing")),
        crate::tranches::reach_method("woodcutting", level("woodcutting")),
        crate::tranches::reach_method("mining", level("mining")),
        "gather:sheep/wool".to_string(),
        "fight:melee/cows".to_string(),
        "fight:melee/chickens".to_string(),
    ];
    out.extend(COIN_SPOTS.iter().map(|spot| format!("fight:melee/{spot}")));
    // Alchemy of the bot's own stock (2026-11-14): the brain prices it
    // per object (`brain_alch`, `alchemy::best_alch`) and publishes it in
    // `method_profit` only when the cast beats selling.
    out.extend(ctx.method_profit.iter().filter(|(method, profit)| method.starts_with("alch:") && *profit > 0.0).map(|(method, _)| method.clone()));
    // 100 % market lots (plan rendements R5): the guide's market rows the
    // brain published for this bot, ranked on their guide profit like
    // any money method; never for an Ironman.
    if !ctx.ironman {
        out.extend(ctx.trades.iter().filter(|offer| offer.profit_6000 > 0.0).map(|offer| offer.method.clone()));
    }
    out.dedup();
    out
}

/// Expected coins per kill of a melee method's spot (F2P drop tables,
/// best NPC of the spot); 0 for anything else.
pub fn coins_per_kill(method: &str) -> f64 {
    let Some(spot) = method.strip_prefix("fight:melee/") else { return 0.0 };
    let Some(entry) = lifescape_gamedata::combat_spots::by_key(spot) else { return 0.0 };
    lifescape_gamedata::drops::drops_for_obj(COINS_OBJ)
        .iter()
        .filter(|drop| entry.npc_names.contains(&drop.npc.as_str()))
        .map(|drop| drop.expected_per_kill())
        .fold(0.0, f64::max)
}

/// Profit per 6000 ticks of a method from the brain's published rates
/// (0 when unmeasured).
fn profit_of(ctx: &ReadyCtx, method: &str) -> f64 {
    if let Some(offer) = crate::trade_craft::offer_in(ctx, method) {
        return offer.profit_6000;
    }
    ctx.method_profit.iter().find(|(known, _)| known == method).map(|(_, profit)| *profit).unwrap_or(0.0)
}

/// A money method the bot can work: a trade lot by its own rule (tools
/// held, a lot fits: `trade_craft::workable`), any other by
/// `method_feasible`.
fn money_feasible(method: &str, ctx: &ReadyCtx) -> bool {
    if crate::trade_craft::is_trade(method) {
        return crate::trade_craft::workable(method, ctx);
    }
    method_feasible(method, ctx)
}

/// Coin and money nodes: the most profitable workable money method.
/// A market maker's post (`market:make` only, plan Q6) keeps its single
/// leg; that leg never ranks as a money method elsewhere.
pub(crate) fn resolve_money(node: &mut Node, ctx: &ReadyCtx) {
    let make = crate::market_maker::MAKE_METHOD;
    let post_only = !node.alternatives.is_empty() && node.alternatives.iter().all(|alt| alt.method == make);
    if post_only {
        for alt in &mut node.alternatives {
            alt.est_ticks = 6000;
        }
        node.method = Some(make.to_string());
        return;
    }
    // Inputs bought for this node's last trade lot are crafted first (R5).
    if crate::trade_craft::sticky(node, ctx) {
        return;
    }
    // The money guide's lines (R7): the draw of `guide_pick` among those
    // the bot works now first. Nothing drawable: the static methods.
    if crate::money_pick::resolve_from_guide(node, ctx) {
        return;
    }
    let mut methods = money_methods(ctx);
    for alt in &node.alternatives {
        // A trade lot no longer offered (guide moved, Iron) never ranks.
        if !methods.contains(&alt.method) && alt.method != make && !crate::trade_craft::is_trade(&alt.method) {
            methods.push(alt.method.clone());
        }
    }
    // A method a dependency cycle went through never comes back
    // (`lifecycle_cycle`: its prerequisite needs the coins it earns).
    methods.retain(|method| !node.excluded.contains(method));
    let ranked: Vec<Alternative> = methods
        .iter()
        .map(|method| {
            let profit = profit_of(ctx, method);
            // Higher profit = cheaper in tick-equivalents (6000 ticks per
            // window, scaled by profit; unmeasured methods tie at 6000).
            let est = if profit > 0.0 { (6000.0 * 1000.0 / (profit + 1000.0)) as u64 } else { 6000 };
            Alternative { method: method.clone(), est_ticks: est, est_coins: 0 }
        })
        .collect();
    // Order (user 2026-11-17, jim_ellis: a 10-HP bot sent to hunt men for
    // coins, food required, none owned, none affordable): a method the
    // bot works NOW first (`food_gate::ready_now`: tools held, food in
    // hand where the spot bites), then a feasible one whose prerequisite
    // is still to obtain, then the rest; within a class the published
    // profit (saturated rates). A coin target breaks ties by coin drops
    // (a coin drop is coins at once, goods wait for a buyer): unmeasured
    // methods still send a coin target to men first.
    let coin_first = matches!(node.kind, crate::model::NodeKind::HaveCoins { .. });
    let class = |method: &str| {
        // A trade lot starts now when one fits: its purchases are its work.
        if crate::trade_craft::is_trade(method) {
            if crate::trade_craft::workable(method, ctx) {
                0
            } else {
                2
            }
        } else if crate::food_gate::ready_now(method, ctx) {
            0
        } else if method_feasible(method, ctx) {
            1
        } else {
            2
        }
    };
    let mut keyed: Vec<(u8, f64, Alternative)> =
        ranked.into_iter().map(|alt| (class(&alt.method), if coin_first { coins_per_kill(&alt.method) } else { 0.0 }, alt)).collect();
    keyed.sort_by(|(left_class, left_coins, left), (right_class, right_coins, right)| {
        left_class.cmp(right_class).then_with(|| left.est_ticks.cmp(&right.est_ticks)).then_with(|| right_coins.total_cmp(left_coins))
    });
    let ranked: Vec<Alternative> = keyed.into_iter().map(|(_, _, alt)| alt).collect();
    node.method = ranked.iter().find(|alt| money_feasible(&alt.method, ctx)).map(|alt| alt.method.clone());
    // A trade lot sizes on the wallet and the asks, and blocks on the
    // inputs to buy (`trade_craft::lock`).
    crate::trade_craft::lock(node, ctx);
    if node.method.is_none() {
        if let Some(first) = ranked.first() {
            // Gear held: the method is unworkable for another reason
            // (quarantine, quest, combat fit), never a held tool "missing".
            if let Some(need) = required_tool(&first.method) {
                let reason = match need.missing(ctx) {
                    Some(tool) => format!("missing:{tool}"),
                    None => format!("unworkable:{}", first.method),
                };
                node.status = NodeStatus::Blocked { reason };
            }
        }
    }
    node.alternatives = ranked;
}

#[cfg(test)]
mod tests {
    use super::{coins_per_kill, resolve_money};
    use crate::expand::ReadyCtx;
    use crate::model::{Node, NodeKind};

    /// NPC shops never buy (PRD3): coins enter only as drops. Between
    /// methods of equal published profit (unmeasured), a coin target goes
    /// to the spot that drops the most coins (user 2026-11-17: the profit
    /// ranks first, coin drops only break ties).
    #[test]
    fn alching_own_stock_is_a_money_method_with_its_runes() {
        let mut ctx = ReadyCtx::blind(0.5);
        ctx.method_profit.push(("alch:steel_platebody".to_string(), 200_000.0));
        ctx.bank_known = true;
        ctx.tools = vec!["naturerune".to_string(), "staff_of_fire".to_string(), "bronze_pickaxe".to_string(), "bronze_axe".to_string()];
        let mut money = Node::new("n10", NodeKind::MakeMoney, Vec::new(), 0);
        resolve_money(&mut money, &ctx);
        assert_eq!(money.method.as_deref(), Some("alch:steel_platebody"), "{:?}", money.alternatives);
        ctx.tools.retain(|tool| tool != "naturerune");
        let mut broke = Node::new("n11", NodeKind::MakeMoney, Vec::new(), 0);
        resolve_money(&mut broke, &ctx);
        assert_ne!(broke.method.as_deref(), Some("alch:steel_platebody"), "no nature rune, no cast");
    }

    #[test]
    fn coin_targets_farm_coin_drops_first() {
        assert!((coins_per_kill("fight:melee/men") - 8.2).abs() < 0.1, "men drop ~8.2 coins: {}", coins_per_kill("fight:melee/men"));
        assert!(coins_per_kill("fight:melee/goblins") > 1.0);
        assert_eq!(coins_per_kill("fight:melee/cows"), 0.0);
        let ctx = ReadyCtx::blind(0.5);
        let mut coins = Node::new("n9", NodeKind::HaveCoins { amount: 200 }, Vec::new(), 200);
        resolve_money(&mut coins, &ctx);
        assert_eq!(coins.method.as_deref(), Some("fight:melee/men"));
        let order: Vec<&str> = coins.alternatives.iter().map(|alt| alt.method.as_str()).collect();
        assert_eq!(&order[..2], &["fight:melee/men", "fight:melee/goblins"], "{order:?}");
        // A plain money engagement keeps the profit ranking.
        let mut money = Node::new("n10", NodeKind::MakeMoney, Vec::new(), 0);
        resolve_money(&mut money, &ctx);
        assert_ne!(money.method.as_deref(), Some("fight:melee/men"), "unmeasured ties keep the historic order");
    }
}

#[cfg(test)]
#[path = "idle_unlock_tests.rs"]
mod idle_unlock_tests;
