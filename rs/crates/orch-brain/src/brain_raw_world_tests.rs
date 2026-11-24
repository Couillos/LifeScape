//! The raw world of the equivalence benches (analysis 5.3, rule 2): a `Job`
//! with a live JSON state, a test market service (its own sqlite file, on
//! tmpfs when there is one) installed as this thread's `market_service::
//! shared()`, and the fleet's crowd table. The benches read it through the
//! SAME functions as production (`gather`, `gather_market`, `ready_ctx`,
//! `build_ctx`, `LiveUpkeepFx::clock` ...) and compare the old and the new
//! decision from what those return: no input field is filled by a test.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lifescape_json::{object, str_value, Value};
use lifescape_orch_market::market_service::MarketService;
use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::{LaunchInputs, Life};

use crate::brain_launch_pass::LaunchPass;
use crate::fleet::Fleet;
use crate::jobs::Job;

/// A fleet that is never asked anything (the readers under test only read
/// the job).
pub(crate) struct IdleFleet {
    jobs: Mutex<HashMap<String, Arc<Job>>>,
}

impl IdleFleet {
    pub(crate) fn shared() -> Arc<dyn Fleet> {
        Arc::new(IdleFleet { jobs: Mutex::new(HashMap::new()) })
    }
}

impl Fleet for IdleFleet {
    fn jobs(&self) -> &Mutex<HashMap<String, Arc<Job>>> {
        &self.jobs
    }
    fn is_running(&self) -> bool {
        true
    }
    fn proc_alive(&self, _account: &str) -> bool {
        false
    }
    fn send_worker(&self, _account: &str, _request: &Value, _timeout: Duration) -> Result<Value, String> {
        Err("idle fleet".to_string())
    }
    fn run_job(&self, _account: &str, _script: &str, _args: &[Value]) -> Value {
        Value::Null
    }
    fn open_job(self: Arc<Self>, _account: &str, _no_login: bool) -> Value {
        Value::Null
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Unix seconds now.
pub(crate) fn wall_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
}

/// The least heartbeat (display) experience of base level `level`, by the
/// SDK table the brain reads (`brain_market::live_levels`).
pub(crate) fn xp_of_level(level: u8) -> i64 {
    let (mut low, mut high) = (0i32, 300_000_000i32);
    while low < high {
        let middle = low + (high - low) / 2;
        if lifescape_sdk::snapshot::level_for_xp(middle) >= level {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    i64::from(low)
}

/// One test market service, its database deleted on drop. Never opened on
/// the live `run/` (the `test-isolation` paths, and a path of its own).
pub(crate) struct TestMarket {
    pub service: Arc<MarketService>,
    path: PathBuf,
}

impl TestMarket {
    pub(crate) fn open(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let stamp = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = if std::path::Path::new("/dev/shm").is_dir() { PathBuf::from("/dev/shm") } else { std::env::temp_dir() };
        let path = base.join(format!("lifescape-raw-world-{}-{tag}-{stamp}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let service = Arc::new(MarketService::open(&path, lifescape_orch_market::market_seed::catalogue()).expect("test market opens"));
        TestMarket { service, path }
    }
}

impl Drop for TestMarket {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut name = self.path.clone().into_os_string();
            name.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(name));
        }
    }
}

/// What the world holds, as the bench draws it.
#[derive(Default)]
pub(crate) struct Seed {
    pub coins: i64,
    /// `(object id, units)` carried or banked (what the market ledger mirrors).
    pub holdings: Vec<(i64, i64)>,
    /// `(object id, units, price)` resting asks of OTHER accounts.
    pub asks: Vec<(i64, i64, i64)>,
    /// `(object id, units, price)` the bot lists.
    pub sells: Vec<(i64, i64, i64)>,
    /// `(object id, units, price)` the bot bids (below every ask).
    pub buys: Vec<(i64, i64, i64)>,
    /// Other bots working `(site, method)`: the fleet's crowd.
    pub crowd: Vec<((i64, i64, i64), String)>,
    /// More orders of the bot, with a motive and a fate: `(buy, object id,
    /// units, price, market-make motive, still live)`. A dead one is
    /// cancelled after it rested (the store keeps the row).
    pub orders: Vec<(bool, i64, i64, i64, bool, bool)>,
}

pub(crate) struct RawWorld<'a> {
    pub job: Arc<Job>,
    pub live: Arc<Value>,
    pub orch: Arc<dyn Fleet>,
    pub market: Option<&'a TestMarket>,
}

fn place(service: &MarketService, account: &str, side: &str, item: i64, qty: i64, price: i64) {
    place_with(service, account, side, item, qty, price, "");
}

fn place_with(service: &MarketService, account: &str, side: &str, item: i64, qty: i64, price: i64, motive: &str) {
    let mut request = vec![
        ("op", str_value("place")),
        ("account", str_value(account)),
        ("side", str_value(side)),
        ("item", Value::Int(item)),
        ("qty", Value::Int(qty)),
        ("price", Value::Int(price)),
    ];
    if !motive.is_empty() {
        request.push(("motive", str_value(motive)));
    }
    let _ = service.handle(&object(request));
}

impl<'a> RawWorld<'a> {
    pub(crate) fn new(account: &str, live: Value, market: Option<&'a TestMarket>) -> Self {
        let job = Arc::new(Job::new(account));
        let live = Arc::new(live);
        job.inner.lock().expect("job").live = Some(live.clone());
        RawWorld { job, live, orch: IdleFleet::shared(), market }
    }

    /// Runs `body` with this world's market as the thread's `shared()`.
    pub(crate) fn run<R>(&self, body: impl FnOnce() -> R) -> R {
        match self.market {
            Some(market) => lifescape_orch_market::market_daemon::test_service::with_service(market.service.clone(), body),
            None => body(),
        }
    }

    fn seller(&self) -> String {
        format!("{}_seller", self.job.account)
    }

    /// Fills the market (ledger and orders) and the fleet's crowd table.
    pub(crate) fn seed(&self, seed: &Seed) {
        for (index, (site, method)) in seed.crowd.iter().enumerate() {
            crate::site_crowd::note(&format!("{}_crowd{index}", self.job.account), Some(*site), method);
        }
        let Some(market) = self.market else { return };
        let service = &market.service;
        let (account, seller) = (self.job.account.clone(), self.seller());
        let ask_stock: Vec<(i64, i64)> = seed.asks.iter().map(|(item, qty, _)| (*item, *qty)).collect();
        service.sync_holdings(&seller, 0, &ask_stock);
        for (item, qty, price) in &seed.asks {
            place(service, &seller, "sell", *item, *qty, *price);
        }
        service.sync_holdings(&account, seed.coins, &seed.holdings);
        for (item, qty, price) in &seed.sells {
            place(service, &account, "sell", *item, *qty, *price);
        }
        for (item, qty, price) in &seed.buys {
            place(service, &account, "buy", *item, *qty, *price);
        }
        for (buy, item, qty, price, make, live) in &seed.orders {
            let side = if *buy { "buy" } else { "sell" };
            place_with(service, &account, side, *item, *qty, *price, if *make { lifescape_orch_market::market_lineage::MARKET_MAKE } else { "" });
            if !live {
                let side = if *buy { lifescape_store::OrderSide::Buy } else { lifescape_store::OrderSide::Sell };
                service.cancel_open_because(&account, *item, side, "bench");
            }
        }
    }

    /// Takes the world's orders and crowd back out (the service is shared
    /// by the scenarios of one life).
    pub(crate) fn clear(&self, seed: &Seed) {
        for index in 0..seed.crowd.len() {
            crate::site_crowd::note(&format!("{}_crowd{index}", self.job.account), None, "");
        }
        if let Some(market) = self.market {
            for account in [self.job.account.clone(), self.seller()] {
                let _ = market.service.retire_account(&account, "bench");
            }
        }
    }

    /// The launch pass's reads up to the resolve context, through the
    /// production functions: `gather` at the start, `build_ctx` (which runs
    /// `ready_ctx`, `gather_market` and the publishers) at its point.
    pub(crate) fn launch_read(&self, life: &Life, position: usize) -> (LaunchInputs, ReadyCtx, i64) {
        self.run(|| {
            let inputs = crate::brain_launch_inputs::gather(&self.job, &self.live);
            let quests_done = crate::brain_rescue::quests_done(life);
            let mut pass = LaunchPass { orch: &self.orch, job: &self.job, life: life.clone(), live: self.live.clone(), inputs, dirty: false, quoting: false, waiting: Vec::new() };
            let picked = crate::brain_launch_node::build_ctx(&mut pass, position, quests_done);
            (pass.inputs, picked.ctx, picked.committed)
        })
    }
}

/// A live pack or bank row, as the heartbeat writes it.
pub(crate) fn stack_value(name: &str, count: i64) -> Option<Value> {
    let obj = lifescape_gamedata::objs_by_name(name).into_iter().next()?;
    Some(object(vec![("id", Value::Int(obj.obj_id as i64)), ("name", str_value(obj.display.unwrap_or(name))), ("count", Value::Int(count))]))
}

/// Object id of `name` (first dump row of that name).
pub(crate) fn object_id(name: &str) -> Option<i64> {
    lifescape_gamedata::objs_by_name(name).into_iter().next().map(|obj| obj.obj_id as i64)
}
