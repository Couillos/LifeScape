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
/// the node's object, base levels, and the bot's method profits read in
/// the precomputed money guide (`guide_coins`); none before the first
/// set.
pub fn fill_ctx(ctx: &mut ReadyCtx, node: &Node, live: &Value) {
    ctx.levels = live_levels(live);
    if let NodeKind::ObtainItems { item, .. } = &node.kind {
        if let (Some(service), Some(id)) = (crate::market_service::shared(), obj_id(item)) {
            ctx.best_ask = service.lock_engine().books.get(&id).and_then(|book| book.best().1).map(|price| crate::market_units::to_unit(id, price));
            ctx.price = service.reference(id).price.max(1);
        }
    }
    if let Some(set) = crate::guides_store::current() {
        guide_coins(ctx, node, &set);
    }
}

/// The bot's method profits read in the money guide `set` (one source,
/// `brain_d17`), from `ctx`'s levels: a money node's methods carry their
/// guide profit (`method_profit`: the best priced row of the method's
/// activity in the bot's own band rows). The guide already prices what
/// the market absorbs (D12): no saturation on top. The grind of a
/// purchase (`coins_per_tick`) is the bot's measured time value
/// (`brain_plan::ready_ctx`, D1), never its D17: left as found.
pub fn guide_coins(ctx: &mut ReadyCtx, node: &Node, set: &crate::guides_store::GuideSet) {
    let levels = ctx.levels.clone();
    let level_of = |skill: &str| levels.iter().find(|(name, _)| name == skill).map(|(_, level)| *level).unwrap_or(1);
    let read = crate::brain_money::bot_rows(set, &level_of, &[]);
    if matches!(node.kind, NodeKind::MakeMoney | NodeKind::HaveCoins { .. }) {
        for method in lifescape_planner::resolve_obtain::money_methods(ctx) {
            if let Some(profit) = crate::brain_d17::method_profit_in(&read.rows, &method) {
                ctx.method_profit.push((method, profit));
            }
        }
    }
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
    let resting = service.resting_rows(&job.account, id, lifescape_store::OrderSide::Buy);
    // Plan équilibre step 4: its own `need` bid follows the derived
    // reserve of NOW (real time value, shared demand) and the need of now,
    // never those of its placement (live 2026-11-19: coal bids at ceiling
    // 1 from a reserve of -67, kept for hours). Any other bid is left as
    // is. A need bid filled meanwhile falls through to the checks below:
    // never a new bid beside one in delivery.
    if resting.iter().any(|row| row.motive == crate::market_lineage::NEED) {
        if refresh_standing_bid(job, &service, id, item, *count, ctx, derived) {
            return;
        }
        if !service.resting_rows(&job.account, id, lifescape_store::OrderSide::Buy).is_empty() {
            return;
        }
    } else if !resting.is_empty() {
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

/// The account's resting `need` bid on `item` (object id `item_id`)
/// brought to the bounds a new standing bid would get now
/// (`refresh_need_bid`). Logs only what moved (price, ceiling,
/// remainder) and a refusal only when it is new (`refusal_is_new`).
/// False when no need bid rests any more (filled meanwhile): the caller
/// goes through its in-flight check before any new bid.
fn refresh_standing_bid(job: &Job, service: &crate::market_service::MarketService, item_id: i64, item: &str, count: i64, ctx: &ReadyCtx, derived: &dyn Fn() -> Option<f64>) -> bool {
    let wallet = lifescape_planner::resolve_obtain::bid_ceiling(ctx, lifescape_planner::resolve_obtain::buy_quantity(ctx, count));
    // The remainder is cut to the NEED of now (re-review lot 4, point 3),
    // never to what the wallet buys at the best ask: a wallet-shrunk bid
    // would never grow back; an uncovered raise is refused by the market.
    let quantity = (count.max(1) - ctx.held).max(1);
    let reserve = derived();
    let (open, ceiling) = match reserve {
        Some(reserve) => lifescape_planner::derived_demand::derived_bounds(reserve, wallet),
        None => (1, wallet),
    };
    let derived_note = reserve.map(|value| format!(", derived reserve {value:.0}")).unwrap_or_default();
    match refresh_need_bid(service, &job.account, item_id, quantity, open, ceiling) {
        Ok(Some(refreshed)) => {
            forget_refusal(&job.account, item_id);
            if refreshed.changed() {
                let old_ceiling = refreshed.old_ceiling.map(|value| value.to_string()).unwrap_or_else(|| "none".to_string());
                crate::logln!(
                    "brain {}: standing bid {item} kept @ {} (was {}), ceiling {} (was {old_ceiling}), remainder {} (was {}) for a need of {quantity}{derived_note}",
                    job.account,
                    refreshed.price,
                    refreshed.old_price,
                    refreshed.ceiling,
                    refreshed.remaining,
                    refreshed.old_remaining
                );
            }
            true
        }
        Ok(None) => false,
        Err(error) => {
            if refusal_is_new(&job.account, item_id, &error) {
                crate::logln!("brain {}: standing bid {item} not refreshed (ceiling {ceiling}{derived_note}): {error}", job.account);
            }
            true
        }
    }
}

/// The `need` bid of `account` on `item_id` brought to `open`/`ceiling`
/// and its remainder cut to `quantity` (`market_reprice::
/// refresh_resting_bid`); `Ok(None)` when none rests.
pub(crate) fn refresh_need_bid(service: &crate::market_service::MarketService, account: &str, item_id: i64, quantity: i64, open: i64, ceiling: i64) -> Result<Option<crate::market_reprice::RestingRefresh>, String> {
    crate::market_reprice::refresh_resting_bid(service, account, item_id, quantity, open, ceiling, crate::market_lineage::NEED)
}

type RefusalTable = std::sync::Mutex<std::collections::HashMap<(String, i64), String>>;

fn refusals() -> &'static RefusalTable {
    static TABLE: std::sync::OnceLock<RefusalTable> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// True when `error` is not the last refusal logged for this account's
/// standing bid on `item_id` (it is remembered).
pub(crate) fn refusal_is_new(account: &str, item_id: i64, error: &str) -> bool {
    let mut table = refusals().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    table.insert((account.to_string(), item_id), error.to_string()).as_deref() != Some(error)
}

fn forget_refusal(account: &str, item_id: i64) {
    refusals().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&(account.to_string(), item_id));
}

/// Reservation price of what the obtain node at `index` gets, when it
/// feeds a product the bot's graph makes (derived demand, plan équilibre
/// step 4: the product at what it sells for on the shared demand of the
/// graph's lot, `derived_demand::product_value` — others' bids and a
/// day's sales at the current price, less the supply on its way and the
/// units other producers claimed —; other inputs at the best ask or
/// current price; the bot's own rates and its measured coin rate, D1).
/// `None` otherwise.
pub(crate) fn derived_reserve(life: &lifescape_planner::Life, index: usize, live: &Value) -> Option<f64> {
    let node = life.nodes.get(index)?;
    let consumed = life.nodes.iter().any(|other| other.deps.contains(&node.id) && matches!(other.kind, NodeKind::ObtainItems { .. }));
    if !consumed || life.profile.ironman {
        return None;
    }
    let _phase = crate::cpu_profile::phase("market.derived_reserve");
    let reader = crate::brain_bids::MarketReader::live(&life.account);
    crate::brain_bids::with_bot_state(life, live, |state| {
        let rate = crate::brain_bids::bot_rate(life);
        let value_of = |item: &str| lifescape_planner::derived_demand::product_value(reader.view(item).as_ref(), lifescape_planner::derived_demand::product_lot(&life.nodes, item));
        let input_price = |item: &str| reader.view(item).and_then(|view| view.best_ask().or(view.current)).unwrap_or(1) as f64;
        let prices = lifescape_planner::derived_demand::DerivedPrices { value_of: &value_of, input_price: &input_price, units_per_6000: state.units, coin_rate: rate };
        lifescape_planner::derived_demand::sale_reserve(&life.nodes, index, &life.goal, &prices)
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
///
/// `worth`: what one unit is worth to this buyer (its own make cost at
/// its coin rate, `brain_bids::production_costs_for`): the bid opens
/// there, capped by the wallet (user 2026-11-18 levier 2). `reserve`:
/// the input's reservation price when it feeds a product
/// (`derived_reserve`): the ceiling never passes it.
pub fn place_obtain_bid(job: &Job, node: &Node, ctx: &ReadyCtx, worth: Option<i64>, reserve: Option<f64>) -> Result<i64, String> {
    let NodeKind::ObtainItems { item, count } = &node.kind else { return Err("not an obtain node".to_string()) };
    let service = crate::market_service::shared().ok_or("market service not running")?;
    let id = obj_id(item).ok_or_else(|| format!("unknown item {item}"))?;
    let quantity = lifescape_planner::resolve_obtain::buy_quantity(ctx, *count);
    let ceiling = lifescape_planner::derived_demand::obtain_ceiling(lifescape_planner::resolve_obtain::bid_ceiling(ctx, quantity), reserve);
    // The wallet the market lets a NEW bid spend is what its ledger holds
    // net of the account's other resting bids, not the pack's coins
    // (2026-11-21: "buyer holds 83092 coins, needs 83132", a mage's
    // second input refused three times, its training abandoned).
    let resting_bid = service
        .lock_store()
        .account_orders(&job.account)
        .unwrap_or_default()
        .iter()
        .any(|row| row.item_id == id && row.side == lifescape_store::OrderSide::Buy && row.remaining > 0 && matches!(row.status, lifescape_store::OrderStatus::Open | lifescape_store::OrderStatus::Matched));
    let (quantity, ceiling) = if resting_bid { (quantity, ceiling) } else { within_free_coins(quantity, ceiling, service.free_coverage(&job.account, lifescape_store::OrderSide::Buy, id).1) };
    let (order, price, kept) = match keep_need_bid(&service, &job.account, id, quantity, worth.unwrap_or(1), ceiling)? {
        crate::market_reprice::StandingBid::Placed { order, price } => (order, price, false),
        crate::market_reprice::StandingBid::Kept { order, price } => (order, price, true),
    };
    let how = if kept { "kept (its lineage goes on)" } else { "placed" };
    let worth_note = worth.map(|value| format!(", worth {value}")).unwrap_or_default();
    let reserve_note = reserve.map(|value| format!(", derived reserve {value:.0}")).unwrap_or_default();
    crate::logln!("brain {}: bid {quantity}/{count} {item} @ {price} {how} (ceiling {ceiling}{worth_note}{reserve_note}, held {}), awaiting the fill", job.account, ctx.held);
    Ok(order)
}

/// A new bid's `quantity` and per-unit `ceiling` brought under what the
/// market lets the account spend (`free` coins): the units shrink to what
/// one coin each buys, the ceiling to what `free` covers over them (never
/// below 1 coin: with nothing free the market still refuses, typed).
pub(crate) fn within_free_coins(quantity: i64, ceiling: i64, free: i64) -> (i64, i64) {
    let quantity = quantity.min(free).max(1);
    (quantity, ceiling.min(free / quantity).max(1))
}

/// The `need` bid of `account` on `item` for `quantity`, ceiling
/// `ceiling`: the resting one taken over, else a new lineage
/// (`market_reprice::keep_bid_from`, motive `need`).
pub(crate) fn keep_need_bid(service: &crate::market_service::MarketService, account: &str, item: i64, quantity: i64, worth: i64, ceiling: i64) -> Result<crate::market_reprice::StandingBid, String> {
    crate::market_reprice::keep_bid_from(service, account, item, quantity, worth, ceiling, crate::market_lineage::NEED)
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
    // The need is met: a bid answer's reservation of these asks ends.
    crate::market_claims::purchase_done(account, item);
    let (Some(service), Some(id)) = (crate::market_service::shared(), obj_id(item)) else { return 0 };
    service.cancel_open_because(account, id, lifescape_store::OrderSide::Buy, "need_met")
}

#[cfg(test)]
mod tests {
    use super::{live_levels, within_free_coins};
    use lifescape_json::{object, Value};

    /// A bid opens under the coins the market lets the account spend:
    /// 752 units at a ceiling of 8 against 5 000 free coins shrink to a
    /// ceiling of 6; more units than coins shrink to the coins.
    #[test]
    fn a_new_bid_stays_under_the_free_coins() {
        assert_eq!(within_free_coins(752, 8, 5000), (752, 6));
        assert_eq!(within_free_coins(752, 5, 5000), (752, 5), "a ceiling already under the wallet stays");
        assert_eq!(within_free_coins(752, 8, 300), (300, 1));
        assert_eq!(within_free_coins(10, 8, 0), (1, 1), "nothing free: the market refuses it, typed");
    }

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

#[cfg(test)]
#[path = "brain_market_refresh_tests.rs"]
mod refresh_tests;
