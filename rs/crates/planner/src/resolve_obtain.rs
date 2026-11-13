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
//!    `have_coins` node that earns them). Never a silent substitute.
//!
//! Coin and money nodes pick the most profitable money method the bot
//! can work at its levels (published rates, market prices), not the
//! fastest one.

use crate::cost::{cost_farm, cost_market_buy};
use crate::expand::ReadyCtx;
use crate::model::{Alternative, Node, NodeStatus};
use crate::obtain_legs::{production_legs, tradeable_f2p};
use crate::tools::{method_feasible, required_tool};

/// Live level of `skill` (unknown = not gated: offline contexts).
fn level_of(ctx: &ReadyCtx, skill: &str) -> Option<u8> {
    ctx.levels.iter().find(|(name, _)| name == skill).map(|(_, level)| *level)
}

/// Per-unit ceiling of a bid: what the wallet covers, capped at three
/// times the market value (never less than 20 coins: an untraded object
/// must be biddable above its 1-coin default).
pub fn bid_ceiling(ctx: &ReadyCtx, count: i64) -> i64 {
    let affordable = ctx.coins / count.max(1);
    affordable.min(fair_unit(ctx)).max(1)
}

/// One unit's fair price: three times the market value, at least 20.
fn fair_unit(ctx: &ReadyCtx) -> i64 {
    (ctx.price.max(1) * 3).max(20)
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
    for method in &candidates {
        if method == "market:buy" || method == "shop:buy" || method == "market:bid" {
            continue;
        }
        let Some(leg) = legs.iter().find(|leg| &leg.method == method) else { continue };
        let est = cost_farm(count, ctx.yield_per_6000.max(1.0), ctx.trip_ticks);
        let alt = Alternative { method: method.clone(), est_ticks: est, est_coins: 0 };
        priced.push(alt.clone());
        if let Some((skill, needed)) = &leg.skill {
            if level_of(ctx, skill).is_some_and(|have| i64::from(have) < *needed) {
                if level_gap.as_ref().is_none_or(|(_, known)| needed < known) {
                    level_gap = Some((skill.clone(), *needed));
                }
                continue;
            }
        }
        if !method_feasible(method, ctx) {
            if tool_gap.is_none() {
                tool_gap = required_tool(method).map(|need| need.tool.to_string());
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
    let shop_listed = candidates.iter().any(|method| method == "shop:buy") && crate::expand::source_flags(item).1;
    if shop_listed {
        priced.push(Alternative {
            method: "shop:buy".to_string(),
            est_ticks: cost_market_buy(ctx.trip_ticks, ctx.shop_price.max(1), count, ctx.coins_per_tick.max(0.01)),
            est_coins: shop_cover,
        });
    }
    // 1. A covered ask: take it, whole or the part the wallet covers
    // (a fair ask only: at most the bid ceiling of one unit).
    let ask_covered = market_ok && ctx.best_ask.is_some_and(|ask| ask * need <= ctx.coins);
    let ask_partial = market_ok && ctx.best_ask.is_some_and(|ask| ask <= ctx.coins && ask <= fair_unit(ctx));
    let producible = workable.iter().min_by_key(|alt| alt.est_ticks).cloned();
    let pick: Option<String> = if (ask_covered || ask_partial) && producible.is_some() {
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
        if market_ok && ctx.coins >= need {
            priced.push(Alternative { method: "market:bid".to_string(), est_ticks: 0, est_coins: bid_ceiling(ctx, need) * need });
        }
        Some(best.method.clone())
    } else if shop_listed && shop_cover <= ctx.coins {
        // 3. Merchant, D-A allowed.
        Some("shop:buy".to_string())
    } else if market_ok && ctx.coins >= unit {
        // 4. Resting bid for what the wallet covers, the brain waits for
        // the fill.
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
        let reason = if let Some((skill, level)) = level_gap {
            Some(format!("missing:level:{skill}:{level}"))
        } else if let Some(tool) = tool_gap {
            Some(format!("missing:{tool}"))
        } else if market_ok || shop_listed {
            let price = if shop_listed && !market_ok { ctx.shop_price.max(1) } else { unit };
            Some(format!("missing:coins:{}", price * need))
        } else {
            None
        };
        if let Some(reason) = reason {
            node.status = NodeStatus::Blocked { reason };
        }
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
    ctx.method_profit.iter().find(|(known, _)| known == method).map(|(_, profit)| *profit).unwrap_or(0.0)
}

/// Coin and money nodes: the most profitable workable money method.
/// Merchant nodes (`market:flip` only) keep their single leg.
pub(crate) fn resolve_money(node: &mut Node, ctx: &ReadyCtx) {
    let flip_only = !node.alternatives.is_empty() && node.alternatives.iter().all(|alt| alt.method == "market:flip");
    if flip_only {
        for alt in &mut node.alternatives {
            alt.est_ticks = 6000;
        }
        node.method = Some("market:flip".to_string());
        return;
    }
    let mut methods = money_methods(ctx);
    for alt in &node.alternatives {
        if !methods.contains(&alt.method) && alt.method != "market:flip" {
            methods.push(alt.method.clone());
        }
    }
    let mut ranked: Vec<Alternative> = methods
        .iter()
        .map(|method| {
            let profit = profit_of(ctx, method);
            // Higher profit = cheaper in tick-equivalents (6000 ticks per
            // window, scaled by profit; unmeasured methods tie at 6000).
            let est = if profit > 0.0 { (6000.0 * 1000.0 / (profit + 1000.0)) as u64 } else { 6000 };
            Alternative { method: method.clone(), est_ticks: est, est_coins: 0 }
        })
        .collect();
    if !ctx.ironman && node.alternatives.iter().any(|alt| alt.method == "market:flip") {
        ranked.push(Alternative { method: "market:flip".to_string(), est_ticks: 6000, est_coins: 0 });
    }
    // A coin target is met by coins in the wallet: goods only turn into
    // coins when a buyer shows up, a coin drop is coins at once. Coin
    // spots lead (most coins per kill first); the site pick falls back
    // down the list when the bot's combat level cannot work them yet.
    let coin_first = matches!(node.kind, crate::model::NodeKind::HaveCoins { .. });
    let mut keyed: Vec<(f64, Alternative)> =
        ranked.into_iter().map(|alt| (if coin_first { coins_per_kill(&alt.method) } else { 0.0 }, alt)).collect();
    keyed.sort_by(|(left_coins, left), (right_coins, right)| {
        right_coins.total_cmp(left_coins).then_with(|| left.est_ticks.cmp(&right.est_ticks))
    });
    let ranked: Vec<Alternative> = keyed.into_iter().map(|(_, alt)| alt).collect();
    node.method = ranked.iter().find(|alt| method_feasible(&alt.method, ctx)).map(|alt| alt.method.clone());
    if node.method.is_none() {
        if let Some(need) = ranked.first().and_then(|first| required_tool(&first.method)) {
            node.status = NodeStatus::Blocked { reason: format!("missing:{}", need.tool) };
        }
    }
    node.alternatives = ranked;
}

#[cfg(test)]
mod tests {
    use super::{coins_per_kill, resolve_money};
    use crate::expand::ReadyCtx;
    use crate::model::{Node, NodeKind};

    /// NPC shops never buy (PRD3): coins enter only as drops. A coin
    /// target goes to the spot that drops the most coins; the site pick
    /// falls back down the list when combat is too low.
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
