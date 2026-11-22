#![forbid(unsafe_code)]
//! Money and coin nodes choosing from the MONEY GUIDE (plan rendements
//! R7, D10, D13, D14, D17, D22): make_money, have_coins (a purchase's
//! coins, a tool's financing) read the precomputed guide
//! (`guides_store::current()`, never a build on demand, the asks of its
//! cycle: decision 2026-11-17 (e)) and hand the resolver the lines the
//! bot may take (`ReadyCtx::money_guide`, drawn by
//! `lifescape_planner::money_pick`):
//! - the rows of the bot: for each F2P skill, the money view of its
//!   level in that skill, the rows of the activities that skill bands
//!   (the yields of the bot's own band); rows without a band skill
//!   (wool, a ground pickup) once, from its lowest level's view;
//! - the recipes on its OWN stock (`GuideFilter::held`: its free stock
//!   minus what it keeps, re-evaluated on that stock by the set, only the
//!   recipes whose every stock input it holds): the alchemy of its
//!   objects (`alch:`) and every other transformation a money node
//!   launches (cook its catch, smelt its ores, smith its bars, spin its
//!   wool, tan and sew its hides, cut its gems, fire its pots,
//!   runecraft its essence...: decision 2026-11-17, R7 suite), its lot
//!   the actions its free stock covers (`stock_lot::lot_of`, the run
//!   stops once it is used, `--use`);
//! - each row mapped to the planner method it launches: a farm row its
//!   own method when a money node maps it (`mapping`), a market row the
//!   100 % market lot `trade:craft/<recipe>` when the bot has its offer
//!   (`brain_trade::publish`, its lot sized on the wallet), a recipe on
//!   stock its own method; chains are not launched by a money node;
//! - each judged on the bot's live state (`guide_feasible::feasible_for`:
//!   levels, combat, quests, stock, free wallet, the cycle's asks):
//!   feasible, or within reach after a tool or a lot's coins; priced and
//!   earning only (a line that loses coins is no money method);
//! - each row's profit read for THIS bot now (plan équilibre step 2,
//!   `guide_share::SharedDemand`): its products on the demand the other
//!   producers' live claims and the resting asks leave; a run launched
//!   posts its own units on the order board (`claim_launch`);
//! - one line per method (the best profit), its weight the profit times
//!   the profile (`guide_pick::merit_money`: market_affinity on a market
//!   line), its runs the yield's measured runs (exploration, D14).
//!
//! No guide yet, or no earning line within reach: nothing published,
//! logged; the static money methods rank as before R7.

use std::collections::HashMap;

use lifescape_json::Value;
use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::guide_feasible::{feasible_for, BotState, Feasibility};
use lifescape_planner::guide_model::{Family, MoneyRow, Quote};
use lifescape_planner::guide_pick::{merit_money, GuideLine};
use lifescape_planner::guide_share::SharedDemand;
use lifescape_planner::{Life, Node, NodeKind};

use crate::guides_store::GuideSet;
use crate::jobs::Job;

/// The rows a bot reads (module doc): `rows` borrowed from the set's
/// views (never copied: a view holds thousands), `on_stock` the stock
/// recipes a money node launches (`alch:`) re-evaluated on the bot's
/// stock (they replace the view's), and the read errors (an object the
/// cycle never quoted: the recipe then stays the view's).
pub struct BotRows<'a> {
    pub rows: Vec<&'a MoneyRow>,
    pub on_stock: Vec<MoneyRow>,
    pub errors: Vec<String>,
}

impl BotRows<'_> {
    /// Every row, the re-evaluated recipes in place of the view's.
    pub fn all(&self) -> Vec<&MoneyRow> {
        self.rows.iter().copied().chain(self.on_stock.iter()).collect()
    }
}

/// A stock recipe a money node launches (module doc): alchemy, or any
/// transformation of the bot's stock its script runs as a lot.
pub(crate) fn stock_recipe(row: &MoneyRow) -> bool {
    row.family == Family::Recipe && (row.method.starts_with("alch:") || (lifescape_planner::stock_lot::stock_inputs(&row.method).is_some() && money_launchable(&row.method)))
}

/// True when the bot holds some of every stock input of a recipe row:
/// only those are re-evaluated on its stock (one lot is the condition
/// `feasible_for` judges after).
fn holds_stock(row: &MoneyRow, stock: &[(String, f64)]) -> bool {
    row.prereqs.stock.iter().all(|(item, _)| stock.iter().any(|(name, units)| name == item && *units >= 1.0))
}

/// The rows of the set a bot reads (module doc), activity order of each
/// view; a level of the bot reads its band's view, gated activities out.
pub fn bot_rows<'a>(set: &'a GuideSet, level_of: &dyn Fn(&str) -> u8, stock: &[(String, f64)]) -> BotRows<'a> {
    let mut levels: Vec<u8> = lifescape_planner::guide_methods::SKILLS.iter().map(|skill| level_of(skill).clamp(1, 99)).collect();
    levels.sort_unstable();
    levels.dedup();
    let mut out = BotRows { rows: Vec::new(), on_stock: Vec::new(), errors: Vec::new() };
    let mut unbanded_taken = false;
    for level in levels {
        let view = set.money_view(Some(level));
        let keeps = |row: &MoneyRow| {
            if lifescape_planner::guide_methods::gate_level(&row.method) > level {
                return false;
            }
            match lifescape_planner::activity::band_skill(&row.method, &row.style) {
                Some(skill) => level_of(&skill).clamp(1, 99) == level,
                None => !unbanded_taken,
            }
        };
        let replaced = if stock.is_empty() {
            Default::default()
        } else {
            set.recipes_on_stock(view, stock, &|row| row.family == Family::Recipe && holds_stock(row, stock) && stock_recipe(row) && keeps(row)).unwrap_or_else(|error| {
                out.errors.push(error);
                Default::default()
            })
        };
        for (index, row) in view.rows.iter().enumerate().filter(|(_, row)| keeps(row)) {
            match replaced.get(&index) {
                Some(on_stock) => out.on_stock.push(on_stock.clone()),
                None => out.rows.push(row),
            }
        }
        unbanded_taken = true;
    }
    out
}

/// True for a farm method a money node launches.
fn money_launchable(method: &str) -> bool {
    if lifescape_planner::tranches::is_ladder(method) {
        return false;
    }
    // Static per method (the mapping tables): read once per process, a
    // publication asks it for every row (~1 500).
    static KNOWN: std::sync::OnceLock<std::sync::Mutex<HashMap<String, bool>>> = std::sync::OnceLock::new();
    let known = KNOWN.get_or_init(Default::default);
    if let Some(launchable) = known.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(method) {
        return *launchable;
    }
    let probe = Node::new("probe", NodeKind::MakeMoney, Vec::new(), 0);
    let launchable = lifescape_planner::map_method(method, &probe, None).is_some();
    known.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(method.to_string(), launchable);
    launchable
}

/// The planner method a row launches for this bot (module doc), `None`
/// for a row a money node never works.
fn launched(row: &MoneyRow, ctx: &ReadyCtx) -> Option<String> {
    match row.family {
        Family::Farm => money_launchable(&row.method).then(|| row.method.clone()),
        Family::Market => {
            let method = lifescape_planner::trade_craft::method_of(&row.method);
            ctx.trades.iter().any(|offer| offer.method == method).then_some(method)
        }
        Family::Recipe => stock_recipe(row).then(|| row.method.clone()),
        Family::Chain => None,
    }
}

/// The lines of `rows` the bot may take (module doc), one per method. A
/// recipe's stock is judged on `stock` (its free stock minus what it
/// keeps: never its gear, its tools, its plan's inputs), every other
/// object on `bot.held`. A recipe's lot never makes more than its
/// product's market absorbs (point 2, user 2026-11-18:
/// `product_room(method)`, actions, `None` unknown): a lot it cannot
/// sell is no line. A row's profit is the one THIS bot reads now
/// (`shared`, plan équilibre step 2: its products on the demand the
/// other producers' live claims leave, `guide_share`); a trade lot its
/// offer's (already shared); a line that no longer earns is none.
pub fn lines_for(rows: &[&MoneyRow], ctx: &ReadyCtx, bot: &BotState, stock: &[(String, f64)], affinity: f64, product_room: &dyn Fn(&str) -> Option<i64>, shared: &dyn Fn(&MoneyRow) -> f64) -> Vec<GuideLine> {
    let mut best: HashMap<String, GuideLine> = HashMap::new();
    for row in rows.iter().filter(|row| row.priced) {
        let Some(method) = launched(row, ctx) else { continue };
        // A trade lot ranks on the lot this bot really takes (demand,
        // wallet, asks: `lot_size`), its purchase spread over that lot
        // only (review lot 2, point 5); no lot fits: its offer's.
        let profit = match lifescape_planner::trade_craft::offer_in(ctx, &method) {
            Some(offer) => match lifescape_planner::trade_craft::lot_size(offer, ctx.trade_wallet).lot {
                lot if lot >= 1 => offer.lot_rate(lot),
                _ => offer.profit_6000,
            },
            // A ground spawn's output is shared with the bots already on
            // it (`spawn_share`, live 2026-11-22: ~10 fillers on one tile).
            None => shared(row) * lifescape_planner::spawn_share::crowd_factor(&method, &ctx.seats),
        };
        if profit <= 0.0 {
            continue;
        }
        let stock_held = |item: &str| {
            if row.prereqs.stock.iter().any(|(name, _)| name == item) {
                stock.iter().filter(|(name, _)| name == item).map(|(_, units)| *units).sum()
            } else {
                (bot.held)(item)
            }
        };
        let judged = BotState { held: &stock_held, ..*bot };
        let feasible = match feasible_for(*row, &judged) {
            Feasibility::Feasible => true,
            judged if judged.within_reach() => false,
            _ => continue,
        };
        // A recipe on its own stock runs the lot its free stock covers.
        let lot = if row.family == Family::Recipe { lifescape_planner::stock_lot::lot_of(&method, &stock_held) } else { 0 };
        let lot = if lot > 0 { product_room(&method).map_or(lot, |room| lot.min(room)) } else { lot };
        if row.family == Family::Recipe && lifescape_planner::stock_lot::stock_inputs(&method).is_some() && lot == 0 {
            continue;
        }
        let line = GuideLine { method: method.clone(), family: row.family.as_str().to_string(), profit_6000: profit, weight: merit_money(profit, row.family == Family::Market, affinity), runs: row.confidence.runs, feasible, lot };
        let better = best.get(&method).is_none_or(|known| (line.feasible, line.profit_6000) > (known.feasible, known.profit_6000));
        if better {
            best.insert(method, line);
        }
    }
    let mut out: Vec<GuideLine> = best.into_values().collect();
    out.sort_by(|left, right| right.profit_6000.total_cmp(&left.profit_6000).then_with(|| left.method.cmp(&right.method)));
    out
}

/// True for a money node that draws from the guide (a maker's post never
/// does: its one leg, plan Q6).
fn draws_money(node: &Node) -> bool {
    let make = lifescape_planner::market_maker::MAKE_METHOD;
    matches!(node.kind, NodeKind::MakeMoney | NodeKind::HaveCoins { .. }) && !(!node.alternatives.is_empty() && node.alternatives.iter().all(|alt| alt.method == make))
}

/// Publishes the bot's money lines in `ctx` (module doc). After
/// `brain_trade::publish` (the trade offers it maps market rows to).
pub(crate) fn publish(ctx: &mut ReadyCtx, node: &Node, life: &Life, live: &Value, job: &Job) {
    if !draws_money(node) {
        return;
    }
    // Its own candidates (the alternatives a previous resolve left) never
    // reserve the stock from it.
    let keep = crate::brain_alch::kept_except(life, live, Some(&node.id));
    publish_keeping(ctx, node, life, live, job, &keep);
}

/// `publish` with what the bot keeps already read (`keep`: its
/// `brain_alch::kept_except` for `node`; a filler window reads it once
/// for its alchemy and its money lines, lot 3 review, CPU).
pub(crate) fn publish_keeping(ctx: &mut ReadyCtx, node: &Node, life: &Life, live: &Value, job: &Job, keep: &[String]) {
    if !draws_money(node) {
        return;
    }
    let Some(set) = crate::guides_store::current() else {
        crate::logln!("brain {}: {} money: no guide published yet, static money methods", job.account, node.id);
        return;
    };
    let levels = ctx.levels.clone();
    let level_of = |skill: &str| levels.iter().find(|(name, _)| name == skill).map(|(_, level)| *level).unwrap_or(1);
    let stock = crate::brain_alch::own_stock(live, &life.account, keep);
    let read = bot_rows(&set, &level_of, &stock);
    let rows = read.all();
    for error in &read.errors {
        crate::logln!("brain {}: {} money guide on its stock: {error} (recipes read without its stock)", job.account, node.id);
    }
    // The job's holdings read once (`brain_plan::Holdings`), each object
    // counted once: the same figures as `holdings_count`.
    let holdings = crate::brain_plan::Holdings::of(job);
    let held = |item: &str| holdings.count(item) as f64;
    let asks = |item: &str| crate::brain_trade::cycle_asks(&set, item);
    let wallet = crate::brain_gear::free_coins(job, None);
    let quests = ctx.quests_done.clone();
    let bot = BotState { level_of: &level_of, combat: ctx.combat, quests_done: &quests, held: &held, coins: Some(wallet as f64), ironman: life.profile.ironman, asks: &asks };
    let service = crate::market_service::shared();
    // The other producers' claims as they stand NOW (plan équilibre
    // step 2, the public board): supply on its way, never this bot's.
    let claims = crate::market_claims::claimed_by_others(&job.account);
    let claimed = |item: &str| claims.get(item).copied().unwrap_or(0).max(0);
    let product_room = |method: &str| -> Option<i64> {
        let (product, way) = lifescape_planner::method_way::method_way(method)?;
        let id = crate::brain_gear::obj_id(&product)?;
        let view = service.as_ref()?.public_view(&job.account, id);
        let per_action = way.qty.map_or(1, |(least, most)| (least + most + 1) / 2).max(1);
        // The others' claimed units fill the room before this lot.
        let ahead = holdings.count(&product) + claimed(&product);
        Some(lifescape_planner::market_fit::absorbable_actions(&view, ahead, per_action))
    };
    let quote = |item: &str| set.quotes.get(item).cloned().map(|quote| Quote { claimed: claimed(item) as f64, ..quote });
    let shop = |item: &str| crate::web_guides::npc_volume_value(item);
    let demand = SharedDemand::new(&quote, &shop);
    let shared = |row: &MoneyRow| demand.profit(row);
    let lines = lines_for(&rows, ctx, &bot, &stock, life.profile.market_affinity, &product_room, &shared);
    if lines.is_empty() {
        crate::logln!("brain {}: {} money: no earning guide line within reach ({} rows read), static money methods", job.account, node.id, rows.len());
    }
    ctx.money_guide = lines;
}

/// What the run of a money or coin node brings to the market (plan
/// équilibre step 2): its product and units. A 100 % market lot its lot
/// (`node.batch`, products); a recipe on its own stock its lot of
/// actions times the products an action yields (a filler window: at
/// most what its budget makes at its measured rate); any other method its
/// units per 6000 ticks measured for the bot (`per_6000(method,
/// product)`) over the run's `budget_ticks`. `None`: nothing for the
/// market (alchemy and coins, a fight without a product, no rate).
pub fn run_claim(node: &Node, method: &str, budget_ticks: u64, per_6000: &dyn Fn(&str, &str) -> Option<f64>) -> Option<(String, i64)> {
    if method.starts_with("alch:") {
        return None;
    }
    let recipe = lifescape_planner::trade_craft::inner(method);
    let product = lifescape_planner::guide_methods::own_product(recipe).filter(|product| product != "coins")?;
    // Only a recipe on its own stock reads the node's batch (its lot of
    // actions, review lot 2 point 6): a batch an earlier stock lot left
    // never sizes a harvest.
    let stock_lot = lifescape_planner::stock_lot::stock_inputs(recipe).is_some();
    let units = if lifescape_planner::trade_craft::is_trade(method) {
        node.batch
    } else if stock_lot && node.batch > 0 {
        let (_, way) = lifescape_planner::method_way::method_way(recipe)?;
        let out = if way.expected_out() > 0.0 { way.expected_out() } else { 1.0 };
        let lot = (node.batch as f64 * out).floor() as i64;
        // A filler window stops at its budget (review lot 3, point 3):
        // what it makes at its measured rate, never more than its lot.
        let window = crate::brain_filler::is_filler_node(&node.id).then(|| per_6000(method, &product)).flatten();
        window.map_or(lot, |rate| lot.min(crate::market_claims::run_units(rate, budget_ticks as f64)))
    } else {
        crate::market_claims::run_units(per_6000(method, &product)?, budget_ticks as f64)
    };
    (units > 0).then_some((product, units))
}

/// Posts the run a money or coin node (or a filler window) launches on
/// the public order board (plan équilibre step 2, `market_claims`): its
/// product's units (`run_claim`) for twice its budget at most, until
/// its report (`release_run`). A run that brings nothing to the market
/// ends the node's previous claim.
pub(crate) fn claim_launch(account: &str, node: &Node, method: &str, budget: Option<(u64, u64)>, live: &Value) {
    if !matches!(node.kind, NodeKind::MakeMoney | NodeKind::HaveCoins { .. }) {
        return;
    }
    let Some((_, ticks)) = budget else { return };
    let snapshot = crate::rates_service::live_snapshot();
    let per_6000 = |method: &str, product: &str| match crate::brain_plan::farm_yield(&snapshot, method, &crate::rates_names::display_of(product), live) {
        Ok(rate) if rate > 0.0 => Some(rate),
        Ok(_) => None,
        Err(error) => {
            crate::rates_read::log_rate_error(&error);
            None
        }
    };
    let Some((product, units)) = run_claim(node, method, ticks, &per_6000) else {
        crate::market_claims::release_run(account, &node.id);
        return;
    };
    let secs = crate::market_claims::claim_secs(ticks as f64, crate::market_eta::live_tick_ms());
    crate::market_claims::claim_run(account, &node.id, &product, units, secs);
    crate::logln!("brain {account}: {} claims {units} {product} on the order board ({method}, {ticks} ticks)", node.id);
}

#[cfg(test)]
#[path = "brain_money_tests.rs"]
mod tests;
