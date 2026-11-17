#![forbid(unsafe_code)]
//! Market makers for the operator (plan marché Q6): `orchestrator
//! economy` and the dashboard economy page, never read by a bot. Per
//! maker account (life files with the `market_maker` role): slice and
//! phase, capital (earning: the live wallet the switch reads; quoting:
//! coins at the switch plus P&L), stock by object, mean
//! quoted spread (ask over bid of its two-sided objects), P&L (realized
//! coins of its deals since the switch plus the stock they left, marked
//! at the current price) and its share of the resting units of its
//! slice's books (both sides). Read from the store, the life files and
//! the merged views (`run/users/<account>.json`, wallets)
//! only: offline, the daemon may be down.

use std::collections::HashMap;

use lifescape_json::{object, str_value, Value};
use lifescape_planner::{MakerPhase, MakerRole};
use lifescape_store::{Order, OrderSide, OrderStatus, Store};

use crate::market_maker::{positions, slice_items, Position};

/// One maker row.
#[derive(Clone, Debug, PartialEq)]
pub struct MakerRow {
    pub account: String,
    pub slice: u32,
    pub slices: u32,
    pub quoting: bool,
    pub capital_target: i64,
    pub capital_at_switch: i64,
    /// Realized coins (sales minus purchases) since the switch.
    pub cash: i64,
    /// Stock bought as a maker, marked at the current price.
    pub stock_value: i64,
    pub pnl: i64,
    /// The capital the switch compares to its target: an earning maker's
    /// wallet (pack, bank, worn coins), a quoting one's coins at the
    /// switch plus P&L. `None`: an earning maker with no snapshot.
    pub capital: Option<i64>,
    /// (object, units, marked value), largest value first.
    pub stock: Vec<(String, i64, i64)>,
    pub mean_spread: Option<f64>,
    pub book_share: Option<f64>,
    pub open_orders: usize,
}

fn resting(order: &Order) -> bool {
    matches!(order.status, OrderStatus::Open | OrderStatus::Matched) && order.remaining > 0
}

/// One row from its inputs (pure): own positions, own resting orders,
/// all resting units per slice object (both sides), marks per object.
pub fn row_from(account: &str, role: &MakerRole, held: &std::collections::BTreeMap<i64, Position>, own: &[Order], slice_units: i64, marks: &HashMap<i64, i64>, wallet: Option<i64>) -> MakerRow {
    let name_of = |id: i64| lifescape_gamedata::obj_by_id(usize::try_from(id).unwrap_or(0)).and_then(|obj| obj.name).unwrap_or("?").to_string();
    let cash: i64 = held.values().map(Position::cash).sum();
    let mut stock: Vec<(String, i64, i64)> = held
        .iter()
        .filter(|(_, position)| position.units > 0)
        .map(|(id, position)| {
            let mark = marks.get(id).copied().filter(|price| *price > 0).unwrap_or(position.cost / position.units.max(1));
            (name_of(*id), position.units, position.units * mark)
        })
        .collect();
    stock.sort_by(|left, right| right.2.cmp(&left.2).then_with(|| left.0.cmp(&right.0)));
    let stock_value: i64 = stock.iter().map(|(_, _, value)| *value).sum();
    let pnl = cash + stock_value;
    let quotes: Vec<&Order> = own.iter().filter(|order| resting(order) && order.motive == crate::market_lineage::MARKET_MAKE).collect();
    let mut sides: HashMap<i64, (Option<i64>, Option<i64>)> = HashMap::new();
    for order in &quotes {
        let entry = sides.entry(crate::market_audit::book_item(order.item_id)).or_default();
        if order.side == OrderSide::Buy {
            entry.0 = Some(entry.0.map_or(order.price, |best: i64| best.max(order.price)));
        } else {
            entry.1 = Some(entry.1.map_or(order.price, |best: i64| best.min(order.price)));
        }
    }
    let spreads: Vec<f64> = sides.values().filter_map(|(bid, ask)| Some(ask.as_ref()?.to_owned() as f64 / (*bid.as_ref()?).max(1) as f64)).collect();
    let mean_spread = (!spreads.is_empty()).then(|| spreads.iter().sum::<f64>() / spreads.len() as f64);
    let own_units: i64 = quotes.iter().map(|order| order.remaining).sum();
    let book_share = (slice_units > 0).then(|| own_units as f64 / slice_units as f64);
    MakerRow {
        account: account.to_string(),
        slice: role.slice,
        slices: role.slices,
        quoting: role.phase == MakerPhase::Quoting,
        capital_target: role.capital_target,
        capital_at_switch: role.capital_at_switch,
        cash,
        stock_value,
        pnl,
        capital: if role.phase == MakerPhase::Quoting { Some(role.capital_at_switch + pnl) } else { wallet },
        stock,
        mean_spread,
        book_share,
        open_orders: quotes.len(),
    }
}

/// Coins of a live snapshot (pack, bank, worn): the wallet the switch
/// reads (`brain_plan::holdings_count`, same `live_items` rule).
pub fn wallet_of(snapshot: &Value) -> i64 {
    crate::live_items::live_units(snapshot, "coins")
}

/// An account's wallet from its merged view on disk
/// (`run/users/<account>.json`, written by the daemon from the pushed
/// snapshot): `None` when absent, unreadable or without inventories.
fn wallet_on_disk(account: &str) -> Option<i64> {
    let (merged, _, _) = crate::paths::user_paths(account);
    let text = std::fs::read_to_string(merged).ok()?;
    let value = lifescape_json::parse(&text).ok()?;
    (!value.get("bank").is_null() || !value.get("inv").is_null()).then(|| wallet_of(&value))
}

/// Maker lives on disk: (account, role as the current partition reads
/// it: `market_maker::current_role`), by account.
pub fn maker_lives(lives_dir: &std::path::Path) -> Vec<(String, MakerRole)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(lives_dir) else { return out };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(account) = path.file_stem().and_then(|stem| stem.to_str()).map(str::to_string) else { continue };
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(value) = lifescape_json::parse(&text) else { continue };
        if let lifescape_planner::Archetype::MarketMaker(role) = lifescape_planner::market_maker::archetype_from_value(&value.get("profile")) {
            out.push((account, crate::market_maker::current_role(role)));
        }
    }
    out.sort_by(|left, right| left.0.cmp(&right.0));
    out
}

/// Every maker's row, read from the store (deals since its switch, its
/// orders, its slice's books, the last fills as marks).
pub fn maker_rows(store: &Store, lives_dir: &std::path::Path) -> Vec<MakerRow> {
    let mut rows = Vec::new();
    for (account, role) in maker_lives(lives_dir) {
        let since = if role.phase == MakerPhase::Quoting { role.switched_at } else { i64::MAX };
        let deals = store.own_deals_since(&account, since).unwrap_or_default();
        let held = positions(&account, &deals);
        let own = store.account_orders(&account).unwrap_or_default();
        let mut slice_units = 0;
        for item in slice_items(role.slice) {
            for side in [OrderSide::Buy, OrderSide::Sell] {
                slice_units += store.open_book(item, side).unwrap_or_default().iter().map(|order| order.remaining).sum::<i64>();
            }
        }
        let marks: HashMap<i64, i64> = held
            .keys()
            .filter_map(|item| {
                let prices: Vec<i64> = store.last_fills(*item, lifescape_market::PRICE_FILLS as i64).unwrap_or_default().into_iter().map(|(price, _)| price).collect();
                Some((*item, lifescape_market::median_price(&prices)?))
            })
            .collect();
        let wallet = if role.phase == MakerPhase::Quoting { None } else { wallet_on_disk(&account) };
        rows.push(row_from(&account, &role, &held, &own, slice_units, &marks, wallet));
    }
    rows
}

/// Dashboard shape of one row.
pub fn row_value(row: &MakerRow) -> Value {
    object(vec![
        ("account", str_value(&row.account)),
        ("slice", Value::Int(i64::from(row.slice))),
        ("slices", Value::Int(i64::from(row.slices))),
        ("phase", str_value(if row.quoting { "quoting" } else { "earning" })),
        ("capital_target", Value::Int(row.capital_target)),
        ("slice_need", Value::Int(crate::market_maker::slice_need(row.slice))),
        ("slice_objects", Value::Int(crate::market_maker::slice_items(row.slice).len() as i64)),
        ("capital", row.capital.map(Value::Int).unwrap_or(Value::Null)),
        ("pnl", Value::Int(row.pnl)),
        ("stock_value", Value::Int(row.stock_value)),
        ("mean_spread", row.mean_spread.map(Value::Float).unwrap_or(Value::Null)),
        ("book_share", row.book_share.map(Value::Float).unwrap_or(Value::Null)),
        ("open_orders", Value::Int(row.open_orders as i64)),
        (
            "stock",
            Value::Arr(row.stock.iter().take(12).map(|(name, units, value)| object(vec![("name", str_value(name)), ("units", Value::Int(*units)), ("value", Value::Int(*value))])).collect()),
        ),
    ])
}

/// `orchestrator economy` section.
pub fn print_rows(rows: &[MakerRow]) {
    println!("market makers (plan marché Q6; operator info, bots never read it):");
    let count = crate::market_maker::slices().len() as u32;
    let needs: Vec<i64> = (0..count).map(crate::market_maker::slice_need).collect();
    let sizes: Vec<usize> = crate::market_maker::slices().iter().map(Vec::len).collect();
    println!(
        "  common capital target {} for every maker; {count} slices balanced by capital: need {}..{}, objects {}..{}",
        crate::market_maker::common_capital_target(),
        needs.iter().min().copied().unwrap_or(0),
        needs.iter().max().copied().unwrap_or(0),
        sizes.iter().min().copied().unwrap_or(0),
        sizes.iter().max().copied().unwrap_or(0)
    );
    if rows.is_empty() {
        println!("  none (deploy: create-user --role market_maker, docs/orchestrator.md)");
        return;
    }
    for row in rows {
        let spread = row.mean_spread.map_or_else(|| "n/a".to_string(), |ratio| format!("x{ratio:.2}"));
        let share = row.book_share.map_or_else(|| "n/a".to_string(), |ratio| format!("{:.1} %", ratio * 100.0));
        let phase = if row.quoting { "quoting" } else { "earning" };
        println!(
            "  {:<12} slice {}/{} ({} objects, need {}) {phase:<8} target {:>7} capital {:>8} P&L {:>+8} (cash {:+}, stock {}) spread {spread} book {share} quotes {}",
            row.account,
            row.slice,
            row.slices,
            crate::market_maker::slice_items(row.slice).len(),
            crate::market_maker::slice_need(row.slice),
            row.capital_target,
            row.capital.map_or_else(|| "n/a".to_string(), |capital| capital.to_string()),
            row.pnl,
            row.cash,
            row.stock_value,
            row.open_orders
        );
        let top: Vec<String> = row.stock.iter().take(6).map(|(name, units, value)| format!("{name} {units} ({value})")).collect();
        if !top.is_empty() {
            println!("    stock: {}", top.join(", "));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(item: i64, side: OrderSide, price: i64, remaining: i64) -> Order {
        Order { item_id: item, side, price, remaining, quantity: remaining, status: OrderStatus::Open, motive: crate::market_lineage::MARKET_MAKE.to_string(), ..Order::default() }
    }

    #[test]
    fn a_row_reads_capital_stock_spread_and_share() {
        let mut role = MakerRole::new(2, 11, 90_000);
        role.phase = MakerPhase::Quoting;
        role.capital_at_switch = 95_000;
        let feather = lifescape_gamedata::objs_by_name("feather")[0].obj_id as i64;
        let mut held = std::collections::BTreeMap::new();
        held.insert(feather, Position { units: 300, cost: 600, bought_units: 500, bought_coins: 1000, sold_units: 200, sold_coins: 700 });
        let own = vec![order(feather, OrderSide::Buy, 2, 100), order(feather, OrderSide::Sell, 3, 100), Order { motive: "surplus".to_string(), ..order(feather, OrderSide::Sell, 9, 5) }];
        let marks: HashMap<i64, i64> = [(feather, 2)].into_iter().collect();
        let row = row_from("maker_two", &role, &held, &own, 800, &marks, Some(1_234));
        assert_eq!(row.cash, -300);
        assert_eq!(row.stock_value, 600, "300 feathers at the current 2");
        assert_eq!((row.pnl, row.capital), (300, Some(95_300)), "a quoting maker: coins at the switch plus P&L");
        assert_eq!(row.mean_spread, Some(1.5), "ask 3 over bid 2; the surplus listing is no quote");
        assert_eq!(row.book_share, Some(0.25), "200 of 800 resting units");
        assert_eq!(row.open_orders, 2);
        assert_eq!(row.stock[0].0, "feather");
        let value = row_value(&row);
        assert_eq!(value.get("phase").as_str(), Some("quoting"));
        assert_eq!(value.get("capital").as_i64(), Some(95_300));
    }

    /// Live 2026-11-17: `economy` printed `capital 0 … cash +0, stock 0`
    /// for the 11 earning makers (carl_miller 42 541 coins, luke_hill
    /// 10 878): an earning maker has no switch nor P&L yet, its capital is
    /// the wallet the switch compares to the target (`holdings_count`
    /// coins: pack, bank, worn), read from the same live snapshot shape.
    #[test]
    fn an_earning_maker_reports_the_wallet_the_switch_reads() {
        use lifescape_json::{object, Value};
        let role = MakerRole::new(2, 11, 103_000);
        let coins = lifescape_gamedata::objs_by_name("coins")[0].obj_id as i64;
        let stack = |count: i64| object(vec![("id", Value::Int(coins)), ("name", str_value("Coins")), ("count", Value::Int(count))]);
        let users_file = object(vec![("inv", Value::Arr(vec![stack(1_541)])), ("bank", Value::Arr(vec![stack(41_000)]))]);
        let wallet = wallet_of(&users_file);
        assert_eq!(wallet, 42_541, "pack plus bank coins");
        let row = row_from("carl_t", &role, &std::collections::BTreeMap::new(), &[], 0, &HashMap::new(), Some(wallet));
        assert_eq!(row.capital, Some(42_541), "the wallet, never 0");
        assert_eq!(lifescape_planner::switch_due(&role, wallet), false, "42 541 < 103 000: still earning");
        assert!(lifescape_planner::switch_due(&role, 103_000), "the switch fires on the same wallet at the target");
        let unknown = row_from("carl_t", &role, &std::collections::BTreeMap::new(), &[], 0, &HashMap::new(), None);
        assert_eq!(unknown.capital, None, "no snapshot: unknown, never 0");
        assert!(row_value(&unknown).get("capital").is_null());
    }
}
