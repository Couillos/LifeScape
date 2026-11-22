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
//!
//! Merchant arbitrage (plan équilibre D2, `brain_arbitrage`): a lot bought
//! on the market for a shop is filed here (`note_arbitrage`) until its
//! trip leaves; the other bots read its units as sold there
//! (`fleet_sales_for`), the selling pass sends it to the shop whatever
//! its stock, for the units it bought (`arbitrage_units`, `shop_split`). The market makers' outlet is what a
//! unit nets there over a lot, the trip paid (`net_shop_outlet`).

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
    /// Units of the line bought for the shop (merchant arbitrage, D2):
    /// priced on the shop's whole proceeds, the trip counts them all; the
    /// other units count their gain over the market.
    pub whole: i64,
}

/// One shop trip: the shop key, its keeper site, the lots.
#[derive(Clone, Debug, PartialEq)]
pub struct ShopSalePlan {
    pub shop: String,
    pub site: (i64, i64, i64),
    pub lines: Vec<SaleLine>,
}

impl ShopSalePlan {
    /// Coins the trip is worth (first-unit value, an upper bound: the
    /// script stops at each line's minimum): the units bought for the shop
    /// (`whole`) the shop's whole proceeds (priced on them, never left
    /// banked), every other unit its gain over the market.
    pub fn gain(&self) -> i64 {
        self.lines
            .iter()
            .map(|line| {
                let whole = line.whole.clamp(0, line.qty.max(0));
                line.value.max(0) * whole + (line.value - line.min_unit + 1).max(0) * (line.qty - whole)
            })
            .sum()
    }

    /// The trip without the lots whose object the graph needs now (`keep`);
    /// `None` when no lot is left.
    pub fn without_kept(mut self, keep: &[String]) -> Option<ShopSalePlan> {
        self.lines.retain(|line| !keep.contains(&line.item));
        (!self.lines.is_empty()).then_some(self)
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

/// How a free pile splits between the shop trip and the market listing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShopSplit {
    /// Units for the shop trip.
    pub to_shop: i64,
    /// Of them, units the arbitrage bought (whole proceeds).
    pub whole: i64,
    /// Units left to the market listing.
    pub to_market: i64,
}

/// Pure (merchant arbitrage, D2): a free pile of `qty` units, `bought` of
/// them bought for a shop, the shop paying more than the market or not.
/// The bought units (never more than the pile) go to the shop whatever
/// the overstock rule and count their whole proceeds; the rest of the
/// pile follows the normal rule: the shop unless `rest_overstocked`, else
/// the market. The market pays more: all of it to the market.
pub fn shop_split(qty: i64, bought: i64, rest_overstocked: bool, shop_beats_market: bool) -> ShopSplit {
    let qty = qty.max(0);
    if !shop_beats_market {
        return ShopSplit { to_shop: 0, whole: 0, to_market: qty };
    }
    let whole = bought.clamp(0, qty);
    let rest = qty - whole;
    let to_shop = whole + if rest_overstocked { 0 } else { rest };
    ShopSplit { to_shop, whole, to_market: qty - to_shop }
}

/// The trip (pure): per shop, the lots it pays more for than the market;
/// the shop with the largest gain (`ShopSalePlan::gain`: the units of
/// `whole_units` bought for a shop count their whole proceeds), its
/// keeper site nearest `position` (x, z, level). `None` when nothing
/// beats the market or the gain is under `min_gain` coins.
pub fn plan_shop_sale(candidates: &[Candidate], position: Option<(i64, i64, i64)>, min_gain: i64, whole_units: &[(String, i64)], shops_of: &dyn Fn(&str) -> Vec<lifescape_gamedata::shop_sale::SaleShop>) -> Option<ShopSalePlan> {
    let mut by_shop: HashMap<String, (Vec<(i64, i64, i64)>, Vec<SaleLine>)> = HashMap::new();
    for (item, qty, pays) in candidates.iter().filter(|(_, qty, _)| *qty > 0) {
        let whole = whole_units.iter().filter(|(name, _)| name == item).map(|(_, units)| *units).sum::<i64>();
        for shop in shops_of(item).into_iter().filter(|shop| shop.value > *pays) {
            let entry = by_shop.entry(shop.shop.clone()).or_insert_with(|| (shop.sites.clone(), Vec::new()));
            entry.1.push(SaleLine { item: item.clone(), qty: *qty, min_unit: pays + 1, value: shop.value, whole });
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

/// Pure: `shops` without the keeper sites behind the Karamja boat when
/// `free_coins` (the wallet the bot's resting buys leave) cannot pay the
/// boat out and back (2 x `BOAT_FARE_GP`); a shop left without a site
/// drops. Live 2026-11-20 (frank_lane: every coin locked by open market
/// orders, `market cleanup` to the Karamja general store failed 45 times
/// across the fleet on the boat's fare).
pub fn payable_sale_sites(shops: Vec<lifescape_gamedata::shop_sale::SaleShop>, free_coins: i64) -> Vec<lifescape_gamedata::shop_sale::SaleShop> {
    let fare_free = free_coins >= 2 * lifescape_gamedata::services::BOAT_FARE_GP;
    shops
        .into_iter()
        .filter_map(|mut shop| {
            shop.sites.retain(|site| fare_free || !lifescape_gamedata::access::behind_boat(site.0, site.1, site.2));
            (!shop.sites.is_empty()).then_some(shop)
        })
        .collect()
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
/// read back into the shop's likely stock. The account's arbitrage lots
/// of the trip's objects are on their way now: the trip counts them.
pub fn take(account: &str, node_id: &str) -> Option<ShopSalePlan> {
    let plan = pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(account)?;
    let trip = SentTrip { account: account.to_string(), shop: plan.shop.clone(), launched: unix_now(), node_id: node_id.to_string(), items: plan.lines.iter().map(|line| (line.item.clone(), line.qty)).collect() };
    sent().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(trip);
    arbitrage_lots().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).retain(|lot| lot.account != account || !plan.lines.iter().any(|line| line.item == lot.item));
    Some(plan)
}

/// One merchant arbitrage lot (plan équilibre D2, `brain_arbitrage`):
/// units really bought on the market for `shop` (its bid's own deals,
/// never the units it asked for) plus the units of the earlier lot still
/// carried. Until the account's shop trip carries them, every OTHER bot
/// reads them as already sold there (several bots never flood one shop
/// on its base stock); the buyer itself does not (they are its trip).
#[derive(Clone, Debug, PartialEq)]
pub struct ArbitrageInFlight {
    pub account: String,
    pub shop: String,
    pub item: String,
    /// The object's book id (its deals are read by it).
    pub item_id: i64,
    /// Units of the account's earlier lot of the object, still held.
    pub carried: i64,
    /// Units the bid bought (its deals since `since`).
    pub bought: i64,
    /// Unix seconds (the store clock) of the bid.
    pub since: i64,
    /// The bid still rests on the book.
    pub resting: bool,
}

impl ArbitrageInFlight {
    /// Units on their way to the shop.
    pub fn units(&self) -> i64 {
        self.carried.max(0) + self.bought.max(0)
    }
}

/// A lot whose trip never left is forgotten after this long (the trip
/// report window).
pub const ARBITRAGE_MEMORY_SECS: i64 = TRIP_REPORT_SECS;

fn arbitrage_lots() -> &'static Mutex<Vec<ArbitrageInFlight>> {
    static TABLE: OnceLock<Mutex<Vec<ArbitrageInFlight>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(Vec::new()))
}

fn live_arbitrage() -> Vec<ArbitrageInFlight> {
    let now = unix_now();
    let mut table = arbitrage_lots().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    table.retain(|lot| now - lot.since <= ARBITRAGE_MEMORY_SECS);
    table.clone()
}

/// Pure: files `lot` in `table`, replacing the account's lot of the same
/// object (one bid per object; the caller carries its units over).
pub fn note_in(table: &mut Vec<ArbitrageInFlight>, lot: ArbitrageInFlight) {
    table.retain(|known| known.account != lot.account || known.item != lot.item);
    table.push(lot);
}

/// Pure: the account's lot of `item` now holds `bought` units of its bid;
/// `resting` false once the bid left the book: a lot with nothing bought
/// nor carried is then dropped (no phantom units).
pub fn settle_in(table: &mut Vec<ArbitrageInFlight>, account: &str, item: &str, bought: i64, resting: bool) {
    for lot in table.iter_mut().filter(|lot| lot.account == account && lot.item == item) {
        lot.bought = bought.max(0);
        lot.resting = resting;
    }
    table.retain(|lot| lot.account != account || lot.item != item || lot.resting || lot.units() > 0);
}

/// Pure: the account's lots holding units, as (object, units).
pub fn units_in(table: &[ArbitrageInFlight], account: &str) -> Vec<(String, i64)> {
    table.iter().filter(|lot| lot.account == account && lot.units() > 0).map(|lot| (lot.item.clone(), lot.units())).collect()
}

/// Pure: units `account` bought of the book `item_id` since `since`
/// (store clock), failed deals left out: what its arbitrage bid filled.
pub fn bought_since(deals: &[lifescape_store::Deal], account: &str, item_id: i64, since: i64) -> i64 {
    deals
        .iter()
        .filter(|deal| deal.buyer == account && deal.status != lifescape_store::DealStatus::Failed && crate::market_audit::book_item(deal.item_id) == item_id && deal.created_clock >= since)
        .map(|deal| deal.quantity.max(0))
        .sum()
}

/// Files the account's arbitrage lot (`note_in`).
pub fn note_arbitrage(lot: ArbitrageInFlight) {
    note_in(&mut arbitrage_lots().lock().unwrap_or_else(|poisoned| poisoned.into_inner()), lot);
}

/// Updates the account's lot of `item` (`settle_in`).
pub fn settle_arbitrage(account: &str, item: &str, bought: i64, resting: bool) {
    settle_in(&mut arbitrage_lots().lock().unwrap_or_else(|poisoned| poisoned.into_inner()), account, item, bought, resting);
}

/// The account's lots (live ones).
pub fn own_arbitrage(account: &str) -> Vec<ArbitrageInFlight> {
    live_arbitrage().into_iter().filter(|lot| lot.account == account).collect()
}

/// Units the account bought to sell to a merchant, per object (units
/// really bought, plus its earlier lot still carried), their trip not
/// sent yet: the selling pass sends THOSE units to the shop whatever the
/// overstock rule, on the shop's whole proceeds (`shop_split`; they were
/// priced unit by unit on its likely stock); the rest of the pile follows
/// the normal rule.
pub fn arbitrage_units(account: &str) -> Vec<(String, i64)> {
    units_in(&live_arbitrage(), account)
}

/// Pure: `sales` plus every lot in flight of ANOTHER account than
/// `account`, as units sold there now.
pub fn with_others_arbitrage(mut sales: ShopSales, lots: &[ArbitrageInFlight], account: &str) -> ShopSales {
    for lot in lots.iter().filter(|lot| lot.account != account && lot.units() > 0) {
        sales.entry((lot.shop.clone(), lot.item.clone())).or_default().push((lot.units(), 0));
    }
    sales
}

/// The fleet's recent sales as `account` reads them: the trips sent
/// (`load_fleet_sales`) and the other accounts' arbitrage lots in flight.
pub fn fleet_sales_for(service: &crate::market_service::MarketService, account: &str) -> ShopSales {
    with_others_arbitrage(load_fleet_sales(service), &live_arbitrage(), account)
}

/// What `shop` likely pays for the unit of rank `rank` of a lot of
/// `item`, `ahead` units of the same seller landing before it: the
/// server's stock-based price at its likely stock (`sales`) plus them.
pub fn shop_unit_value(shop: &str, item: &str, sales: &ShopSales, ahead: i64, rank: i64) -> Option<i64> {
    let sold = sales.get(&(shop.to_string(), item.to_string())).map(Vec::as_slice).unwrap_or(&[]);
    let held = lifescape_gamedata::shop_sale::likely_held(shop, item, sold);
    lifescape_gamedata::shop_sale::unit_value_floor(shop, item, held + ahead.max(0) + rank.max(0))
}

/// What `shop` likely pays for each unit of a lot of `item` (rank 0
/// first), `ahead` units landing before it: the values down to the
/// shop's floor (`calc_shop_value` stops falling there), at most
/// `max_units` of them; a rank past the end pays the last value. `None`:
/// the shop does not buy it. A few reads of the game data (shop list,
/// keeper deltas) per object, never one per unit, and no store read (the
/// arbitrage pass weighs every object an open shop buys).
pub fn shop_ladder(shop: &str, item: &str, sales: &ShopSales, ahead: i64, max_units: i64) -> Option<Vec<i64>> {
    let sold = sales.get(&(shop.to_string(), item.to_string())).map(Vec::as_slice).unwrap_or(&[]);
    let held = lifescape_gamedata::shop_sale::likely_held(shop, item, sold) + ahead.max(0);
    let floor = lifescape_gamedata::shop_sale::unit_value_floor(shop, item, held + i64::from(i32::MAX))?;
    let mut values = Vec::new();
    for rank in 0..max_units.max(1) {
        let value = lifescape_gamedata::shop_sale::unit_value_floor(shop, item, held + rank)?;
        values.push(value);
        if value <= floor {
            break;
        }
    }
    Some(values)
}

/// The value of rank `rank` on a `shop_ladder` (the last value past its
/// end, 0 for an empty ladder).
pub fn ladder_value(ladder: &[i64], rank: i64) -> i64 {
    usize::try_from(rank).ok().and_then(|index| ladder.get(index)).or(ladder.last()).copied().unwrap_or(0)
}

/// What one unit of `item` nets a seller sending `units` of them to the
/// best open shop at its likely stock (`sales`), the trip shared over
/// them at `coin_rate` (`merchant_arbitrage::net_outlet`): the market
/// makers' outlet (D2). `None` when no shop nets a coin.
pub fn net_shop_outlet(item: &str, sales: &ShopSales, units: i64, coin_rate: f64) -> Option<i64> {
    let units = units.max(1);
    lifescape_gamedata::shop_sale::npc_sale_shops(item)
        .iter()
        .filter_map(|shop| {
            let last = shop_unit_value(&shop.shop, item, sales, 0, units - 1)?;
            lifescape_planner::merchant_arbitrage::net_outlet(last as f64, lifescape_planner::merchant_arbitrage::SHOP_SELL_TICKS_PER_UNIT, SHOP_TRIP_TICKS, coin_rate, units)
        })
        .max()
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

    /// Live 2026-11-20 (frank_lane, shaw_sara, mila_wade, nina_rhodes:
    /// `market cleanup --sale-shop generalshop7 --sale-site 2902,3150,0`
    /// failed 45 times: `bank_withdraw 995: 432461 locked by open market
    /// orders, nothing usable`, the boat refused without its fare): a
    /// keeper behind the Karamja boat is no sale site while the free
    /// wallet cannot pay the boat out and back; the mainland sites stay.
    #[test]
    fn a_sale_behind_the_boat_needs_its_fare_free() {
        let shop = |name: &str, sites: Vec<(i64, i64, i64)>| SaleShop { shop: name.to_string(), value: 20, sites, quest: None, quest_points: 0 };
        let shops = vec![shop("generalshop7", vec![(2902, 3150, 0)]), shop("generalshop1", vec![(3212, 3246, 0)])];
        let kept: Vec<String> = payable_sale_sites(shops.clone(), 0).into_iter().map(|shop| shop.shop).collect();
        assert_eq!(kept, vec!["generalshop1".to_string()], "coins all locked: Karamja drops");
        let kept: Vec<String> = payable_sale_sites(shops, 2 * lifescape_gamedata::services::BOAT_FARE_GP).into_iter().map(|shop| shop.shop).collect();
        assert_eq!(kept, vec!["generalshop7".to_string(), "generalshop1".to_string()], "the fare free: Karamja stays");
    }

    /// Live 2026-11-22 (pia_brown, owen_wood): the trip is planned at the
    /// listing pass and launched minutes later; a lot the graph needs by
    /// then (a market answer drawn in between) leaves the trip, and a
    /// trip left without a lot is no trip.
    #[test]
    fn a_lot_the_graph_needs_at_launch_leaves_the_trip() {
        let line = |item: &str, qty: i64| SaleLine { item: item.to_string(), qty, min_unit: 4, value: 9, whole: 0 };
        let plan = ShopSalePlan { shop: "taxfree".to_string(), site: (3025, 3701, 0), lines: vec![line("iron_ore", 244), line("oak_logs", 663)] };
        let kept = plan.clone().without_kept(&["iron_ore".to_string()]).expect("oak logs still go");
        assert_eq!(kept.lines, vec![line("oak_logs", 663)]);
        assert_eq!(plan.clone().without_kept(&[]).expect("nothing kept"), plan);
        assert_eq!(plan.without_kept(&["iron_ore".to_string(), "oak_logs".to_string()]), None);
    }

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
        let plan = plan_shop_sale(&candidates, Some((3220, 3218, 0)), 100, &[], &shops).expect("daggers pay the trip");
        assert_eq!(plan.shop, "generalshop1", "Lumbridge is nearer than Falador");
        assert_eq!(plan.lines, vec![SaleLine { item: "iron_dagger".to_string(), qty: 40, min_unit: 1, value: 14, whole: 0 }], "helms fetch 12 on the market: never sold for 9");
        assert_eq!(plan.args()[..4], ["--sale-shop", "generalshop1", "--sale-site", "3212,3246,0"]);
        assert!(plan.args().contains(&"iron_dagger:40:1".to_string()));
        assert_eq!(plan_shop_sale(&candidates, None, 10_000, &[], &shops), None, "a trip that does not pay waits");
        assert_eq!(plan_shop_sale(&[("bronze_med_helm".to_string(), 80, 12)], None, 1, &[], &shops), None);
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
        if let Some(plan) = plan_shop_sale(&candidates, Some((3092, 3243, 0)), 1, &[], &fresh) {
            assert_ne!(plan.shop, "taxfree", "never a trip whose first unit pays under 1212");
            assert!(plan.lines.iter().all(|line| line.value >= line.min_unit), "{plan:?}");
        }
        let later = fleet_sales(&trips, &reports, 1030 + 3000, 600);
        let rested = |item: &str| shops_at_likely_stock(item, &later);
        let plan = plan_shop_sale(&candidates, Some((3092, 3243, 0)), 1, &[], &rested).expect("restocked down: the trip pays again");
        assert_eq!(plan.lines[0].value, 3120, "a base-stock first unit (Horvik pays as much, nearer)");
        assert_eq!(rested("mithril_platebody").into_iter().find(|shop| shop.shop == "taxfree").map(|shop| shop.value), Some(3120), "taxfree back to its base");
        assert_eq!(shops_at_likely_stock("mithril_platebody", &ShopSales::new()).first().map(|shop| shop.value), Some(3120), "no sale: the base stock");
    }

    /// Plan équilibre D2: a merchant arbitrage lot in flight counts at
    /// its shop for every other bot (its units sold now: a second buyer
    /// reads the floor, never the base stock), never for its own buyer
    /// (the units are its trip). The makers' outlet nets the trip.
    #[test]
    fn an_arbitrage_lot_in_flight_counts_for_the_others_only() {
        let shop = lifescape_gamedata::shop_sale::npc_sale_shops("yew_logs").into_iter().next().expect("a shop buys yew logs").shop;
        let lots = vec![ArbitrageInFlight { account: "carl_miller".to_string(), shop: shop.clone(), item: "yew_logs".to_string(), item_id: 1515, carried: 0, bought: 300, since: 1000, resting: false }];
        let others = with_others_arbitrage(ShopSales::new(), &lots, "amber_moore");
        assert_eq!(others.get(&(shop.clone(), "yew_logs".to_string())), Some(&vec![(300, 0)]));
        assert!(with_others_arbitrage(ShopSales::new(), &lots, "carl_miller").is_empty(), "its own lot is its trip");
        let base = shop_unit_value(&shop, "yew_logs", &ShopSales::new(), 0, 0).expect("the shop buys");
        assert!(base > 16, "base stock: {base}");
        assert_eq!(shop_unit_value(&shop, "yew_logs", &others, 0, 0), Some(16), "300 logs ahead: the floor");
        assert_eq!(shop_unit_value(&shop, "yew_logs", &ShopSales::new(), 300, 0), Some(16), "its own units ahead too");
        assert_eq!(net_shop_outlet("yew_logs", &others, 500, 0.28), Some(15), "16 less the trip shared over 500 logs");
        assert_eq!(net_shop_outlet("yew_logs", &others, 500, 20.0), None, "a dear trip nets nothing");
    }

    /// Review lot 5: a lot counts the units its bid really bought, never
    /// the units it asked for. A bid that buys nothing and is withdrawn
    /// leaves no lot (no phantom stock at the shop for the others, no
    /// overstock exemption for the buyer); a partly filled one keeps what
    /// it bought; units carried from the earlier lot count.
    #[test]
    fn an_arbitrage_lot_counts_only_what_its_bid_bought() {
        let lot = |account: &str, carried: i64| ArbitrageInFlight { account: account.to_string(), shop: "generalshop1".to_string(), item: "yew_logs".to_string(), item_id: 1515, carried, bought: 0, since: 1000, resting: true };
        let key = ("generalshop1".to_string(), "yew_logs".to_string());
        let mut table = Vec::new();
        note_in(&mut table, lot("carl_miller", 0));
        assert!(with_others_arbitrage(ShopSales::new(), &table, "amber_moore").is_empty(), "1000 asked, nothing bought yet: nothing at the shop");
        assert!(units_in(&table, "carl_miller").is_empty(), "no overstock exemption for an unfilled bid");
        settle_in(&mut table, "carl_miller", "yew_logs", 300, true);
        assert_eq!(with_others_arbitrage(ShopSales::new(), &table, "amber_moore").get(&key), Some(&vec![(300, 0)]));
        assert_eq!(units_in(&table, "carl_miller"), vec![("yew_logs".to_string(), 300)]);
        settle_in(&mut table, "carl_miller", "yew_logs", 300, false);
        assert_eq!(table.len(), 1, "withdrawn after 300 bought: the 300 stay in flight");
        note_in(&mut table, lot("wolfe_carl", 0));
        settle_in(&mut table, "wolfe_carl", "yew_logs", 0, false);
        assert!(table.iter().all(|lot| lot.account != "wolfe_carl"), "withdrawn with nothing bought: dropped");
        note_in(&mut table, lot("carl_miller", 300));
        assert_eq!(table.len(), 1, "one lot per object: the new bid replaces it, the 300 carried");
        assert_eq!(with_others_arbitrage(ShopSales::new(), &table, "amber_moore").get(&key), Some(&vec![(300, 0)]));
        let deal = |buyer: &str, item_id: i64, quantity: i64, created_clock: i64| lifescape_store::Deal {
            deal_id: 1,
            buyer: buyer.to_string(),
            seller: "woodcutter".to_string(),
            item_id,
            quantity,
            price: 1,
            meeting_x: 0,
            meeting_y: 0,
            meeting_level: 0,
            status: lifescape_store::DealStatus::Matched,
            fail_reason: String::new(),
            deadline_tick: 0,
            created_clock,
            updated_clock: created_clock,
            taker_side: None,
        };
        let deals = vec![deal("carl_miller", 1515, 200, 999), deal("carl_miller", 1515, 120, 1000), deal("carl_miller", 1515, 80, 1200), deal("carl_miller", 1513, 50, 1200), deal("amber_moore", 1515, 70, 1200)];
        assert_eq!(bought_since(&deals, "carl_miller", 1515, 1000), 200, "its own buys of that object since the bid");
    }

    /// Review lot 5: an arbitrage lot was priced on what the shop pays for
    /// all of it, not on its gain over the market: a maker bids 14, the
    /// shop pays 16, the 200 logs still pay the trip (3 200 coins against
    /// 420) and never wait in the bank; a lot of plain surplus still needs
    /// its gain over the market (400 < 420).
    #[test]
    fn an_arbitrage_lot_pays_its_trip_on_its_whole_proceeds() {
        let shops = |item: &str| match item {
            "yew_logs" => vec![SaleShop { shop: "generalshop1".to_string(), value: 16, sites: vec![(3212, 3246, 0)], ..SaleShop::default() }],
            _ => Vec::new(),
        };
        let candidates = vec![("yew_logs".to_string(), 200, 14)];
        assert_eq!(plan_shop_sale(&candidates, None, 420, &[], &shops), None, "surplus: 2 a log over the market, 400 < 420");
        let plan = plan_shop_sale(&candidates, None, 420, &[("yew_logs".to_string(), 200)], &shops).expect("bought for the shop: it goes");
        assert_eq!(plan.gain(), 3200);
        assert_eq!(plan.lines[0].min_unit, 15, "never a log under what the market pays + 1");
    }

    /// Re-review lot 5: only the units the arbitrage BOUGHT escape the
    /// overstock rule and count their whole proceeds. A woodcutter holds
    /// 5000 logs it cut, its arbitrage bid bought 50 at 1: those 50 go to
    /// the shop whatever the overstock, the 4950 others follow the normal
    /// rule (overstocked: the market); a pile the trip carries counts 50 x
    /// 16 + 4950 x its gain over the market, never 5000 x 16.
    #[test]
    fn only_the_units_bought_for_the_shop_escape_the_surplus_rules() {
        assert_eq!(shop_split(5000, 50, true, true), ShopSplit { to_shop: 50, whole: 50, to_market: 4950 }, "the 4950 cut logs are overstock");
        assert_eq!(shop_split(5000, 50, false, true), ShopSplit { to_shop: 5000, whole: 50, to_market: 0 }, "not overstocked: all go, 50 of them whole");
        assert_eq!(shop_split(5000, 0, true, true), ShopSplit { to_shop: 0, whole: 0, to_market: 5000 }, "nothing bought: the normal rule");
        assert_eq!(shop_split(30, 50, true, true), ShopSplit { to_shop: 30, whole: 30, to_market: 0 }, "never more than the pile");
        assert_eq!(shop_split(5000, 50, true, false), ShopSplit { to_shop: 0, whole: 0, to_market: 5000 }, "the market pays more: the market");
        let shops = |item: &str| match item {
            "yew_logs" => vec![SaleShop { shop: "generalshop1".to_string(), value: 16, sites: vec![(3212, 3246, 0)], ..SaleShop::default() }],
            _ => Vec::new(),
        };
        let pile = vec![("yew_logs".to_string(), 5000, 14)];
        let plan = plan_shop_sale(&pile, None, 420, &[("yew_logs".to_string(), 50)], &shops).expect("a trip");
        assert_eq!(plan.gain(), 50 * 16 + 4950 * 2, "never 5000 x 16");
        assert_eq!(plan.lines[0].whole, 50);
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
