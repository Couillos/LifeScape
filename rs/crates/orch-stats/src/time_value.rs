#![forbid(unsafe_code)]
//! Value of a bot's time = what its work REALLY earned (plan équilibre
//! step 1, decision D1, 2026-11-19). Before, `brain_d17::coin_rate` read
//! the bot's D17, the best theoretical money-guide row within its reach
//! (chains it never ran included): 1.1 to 10.7 coins per tick for ~0.25
//! really created, every reader overpriced the bot's time.
//!
//! - Measure: the contract-2 task reports created in the last
//!   `WINDOW_SECS` (`Store::work_flows_since`): per account, its work
//!   ticks (pauses excluded) and its flows netted per item.
//! - Value: every item at its current price (`rates_price::
//!   price_estimate`, the one current-price reader; coins at 1): products,
//!   by-products, loot and coins in, inputs, consumables, fees, deaths
//!   and discards out; an NPC sale counts its coins minus the item at its
//!   current price. `UNVALUED_CAUSES` never count: a player trade moves
//!   wealth without creating it (a gift of coins would inflate the
//!   receiver), an unattributed flow is an alert nothing explains.
//! - Rate: value / work ticks of the account when it worked at least
//!   `MIN_MEASURED_WORK_TICKS`; otherwise the fleet's measured mean (its
//!   value over its work ticks, staff and excluded accounts left out,
//!   AGENTS.md rule 11); nothing measured anywhere: the floor. Never under
//!   `market_fit::MIN_COIN_RATE` (a tick is never free).
//! - Window: 12 h. A run lasts minutes, but the sale of its produce (bank
//!   cleanups sold to shops, market fills) comes in waves of hours: over
//!   3 to 6 h one shop sale moves an account threefold (live 2026-11-19,
//!   ella_quill -0.09 to 0.28 by window); over 24 h the rate remembers a
//!   level or an activity left long ago. 12 h holds ~190 reports per
//!   account and is the plan's audit window (fleet 0.28 coins/tick on the
//!   2026-11-19 copy, 138 ms to build).
//! - Refresh: at most every `REFRESH` (the guides' cadence: prices move
//!   on minutes), by the first reader past the deadline; concurrent
//!   readers wait for that build rather than run their own. The build
//!   takes the market store's lock (two queries through the reports'
//!   clock index, one price read per item): no caller may hold that lock
//!   while it reads a rate.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use lifescape_planner::market_fit::MIN_COIN_RATE;
use lifescape_store::{AccountWork, Store};

/// Window of the measure (seconds of store clock, module doc).
pub const WINDOW_SECS: i64 = 12 * 3600;

/// Work ticks under which an account has no measure of its own (one
/// guide window: under it, one purchase or cleanup run decides).
pub const MIN_MEASURED_WORK_TICKS: i64 = 6000;

/// Flow causes that are not the bot's own gain (module doc).
pub const UNVALUED_CAUSES: [&str; 2] = [crate::flow_causes::cause::TRADED, crate::flow_causes::cause::UNATTRIBUTED];

/// Period between two measures (module doc).
pub const REFRESH: Duration = Duration::from_secs(300);

/// Coins valued and work ticks of a measure.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measured {
    pub value: f64,
    pub work_ticks: i64,
}

impl Measured {
    /// Coins per tick of work.
    pub fn per_tick(&self) -> f64 {
        self.value / self.work_ticks.max(1) as f64
    }
}

/// Where a bot's rate comes from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RateSource {
    /// Its own measure.
    Own(Measured),
    /// No measure of its own: the fleet's.
    Fleet(Measured),
    /// Nothing measured anywhere.
    Floor,
}

/// Every account's measure and the fleet's (module doc).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TimeValues {
    /// By lowercase account name.
    accounts: BTreeMap<String, Measured>,
    fleet: Option<Measured>,
}

impl TimeValues {
    /// Measures of `works` valued at `price` (coins per unit of an item
    /// display name); `fleet_member` tells the accounts the fleet's mean
    /// counts.
    pub fn from_work(works: &[AccountWork], price: &mut dyn FnMut(&str) -> f64, fleet_member: &dyn Fn(&str) -> bool) -> TimeValues {
        let mut accounts: BTreeMap<String, Measured> = BTreeMap::new();
        let mut fleet = Measured { value: 0.0, work_ticks: 0 };
        for work in works {
            let value: f64 = work.flows.iter().map(|(item, quantity)| *quantity as f64 * price(item)).sum();
            let own = accounts.entry(work.account.to_lowercase()).or_insert(Measured { value: 0.0, work_ticks: 0 });
            own.value += value;
            own.work_ticks += work.work_ticks;
            if fleet_member(&work.account) {
                fleet.value += value;
                fleet.work_ticks += work.work_ticks;
            }
        }
        TimeValues { accounts, fleet: (fleet.work_ticks >= MIN_MEASURED_WORK_TICKS).then_some(fleet) }
    }

    /// Where `account`'s rate comes from (module doc).
    pub fn source(&self, account: &str) -> RateSource {
        let own = self.accounts.get(&account.to_lowercase()).filter(|measured| measured.work_ticks >= MIN_MEASURED_WORK_TICKS);
        match (own, self.fleet) {
            (Some(measured), _) => RateSource::Own(*measured),
            (None, Some(fleet)) => RateSource::Fleet(fleet),
            (None, None) => RateSource::Floor,
        }
    }

    /// Coins one tick of `account`'s time is worth (module doc).
    pub fn rate(&self, account: &str) -> f64 {
        rate_of_source(self.source(account))
    }

    /// The fleet's measure, `None` when it worked too little.
    pub fn fleet(&self) -> Option<Measured> {
        self.fleet
    }

    /// Accounts with a measure of their own.
    pub fn measured_accounts(&self) -> usize {
        self.accounts.values().filter(|measured| measured.work_ticks >= MIN_MEASURED_WORK_TICKS).count()
    }
}

/// Coins per tick of a source, never under the floor.
pub fn rate_of_source(source: RateSource) -> f64 {
    match source {
        RateSource::Own(measured) | RateSource::Fleet(measured) => measured.per_tick().max(MIN_COIN_RATE),
        RateSource::Floor => MIN_COIN_RATE,
    }
}

/// The measure of `store` at store clock `now` (module doc): the window's
/// work, each item valued once at its current price, the fleet without
/// the excluded accounts. Live: the wall clock; an offline copy: its
/// newest report (`Store::newest_report_clock`).
pub fn measure_in(store: &Store, now: i64) -> Result<TimeValues, lifescape_store::Error> {
    let works = store.work_flows_since(now - WINDOW_SECS, &UNVALUED_CAUSES)?;
    let mut prices: HashMap<String, f64> = HashMap::new();
    let mut price = |item: &str| *prices.entry(item.to_string()).or_insert_with(|| crate::rates_price::price_estimate_sql(store, item, crate::market_service::MarketService::now_clock()).0);
    let excluded = crate::economy_guard::excluded_accounts();
    let fleet_member = |account: &str| !excluded.contains(&account.to_lowercase());
    Ok(TimeValues::from_work(&works, &mut price, &fleet_member))
}

struct Cache {
    values: Option<Arc<TimeValues>>,
    built: Option<Instant>,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Cache { values: None, built: None }))
}

/// Held while one reader builds the measure: the others wait for it.
fn build_lock() -> &'static Mutex<()> {
    static BUILD: OnceLock<Mutex<()>> = OnceLock::new();
    BUILD.get_or_init(|| Mutex::new(()))
}

/// The cached measure while it is younger than `REFRESH` (`Some(None)`:
/// the last build failed, nothing measured).
fn fresh() -> Option<Option<Arc<TimeValues>>> {
    let cached = cache().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    cached.built.filter(|built| built.elapsed() < REFRESH).map(|_| cached.values.clone())
}

/// The live measure (module doc), rebuilt past `REFRESH` from the market
/// service's store; `None` without a market service or after a store
/// error (logged): nothing measured.
pub fn current() -> Option<Arc<TimeValues>> {
    if let Some(found) = fresh() {
        return found;
    }
    let _building = build_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(found) = fresh() {
        return found;
    }
    let service = crate::market_service::shared()?;
    let started = Instant::now();
    let built = {
        let store = service.lock_store();
        measure_in(&store, crate::market_service::MarketService::now_clock())
    };
    let values = match built {
        Ok(values) => {
            let fleet = values.fleet().map(|fleet| format!("{:.3} coins/tick ({:.0} coins over {} work ticks)", fleet.per_tick(), fleet.value, fleet.work_ticks)).unwrap_or_else(|| "unmeasured".to_string());
            crate::logln!("time value: {} accounts measured over {} h, fleet {fleet}, built in {} ms", values.measured_accounts(), WINDOW_SECS / 3600, started.elapsed().as_millis());
            Some(Arc::new(values))
        }
        Err(error) => {
            crate::logln!("time value: store refused the measure ({error}): every bot at the floor until the next try in {} s", REFRESH.as_secs());
            None
        }
    };
    let mut cached = cache().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    cached.values = values.clone();
    cached.built = Some(Instant::now());
    values
}

/// Coins one tick of `account`'s time is worth in the live measure
/// (module doc): the floor when nothing is measured.
pub fn rate(account: &str) -> f64 {
    current().map(|values| values.rate(account)).unwrap_or(MIN_COIN_RATE)
}

#[cfg(test)]
#[path = "time_value_tests.rs"]
mod tests;
