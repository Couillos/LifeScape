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
use lifescape_planner::guide_model::{Family, MoneyRow};
use lifescape_planner::guide_pick::{merit_money, GuideLine};
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
            set.recipes_on_stock(view, stock, &|row| stock_recipe(row) && holds_stock(row, stock) && keeps(row)).unwrap_or_else(|error| {
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
    if method == lifescape_planner::guide_methods::LADDER {
        return false;
    }
    let probe = Node::new("probe", NodeKind::MakeMoney, Vec::new(), 0);
    lifescape_planner::map_method(method, &probe, None).is_some()
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
/// object on `bot.held`.
pub fn lines_for(rows: &[&MoneyRow], ctx: &ReadyCtx, bot: &BotState, stock: &[(String, f64)], affinity: f64) -> Vec<GuideLine> {
    let mut best: HashMap<String, GuideLine> = HashMap::new();
    for row in rows.iter().filter(|row| row.priced && row.profit_6000 > 0.0) {
        let Some(method) = launched(row, ctx) else { continue };
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
        // A trade lot ranks on its offer's profit (the lot it can buy).
        let profit = lifescape_planner::trade_craft::offer_in(ctx, &method).map(|offer| offer.profit_6000).unwrap_or(row.profit_6000);
        // A recipe on its own stock runs the lot its free stock covers.
        let lot = if row.family == Family::Recipe { lifescape_planner::stock_lot::lot_of(&method, &stock_held) } else { 0 };
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
    let Some(set) = crate::guides_store::current() else {
        crate::logln!("brain {}: {} money: no guide published yet, static money methods", job.account, node.id);
        return;
    };
    let levels = ctx.levels.clone();
    let level_of = |skill: &str| levels.iter().find(|(name, _)| name == skill).map(|(_, level)| *level).unwrap_or(1);
    // Its own candidates (the alternatives a previous resolve left) never
    // reserve the stock from it.
    let stock = crate::brain_alch::own_stock(live, &life.account, &crate::brain_alch::kept_except(life, live, Some(&node.id)));
    let read = bot_rows(&set, &level_of, &stock);
    let rows = read.all();
    for error in &read.errors {
        crate::logln!("brain {}: {} money guide on its stock: {error} (recipes read without its stock)", job.account, node.id);
    }
    let held = |item: &str| crate::brain_plan::holdings_count(job, item) as f64;
    let asks = |item: &str| crate::brain_trade::cycle_asks(&set, item);
    let wallet = crate::brain_gear::free_coins(job, None);
    let quests = ctx.quests_done.clone();
    let bot = BotState { level_of: &level_of, combat: ctx.combat, quests_done: &quests, held: &held, coins: Some(wallet as f64), ironman: life.profile.ironman, asks: &asks };
    let lines = lines_for(&rows, ctx, &bot, &stock, life.profile.market_affinity);
    if lines.is_empty() {
        crate::logln!("brain {}: {} money: no earning guide line within reach ({} rows read), static money methods", job.account, node.id, rows.len());
    }
    ctx.money_guide = lines;
}

#[cfg(test)]
#[path = "brain_money_tests.rs"]
mod tests;
