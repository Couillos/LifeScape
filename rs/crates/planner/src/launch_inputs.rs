#![forbid(unsafe_code)]
//! Typed inputs of one brain launch pass (lot 1 of the launch refactor):
//! the live facts `maybe_launch` reads, as plain data. The orchestrator
//! fills it (`brain_launch_inputs::gather`), this file only counts. The
//! counting helpers restate, byte for byte, the formulas the brain used
//! on the raw live snapshot (`live_items::live_units`, `holdings_count`,
//! `free_tool_names`, `committed_coins`, `listed_units`), so a pass that
//! reads this struct decides what the scattered reads decided.
//!
//! Values that depend on the market (locks, listings, committed coins)
//! go stale the moment a pass acts on the market: the orchestrator
//! re-gathers them at the points marked `// lot 3:`.

use std::collections::BTreeMap;

use lifescape_json::Value;

/// A site: `(x, z, level)`, as `site_crowd` keys them.
pub type Site = (i64, i64, i64);

/// One live pack / bank / worn entry, as the heartbeat shapes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveStack {
    /// An object entry: the debug name its id resolves to (when the dump
    /// knows it), its own `name` field, its count as read (1 when absent).
    Obj { id_name: Option<String>, name: Option<String>, count: i64 },
    /// An older snapshot shape: a bare name, one unit.
    Name(String),
}

impl LiveStack {
    /// Debug name of the entry (id first, `name` as the fallback).
    fn debug_name(&self) -> Option<&str> {
        match self {
            LiveStack::Obj { id_name, name, .. } => id_name.as_deref().or(name.as_deref()),
            LiveStack::Name(name) => Some(name),
        }
    }

    /// True when the entry is a stack of `item` (a banknote counts as the
    /// goods): by id when the entry has one, else by its debug or display
    /// name (`live_items::entry_is`).
    pub fn is(&self, item: &str) -> bool {
        match self {
            LiveStack::Obj { id_name, name, .. } => matches_item(id_name.as_deref(), name.as_deref(), item),
            LiveStack::Name(name) => matches_item(None, Some(name), item),
        }
    }

    /// Units the entry counts for (`live_items::live_units`).
    pub fn units(&self) -> i64 {
        match self {
            LiveStack::Obj { count, .. } => (*count).max(0),
            LiveStack::Name(_) => 1,
        }
    }
}

/// One live entry as a stack (`None`: neither an object nor a bare name).
pub fn stack_of(entry: &Value) -> Option<LiveStack> {
    match entry {
        Value::Obj(_) => Some(LiveStack::Obj {
            id_name: entry
                .get("id")
                .as_i64()
                .and_then(|id| usize::try_from(id).ok())
                .and_then(lifescape_gamedata::obj_by_id)
                .and_then(|obj| obj.name)
                .map(str::to_string),
            name: entry.get("name").as_str().map(str::to_string),
            count: entry.get("count").as_i64().unwrap_or(1),
        }),
        Value::Str(name) => Some(LiveStack::Name(name.clone())),
        _ => None,
    }
}

/// The entries of one live container (`inv`, `bank`, `worn`), in order;
/// entries that are neither an object nor a bare name never count.
pub fn read_stacks(live: &Value, key: &str) -> Vec<LiveStack> {
    live.get(key).as_arr().map(Vec::as_slice).unwrap_or_default().iter().filter_map(stack_of).collect()
}

/// The ONE rule of the brain's holdings reads (tour 3 point 7): an entry
/// is the object its id names (`id_name`, its banknote counting as the
/// goods), the name (debug or display) only for an entry without a known id.
fn matches_item(id_name: Option<&str>, name: Option<&str>, item: &str) -> bool {
    let wanted = item.to_lowercase();
    let same = |name: &str| {
        let name = name.to_lowercase();
        name == wanted || name.strip_prefix("cert_") == Some(wanted.as_str())
    };
    match id_name {
        Some(id_name) => same(id_name),
        None => name.is_some_and(|name| same(name) || name.eq_ignore_ascii_case(&display(item))),
    }
}

fn id_name_of(entry: &Value) -> Option<&'static str> {
    entry.get("id").as_i64().and_then(|id| usize::try_from(id).ok()).and_then(lifescape_gamedata::obj_by_id).and_then(|obj| obj.name)
}

/// True when the live `entry` is a stack of `item` (a banknote counts as
/// the goods): by id when the entry has one, else by its debug or display
/// name (`matches_item`, the same rule as `LiveStack::is`).
pub fn entry_is(entry: &Value, item: &str) -> bool {
    match entry {
        Value::Obj(_) => matches_item(id_name_of(entry), entry.get("name").as_str(), item),
        Value::Str(name) => matches_item(None, Some(name), item),
        _ => false,
    }
}

/// Units of `item` the live snapshot holds (pack, bank, worn), listed or
/// not: the ONE counting rule, `LaunchInputs::raw_units` is its typed twin.
pub fn live_units(live: &Value, item: &str) -> i64 {
    let mut total = 0i64;
    for key in ["inv", "bank", "worn"] {
        for entry in live.get(key).as_arr().map(Vec::as_slice).unwrap_or_default() {
            if !entry_is(entry, item) {
                continue;
            }
            total += match entry {
                Value::Obj(_) => entry.get("count").as_i64().unwrap_or(1).max(0),
                _ => 1,
            };
        }
    }
    total
}

/// Units of `item` (debug name) locked in `locks` (base object id keyed).
pub fn locked_in(locks: &BTreeMap<i64, i64>, item: &str) -> i64 {
    obj_id(item).and_then(|id| locks.get(&base_of(id)).copied()).unwrap_or(0)
}

/// Held units the bot may use.
pub fn free_count(held: i64, locked: i64) -> i64 {
    (held - locked.max(0)).max(0)
}

/// Tool names (one per name, first-seen order) of live entries `(name,
/// count)` that still have a unit free of `locked`.
pub fn free_tool_names(entries: &[(String, i64)], locked: &dyn Fn(&str) -> i64) -> Vec<String> {
    let mut totals: Vec<(String, i64)> = Vec::new();
    for (name, count) in entries {
        match totals.iter_mut().find(|(known, _)| known.eq_ignore_ascii_case(name)) {
            Some(slot) => slot.1 = slot.1.saturating_add((*count).max(1)),
            None => totals.push((name.clone(), (*count).max(1))),
        }
    }
    totals.into_iter().filter(|(name, total)| free_count(*total, locked(name)) > 0).map(|(name, _)| name).collect()
}

fn display(item: &str) -> String {
    lifescape_gamedata::objs_by_name(item).into_iter().next().and_then(|obj| obj.display.or(obj.name)).unwrap_or(item).to_string()
}

/// Object id of `item` (first dump row of that name).
fn obj_id(item: &str) -> Option<i64> {
    lifescape_gamedata::objs_by_name(item).into_iter().next().map(|obj| obj.obj_id as i64)
}

/// Base object id: a banknote reads as its goods (`market_audit::base_of`).
pub fn base_of(id: i64) -> i64 {
    let Ok(index) = usize::try_from(id) else { return id };
    let name = lifescape_gamedata::obj_by_id(index).and_then(|entry| entry.name).unwrap_or_default();
    match name.strip_prefix("cert_") {
        Some(base) => lifescape_gamedata::objs_by_name(base).into_iter().next().map(|entry| entry.obj_id as i64).unwrap_or(id),
        None => id,
    }
}

/// The live facts of one launch pass. No `Default`: the production reader
/// (`brain_launch_inputs::gather`) writes every field by name, so a field
/// added here is a compile error there until it is read (2026-11-23: a
/// defaulted `now: 0` shipped to production).
#[derive(Debug, Clone)]
pub struct LaunchInputs {
    /// `(x, z, level)` when the snapshot's position has exactly 3 parts.
    pub position: Option<Site>,
    /// The position the resolve ctx reads (3 parts or more).
    pub ctx_position: Option<Site>,
    /// The pack streamed (`inv_streamed == true`).
    pub inv_streamed: bool,
    /// The bank streamed (`bank_streamed == true`).
    pub bank_streamed: bool,
    /// Combat level as `live_combat` reads it.
    pub combat: u8,
    /// Distinct banked types with a positive count (`bank_types`).
    pub bank_types: usize,
    /// Carried + banked stacks, one row per object (`stacks_of`).
    pub stacks: Vec<(String, i64)>,
    /// Debug names of the worn entries with a positive count.
    pub worn: Vec<String>,
    /// Raw pack / bank / worn entries.
    pub inv: Vec<LiveStack>,
    pub bank: Vec<LiveStack>,
    pub wearing: Vec<LiveStack>,
    /// Base object id -> units locked by the bot's own resting sells and
    /// seller deals (market state: re-gathered after market actions).
    pub locks: BTreeMap<i64, i64>,
    /// Object id -> units of the bot's resting sells (Open / Matched,
    /// remaining > 0).
    pub listed: BTreeMap<i64, i64>,
    /// `(object id, coins)` of every cancellable resting buy.
    pub commit_rows: Vec<(i64, i64)>,
    /// Coins of in-flight buyer deals plus gifts owed.
    pub commit_flat: i64,
    /// The fleet's sites with their head count (`site_crowd::others`).
    pub crowd: Vec<(Site, u32)>,
    /// Engaged seats (`site_crowd::seats`).
    pub seats: Vec<(Site, String, u32)>,
    /// A launch was noted in the last 30 s (`recent_launch`).
    pub recent_launch: bool,
    /// A report of the account is queued, not folded yet.
    pub report_in_flight: bool,
    /// A report kept pending on a full inbox (true when the job lock is
    /// contended, as the idle detector reads it).
    pub report_pending: bool,
    // Lot 3: plain data the launch decision (`launch_decide`) reads.
    /// Unix seconds of the pass (history stamps, ban and cooldown ages).
    pub now: i64,
    /// Wall milliseconds of one game tick (`market_eta::live_tick_ms`).
    pub tick_ms: u64,
    /// Live level of every skill as `brain_succession::live_level` reads
    /// it (the snapshot's own `level` field), skill name first.
    pub levels: Vec<(String, u8)>,
    /// The market service is up (fighters read its references).
    pub market_up: bool,
    /// Ticks of one filler window (`brain_filler::FILLER_TICKS`).
    pub filler_ticks: u64,
}

impl LaunchInputs {
    /// An empty pass for tests that set only the fields they exercise
    /// (everything off, clock and tick at fixed values). Production never
    /// builds one: `gather` names every field.
    #[cfg(any(test, feature = "test-support"))]
    pub fn blank_for_tests() -> Self {
        LaunchInputs {
            position: None,
            ctx_position: None,
            inv_streamed: false,
            bank_streamed: false,
            combat: 0,
            bank_types: 0,
            stacks: Vec::new(),
            worn: Vec::new(),
            inv: Vec::new(),
            bank: Vec::new(),
            wearing: Vec::new(),
            locks: BTreeMap::new(),
            listed: BTreeMap::new(),
            commit_rows: Vec::new(),
            commit_flat: 0,
            crowd: Vec::new(),
            seats: Vec::new(),
            recent_launch: false,
            report_in_flight: false,
            report_pending: false,
            now: 0,
            tick_ms: 0,
            levels: Vec::new(),
            market_up: false,
            filler_ticks: 0,
        }
    }

    /// Live level of `skill` (1 for a skill the snapshot does not list),
    /// as `brain_succession::live_level` reads it.
    pub fn level_of(&self, skill: &str) -> u8 {
        self.levels.iter().find(|(name, _)| name == skill).map_or(1, |(_, level)| *level)
    }

    /// Game ticks of `seconds` of wall time (`brain_stuck::seconds_to_ticks`).
    pub fn ticks_of(&self, seconds: i64) -> u64 {
        (seconds.max(0) as u64).saturating_mul(1000) / self.tick_ms.max(1)
    }

    fn all(&self) -> impl Iterator<Item = &LiveStack> {
        self.inv.iter().chain(self.bank.iter()).chain(self.wearing.iter())
    }

    /// Every unit held, listed or not (`raw_holdings_count`).
    pub fn raw_units(&self, item: &str) -> i64 {
        self.all().filter(|stack| stack.is(item)).map(LiveStack::units).sum()
    }

    /// Units of `item` locked by the bot's own sells (`locked_in`).
    pub fn locked(&self, item: &str) -> i64 {
        locked_in(&self.locks, item)
    }

    /// Held units the bot may use: raw minus locks, coins never locked
    /// (`holdings_count`).
    pub fn held(&self, item: &str) -> i64 {
        let total = self.raw_units(item);
        if item.eq_ignore_ascii_case("coins") {
            return total;
        }
        free_count(total, self.locked(item))
    }

    /// Units of `item` the bot's resting sells list (`listed_units`).
    pub fn listed_units(&self, item: &str) -> i64 {
        obj_id(item).and_then(|id| self.listed.get(&id).copied()).unwrap_or(0)
    }

    /// Copies of `item` free of the bot's own listings (P2-7).
    pub fn free_of_listings(&self, item: &str) -> i64 {
        (self.held(item) - self.listed_units(item)).max(0)
    }

    /// The whole family counts (`tools::family_held` over `held`).
    pub fn family_held(&self, item: &str) -> i64 {
        crate::tools::family_held(item, i64::MAX / 4, &|name| self.held(name))
    }

    /// Free-of-listings count: the exact piece, or the whole family.
    pub fn free_now(&self, item: &str, exact: bool) -> i64 {
        if exact {
            self.free_of_listings(item)
        } else {
            crate::tools::family_held(item, i64::MAX / 4, &|name| self.free_of_listings(name))
        }
    }

    /// Carried debug names with a unit free of the locks (`carried_tools`).
    pub fn carried_tools(&self) -> Vec<String> {
        let entries: Vec<(String, i64)> = self
            .all()
            .filter_map(|stack| {
                let (name, count) = match stack {
                    LiveStack::Obj { count, .. } => (stack.debug_name(), *count),
                    LiveStack::Name(name) => (Some(name.as_str()), 1),
                };
                (count > 0).then_some(name).flatten().map(|name| (name.to_string(), count))
            })
            .collect();
        free_tool_names(&entries, &|name| self.locked(name))
    }

    /// Coins the resting buys and in-flight deals commit, except on
    /// `except` (`committed_coins`).
    pub fn committed(&self, except: Option<i64>) -> i64 {
        self.commit_rows.iter().filter(|(item, _)| Some(*item) != except).map(|(_, coins)| coins).sum::<i64>() + self.commit_flat
    }
}

/// What the resolve context of a node counts from the inputs (`build_ctx`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeCounts {
    /// Free wallet: held coins minus what the other resting buys commit
    /// (obtain nodes), the bare held coins otherwise.
    pub coins: i64,
    /// Units of the obtain's item held (`None`: not an obtain).
    pub held: Option<i64>,
    /// Coins the bot's other resting buys commit (obtain nodes only).
    pub committed: i64,
}

/// The wallet and holdings the resolve context carries.
pub fn node_counts(life: &crate::Life, position: usize, inputs: &LaunchInputs) -> NodeCounts {
    // Own wallet gates the market leg (same coverage rule as the
    // carnet): a broke bot farms instead of placing doomed orders.
    let mut coins = inputs.held("coins");
    // A purchase bids on the FREE wallet (review 2026-11-13): coins minus
    // what this bot's other resting buys (gear step, tool wishes) commit.
    let mut committed = 0;
    let mut held = None;
    if let crate::NodeKind::ObtainItems { item, .. } = &life.nodes[position].kind {
        // The whole family counts (a steel pickaxe meets `bronze_pickaxe`,
        // a staff of air any number of air runes), except for the goal's
        // own production quota (air runes to craft are runes, not a staff)
        // and a gear step, which wants that very piece (a kit
        // `bronze_pickaxe` for a bot whose rune pickaxe is above its
        // mining level never closes on the rune one).
        let exact = crate::gear_ladder::is_gear_obtain(life, position);
        let quota = exact || matches!(&life.goal, crate::GoalKind::Produce { item: made, .. } if made == item);
        held = Some(if quota { inputs.held(item) } else { inputs.family_held(item) });
        committed = inputs.committed(obj_id(item));
        coins = (coins - committed).max(0);
    }
    NodeCounts { coins, held, committed }
}

#[cfg(test)]
#[path = "launch_inputs_tests.rs"]
mod tests;
