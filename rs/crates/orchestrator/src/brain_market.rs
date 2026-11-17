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
        // Shared table (an unopenable store serves it empty: no profit
        // pushed, as the early return did).
        let rows = crate::rates_service::cached_profit_rows();
        if rows.is_empty() {
            return;
        }
        // Saturation (user rule 2026-11-14): a product under a wall of
        // asks is worth the share the market absorbs (public book only).
        // Every bid counts as absorption here (own ones too: a buyer is
        // a buyer); no account is left out.
        let reader = crate::brain_bids::MarketReader::live("");
        for method in lifescape_planner::resolve_obtain::money_methods(ctx) {
            let Some(script) = crate::brain_plan::rate_key(&method, node) else { continue };
            let best = rows.iter().filter(|row| row.method == script).map(|row| row.profit).fold(f64::MIN, f64::max);
            if best > f64::MIN {
                ctx.method_profit.push((method.clone(), saturated_profit(&method, best, &|item| reader.view(item), &|item| crate::brain_bids::units_per_6000(&snapshot, &method, item))));
            }
        }
    }
}

/// Object a money method sells (`gather:woodcut/willow_logs` →
/// `willow_logs`, `gather:sheep/wool` → `wool`); fights loot a mix and
/// name none.
pub fn money_product(method: &str) -> Option<&str> {
    let rest = method.strip_prefix("gather:")?;
    rest.rsplit('/').next().filter(|item| !item.is_empty())
}

/// A money method's published profit weighed by the saturation of its
/// product (`market_fit::saturation_factor`: bids + a day of volume over
/// resting asks + one window's batch). A loss stays a loss; a method
/// naming no product keeps its profit.
pub fn saturated_profit(
    method: &str,
    profit: f64,
    view: &dyn Fn(&str) -> Option<lifescape_planner::market_fit::BookView>,
    units: &dyn Fn(&str) -> f64,
) -> f64 {
    let Some(item) = money_product(method) else { return profit };
    if profit <= 0.0 {
        return profit;
    }
    profit * lifescape_planner::market_fit::saturation_factor(view(item).as_ref(), units(item))
}

/// D-B: a production leg launched for a tradeable object keeps a bid on
/// the market beside it (resolution marked it `market:bid`). Placed
/// daemon-side, never twice for the same object. Derived demand
/// (2026-11-14): when the object is an input of a product the bot's
/// graph makes, `derived` gives its reservation price
/// (`planner::derived_demand`): the bid's ceiling is the wallet's capped
/// by it, and it opens at half of it when the book is cheaper.
pub fn place_standing_bid(job: &Job, node: &Node, ctx: &ReadyCtx, derived: &dyn Fn() -> Option<f64>) {
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
    // A bid swept whole into deals leaves no resting row, its lots still
    // on their way: it stands until the handover.
    let deals = service.lock_store().open_deals_for_account(&job.account).unwrap_or_default();
    if buying_in_flight(&job.account, id, &deals) {
        return;
    }
    // What is still missing, capped by the wallet; at the best ask when
    // the wallet covers it (the bid crosses at once: market first), else
    // at the bot's own last fill on the item (the reference only when it
    // never bought it, 2026-11-14), walking up to the ceiling.
    let quantity = lifescape_planner::resolve_obtain::buy_quantity(ctx, *count);
    let wallet = lifescape_planner::resolve_obtain::bid_ceiling(ctx, quantity);
    let reserve = derived();
    let (open, ceiling) = match reserve {
        Some(reserve) => lifescape_planner::derived_demand::derived_bounds(reserve, wallet),
        None => (1, wallet),
    };
    // Credibility (plan marché §3.6, C5): beside a production the bid is
    // optional; one under the object's public floor is noise, never placed.
    let floor = crate::market_lineage::live_floor(&service, id, item);
    if !crate::market_lineage::credible(ceiling, floor) {
        crate::logln!("brain {}: no standing bid on {item}: ceiling {ceiling} under its public floor {floor}, production alone", job.account);
        return;
    }
    let price = lifescape_market::remembered_bid(ctx.price.max(1), ctx.best_ask, ceiling, service.own_buy_price(&job.account, id)).max(open).min(ceiling.max(1));
    // Never traded, no covered ask (M7): on the public anchors, under the
    // derived value, over the sellers' public floor.
    let covered = ctx.best_ask.is_some_and(|ask| ask >= 1 && ask <= ceiling);
    let price = if covered || service.reference(id).traded { price } else { crate::market_reprice::discovery_bid(&service, &job.account, id, reserve.map(|value| value.round() as i64), ceiling).unwrap_or(price) };
    let reply = service.handle(&object(vec![
        ("op", str_value("place")),
        ("account", str_value(&job.account)),
        ("side", str_value("buy")),
        ("item", Value::Int(id)),
        ("qty", Value::Int(quantity)),
        ("price", Value::Int(price)),
        ("ceiling", Value::Int(ceiling.max(price))),
        ("motive", str_value(crate::market_lineage::NEED)),
    ]));
    let derived_note = reserve.map(|value| format!(", derived reserve {value:.0}")).unwrap_or_default();
    crate::logln!(
        "brain {}: standing bid {quantity}/{count} {item} @ {price} (ceiling {ceiling}{derived_note}) beside production: {}",
        job.account,
        if reply.get("ok").as_bool() == Some(true) { "placed".to_string() } else { format!("refused {:?}", reply.get("error").as_str()) }
    );
}

/// Reservation price of what the obtain node at `index` gets, when it
/// feeds a product the bot's graph makes (derived demand, public values:
/// others' best bid or the current price; other inputs at the best ask or
/// current price; the bot's own rates and coin rate). `None` otherwise.
pub(crate) fn derived_reserve(life: &lifescape_planner::Life, index: usize, live: &Value) -> Option<f64> {
    let node = life.nodes.get(index)?;
    let consumed = life.nodes.iter().any(|other| other.deps.contains(&node.id) && matches!(other.kind, NodeKind::ObtainItems { .. }));
    if !consumed || life.profile.ironman {
        return None;
    }
    let _phase = crate::cpu_profile::phase("market.derived_reserve");
    let reader = crate::brain_bids::MarketReader::live(&life.account);
    let rows = crate::brain_bids::rate_rows();
    crate::brain_bids::with_bot_state(life, live, |state| {
        let rate = lifescape_planner::market_fit::coin_rate(&rows, state.level_of);
        let value_of = |item: &str| reader.view(item).and_then(|view| view.best_bid().max(view.current)).map(|price| price as f64);
        let input_price = |item: &str| reader.view(item).and_then(|view| view.best_ask().or(view.current)).unwrap_or(1) as f64;
        let prices = lifescape_planner::derived_demand::DerivedPrices { value_of: &value_of, input_price: &input_price, units_per_6000: state.units, coin_rate: rate };
        lifescape_planner::derived_demand::input_reserve(&life.nodes, index, &prices)
    })
}

/// True while `account` has a buyer deal on `item` in flight (live
/// 2026-11-13, bd23dqpq: "standing bid 98/98 airrune" swept five sellers
/// at once, 15 s later "97/98" went up for the same need on the same
/// purse; the mind runes before it, 71 then 70 then 40 of 98, ended in
/// six `short` handovers).
pub fn buying_in_flight(account: &str, item: i64, deals: &[lifescape_store::Deal]) -> bool {
    deals.iter().any(|deal| deal.item_id == item && deal.buyer == account)
}

/// A `market:buy` leg, placed daemon-side (no worker round-trip: the
/// order is pure bookkeeping, the handover is the convene loop's): the
/// covered best ask at once, else a bid at its own last fill price (the
/// reference when it never bought the item) walking up to the ceiling. Orders what is still missing, capped by the wallet (a
/// partial buy: the next resolve produces or buys the rest).
/// Persistent need (plan marché M2, C3): the account's resting bid on
/// the object (its `need` lineage first) is TAKEN OVER, its price and
/// walk kept, its ceiling following the wallet; never cancelled and
/// re-placed lower at the remembered price.
pub fn place_obtain_bid(job: &Job, node: &Node, ctx: &ReadyCtx) -> Result<i64, String> {
    let NodeKind::ObtainItems { item, count } = &node.kind else { return Err("not an obtain node".to_string()) };
    let service = crate::market_service::shared().ok_or("market service not running")?;
    let id = obj_id(item).ok_or_else(|| format!("unknown item {item}"))?;
    let quantity = lifescape_planner::resolve_obtain::buy_quantity(ctx, *count);
    let ceiling = lifescape_planner::resolve_obtain::bid_ceiling(ctx, quantity);
    let (order, price, kept) = match keep_need_bid(&service, &job.account, id, quantity, ceiling)? {
        crate::market_reprice::StandingBid::Placed { order, price } => (order, price, false),
        crate::market_reprice::StandingBid::Kept { order, price } => (order, price, true),
    };
    let how = if kept { "kept (its lineage goes on)" } else { "placed" };
    crate::logln!("brain {}: bid {quantity}/{count} {item} @ {price} {how} (ceiling {ceiling}, held {}), awaiting the fill", job.account, ctx.held);
    Ok(order)
}

/// The `need` bid of `account` on `item` for `quantity`, ceiling
/// `ceiling`: the resting one taken over, else a new lineage
/// (`market_reprice::keep_bid_from`, motive `need`).
pub(crate) fn keep_need_bid(service: &crate::market_service::MarketService, account: &str, item: i64, quantity: i64, ceiling: i64) -> Result<crate::market_reprice::StandingBid, String> {
    crate::market_reprice::keep_bid_from(service, account, item, quantity, 1, ceiling, crate::market_lineage::NEED)
}

/// Seconds between two orphan-need sweeps of one account.
pub const ORPHAN_SWEEP_SECS: f64 = 60.0;

fn last_sweep() -> &'static std::sync::Mutex<std::collections::HashMap<String, f64>> {
    static TABLE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, f64>>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Resting `need` bids whose object no open obtain node of the life
/// wants any more (`wanted`: debug names): the need disappeared (node
/// abandoned or withdrawn, goal changed), the bid goes (§3.5). Pure.
pub fn orphan_needs(rows: &[lifescape_store::Order], wanted: &[String], name_of: &dyn Fn(i64) -> Option<String>) -> Vec<i64> {
    rows.iter()
        .filter(|row| crate::brain_gear::cancellable(row) && row.motive == crate::market_lineage::NEED)
        .filter(|row| name_of(row.item_id).is_some_and(|name| !wanted.contains(&name)))
        .map(|row| row.order_id)
        .collect()
}

/// One sweep (rate-limited per account): cancels the orphan need bids
/// (reason `need_gone`). An expired fill wait no longer cancels its bid
/// (M2): this sweep is where a need's bid ends when its node is gone.
pub fn release_orphan_needs(job: &Job, life: &lifescape_planner::Life) -> usize {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs_f64()).unwrap_or(0.0);
    {
        let mut table = last_sweep().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if table.get(&job.account).is_some_and(|last| now - last < ORPHAN_SWEEP_SECS) {
            return 0;
        }
        table.insert(job.account.clone(), now);
    }
    let Some(service) = crate::market_service::shared() else { return 0 };
    let rows = service.lock_store().account_orders(&job.account).unwrap_or_default();
    let wanted = crate::brain_wishlist::wanted_names(life);
    let mut closed = 0;
    for order in orphan_needs(&rows, &wanted, &crate::brain_gear::name_of) {
        let reply = service.handle(&object(vec![("op", str_value("cancel")), ("account", str_value(&job.account)), ("order", Value::Int(order)), ("reason", str_value("need_gone"))]));
        if reply.get("ok").as_bool() == Some(true) {
            closed += 1;
        }
    }
    if closed > 0 {
        crate::logln!("brain {}: {closed} need bid(s) cancelled: no open node wants their object any more", job.account);
    }
    closed
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
/// landed obtain closes its standing bid (reason `need_met`). An expired
/// fill wait keeps its bid (plan marché M2).
pub fn cancel_open_buys(account: &str, item: &str) -> usize {
    let (Some(service), Some(id)) = (crate::market_service::shared(), obj_id(item)) else { return 0 };
    service.cancel_open_because(account, id, lifescape_store::OrderSide::Buy, "need_met")
}

#[cfg(test)]
mod tests {
    use super::live_levels;
    use lifescape_json::{object, Value};

    /// Plan marché §3.5: a need bid whose node is gone goes; one an open
    /// node still wants, a gear bid and a legacy (motive-less) bid stay.
    #[test]
    fn orphan_need_bids_are_the_ones_no_node_wants() {
        let row = |order_id: i64, item_id: i64, motive: &str| lifescape_store::Order {
            order_id,
            account: "fiona_park".to_string(),
            item_id,
            quantity: 5,
            remaining: 5,
            price: 3,
            motive: motive.to_string(),
            ..lifescape_store::Order::default()
        };
        let rows = vec![row(1, 1, "need"), row(2, 2, "need"), row(3, 3, "gear"), row(4, 4, "")];
        let name = |id: i64| Some(format!("item{id}"));
        let wanted = vec!["item2".to_string()];
        assert_eq!(super::orphan_needs(&rows, &wanted, &name), vec![1]);
    }

    /// bd23dqpq: a standing bid matched whole is still buying until the
    /// handover; another item or a sale is not.
    #[test]
    fn a_swept_bid_still_stands() {
        use super::buying_in_flight;
        let deal = |buyer: &str, item: i64| lifescape_store::Deal {
            deal_id: 1685,
            buyer: buyer.to_string(),
            seller: "george_97".to_string(),
            item_id: item,
            quantity: 24,
            price: 1,
            meeting_x: 0,
            meeting_y: 0,
            meeting_level: 0,
            status: lifescape_store::DealStatus::Matched,
            fail_reason: String::new(),
            deadline_tick: 0,
            created_clock: 0,
            updated_clock: 0,
            taker_side: None,
        };
        assert!(buying_in_flight("bd23dqpq", 556, &[deal("bd23dqpq", 556)]));
        assert!(!buying_in_flight("bd23dqpq", 558, &[deal("bd23dqpq", 556)]), "another rune");
        assert!(!buying_in_flight("george_97", 556, &[deal("bd23dqpq", 556)]), "the seller side");
        assert!(!buying_in_flight("bd23dqpq", 556, &[]));
    }

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
