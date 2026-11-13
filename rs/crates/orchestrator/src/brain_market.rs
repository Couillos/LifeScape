#![forbid(unsafe_code)]
//! Brain-side market wiring (PRD3 lot 4), split from `brain.rs`:
//! the market and level inputs of node resolution, the buy ceiling, the
//! standing bid beside a production (D-B), and its cancellation once the
//! node lands.

use lifescape_json::{object, str_value, Value};
use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::{Node, NodeKind};

use crate::jobs::Job;

/// Base levels by skill name from the live XP rows (the level byte of
/// a heartbeat row may be boosted or drained; XP is the base truth).
/// Heartbeat XP is display XP (UPDATE_STAT, the SDK's unit): read it with
/// the SDK table, never `gamedata::xp` (server units, ×10).
pub fn live_levels(live: &Value) -> Vec<(String, u8)> {
    let mut out = Vec::new();
    for row in live.get("skills").as_arr().cloned().unwrap_or_default() {
        let (Some(id), Some(xp)) = (row.get("id").as_i64(), row.get("xp").as_i64()) else { continue };
        let Ok(stat) = u8::try_from(id) else { continue };
        let Ok(display_xp) = i32::try_from(xp) else { continue };
        let level = lifescape_sdk::snapshot::level_for_xp(display_xp).clamp(1, 99);
        out.push((lifescape_sdk::provenance::skill_name(stat).to_string(), level));
    }
    out
}

fn obj_id(item: &str) -> Option<i64> {
    lifescape_gamedata::objs_by_name(item).into_iter().next().map(|obj| obj.obj_id as i64)
}

/// Fills the market and level inputs of `ctx` for `node`: best ask of
/// the node's object, base levels, and the published profit of every
/// money method the bot could work (keyed by the planner method).
pub fn fill_ctx(ctx: &mut ReadyCtx, node: &Node, live: &Value) {
    ctx.levels = live_levels(live);
    if let NodeKind::ObtainItems { item, .. } = &node.kind {
        if let (Some(service), Some(id)) = (crate::market_service::shared(), obj_id(item)) {
            ctx.best_ask = service.lock_engine().books.get(&id).and_then(|book| book.best().1);
            ctx.price = service.reference(id).price.max(1);
        }
    }
    if matches!(node.kind, NodeKind::MakeMoney | NodeKind::HaveCoins { .. }) {
        let snapshot = crate::rates_service::live_snapshot();
        let Ok(store) = lifescape_store::Store::open(&crate::market_service::db_path()) else { return };
        let rows = crate::rates_service::profit_rows(&store, &snapshot);
        for method in lifescape_planner::resolve_obtain::money_methods(ctx) {
            let Some(script) = crate::brain_plan::rate_key(&method, node) else { continue };
            let best = rows.iter().filter(|row| row.method == script).map(|row| row.profit).fold(f64::MIN, f64::max);
            if best > f64::MIN {
                ctx.method_profit.push((method, best));
            }
        }
    }
}

/// D-B: a production leg launched for a tradeable object keeps a bid on
/// the market beside it (resolution marked it `market:bid`). Placed
/// daemon-side, never twice for the same object.
pub fn place_standing_bid(job: &Job, node: &Node, ctx: &ReadyCtx) {
    let NodeKind::ObtainItems { item, count } = &node.kind else { return };
    if !node.alternatives.iter().any(|alt| alt.method == "market:bid") {
        return;
    }
    let (Some(service), Some(id)) = (crate::market_service::shared(), obj_id(item)) else { return };
    // A partly filled bid (`matched`, remainder resting) still stands:
    // only open rows used to count, so bids piled up beside it.
    if !service.resting_rows(&job.account, id, lifescape_store::OrderSide::Buy).is_empty() {
        return;
    }
    // What is still missing, capped by the wallet; at the best ask when
    // it is fair (the bid crosses at once: market first), else at the
    // reference walking up to the ceiling.
    let quantity = lifescape_planner::resolve_obtain::buy_quantity(ctx, *count);
    let ceiling = lifescape_planner::resolve_obtain::bid_ceiling(ctx, quantity);
    let price = match ctx.best_ask {
        Some(ask) if ask <= ceiling => ask,
        _ => ctx.price.max(1).min(ceiling),
    };
    let reply = service.handle(&object(vec![
        ("op", str_value("place")),
        ("account", str_value(&job.account)),
        ("side", str_value("buy")),
        ("item", Value::Int(id)),
        ("qty", Value::Int(quantity)),
        ("price", Value::Int(price)),
        ("ceiling", Value::Int(ceiling.max(price))),
    ]));
    crate::logln!(
        "brain {}: standing bid {quantity}/{count} {item} @ {price} (ceiling {ceiling}) beside production: {}",
        job.account,
        if reply.get("ok").as_bool() == Some(true) { "placed".to_string() } else { format!("refused {:?}", reply.get("error").as_str()) }
    );
}

/// A `market:buy` leg, placed daemon-side (no worker round-trip: the
/// order is pure bookkeeping, the handover is the convene loop's): the
/// covered best ask at once, else a bid at the reference walking up to
/// the ceiling. Orders what is still missing, capped by the wallet (a
/// partial buy: the next resolve produces or buys the rest). Replaces
/// any open bid of the account on that object.
pub fn place_obtain_bid(job: &Job, node: &Node, ctx: &ReadyCtx) -> Result<i64, String> {
    let NodeKind::ObtainItems { item, count } = &node.kind else { return Err("not an obtain node".to_string()) };
    let service = crate::market_service::shared().ok_or("market service not running")?;
    let id = obj_id(item).ok_or_else(|| format!("unknown item {item}"))?;
    service.cancel_open_for(&job.account, id, lifescape_store::OrderSide::Buy);
    let quantity = lifescape_planner::resolve_obtain::buy_quantity(ctx, *count);
    let ceiling = lifescape_planner::resolve_obtain::bid_ceiling(ctx, quantity);
    let (_, ask) = service.lock_engine().books.get(&id).map(|book| book.best()).unwrap_or((None, None));
    let price = match ask {
        Some(best) if best <= ceiling => best,
        _ => ctx.price.max(1).min(ceiling),
    };
    let reply = service.handle(&object(vec![
        ("op", str_value("place")),
        ("account", str_value(&job.account)),
        ("side", str_value("buy")),
        ("item", Value::Int(id)),
        ("qty", Value::Int(quantity)),
        ("price", Value::Int(price)),
        ("ceiling", Value::Int(ceiling.max(price))),
    ]));
    if reply.get("ok").as_bool() != Some(true) {
        return Err(reply.get("error").as_str().unwrap_or("place refused").to_string());
    }
    crate::logln!("brain {}: bid {quantity}/{count} {item} @ {price} (ceiling {ceiling}, held {}), awaiting the fill", job.account, ctx.held);
    Ok(reply.get("order").as_i64().unwrap_or(-1))
}

/// True once the account's buy on `item` (debug name) has run its course:
/// no open or matched buy order on it, and no handover in flight. With
/// holdings above the launch baseline, a partial buy landed.
pub fn buy_closed(account: &str, item: &str) -> bool {
    let (Some(service), Some(id)) = (crate::market_service::shared(), obj_id(item)) else { return false };
    let store = service.lock_store();
    let order_live = store.account_orders(account).unwrap_or_default().iter().any(|row| {
        row.item_id == id
            && row.side == lifescape_store::OrderSide::Buy
            && matches!(row.status, lifescape_store::OrderStatus::Open | lifescape_store::OrderStatus::Matched)
    });
    let deal_live = store.open_deals_for_account(account).unwrap_or_default().iter().any(|deal| deal.item_id == id && deal.buyer == account);
    !order_live && !deal_live
}

/// Cancels the account's open buy orders on `item` (debug name): a
/// landed obtain closes its standing bid, an expired wait its stale one.
pub fn cancel_open_buys(account: &str, item: &str) -> usize {
    let (Some(service), Some(id)) = (crate::market_service::shared(), obj_id(item)) else { return 0 };
    service.cancel_open_for(account, id, lifescape_store::OrderSide::Buy)
}

#[cfg(test)]
mod tests {
    use super::live_levels;
    use lifescape_json::{object, Value};

    #[test]
    fn levels_come_from_xp_not_the_boosted_byte() {
        let row = |id: i64, level: i64, xp: i64| object(vec![("id", Value::Int(id)), ("level", Value::Int(level)), ("xp", Value::Int(xp))]);
        // Heartbeat XP is display XP (the SDK's unit), not the ×10 server
        // table: ryan_park live 2026-11-13, attack 35 = 22428, fishing 11
        // = 1370 (read as fishing 2 → "missing:level:fishing:5" bans).
        // Smithing (13) shows a drained 1 but holds level-15 XP (2411).
        let live = object(vec![("skills", Value::Arr(vec![row(0, 35, 22428), row(10, 11, 1370), row(13, 1, 2411), row(12, 1, 0)]))]);
        let levels = live_levels(&live);
        assert!(levels.contains(&("attack".to_string(), 35)), "{levels:?}");
        assert!(levels.contains(&("fishing".to_string(), 11)), "{levels:?}");
        assert!(levels.contains(&("smithing".to_string(), 15)), "{levels:?}");
        assert!(levels.contains(&("crafting".to_string(), 1)), "{levels:?}");
    }
}
