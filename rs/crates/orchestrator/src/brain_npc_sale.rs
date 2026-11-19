#![forbid(unsafe_code)]
//! NPC shop sales of the unsellable (plan marché Q3, user decision
//! 2026-11-16, an exception to D-A: `docs/decisions.md`). The selling
//! pass (`brain_sales`) keeps off the market every surplus lot an F2P
//! shop pays MORE for than the market (`lifescape_market::outlets`: the
//! best bid, the traded price; nothing for a never-traded object
//! without a bid) and hands the lots here. When the trip pays (the
//! gain over the market beats `SHOP_TRIP_TICKS` at the bot's own coin
//! rate) a sale plan waits for the janitor node (`SpaceCleanup`), whose
//! `market cleanup` run walks to the shop that pays the most in total
//! (its keeper site nearest the bot) and sells unit by unit, never a unit
//! at or under what the market pays (`--shopsell ITEM:QTY:MIN`).
//!
//! The coins the shops pay are new money: the coin ledger tags them
//! `shop_sold`, `orchestrator economy` prints them (Q3 tracking).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use lifescape_planner::{Life, Node, NodeKind, NodeStatus};

/// Ticks a shop trip costs (bank, keeper, back), an estimate in ticks:
/// the sale must gain more than this many ticks of the bot's own rate.
pub const SHOP_TRIP_TICKS: f64 = 1500.0;

/// One lot to sell: the object, its units, the least coins one unit may
/// fetch (market pays + 1) and what the shop pays for the first unit.
#[derive(Clone, Debug, PartialEq)]
pub struct SaleLine {
    pub item: String,
    pub qty: i64,
    pub min_unit: i64,
    pub value: i64,
}

/// One shop trip: the shop key, its keeper site, the lots.
#[derive(Clone, Debug, PartialEq)]
pub struct ShopSalePlan {
    pub shop: String,
    pub site: (i64, i64, i64),
    pub lines: Vec<SaleLine>,
}

impl ShopSalePlan {
    /// Coins gained over the market (first-unit value, an upper bound:
    /// the script stops at each line's minimum).
    pub fn gain(&self) -> i64 {
        self.lines.iter().map(|line| (line.value - line.min_unit + 1).max(0) * line.qty).sum()
    }

    /// `market cleanup` arguments of the trip.
    pub fn args(&self) -> Vec<String> {
        let mut args = vec!["--sale-shop".to_string(), self.shop.clone(), "--sale-site".to_string(), format!("{},{},{}", self.site.0, self.site.1, self.site.2)];
        for line in &self.lines {
            args.push("--shopsell".to_string());
            args.push(format!("{}:{}:{}", line.item, line.qty, line.min_unit));
        }
        args
    }
}

/// A candidate lot: object, free units, what the market pays per unit.
pub type Candidate = (String, i64, i64);

/// The trip (pure): per shop, the lots it pays more for than the market;
/// the shop with the largest gain, its keeper site nearest `position`
/// (x, z, level). `None` when nothing beats the market or the gain is
/// under `min_gain` coins.
pub fn plan_shop_sale(candidates: &[Candidate], position: Option<(i64, i64, i64)>, min_gain: i64, shops_of: &dyn Fn(&str) -> Vec<lifescape_gamedata::shop_sale::SaleShop>) -> Option<ShopSalePlan> {
    let mut by_shop: HashMap<String, (Vec<(i64, i64, i64)>, Vec<SaleLine>)> = HashMap::new();
    for (item, qty, pays) in candidates.iter().filter(|(_, qty, _)| *qty > 0) {
        for shop in shops_of(item).into_iter().filter(|shop| shop.value > *pays) {
            let entry = by_shop.entry(shop.shop.clone()).or_insert_with(|| (shop.sites.clone(), Vec::new()));
            entry.1.push(SaleLine { item: item.clone(), qty: *qty, min_unit: pays + 1, value: shop.value });
        }
    }
    let distance = |site: &(i64, i64, i64)| match position {
        Some((x, z, level)) => (site.0 - x).abs() + (site.1 - z).abs() + if site.2 == level { 0 } else { 1000 },
        None => 0,
    };
    let mut plans: Vec<ShopSalePlan> = by_shop
        .into_iter()
        .filter_map(|(shop, (sites, lines))| {
            let site = sites.iter().min_by_key(|site| distance(site)).copied()?;
            Some(ShopSalePlan { shop, site, lines })
        })
        .collect();
    plans.sort_by(|left, right| right.gain().cmp(&left.gain()).then_with(|| distance(&left.site).cmp(&distance(&right.site))).then_with(|| left.shop.cmp(&right.shop)));
    plans.into_iter().next().filter(|plan| plan.gain() >= min_gain.max(1))
}

/// One shop trip the brain sent (the janitor launch took its plan): the
/// fleet's sales there are read back from the store's `shop_sold` lines
/// of the account's next reports.
#[derive(Clone, Debug, PartialEq)]
pub struct SentTrip {
    pub account: String,
    pub shop: String,
    /// Unix seconds of the launch.
    pub launched: i64,
    /// The janitor node that carries the trip (`--node`): only its own
    /// report is the trip's return.
    pub node_id: String,
    /// Objects of the trip (`gamedata` names) and the units planned.
    pub items: Vec<(String, i64)>,
}

/// Units the fleet sold per (shop, object): (units, ticks ago).
pub type ShopSales = HashMap<(String, String), Vec<(i64, i64)>>;

/// One report row read back (`Store::shop_sales_since`): account, node,
/// created clock in unix seconds, display name sold ("" for a report
/// without NPC sale), units sold.
pub type SaleRow = (String, String, i64, String, i64);

/// A trip's sales are the account's reports created after its launch,
/// before its next trip and at most this many seconds later.
pub const TRIP_REPORT_SECS: i64 = 7200;
/// Trips the daemon remembers (the slowest F2P restock of a big sale
/// outlasts a few hours; a restart forgets them: base stock again).
pub const TRIP_MEMORY_SECS: i64 = 6 * 3600;

/// Pure: the fleet's sales per (shop, object) from the trips the brain
/// sent and the store's report rows: each trip's `shop_sold` units of its
/// objects (display names), as (units, ticks ago) at `tick_ms`. A trip
/// still out (no report of its own node since its launch, within
/// `TRIP_REPORT_SECS`) counts its planned units, aged from the launch: a second bot
/// planned meanwhile never reads the base stock (live wolfe_carl /
/// amber_moore, the Bandit Camp trip lasts ~1000 ticks).
pub fn fleet_sales(trips: &[SentTrip], rows: &[SaleRow], now: i64, tick_ms: u64) -> ShopSales {
    let mut sales = ShopSales::new();
    let tick_ms = i64::try_from(tick_ms.max(1)).unwrap_or(i64::MAX);
    for trip in trips {
        let next_launch = trips.iter().filter(|other| other.account == trip.account && other.launched > trip.launched).map(|other| other.launched).min();
        let until = next_launch.unwrap_or(i64::MAX).min(trip.launched + TRIP_REPORT_SECS);
        let names: Vec<(&str, String, i64)> = trip
            .items
            .iter()
            .map(|(item, planned)| {
                let display = lifescape_gamedata::objs_by_name(item).into_iter().next().and_then(|obj| obj.display).unwrap_or(item.as_str()).to_string();
                (item.as_str(), display, *planned)
            })
            .collect();
        let mut reported = false;
        for (account, node, created, shown, units) in rows {
            // Only the report of the trip's own node is its return (the
            // previous task's report, ingested the same second, is not).
            if *account != trip.account || *node != trip.node_id || *created < trip.launched || *created >= until {
                continue;
            }
            reported = true;
            if *units <= 0 {
                continue;
            }
            let Some((item, _, _)) = names.iter().find(|(_, display, _)| display.eq_ignore_ascii_case(shown)) else { continue };
            let ticks_ago = (now - created).max(0).saturating_mul(1000) / tick_ms;
            sales.entry((trip.shop.clone(), item.to_string())).or_default().push((*units, ticks_ago));
        }
        if !reported && now - trip.launched < TRIP_REPORT_SECS {
            for (item, _, planned) in &names {
                // Aged from the launch: a killed session never pins its
                // whole pile at the shop for two hours.
                let ticks_ago = (now - trip.launched).max(0).saturating_mul(1000) / tick_ms;
                sales.entry((trip.shop.clone(), item.to_string())).or_default().push((*planned, ticks_ago));
            }
        }
    }
    sales
}

/// The F2P shops open to every bot that buy `item`, each valued at the
/// next unit it likely pays (`shop_sale::likely_unit_value` over the
/// fleet's own sales there), best first; a shop that pays under a coin
/// drops out.
pub fn shops_at_likely_stock(item: &str, sales: &ShopSales) -> Vec<lifescape_gamedata::shop_sale::SaleShop> {
    let mut shops: Vec<lifescape_gamedata::shop_sale::SaleShop> = lifescape_gamedata::shop_sale::npc_sale_shops(item)
        .into_iter()
        .filter_map(|mut shop| {
            let sold = sales.get(&(shop.shop.clone(), item.to_string())).map(Vec::as_slice).unwrap_or(&[]);
            shop.value = lifescape_gamedata::shop_sale::likely_unit_value(&shop.shop, item, sold)?;
            (shop.value >= 1).then_some(shop)
        })
        .collect();
    shops.sort_by(|left, right| right.value.cmp(&left.value).then_with(|| left.shop.cmp(&right.shop)));
    shops
}

/// Best value an open shop likely pays for the next unit of `item`
/// (`npc_sale_value` at the likely stock).
pub fn likely_sale_value(item: &str, sales: &ShopSales) -> Option<i64> {
    shops_at_likely_stock(item, sales).first().map(|shop| shop.value)
}

fn sent() -> &'static Mutex<Vec<SentTrip>> {
    static TABLE: OnceLock<Mutex<Vec<SentTrip>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(Vec::new()))
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|elapsed| elapsed.as_secs() as i64).unwrap_or(0)
}

/// Seconds the store rows of `load_fleet_sales` are reused (one read
/// for every bot's selling pass in that window).
pub const SALES_CACHE_SECS: i64 = 45;

/// Cached store rows: (read at, since, rows).
type RowsCache = Option<(i64, i64, std::sync::Arc<Vec<SaleRow>>)>;

fn rows_cache() -> &'static Mutex<RowsCache> {
    static CACHE: OnceLock<Mutex<RowsCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// The fleet's recent sales per (shop, object): the trips the daemon
/// sent (`take`, in memory) and one store read of every report since the
/// oldest of them (`Store::shop_sales_since`), cached
/// `SALES_CACHE_SECS`. No trip remembered: no read at all.
pub fn load_fleet_sales(service: &crate::market_service::MarketService) -> ShopSales {
    let now = unix_now();
    let trips: Vec<SentTrip> = {
        let mut table = sent().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        table.retain(|trip| now - trip.launched <= TRIP_MEMORY_SECS);
        table.clone()
    };
    let Some(since) = trips.iter().map(|trip| trip.launched).min() else { return ShopSales::new() };
    let rows = {
        let mut cache = rows_cache().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match cache.as_ref() {
            Some((read_at, cached_since, rows)) if now - read_at < SALES_CACHE_SECS && *cached_since <= since => rows.clone(),
            _ => {
                let fresh = std::sync::Arc::new(crate::cpu_profile::timed("sales.npc_shop_rows", || service.lock_store().shop_sales_since(since).unwrap_or_default()));
                *cache = Some((now, since, fresh.clone()));
                fresh
            }
        }
    };
    fleet_sales(&trips, &rows, now, crate::market_eta::live_tick_ms())
}

fn pending() -> &'static Mutex<HashMap<String, ShopSalePlan>> {
    static TABLE: OnceLock<Mutex<HashMap<String, ShopSalePlan>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Files (or replaces) the account's pending trip.
pub fn note(account: &str, plan: ShopSalePlan) {
    pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(account.to_string(), plan);
}

/// Takes the account's pending trip (the janitor launch) and remembers it
/// as sent under the janitor node `node_id` that carries it: its sales
/// read back into the shop's likely stock.
pub fn take(account: &str, node_id: &str) -> Option<ShopSalePlan> {
    let plan = pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(account)?;
    let trip = SentTrip { account: account.to_string(), shop: plan.shop.clone(), launched: unix_now(), node_id: node_id.to_string(), items: plan.lines.iter().map(|line| (line.item.clone(), line.qty)).collect() };
    sent().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(trip);
    Some(plan)
}

/// Drops a trip the last selling pass no longer backs.
pub fn forget(account: &str) {
    pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(account);
}

fn waiting(account: &str) -> bool {
    pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).contains_key(account)
}

/// Inserts a janitor node for a pending trip (none open, the janitor
/// not cooling down after a short run). Returns true when inserted.
pub fn maybe_insert(life: &mut Life, now: i64) -> bool {
    if !waiting(&life.account) || lifescape_planner::cleanup_cooling(&life.history, now) {
        return false;
    }
    let open = life.nodes.iter().any(|node| matches!(node.kind, NodeKind::SpaceCleanup) && !matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned));
    if open || lifescape_planner::janitor_stalled(life) {
        return false;
    }
    let id = lifescape_planner::next_id(life);
    lifescape_planner::insert_ready(life, Node::new(&id, NodeKind::SpaceCleanup, Vec::new(), 0));
    true
}

/// The bot's position from its live snapshot (`[level, x, z]`) as
/// (x, z, level).
pub fn live_position(live: &lifescape_json::Value) -> Option<(i64, i64, i64)> {
    crate::verify_assert::position_of(live).map(|(level, x, z)| (x, z, level))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lifescape_gamedata::shop_sale::SaleShop;

    fn shops(item: &str) -> Vec<SaleShop> {
        match item {
            "iron_dagger" => vec![
                SaleShop { shop: "generalshop1".to_string(), value: 14, sites: vec![(3212, 3246, 0)], ..SaleShop::default() },
                SaleShop { shop: "generalshop4".to_string(), value: 14, sites: vec![(2958, 3390, 0)], ..SaleShop::default() },
            ],
            "bronze_med_helm" => vec![SaleShop { shop: "generalshop1".to_string(), value: 9, sites: vec![(3212, 3246, 0)], ..SaleShop::default() }],
            _ => Vec::new(),
        }
    }

    /// Q3: only the lots the shop pays more for than the market go; the
    /// trip goes to the shop paying the most, nearest keeper first.
    #[test]
    fn the_trip_carries_only_what_the_market_pays_less_for() {
        let candidates = vec![("iron_dagger".to_string(), 40, 0), ("bronze_med_helm".to_string(), 80, 12), ("feather".to_string(), 5000, 1)];
        let plan = plan_shop_sale(&candidates, Some((3220, 3218, 0)), 100, &shops).expect("daggers pay the trip");
        assert_eq!(plan.shop, "generalshop1", "Lumbridge is nearer than Falador");
        assert_eq!(plan.lines, vec![SaleLine { item: "iron_dagger".to_string(), qty: 40, min_unit: 1, value: 14 }], "helms fetch 12 on the market: never sold for 9");
        assert_eq!(plan.args()[..4], ["--sale-shop", "generalshop1", "--sale-site", "3212,3246,0"]);
        assert!(plan.args().contains(&"iron_dagger:40:1".to_string()));
        assert_eq!(plan_shop_sale(&candidates, None, 10_000, &shops), None, "a trip that does not pay waits");
        assert_eq!(plan_shop_sale(&[("bronze_med_helm".to_string(), 80, 12)], None, 1, &shops), None);
    }

    /// Live 2026-11-19 (54 shop trips, 743 lots of the day stopped at
    /// once by "the next unit pays under"): the brain valued the Bandit
    /// Camp store at its base stock while the fleet had just sold it the
    /// same object (wolfe_carl, amber_moore: mithril platebodies 12 at a
    /// time). The fleet's own sales there, read back from the store's
    /// `shop_sold` lines of the trips the brain sent, decay by the
    /// engine's restock (one unit per 100 ticks); a trip whose first unit
    /// pays under the minimum is never sent.
    #[test]
    fn a_trip_reads_the_shops_likely_stock() {
        let trips = vec![SentTrip { account: "wolfe_carl".to_string(), shop: "taxfree".to_string(), launched: 1000, node_id: "n2258".to_string(), items: vec![("mithril_platebody".to_string(), 158)] }];
        // The report of the cleanup run 30 s after the launch, and an
        // older one (before the trip): only the first counts.
        let reports = vec![("wolfe_carl".to_string(), "n2201".to_string(), 900, "Mithril platebody".to_string(), 40), ("wolfe_carl".to_string(), "n2258".to_string(), 1030, "Mithril platebody".to_string(), 13)];
        let sales = fleet_sales(&trips, &reports, 1030, 600);
        assert_eq!(sales.get(&("taxfree".to_string(), "mithril_platebody".to_string())), Some(&vec![(13, 0)]));
        let candidates = vec![("mithril_platebody".to_string(), 158, 1211)];
        let fresh = |item: &str| shops_at_likely_stock(item, &sales);
        let taxfree_now = fresh("mithril_platebody").into_iter().find(|shop| shop.shop == "taxfree").map(|shop| shop.value);
        assert_eq!(taxfree_now, Some(1092), "13 just sold: taxfree's next unit pays (600 - 390) x 5.2");
        if let Some(plan) = plan_shop_sale(&candidates, Some((3092, 3243, 0)), 1, &fresh) {
            assert_ne!(plan.shop, "taxfree", "never a trip whose first unit pays under 1212");
            assert!(plan.lines.iter().all(|line| line.value >= line.min_unit), "{plan:?}");
        }
        let later = fleet_sales(&trips, &reports, 1030 + 3000, 600);
        let rested = |item: &str| shops_at_likely_stock(item, &later);
        let plan = plan_shop_sale(&candidates, Some((3092, 3243, 0)), 1, &rested).expect("restocked down: the trip pays again");
        assert_eq!(plan.lines[0].value, 3120, "a base-stock first unit (Horvik pays as much, nearer)");
        assert_eq!(rested("mithril_platebody").into_iter().find(|shop| shop.shop == "taxfree").map(|shop| shop.value), Some(3120), "taxfree back to its base");
        assert_eq!(shops_at_likely_stock("mithril_platebody", &ShopSales::new()).first().map(|shop| shop.value), Some(3120), "no sale: the base stock");
    }

    /// Lot 8 review: a trip launched and not reported yet (the Bandit Camp
    /// trip lasts ~1000 ticks) counts its planned units as sold now, so a
    /// second bot planned meanwhile never reads the base stock; its
    /// report (even one with no NPC sale) replaces the plan; an old trip
    /// never reported counts nothing.
    #[test]
    fn a_trip_in_flight_counts_its_planned_units() {
        let key = ("taxfree".to_string(), "mithril_platebody".to_string());
        let trips = vec![SentTrip { account: "amber_moore".to_string(), shop: "taxfree".to_string(), launched: 1000, node_id: "n2190".to_string(), items: vec![("mithril_platebody".to_string(), 13)] }];
        assert_eq!(fleet_sales(&trips, &[], 1010, 600).get(&key), Some(&vec![(13, 16)]), "out: its 13 units count, aged from the launch (10 s = 16 ticks)");
        // Re-review: the brain ingests the previous task's report and
        // launches the trip in the same second: that report (another
        // node) is never the trip's return.
        let previous_task = vec![("amber_moore".to_string(), "n2188".to_string(), 1000, String::new(), 0)];
        assert_eq!(fleet_sales(&trips, &previous_task, 1010, 600).get(&key), Some(&vec![(13, 16)]), "another node's report: still out");
        let nothing_sold = vec![("amber_moore".to_string(), "n2190".to_string(), 1040, String::new(), 0)];
        assert_eq!(fleet_sales(&trips, &nothing_sold, 1050, 600).get(&key), None, "back with no sale: nothing");
        assert_eq!(fleet_sales(&trips, &[], 1000 + TRIP_REPORT_SECS, 600).get(&key), None, "never reported: forgotten");
        let second = |item: &str| shops_at_likely_stock(item, &fleet_sales(&trips, &[], 1010, 600));
        assert!(second("mithril_platebody").iter().find(|shop| shop.shop == "taxfree").is_some_and(|shop| shop.value < 1212), "the second bot sees 13 held");
    }
}
