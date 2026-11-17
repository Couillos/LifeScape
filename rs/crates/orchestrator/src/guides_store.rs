//! Precomputed guides (plan rendements R4/R7, user 2026-11-17): no
//! request ever builds a guide. A dedicated daemon thread (`guides`)
//! builds one `GuideSet` per cycle in a single engine (every yield,
//! quote, source and D17 price shared by all its views) and publishes
//! it as an `Arc`; the HTTP API (`/api/guides/*`) and the brain (R7,
//! `current`) read the last one with its age, filters applied by
//! reading (`lifescape_planner::guide_read`), never by rebuilding.
//!
//! A set holds, for a bot with no quest done and nothing held:
//! - the catalogue views (money, training: each method at its gate);
//! - one money and one training view per level band (1-5 ... 96-99),
//!   built at the band's LAST level (`band_end`): a bot of the band sees
//!   every activity its level opens; a lower level of the band reads
//!   the same rates (exact at 5, 10, ..., 95, 99), gated activities left
//!   out and accessibility at its own level;
//! - the quotes the cycle read (a recipe re-evaluated on a bot's stock,
//!   `held`, reprices its rows on them: a local recompute of the recipe
//!   rows only, never a build).
//!
//! Cadence `REFRESH_PERIOD` (300 s): its inputs move on a scale of
//! minutes (the price is the median of the last five fills of an
//! object, a yield needs three runs of a key, a run lasts minutes) and
//! a bot changes activity on a goal change (minutes to hours); a cycle
//! costs `cycle_cpu_ms` of one core (measured each cycle, logged, served
//! with the set): ~3.2 s on the live copy of 2026-11-17, ~1 % of a core
//! at 300 s; the set holds ~60 MB (docs/resources.md). The
//! store's waits are tallied incrementally (only new reports read), with
//! a full recount every `FULL_RECOUNT_CYCLES` (a purge of the store).

#![forbid(unsafe_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use lifescape_planner::guide_model::{band_of, GuideFilter, GuideWorld, MoneyRow, Quote, RateAsk, TrainingRow, Unrated};
use lifescape_planner::guide_read::{finish_money, finish_training, finish_training_with, MoneyView, TrainingView};
use lifescape_store::Store;

/// Period between two builds of the set (module docs).
pub const REFRESH_PERIOD: Duration = Duration::from_secs(300);
/// Delay of the first build after the daemon starts (after the restore
/// spike of the workers' re-adoption).
pub const START_DELAY: Duration = Duration::from_secs(20);
/// Sleep between two checks of the daemon's stop flag while waiting for
/// the next cycle (one wake-up per 5 s at rest).
pub const IDLE_POLL: Duration = Duration::from_secs(5);
/// Cycles between two full recounts of the store's waits.
pub const FULL_RECOUNT_CYCLES: u32 = 12;

/// Last level of the band of `level` (the level its views are built at).
pub fn band_end(level: u8) -> u8 {
    (band_of(level) + 4).min(99) as u8
}

/// Levels the band views are built at: 5, 10, ..., 95, 99.
pub fn band_levels() -> Vec<u8> {
    let mut out: Vec<u8> = (1..=99u8).map(band_end).collect();
    out.dedup();
    out
}

/// Market pauses and quest durations of the store, tallied report by
/// report in report order (`web_guides::store_waits` is a full tally).
#[derive(Debug, Clone, Default)]
pub struct WaitsTally {
    after: i64,
    paused: f64,
    pauses: usize,
    quests: HashMap<String, (f64, usize)>,
}

impl WaitsTally {
    /// Reads the reports after the last one tallied.
    pub fn advance(&mut self, store: &Store) -> Result<(), String> {
        loop {
            let page = store.measures_after(self.after, crate::web_guides::MEASURE_PAGE).map_err(|error| format!("guides measures: {error}"))?;
            let Some(last) = page.last() else { return Ok(()) };
            self.after = last.report_id;
            for stored in &page {
                if stored.measure.pause_ticks > 0 {
                    self.paused += stored.measure.pause_ticks as f64;
                    self.pauses += 1;
                }
                if let Some(quest) = stored.measure.planner_method.strip_prefix("quest:") {
                    if stored.stop == "goal_reached" && stored.measure.work_ticks > 0 {
                        let entry = self.quests.entry(quest.to_string()).or_insert((0.0, 0));
                        entry.0 += (stored.measure.work_ticks + stored.measure.pause_ticks.max(0)) as f64;
                        entry.1 += 1;
                    }
                }
            }
        }
    }

    /// The waits tallied so far.
    pub fn waits(&self) -> crate::web_guides::StoreWaits {
        crate::web_guides::StoreWaits {
            market_wait: (self.pauses > 0).then(|| self.paused / self.pauses as f64),
            quests: self.quests.iter().map(|(key, (ticks, runs))| (key.clone(), ticks / *runs as f64)).collect(),
        }
    }
}

/// One published build of every guide view.
pub struct GuideSet {
    /// Wall clock of the build's end, Unix milliseconds.
    pub built_unix_ms: i64,
    pub built: Instant,
    /// Wall time and CPU time (this thread) of the cycle, milliseconds.
    pub cycle_ms: u64,
    pub cycle_cpu_ms: Option<u64>,
    pub market_wait_ticks: Option<f64>,
    pub money_catalog: MoneyView,
    pub training_catalog: TrainingView,
    /// (band end, view), ascending.
    pub money_bands: Vec<(u8, MoneyView)>,
    pub training_bands: Vec<(u8, TrainingView)>,
    /// Quotes the cycle read, by object (debug name).
    pub quotes: HashMap<String, Quote>,
    /// Engine events and world calls of the cycle (`web_guides`).
    pub stats: lifescape_json::Value,
}

/// CPU time of the calling thread in milliseconds (Linux
/// `/proc/thread-self/stat`, utime + stime at 100 ticks a second);
/// `None` elsewhere.
pub fn thread_cpu_ms() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/thread-self/stat").ok()?;
    let after = text.rsplit_once(')')?.1;
    let fields: Vec<&str> = after.split_whitespace().collect();
    // Fields after the command: state is 3rd overall, utime 14th, stime 15th.
    let user: u64 = fields.get(11)?.parse().ok()?;
    let system: u64 = fields.get(12)?.parse().ok()?;
    Some((user + system) * 10)
}

/// Builds every view from `store` (prices) and `snapshot` (yields).
pub fn build_set(store: &Store, snapshot: &crate::rates_service::Snapshot, waits: &crate::web_guides::StoreWaits) -> GuideSet {
    let started = Instant::now();
    let cpu_before = thread_cpu_ms();
    let measured = crate::web_guides::measured_keys(snapshot);
    let now = crate::rates_service::now_secs();
    let prices: RefCell<HashMap<String, Quote>> = RefCell::new(HashMap::new());
    let rate = |ask: &RateAsk| crate::web_guides::answer_of(snapshot, ask, now);
    let price = |item: &str| {
        if let Some(known) = prices.borrow().get(item) {
            return known.clone();
        }
        let quote = crate::web_guides::quote_of(store, item);
        prices.borrow_mut().insert(item.to_string(), quote.clone());
        quote
    };
    let npc_sale = |item: &str| crate::web_guides::npc_volume_value(item);
    let shop_buy = |item: &str| crate::web_guides::shop_window(item);
    let quest_ticks = |key: &str| waits.quests.get(key).copied();
    let world = GuideWorld { rate: &rate, price: &price, npc_sale: &npc_sale, shop_buy: &shop_buy, market_wait_ticks: waits.market_wait, quest_ticks: &quest_ticks, measured: &measured };
    let engine = lifescape_planner::guide_engine::Engine::new(&world);
    let mut money_bands = Vec::new();
    let mut training_bands = Vec::new();
    // Level by level, one level's yields in the memo at a time (bounded
    // memory, each yield read once per level): every D17 price the
    // views ask (the catalogue's chains at each gate, the training
    // catalogue at each band's first level), then the band views at the
    // band's last level; the catalogues then read their own rows only.
    let bands = band_levels();
    let mut levels: std::collections::BTreeSet<u8> = bands.iter().copied().collect();
    levels.extend(bands.iter().map(|end| band_of(*end).clamp(1, 99) as u8));
    levels.extend(lifescape_planner::guide_money::entries(&engine).iter().map(|entry| lifescape_planner::guide_methods::gate_level(&entry.method)));
    for level in levels {
        // A level whose views are built reads all its yields once; a
        // level only priced (D17) reads those its pass needs.
        if bands.contains(&level) || band_of(level) == i64::from(level) {
            lifescape_planner::guide_money::read_level(&engine, level);
        }
        let _ = lifescape_planner::guide_money::best_profit_outside_chains(&engine, level, &[]);
        if band_of(level) == i64::from(level) {
            let _ = lifescape_planner::guide_money::best_accessible_profit(&engine, level, &[]);
        }
        if bands.contains(&level) {
            let money = lifescape_planner::guide_money::money_view(&engine, Some(level), &[], &[]);
            lifescape_planner::guide_money::seed_best_accessible_profit(&engine, &money, &[]);
            training_bands.push((level, lifescape_planner::guide_training::training_view(&engine, Some(level), &[])));
            money_bands.push((level, money));
        }
        engine.forget_answers();
    }
    let money_catalog = lifescape_planner::guide_money::money_view(&engine, None, &[], &[]);
    let training_catalog = lifescape_planner::guide_training::training_view(&engine, None, &[]);
    let stats = crate::web_guides::engine_stats(&engine);
    drop(engine);
    let cycle_cpu_ms = match (cpu_before, thread_cpu_ms()) {
        (Some(before), Some(after)) => Some(after.saturating_sub(before)),
        _ => None,
    };
    GuideSet {
        built_unix_ms: (crate::rates_service::now_secs() * 1000),
        built: Instant::now(),
        cycle_ms: started.elapsed().as_millis() as u64,
        cycle_cpu_ms,
        market_wait_ticks: waits.market_wait,
        money_catalog,
        training_catalog,
        money_bands,
        training_bands,
        quotes: prices.into_inner(),
        stats,
    }
}

impl GuideSet {
    /// The money view a filter reads: its level's band, else the catalogue.
    pub fn money_view(&self, level: Option<u8>) -> &MoneyView {
        match level {
            Some(level) => self.money_bands.iter().find(|(end, _)| *end == band_end(level)).map(|(_, view)| view).unwrap_or(&self.money_catalog),
            None => &self.money_catalog,
        }
    }

    /// The training view a filter reads.
    pub fn training_view(&self, level: Option<u8>) -> &TrainingView {
        match level {
            Some(level) => self.training_bands.iter().find(|(end, _)| *end == band_end(level)).map(|(_, view)| view).unwrap_or(&self.training_catalog),
            None => &self.training_catalog,
        }
    }

    /// Age of the set.
    pub fn age(&self) -> Duration {
        self.built.elapsed()
    }

    /// The money guide of `filter`: the view of its level read with it;
    /// `held` re-evaluates the view's recipe rows on that stock (their
    /// own rows only, on the cycle's quotes).
    pub fn money(&self, filter: &GuideFilter) -> Result<(Vec<MoneyRow>, Vec<Unrated>), String> {
        let view = self.money_view(filter.level);
        let replaced = if filter.held.is_empty() { HashMap::new() } else { self.recipes_on_stock(view, &filter.held, &|_| true)? };
        Ok(finish_money(view, filter, &replaced))
    }

    /// The training guide of `filter`: the view of its level read with
    /// it; the D17 price re-read from the money view of the same bot when
    /// it differs from the view's own (quests done, a lower level); with
    /// `held`, the recipes of the money view on that stock read as
    /// training lines (`stock_training`, R7).
    pub fn training(&self, filter: &GuideFilter) -> (Vec<TrainingRow>, Vec<Unrated>) {
        let view = self.training_view(filter.level);
        let own_d17 = filter.quests_done.is_empty() && filter.level == view.level;
        if own_d17 && filter.held.is_empty() {
            return finish_training(view, filter, None);
        }
        let memo: RefCell<HashMap<i64, Option<f64>>> = RefCell::new(HashMap::new());
        let d17 = |band: i64| -> Option<f64> {
            if let Some(known) = memo.borrow().get(&band) {
                return *known;
            }
            // The bot's level, else the band's first (the catalogue's).
            let level = filter.level.unwrap_or(band.clamp(1, 99) as u8);
            let bot = GuideFilter { level: Some(level), quests_done: filter.quests_done.clone(), ..GuideFilter::default() };
            let found = lifescape_planner::guide_read::best_profit_in(self.money_view(Some(level)), &bot);
            memo.borrow_mut().insert(band, found);
            found
        };
        let extra = self.stock_training(filter, &d17);
        if own_d17 {
            finish_training_with(view, filter, None, extra)
        } else {
            finish_training_with(view, filter, Some(&d17), extra)
        }
    }

    /// Training lines on the reader's own stock (R7, decision 2026-11-17
    /// (a): the alchemy of the bot's own objects, cooking its own catch):
    /// each recipe row of the money view whose stock the reader holds
    /// (one unit or more; one lot is the condition `feasible_for` judges),
    /// read for each skill it trains (`guide_training::stock_rows`), its
    /// coins converted at the reader's D17 of the row's band. The rows are
    /// the view's own (a full window on the cycle's quotes), never a
    /// rebuild.
    pub fn stock_training(&self, filter: &GuideFilter, d17: &dyn Fn(i64) -> Option<f64>) -> Vec<TrainingRow> {
        if filter.held.is_empty() {
            return Vec::new();
        }
        let money = self.money_view(filter.level);
        let holds = |item: &str| filter.held.iter().any(|(name, units)| name == item && *units >= 1.0);
        let mut out = Vec::new();
        for place in &money.recipes {
            let Some(recipe) = money.rows.get(place.row) else { continue };
            if !recipe.prereqs.stock.iter().any(|(item, _)| holds(item)) {
                continue;
            }
            let level = filter.level.unwrap_or_else(|| lifescape_planner::guide_methods::gate_level(&recipe.method)).clamp(1, 99);
            out.extend(lifescape_planner::guide_training::stock_rows(recipe, level, d17(recipe.band)));
        }
        out
    }

    /// Recipe rows of `view` re-evaluated on `held` (R7), by row index,
    /// priced on the cycle's quotes; an object the cycle never quoted is
    /// an error (never a guessed price).
    pub fn recipes_on_stock(&self, view: &MoneyView, held: &[(String, f64)], only: &dyn Fn(&MoneyRow) -> bool) -> Result<HashMap<usize, MoneyRow>, String> {
        let unquoted: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let rate = |ask: &RateAsk| Err(format!("guides store: no yield read for {} on a stock", ask.method));
        let price = |item: &str| match self.quotes.get(item) {
            Some(quote) => quote.clone(),
            None => {
                unquoted.borrow_mut().push(item.to_string());
                Quote::default()
            }
        };
        let npc_sale = |item: &str| crate::web_guides::npc_volume_value(item);
        let shop_buy = |item: &str| crate::web_guides::shop_window(item);
        let quest_ticks = |_: &str| None;
        let world = GuideWorld { rate: &rate, price: &price, npc_sale: &npc_sale, shop_buy: &shop_buy, market_wait_ticks: self.market_wait_ticks, quest_ticks: &quest_ticks, measured: &[] };
        let engine = lifescape_planner::guide_engine::Engine::new(&world);
        let replaced: HashMap<usize, MoneyRow> = view
            .recipes
            .iter()
            .filter(|place| view.rows.get(place.row).is_some_and(only))
            .map(|place| (place.row, lifescape_planner::guide_money::recipe_row(&engine, &place.ask, &place.answer, held)))
            .collect();
        let missing = unquoted.into_inner();
        if !missing.is_empty() {
            return Err(format!("guides store: held stock prices objects the cycle never quoted: {}", missing.join(", ")));
        }
        Ok(replaced)
    }
}

fn slot() -> &'static Mutex<Option<Arc<GuideSet>>> {
    static SLOT: OnceLock<Mutex<Option<Arc<GuideSet>>>> = OnceLock::new();
    SLOT.get_or_init(Default::default)
}

/// The last published set (an `Arc` clone, never a copy), `None` before
/// the first build: the in-process read of the brain (R7) and the API.
pub fn current() -> Option<Arc<GuideSet>> {
    slot().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
}

/// Publishes a set (readers holding the previous one keep it).
pub fn publish(set: GuideSet) {
    *slot().lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::new(set));
}

/// One cycle: tally the new reports, build, publish, log the cost.
pub fn run_cycle(tally: &mut WaitsTally, cycle: u32) -> Result<(), String> {
    let store = Store::open(&crate::market_service::db_path()).map_err(|error| format!("guides store: {error}"))?;
    if cycle % FULL_RECOUNT_CYCLES == 0 {
        *tally = WaitsTally::default();
    }
    tally.advance(&store)?;
    let set = build_set(&store, &crate::rates_service::live_snapshot(), &tally.waits());
    crate::logln!(
        "guides: built {} money + {} training views in {} ms ({} ms CPU), {} quotes",
        set.money_bands.len() + 1,
        set.training_bands.len() + 1,
        set.cycle_ms,
        set.cycle_cpu_ms.map(|cpu| cpu.to_string()).unwrap_or_else(|| "?".to_string()),
        set.quotes.len()
    );
    publish(set);
    Ok(())
}

/// Starts the `guides` thread (daemon only).
pub fn start(orch: Arc<crate::daemon::Orchestrator>) {
    let spawned = std::thread::Builder::new().name("guides".to_string()).spawn(move || {
        let mut due = Instant::now() + START_DELAY;
        let mut tally = WaitsTally::default();
        let mut cycle: u32 = 0;
        loop {
            while Instant::now() < due {
                if !orch.running.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(IDLE_POLL);
            }
            due = Instant::now() + REFRESH_PERIOD;
            if let Err(error) = run_cycle(&mut tally, cycle) {
                crate::logln!("guides: build failed: {error}");
            }
            cycle = cycle.wrapping_add(1);
        }
    });
    if let Err(error) = spawned {
        crate::logln!("guides: thread not started: {error}");
    }
}
