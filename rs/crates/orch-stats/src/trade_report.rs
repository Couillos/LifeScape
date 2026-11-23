#![forbid(unsafe_code)]
//! 100 % market lots for the operator (plan rendements R5, §5 "profit
//! prévu vs réalisé suivi page economy"): `orchestrator economy` and the
//! dashboard economy page, never read by a bot. One row per lot recorded
//! in a life (`memory.trades`, `lifescape_planner::trade_craft`): the
//! guide's predicted profit, the purchases (the account's buys of the
//! lot's inputs since the lock, capped at the units planned), the craft
//! (products made, its yield per 6000 work ticks against the guide's),
//! the sales (the account's sells of the product, capped at the products
//! made) and the realized coins (sales minus purchases). A lot's deals
//! end where the account's next lot starts (lots are sequential). Read
//! from the store and the life files only: offline, the daemon may be
//! down. Staff accounts are excluded (rule 11).

use lifescape_json::{object, str_value, Value};
use lifescape_planner::trade_craft::TradeLot;
use lifescape_store::{Deal, Store};

/// One lot as the operator sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct TradeRow {
    pub account: String,
    pub node: String,
    pub method: String,
    pub product: String,
    pub lot: i64,
    pub bound: String,
    pub opened: i64,
    /// Guide profit of the lot.
    pub predicted: i64,
    /// Coins the purchases cost at the book when planned.
    pub planned_cost: i64,
    pub planned_units: i64,
    pub bought_units: i64,
    pub bought_coins: i64,
    pub made: Option<i64>,
    /// Products per 6000 work ticks of the craft run, and the guide's.
    pub craft_yield: Option<f64>,
    pub guide_per_6000: f64,
    pub sold_units: i64,
    pub sold_coins: i64,
    /// Sales minus purchases so far.
    pub realized: i64,
    /// `buying`, `crafting`, `selling` or `closed`.
    pub status: &'static str,
}

fn obj_id(name: &str) -> Option<i64> {
    lifescape_gamedata::objs_by_name(name).into_iter().next().map(|obj| obj.obj_id as i64)
}

/// Units and coins of `deals` that `take` keeps, first deals first, at
/// most `cap` units (a deal past the cap counts its share).
fn capped(deals: &[&Deal], cap: i64) -> (i64, i64) {
    let (mut units, mut coins) = (0i64, 0i64);
    for deal in deals {
        let room = (cap - units).max(0);
        if room == 0 {
            break;
        }
        let taken = deal.quantity.min(room);
        units += taken;
        coins += crate::market_units::deal_coins(deal.item_id, taken, deal.price);
    }
    (units, coins)
}

/// One row from its inputs (pure): the lot, the account's non-failed
/// deals, and the clock its next lot opened (`None`: the last one).
pub fn lot_row(account: &str, lot: &TradeLot, deals: &[Deal], until: Option<i64>) -> TradeRow {
    let inside: Vec<&Deal> = deals.iter().filter(|deal| deal.created_clock >= lot.opened && until.is_none_or(|end| deal.created_clock < end)).collect();
    let (mut bought_units, mut bought_coins) = (0i64, 0i64);
    for (item, units) in &lot.buys {
        let Some(id) = obj_id(item) else { continue };
        let buys: Vec<&Deal> = inside.iter().copied().filter(|deal| deal.buyer == account && crate::market_audit::book_item(deal.item_id) == id).collect();
        let (units, coins) = capped(&buys, *units);
        bought_units += units;
        bought_coins += coins;
    }
    let product = obj_id(&lot.product);
    let sells: Vec<&Deal> = inside.iter().copied().filter(|deal| deal.seller == account && product.is_some_and(|id| crate::market_audit::book_item(deal.item_id) == id)).collect();
    let (sold_units, sold_coins) = capped(&sells, lot.made.unwrap_or(lot.lot));
    let planned_units: i64 = lot.buys.iter().map(|(_, units)| *units).sum();
    let status = match lot.made {
        None if bought_units < planned_units => "buying",
        None => "crafting",
        Some(made) if sold_units < made => "selling",
        Some(_) => "closed",
    };
    TradeRow {
        account: account.to_string(),
        node: lot.node.clone(),
        method: lot.method.clone(),
        product: lot.product.clone(),
        lot: lot.lot,
        bound: lot.bound.clone(),
        opened: lot.opened,
        predicted: lot.predicted,
        planned_cost: lot.cost,
        planned_units,
        bought_units,
        bought_coins,
        made: lot.made,
        craft_yield: lot.craft_yield(),
        guide_per_6000: lot.guide_per_6000,
        sold_units,
        sold_coins,
        realized: sold_coins - bought_coins,
        status,
    }
}

/// Lots recorded in the life files: (account, lots oldest first), by
/// account, staff and excluded accounts out.
pub fn trade_lives(lives_dir: &std::path::Path) -> Vec<(String, Vec<TradeLot>)> {
    let excluded = crate::economy_guard::excluded_accounts();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(lives_dir) else { return out };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(account) = path.file_stem().and_then(|stem| stem.to_str()).map(str::to_string) else { continue };
        if excluded.contains(&account) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(value) = lifescape_json::parse(&text) else { continue };
        let lots = lifescape_planner::trade_craft::lots_from_value(&value.get("memory").get("trades"));
        if !lots.is_empty() {
            out.push((account, lots));
        }
    }
    out.sort_by(|left, right| left.0.cmp(&right.0));
    out
}

/// Every lot's row, newest first.
pub fn trade_rows(store: &Store, lives_dir: &std::path::Path) -> Vec<TradeRow> {
    let mut rows = Vec::new();
    for (account, lots) in trade_lives(lives_dir) {
        let since = lots.iter().map(|lot| lot.opened).min().unwrap_or(0);
        let deals = store.own_deals_since(&account, since).unwrap_or_default();
        for (index, lot) in lots.iter().enumerate() {
            let until = lots.get(index + 1).map(|next| next.opened);
            rows.push(lot_row(&account, lot, &deals, until));
        }
    }
    rows.sort_by(|left, right| right.opened.cmp(&left.opened).then_with(|| left.account.cmp(&right.account)));
    rows
}

/// Predicted and realized coins of the closed lots: (lots, predicted,
/// realized).
pub fn closed_totals(rows: &[TradeRow]) -> (usize, i64, i64) {
    rows.iter().filter(|row| row.status == "closed").fold((0, 0, 0), |(lots, predicted, realized), row| (lots + 1, predicted + row.predicted, realized + row.realized))
}

/// Dashboard shape of one row.
pub fn row_value(row: &TradeRow) -> Value {
    object(vec![
        ("account", str_value(&row.account)),
        ("node", str_value(&row.node)),
        ("method", str_value(&row.method)),
        ("product", str_value(&row.product)),
        ("lot", Value::Int(row.lot)),
        ("bound", str_value(&row.bound)),
        ("opened", Value::Int(row.opened)),
        ("predicted", Value::Int(row.predicted)),
        ("planned_cost", Value::Int(row.planned_cost)),
        ("planned_units", Value::Int(row.planned_units)),
        ("bought_units", Value::Int(row.bought_units)),
        ("bought_coins", Value::Int(row.bought_coins)),
        ("made", row.made.map(Value::Int).unwrap_or(Value::Null)),
        ("craft_yield", row.craft_yield.map(Value::Float).unwrap_or(Value::Null)),
        ("guide_per_6000", Value::Float(row.guide_per_6000)),
        ("sold_units", Value::Int(row.sold_units)),
        ("sold_coins", Value::Int(row.sold_coins)),
        ("realized", Value::Int(row.realized)),
        ("status", str_value(row.status)),
    ])
}

/// `orchestrator economy` section.
pub fn print_rows(rows: &[TradeRow]) {
    println!("100 % market lots (plan rendements R5; predicted = guide, realized = sales - purchases):");
    if rows.is_empty() {
        println!("  none yet");
        return;
    }
    let (closed, predicted, realized) = closed_totals(rows);
    println!("  closed lots: {closed}, predicted {predicted:+}, realized {realized:+}");
    for row in rows.iter().take(30) {
        let made = row.made.map_or_else(|| "-".to_string(), |made| made.to_string());
        let craft = row.craft_yield.map_or_else(|| "-".to_string(), |rate| format!("{rate:.0}"));
        println!(
            "  {:<12} {:<34} lot {:>4} ({:<7}) {:<8} bought {:>4}/{:<4} for {:>7} made {:>4} ({craft} / 6000, guide {:.0}) sold {:>4} for {:>7} predicted {:>+7} realized {:>+7}",
            row.account,
            row.method,
            row.lot,
            row.bound,
            row.status,
            row.bought_units,
            row.planned_units,
            row.bought_coins,
            made,
            row.guide_per_6000,
            row.sold_units,
            row.sold_coins,
            row.predicted,
            row.realized
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{closed_totals, lot_row};
    use lifescape_planner::trade_craft::TradeLot;
    use lifescape_store::{Deal, DealStatus};

    fn deal(id: i64, buyer: &str, seller: &str, item: &str, quantity: i64, price: i64, clock: i64) -> Deal {
        Deal {
            deal_id: id,
            buyer: buyer.to_string(),
            seller: seller.to_string(),
            item_id: lifescape_gamedata::objs_by_name(item)[0].obj_id as i64,
            quantity,
            price,
            meeting_x: 0,
            meeting_y: 0,
            meeting_level: 0,
            status: DealStatus::Settled,
            fail_reason: String::new(),
            deadline_tick: 0,
            created_clock: clock,
            updated_clock: clock,
            taker_side: None,
        }
    }

    fn lot(made: Option<i64>) -> TradeLot {
        TradeLot {
            node: "n7".to_string(),
            method: "trade:craft/smelt:bronze_bar".to_string(),
            product: "bronze_bar".to_string(),
            lot: 22,
            buys: vec![("copper_ore".to_string(), 22), ("tin_ore".to_string(), 22)],
            cost: 484,
            bound: "capital".to_string(),
            predicted: 396,
            guide_per_6000: 100.0,
            opened: 1_000,
            made,
            craft_ticks: 1_260,
        }
    }

    /// Plan R5: profit predicted (guide) against realized (deals): the
    /// lot's buys of its inputs since the lock, capped at the units
    /// planned; its sales of the product, capped at the products made;
    /// deals before the lock or after the next lot left out.
    #[test]
    fn realized_profit_reads_the_lot_deals_against_the_prediction() {
        let deals = vec![
            deal(1, "bob", "ann", "copper_ore", 5, 9, 900),
            deal(2, "bob", "ann", "copper_ore", 22, 10, 1_010),
            deal(3, "bob", "cid", "tin_ore", 30, 12, 1_020),
            deal(4, "dan", "bob", "bronze_bar", 15, 40, 1_500),
            deal(5, "eve", "bob", "bronze_bar", 10, 38, 1_600),
            deal(6, "bob", "ann", "copper_ore", 22, 10, 5_000),
        ];
        let buying = lot_row("bob", &lot(None), &deals[..2], None);
        assert_eq!((buying.bought_units, buying.bought_coins, buying.status), (22, 220, "buying"), "tin still to buy; the deal before the lock is out");
        let crafting = lot_row("bob", &lot(None), &deals, Some(4_000));
        assert_eq!((crafting.bought_units, crafting.bought_coins, crafting.status), (44, 220 + 264, "crafting"), "capped at the units planned, the next lot's deal out");
        let selling = lot_row("bob", &lot(Some(21)), &deals, Some(4_000));
        assert_eq!((selling.sold_units, selling.sold_coins), (21, 15 * 40 + 6 * 38), "sales capped at the products made");
        assert_eq!(selling.status, "closed");
        assert_eq!(selling.realized, 15 * 40 + 6 * 38 - 484);
        assert_eq!(selling.predicted, 396);
        assert!((selling.craft_yield.unwrap_or(0.0) - 100.0).abs() < 1e-9);
        let partial = lot_row("bob", &lot(Some(21)), &deals[..4], Some(4_000));
        assert_eq!(partial.status, "selling");
        assert_eq!(closed_totals(&[partial, selling.clone()]), (1, 396, selling.realized));
    }
}
