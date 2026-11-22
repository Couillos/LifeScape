#![forbid(unsafe_code)]
//! Brain planning context (split from brain.rs, same behavior):
//! public-data inputs for node resolution (money-method pick, price and
//! yield context, wallet and bank gauges). Never another bot's state.

use crate::jobs::Job;
use lifescape_json::Value;
use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::{Life, Node, NodeKind};

/// Fixed round-trip estimate to Varrock for cost comparisons.
// shortcut: fixed 800-tick round trip; replaced by measured convene
// ETAs once the brain reads them (L8).
pub const TRIP_TICKS: u64 = 800;

/// Pure money-method pick over published profits: the trend bonus
/// (score × revenue × weight) can flip a close call toward a shortage
/// item without ever reading another bot.
pub fn choose_money(profits: &[(String, f64, f64, f64)], trend_weight: f64) -> Option<String> {
    let mut best: Option<(String, f64)> = None;
    for (method, profit, revenue, trend) in profits {
        let score = lifescape_planner::rank_money_method(*profit, trend.clamp(-1.0, 1.0), *revenue, trend_weight);
        if best.as_ref().is_none_or(|(_, top)| score > *top) {
            best = Some((method.clone(), score));
        }
    }
    best.map(|(method, _)| method)
}

fn display_of(item_debug: &str) -> String {
    lifescape_gamedata::objs_by_name(item_debug)
        .into_iter()
        .next()
        .and_then(|entry| entry.display.or(entry.name))
        .unwrap_or(item_debug)
        .to_string()
}

fn shop_floor(item_debug: &str) -> i64 {
    // The server formula (`calc_shop_value`, keeper multiplier included,
    // Zaff's too): the price a bot pays at the cheapest F2P keeper.
    lifescape_gamedata::obtain::merchant_price(item_debug).unwrap_or(1).max(1)
}

/// Units the F2P keepers sell of `item_debug` in one window (base stock
/// + restock); `None` when no keeper ladder is known.
fn shop_window_units(item_debug: &str) -> Option<i64> {
    let steps = lifescape_gamedata::shop_buy::purchase_steps(item_debug, lifescape_planner::guide_model::WINDOW_TICKS as i64);
    (!steps.is_empty()).then(|| steps.iter().map(|step| step.units).sum())
}

/// Debug names of everything the live bot owns (pack, bank, worn).
pub(crate) fn live_owned(live: &Value) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for key in ["inv", "bank", "worn"] {
        for entry in live.get(key).as_arr().cloned().unwrap_or_default() {
            // By object id (tour 3 point 7); a bare name as before.
            let owned = match &entry {
                Value::Obj(_) if entry.get("id").as_i64().is_some() => live_debug_name(&entry).unwrap_or_default(),
                Value::Obj(_) => crate::rates_names::debug_of(entry.get("name").as_str().unwrap_or("")),
                Value::Str(name) => crate::rates_names::debug_of(name),
                _ => String::new(),
            };
            if !owned.is_empty() {
                names.push(owned);
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Units of `display` per 6000 ticks of `method` for this bot: its live
/// level in the method's skill, its best tool tier of the method's
/// family (`rates_read`: n >= 3 blended with the game prior, else the
/// prior). A method nothing estimates is a typed error (logged by the
/// caller), never a blind figure.
pub(crate) fn farm_yield(snapshot: &crate::rates_service::Snapshot, method: &str, display: &str, live: &Value) -> Result<f64, crate::rates_read::RateError> {
    let level = match crate::rates_measure::band_skill(method, "") {
        Some(skill) => crate::brain_succession::live_level(live, &skill),
        None => 1,
    };
    let tool = crate::report_meta::best_tools(method, &live_owned(live)).join("+");
    crate::rates_read::gained_per_6000(snapshot.book(), method, display, level, &tool)
}

/// Public-data planning context for one node: market reference (or shop
/// fallback), the published yield of its production legs for this bot
/// (live level and tool), trend score.
pub(crate) fn ready_ctx(life: &Life, node: &Node, live: &Value) -> ReadyCtx {
    let mut ctx = ReadyCtx::blind(life.profile.market_affinity);
    ctx.ironman = life.profile.ironman;
    ctx.trip_ticks = TRIP_TICKS;
    ctx.quarantined = crate::method_health::quarantined();
    ctx.cooled = crate::brain_cool::active(life);
    // A market maker's post (plan Q6): its slice's tile by the fountain.
    ctx.maker_post = lifescape_planner::maker_role(life).map(|role| lifescape_planner::maker_post(&role));
    // The grind of a purchase: what one tick of this bot's work really
    // earns (D1, `brain_d17::coin_rate`), never its best guide line.
    ctx.coins_per_tick = crate::brain_d17::coin_rate(&life.account);
    let item_debug = match &node.kind {
        NodeKind::ObtainItems { item, .. } => item.clone(),
        _ => String::new(),
    };
    let snapshot = crate::rates_service::live_snapshot();
    if let Ok(store) = lifescape_store::Store::open(&crate::market_service::db_path()) {
        if !item_debug.is_empty() {
            let display = display_of(&item_debug);
            let (price, _) = crate::rates_service::price_estimate(&store, &display);
            ctx.price = price.max(1.0) as i64;
            ctx.shop_price = shop_floor(&item_debug);
            ctx.shop_window_units = shop_window_units(&item_debug);
            ctx.yield_per_6000 = obtain_yield(&snapshot, node, &item_debug, &display, live);
            ctx.leg_costs = leg_costs(&snapshot, &store, node, &item_debug, &display, live, ctx.coins_per_tick);
        }
    }
    ctx
}

/// Each production leg of an obtain node with its OWN estimate: the
/// units per 6000 ticks of that method for this bot (`farm_yield`: the
/// measured rate blended with the game prior) and the ticks its consumed
/// inputs cost per unit (their market price at the bot's coin rate), so
/// a smelt and a ground pickup of the same bar never share one figure
/// (live 2026-11-22: `ground:iron_bar` 555 ticks per bar won the tie
/// over smelting). A leg the rates cannot estimate carries yield 0 (the
/// planner reads it as unusable), the error logged.
fn leg_costs(snapshot: &crate::rates_service::Snapshot, store: &lifescape_store::Store, node: &Node, item_debug: &str, display: &str, live: &Value, coins_per_tick: f64) -> Vec<lifescape_planner::cost::LegCost> {
    let trade = ["market:", "shop:", "dialogue:"];
    let candidates: Vec<String> = node.alternatives.iter().map(|alt| alt.method.clone()).filter(|method| !trade.iter().any(|prefix| method.starts_with(prefix))).collect();
    let mut out = Vec::new();
    for leg in lifescape_planner::obtain_legs::production_legs(item_debug) {
        if !candidates.is_empty() && !candidates.contains(&leg.method) {
            continue;
        }
        let yield_per_6000 = match farm_yield(snapshot, &leg.method, display, live) {
            Ok(rate) => rate.max(0.0),
            Err(error) => {
                crate::rates_read::log_rate_error(&error);
                0.0
            }
        };
        let input_coins: f64 = leg
            .inputs
            .iter()
            .map(|(input, quantity)| crate::rates_service::price_estimate(store, &display_of(input)).0 * *quantity as f64)
            .sum();
        out.push(lifescape_planner::cost::LegCost { method: leg.method, yield_per_6000, input_ticks_per_unit: input_coins / coins_per_tick.max(0.01) });
    }
    out
}

/// Yield of an obtain node: its first production leg the rates estimate
/// for this bot (gather, recipe, drop farm), 0 (unpriceable: the farm
/// leg costs `cost_farm`'s maximum) when none can, each error logged.
fn obtain_yield(snapshot: &crate::rates_service::Snapshot, node: &Node, item_debug: &str, display: &str, live: &Value) -> f64 {
    let trade = ["market:", "shop:", "dialogue:"];
    let mut methods: Vec<String> = node.alternatives.iter().map(|alt| alt.method.clone()).filter(|method| !trade.iter().any(|prefix| method.starts_with(prefix))).collect();
    if methods.is_empty() {
        methods = lifescape_planner::obtain_legs::production_legs(item_debug).into_iter().map(|leg| leg.method).collect();
    }
    for method in methods {
        match farm_yield(snapshot, &method, display, live) {
            Ok(rate) if rate > 0.0 => return rate,
            Ok(_) => {}
            Err(error) => crate::rates_read::log_rate_error(&error),
        }
    }
    0.0
}

/// Units of `item_debug` the bot holds AND may use (pack, bank, worn):
/// what its own sell orders and seller deals lock is not usable (live
/// 2026-11-13, b3gybcev: a listed hammer read as held). Coins are the
/// raw wallet (their commitments: `brain_gear::free_coins`).
pub(crate) fn holdings_count(job: &Job, item_debug: &str) -> i64 {
    let total = raw_holdings_count(job, item_debug);
    if item_debug.eq_ignore_ascii_case("coins") {
        return total;
    }
    let locks = crate::brain_locks::locked_items(&job.account);
    crate::brain_locks::free_count(total, crate::brain_locks::locked_in(&locks, item_debug))
}

/// `holdings_count` for many objects in one read (lot 3 review, CPU: a
/// money guide publication asked it ~270 times, each a copy of the live
/// snapshot and two store queries for the locks, ~160 ms): the job's
/// live snapshot and its locks read once, each object counted once.
pub(crate) struct Holdings {
    live: Value,
    locks: std::collections::BTreeMap<i64, i64>,
    counted: std::cell::RefCell<std::collections::HashMap<String, i64>>,
}

impl Holdings {
    /// The job's holdings as they stand now.
    pub(crate) fn of(job: &Job) -> Self {
        let live = job.inner.try_lock().map(|inner| inner.live.clone()).unwrap_or(None).unwrap_or(Value::Null);
        Holdings { live, locks: crate::brain_locks::locked_items(&job.account), counted: Default::default() }
    }

    /// What `holdings_count` reads for `item_debug`.
    pub(crate) fn count(&self, item_debug: &str) -> i64 {
        if let Some(known) = self.counted.borrow().get(item_debug) {
            return *known;
        }
        let total = crate::live_items::live_units(&self.live, item_debug);
        let units = if item_debug.eq_ignore_ascii_case("coins") { total } else { crate::brain_locks::free_count(total, crate::brain_locks::locked_in(&self.locks, item_debug)) };
        self.counted.borrow_mut().insert(item_debug.to_string(), units);
        units
    }
}

/// Every unit held, listed or not (pack, bank, worn).
pub(crate) fn raw_holdings_count(job: &Job, item_debug: &str) -> i64 {
    let live = job.inner.try_lock().map(|inner| inner.live.clone()).unwrap_or(None).unwrap_or(Value::Null);
    // By object id, never by the shared display (tour 3 point 7: 7
    // unstrung emerald amulets read as 7 strung ones).
    crate::live_items::live_units(&live, item_debug)
}

/// Distinct banked types with a positive count (the space-pressure gauge,
/// L8: cleanup at 55 of 60 F2P slots). Name or id keys the type, so both
/// live snapshot shapes count.
pub(crate) fn bank_types(job: &Job) -> usize {
    let live = job.inner.try_lock().map(|inner| inner.live.clone()).unwrap_or(None).unwrap_or(Value::Null);
    let mut types = std::collections::HashSet::new();
    for entry in live.get("bank").as_arr().cloned().unwrap_or_default() {
        match &entry {
            Value::Obj(_) => {
                if entry.get("count").as_i64().unwrap_or(1) <= 0 {
                    continue;
                }
                if let Some(name) = entry.get("name").as_str() {
                    types.insert(name.to_lowercase());
                } else if let Some(id) = entry.get("id").as_i64() {
                    types.insert(format!("id{id}"));
                }
            }
            Value::Str(name) => {
                types.insert(name.to_lowercase());
            }
            _ => {}
        }
    }
    types.len()
}

/* P1.6 D52: sale gate (worn, kept work tools, graph items never list).
 * The blanket tool list (every axe, pickaxe, net, … never sold by anyone)
 * is gone (user rule 2026-11-13: tools sell): the tools a bot works with
 * reach the gate through its keep list (`brain_tools::kept_tools`). */
/// D52 sale gate: true when `name` must never list (worn, graph keep with
/// the kept work tools, coins).
pub fn sale_blocked(name: &str, keep: &[String], worn: &[String]) -> bool {
    if name.eq_ignore_ascii_case("coins") {
        return true;
    }
    if worn.iter().any(|held| held.eq_ignore_ascii_case(name)) {
        return true;
    }
    keep.iter().any(|kept| kept.eq_ignore_ascii_case(name))
}

/// Merge the graph keep-list with the live worn names (D52): sorted, deduplicated.
pub fn merge_keep(graph: Vec<String>, worn: Vec<String>) -> Vec<String> {
    let mut keep = graph;
    keep.extend(worn);
    keep.sort_by_key(|name| name.to_lowercase());
    keep.dedup_by(|second, first| second.eq_ignore_ascii_case(first));
    keep
}

/// Debug name of one live stack. The snapshot carries display names
/// ("Small fishing net") plus the obj id; the planner, the tool gate and
/// `--keep` speak debug names (`net`), so the id decides. A bare string
/// entry (older snapshot shape) is taken as is.
pub(crate) fn live_debug_name(entry: &Value) -> Option<String> {
    match entry {
        Value::Obj(_) => {
            if let Some(id) = entry.get("id").as_i64() {
                if let Some(name) = usize::try_from(id).ok().and_then(lifescape_gamedata::obj_by_id).and_then(|obj| obj.name) {
                    return Some(name.to_string());
                }
            }
            entry.get("name").as_str().map(str::to_string)
        }
        Value::Str(name) => Some(name.clone()),
        _ => None,
    }
}

fn live_names(job: &Job, key: &str) -> Vec<String> {
    let live = job.inner.try_lock().map(|inner| inner.live.clone()).unwrap_or(None).unwrap_or(Value::Null);
    let mut names = Vec::new();
    for entry in live.get(key).as_arr().cloned().unwrap_or_default() {
        if entry.get("count").as_i64().unwrap_or(1) <= 0 {
            continue;
        }
        if let Some(name) = live_debug_name(&entry) {
            names.push(name);
        }
    }
    names
}

/// Live worn debug names (D52) for the `--keep` merge.
pub(crate) fn worn_names(job: &Job) -> Vec<String> {
    live_names(job, "worn")
}

/// Carried debug names (live pack the brain already reads) for the
/// planner tool gate: only positive counts ride along.
pub(crate) fn carried_tools(job: &Job) -> Vec<String> {
    let mut entries = live_entries(job, "inv");
    entries.extend(live_entries(job, "bank"));
    // A wielded pickaxe or axe works too (PRD3 lot 5).
    entries.extend(live_entries(job, "worn"));
    // A tool locked by the bot's own listing is not carried (b3gybcev).
    let locks = crate::brain_locks::locked_items(&job.account);
    crate::brain_locks::free_tool_names(&entries, &|name: &str| crate::brain_locks::locked_in(&locks, name))
}

/// Live `(debug name, count)` entries of one container.
fn live_entries(job: &Job, key: &str) -> Vec<(String, i64)> {
    let live = job.inner.try_lock().map(|inner| inner.live.clone()).unwrap_or(None).unwrap_or(Value::Null);
    let mut out = Vec::new();
    for entry in live.get(key).as_arr().cloned().unwrap_or_default() {
        let count = entry.get("count").as_i64().unwrap_or(1);
        if count <= 0 {
            continue;
        }
        if let Some(name) = live_debug_name(&entry) {
            out.push((name, count));
        }
    }
    out
}

/// Live carried + banked stacks (debug name, summed quantity), a
/// banknote under its object's name (`stacks_of`).
pub(crate) fn live_stacks(job: &Job) -> Vec<(String, i64)> {
    let live = job.inner.try_lock().map(|inner| inner.live.clone()).unwrap_or(None).unwrap_or(Value::Null);
    stacks_of(&live)
}

/// Carried + banked stacks of a snapshot: one row per OBJECT, its notes
/// summed in (user 2026-11-19: « les items et les banknote de ces items
/// sont les mêmes items », measured, valued and traded the same). Counts
/// what the bot owns; a recipe still takes only unnoted units (scripts).
pub(crate) fn stacks_of(live: &Value) -> Vec<(String, i64)> {
    let mut totals: std::collections::HashMap<String, (String, i64)> = std::collections::HashMap::new();
    for key in ["inv", "bank"] {
        for entry in live.get(key).as_arr().cloned().unwrap_or_default() {
            let Some(name) = live_debug_name(&entry) else { continue };
            let name = lifescape_gamedata::base_obj_name(&name).to_string();
            let qty = entry.get("count").as_i64().unwrap_or(1);
            if qty <= 0 {
                continue;
            }
            totals
                .entry(name.to_lowercase())
                .and_modify(|held| held.1 = held.1.saturating_add(qty))
                .or_insert((name, qty));
        }
    }
    totals.into_values().collect()
}

// P1.6 + P2.5 chaînes/choix: an expired rescue (typed `RescueBudget`
// in `last_error`, dossier on disk) replans like a failure, even when
// no task report arrives. Pure bridge: `on_supervise` folds it into
// the `needs_replan` path, which stays non-null for it (tested below).
pub fn rescue_replan_cause(last_error: &Value) -> Option<lifescape_planner::ReplanCause> {
    if last_error.get("type").as_str() == Some("RescueBudget") {
        Some(lifescape_planner::ReplanCause::Failed)
    } else {
        None
    }
}

/// Bot combat level for the gate (cerveau): the live heartbeat value
/// (server XP-max formula), else recomputed from the skill XP rows
/// with the same formula, else 3 (fresh account).
pub(crate) fn live_combat(live: &Value) -> u8 {
    if let Some(level) = live.get("combat").as_i64() {
        return level.clamp(1, 126) as u8;
    }
    let stats: std::collections::HashMap<u8, (u8, i32)> = live
        .get("skills")
        .as_arr()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| {
            let id = row.get("id").as_i64()?;
            let xp = row.get("xp").as_i64()?;
            let level = row.get("level").as_i64().unwrap_or(1).clamp(1, 99) as u8;
            Some((id as u8, (level, xp as i32)))
        })
        .collect();
    lifescape_sdk::snapshot::combat_level(&stats).unwrap_or(3)
}

/// Échec de voyage : la signature vit dans `last_error.message`
/// ("travel failure NoRoute ...", "bank unreachable ...").
pub fn is_travel_failure(last_error: &Value) -> bool {
    let message = last_error.get("message").as_str().unwrap_or("").to_lowercase();
    ["noroute", "travel failure", "unreachable", "unset_map_flag", "no-route", "no route"]
        .iter()
        .any(|mark| message.contains(mark))
}

/// Ce que le repli voyage décide : réessayer (il reste une
/// alternative) ou abandon typé (plus aucun exécutable).
pub enum TravelFold {
    Retry,
    NoRoute(String),
}

/// Bannit la méthode prouvée injoignable : retirée des alternatives,
/// verrou effacé, jamais relancée telle quelle.
pub fn fold_travel_failure(node: &mut Node, method: &str) -> TravelFold {
    node.alternatives.retain(|alt| alt.method != method);
    if node.method.as_deref() == Some(method) {
        node.method = None;
    }
    if node.alternatives.is_empty() {
        TravelFold::NoRoute(method.to_string())
    } else {
        TravelFold::Retry
    }
}

#[cfg(test)]
mod tests {
    use super::{choose_money, live_combat, merge_keep, rescue_replan_cause, sale_blocked};
    use lifescape_json::{object, str_value};
    use lifescape_planner::{Node, NodeKind, NodeStatus};

    /// User 2026-11-19: an object and its banknote are one object. 28
    /// noted yew logs in the pack and 100 banked read as 128 yew_logs,
    /// never a separate `cert_yew_logs` stack (it escaped the keep list,
    /// sold under its own id with no price, double-counted its locks).
    #[test]
    fn a_banknote_stack_reads_as_its_object() {
        let live = lifescape_json::parse(r#"{"inv":[{"id":1516,"count":28},{"id":1351,"count":1}],"bank":[{"id":1515,"count":100}]}"#).expect("json");
        let mut stacks = super::stacks_of(&live);
        stacks.sort();
        assert_eq!(stacks, vec![("bronze_axe".to_string(), 1), ("yew_logs".to_string(), 128)]);
    }

    #[test]
    fn trend_bonus_flips_a_close_money_call() {
        let profits = vec![
            ("gather:fishing/raw_lobster".to_string(), 500.0, 1000.0, 0.0),
            ("gather:woodcut/yew_logs".to_string(), 510.0, 1000.0, -0.5),
        ];
        assert_eq!(choose_money(&profits, 0.1).as_deref(), Some("gather:fishing/raw_lobster"), "flat profits pick the top");
        let shortage = vec![
            ("gather:fishing/raw_lobster".to_string(), 500.0, 1000.0, 0.6),
            ("gather:woodcut/yew_logs".to_string(), 510.0, 1000.0, -0.5),
        ];
        assert_eq!(choose_money(&shortage, 0.1).as_deref(), Some("gather:fishing/raw_lobster"), "shortage bonus keeps lobster");
        let flip = vec![
            ("gather:fishing/raw_lobster".to_string(), 500.0, 1000.0, -0.5),
            ("gather:woodcut/yew_logs".to_string(), 480.0, 1000.0, 0.8),
        ];
        assert_eq!(choose_money(&flip, 0.1).as_deref(), Some("gather:woodcut/yew_logs"), "trend flips the money call");
    }

    /// Rendements R2: the brain asks with the planner method, at the
    /// bot's live level; an empty estimator serves the game prior, a
    /// method nothing estimates is an error, never the old blind 200.
    #[test]
    fn farm_yield_reads_planner_keyed_rates() {
        use super::farm_yield;
        let activity = lifescape_planner::activity::activity_of_method;
        assert_eq!(activity("farm:raw_shrimp"), "gather:fishing/raw_shrimp", "keyed by activity (R2.1)");
        assert_eq!(activity("farm:feather"), "fight:melee/chickens");
        let empty = crate::rates_service::Snapshot::new();
        let live = lifescape_json::parse(r#"{"skills":[{"id":8,"level":1}]}"#).expect("live");
        let low = farm_yield(&empty, "gather:woodcut/logs", "Logs", &live).expect("prior");
        assert!(low > 0.0);
        let skilled = lifescape_json::parse(r#"{"skills":[{"id":8,"level":40}]}"#).expect("live");
        let high = farm_yield(&empty, "gather:woodcut/logs", "Logs", &skilled).expect("prior");
        assert!(high > low, "read at the bot's real level: {high} vs {low}");
        assert!(farm_yield(&empty, "mill:pot_flour", "Pot of flour", &live).is_err(), "no blind default");
    }

    /// D52: worn, graph items (kept work tools included) never sell; a
    /// tool outside the keep list is ordinary surplus (user rule
    /// 2026-11-13: tools sell). L11: a merchant sold its own net; the
    /// net now reaches the gate through the keep list (`brain_tools`).
    #[test]
    fn sale_gate_blocks_worn_and_graph_but_tools_sell() {
        let keep = vec!["feather".to_string(), "net".to_string()];
        let worn = vec!["iron_sword".to_string()];
        assert!(sale_blocked("net", &keep, &[]), "kept work tool never lists");
        assert!(!sale_blocked("bronze_axe", &keep, &[]), "a tool it does not work with sells");
        assert!(!sale_blocked("tinderbox", &[], &[]), "tinderbox sells when unused");
        assert!(sale_blocked("iron_sword", &[], &worn), "worn never lists");
        assert!(sale_blocked("feather", &keep, &[]), "graph item never lists");
        assert!(sale_blocked("coins", &[], &[]), "wallet never lists");
        assert!(!sale_blocked("yew_logs", &keep, &worn), "plain surplus lists");
    }

    #[test]
    fn keep_merge_dedups_worn_into_graph() {
        let merged = merge_keep(vec!["feather".to_string()], vec!["Iron_Sword".to_string(), "feather".to_string()]);
        assert_eq!(merged.len(), 2, "dedup is case-insensitive, got {merged:?}");
        assert!(merged.contains(&"feather".to_string()));
        assert!(merged.contains(&"Iron_Sword".to_string()));
    }

    /// P1.6 reprise: `on_supervise` folds a typed `RescueBudget`
    /// `last_error` into the replan path, so `needs_replan` stays
    /// non-null for it (the node retries with a fresh method).
    #[test]
    fn rescue_budget_last_error_replans() {
        let budget = object(vec![
            ("where", str_value("maze")),
            ("type", str_value("RescueBudget")),
            ("message", str_value("rescue maze over budget")),
        ]);
        let cause = rescue_replan_cause(&budget).expect("RescueBudget replans");
        assert_eq!(lifescape_planner::needs_replan("rescue_budget", &[], false, false, false), Some(cause), "bridge feeds the replan path");
        assert!(rescue_replan_cause(&lifescape_json::Value::Null).is_none(), "no error never replans");
        let other = object(vec![("type", str_value("Locked"))]);
        assert!(rescue_replan_cause(&other).is_none(), "other errors never rescue-replan");
    }

    #[test]
    fn gate_reads_xp_max_combat_then_skills_then_fresh() {
        let heartbeat = object(vec![("combat", lifescape_json::Value::Int(43))]);
        assert_eq!(live_combat(&heartbeat), 43, "heartbeat combat wins");
        let skills = lifescape_json::Value::Arr(
            [0u8, 1, 2, 3, 4, 5, 6]
                .iter()
                .map(|id| {
                    let (level, xp) = if *id == 3 { (10, 1154) } else { (1, 0) };
                    object(vec![
                        ("id", lifescape_json::Value::Int(i64::from(*id))),
                        ("level", lifescape_json::Value::Int(level)),
                        ("xp", lifescape_json::Value::Int(xp)),
                    ])
                })
                .collect(),
        );
        let fallback = object(vec![("skills", skills)]);
        let stats: std::collections::HashMap<u8, (u8, i32)> =
            [(0u8, (1u8, 0i32)), (1, (1, 0)), (2, (1, 0)), (3, (10, 1154)), (4, (1, 0)), (5, (1, 0)), (6, (1, 0))]
                .into_iter()
                .collect();
        assert_eq!(
            live_combat(&fallback),
            lifescape_sdk::snapshot::combat_level(&stats).unwrap_or(3),
            "skills recompute the XP-max formula"
        );
        assert_eq!(live_combat(&lifescape_json::Value::Null), 3, "no snapshot reads fresh");
    }

    /// t06/bpjms7de: the snapshot says "Small fishing net" (id 303), the
    /// tool gate wants `net`: the id decides, or the carried net is never
    /// seen and the node blocks on a net the bot holds.
    #[test]
    fn live_stacks_read_debug_names_from_ids() {
        use super::live_debug_name;
        let net = object(vec![("count", lifescape_json::Value::Int(1)), ("id", lifescape_json::Value::Int(303)), ("name", str_value("Small fishing net"))]);
        assert_eq!(live_debug_name(&net).as_deref(), Some("net"));
        let bare = str_value("coins");
        assert_eq!(live_debug_name(&bare).as_deref(), Some("coins"));
    }

    /// Marche impossible : l'échec de voyage bannit la méthode (jamais
    /// relancée) ; sans alternative, échec typé au lieu d'une boucle.
    #[test]
    fn travel_failure_bans_then_abandons_typed() {
        use super::{fold_travel_failure, is_travel_failure, TravelFold};
        let travel = object(vec![("message", str_value("travel failure NoRoute seg=2 at (0,1,2)"))]);
        assert!(is_travel_failure(&travel), "NoRoute voyage");
        let bank = object(vec![("message", str_value("bank unreachable from (0,3222,3219)"))]);
        assert!(is_travel_failure(&bank), "banque injoignable");
        assert!(!is_travel_failure(&lifescape_json::Value::Null), "sans erreur, rien");
        let plain = object(vec![("message", str_value("died"))]);
        assert!(!is_travel_failure(&plain), "un échec franc n'est pas un voyage");

        let mut node = Node::new("n1", NodeKind::ReachLevel { skill: "fishing".to_string(), level: 5 }, Vec::new(), 5);
        node.status = NodeStatus::Ready;
        node.method = Some("gather:fishing/raw_sardine".to_string());
        node.alternatives.push(lifescape_planner::Alternative { method: "gather:fishing/raw_sardine".to_string(), est_ticks: 6000, est_coins: 0 });
        node.alternatives.push(lifescape_planner::Alternative { method: "gather:fishing/raw_shrimp".to_string(), est_ticks: 7000, est_coins: 0 });
        assert!(matches!(fold_travel_failure(&mut node, "gather:fishing/raw_sardine"), TravelFold::Retry), "il reste une alternative");
        assert!(node.method.is_none(), "verrou effacé");
        assert!(!node.alternatives.iter().any(|alt| alt.method == "gather:fishing/raw_sardine"), "bannie");
        assert!(matches!(fold_travel_failure(&mut node, "gather:fishing/raw_shrimp"), TravelFold::NoRoute(method) if method == "gather:fishing/raw_shrimp"), "vide = typé");
    }

}
