//! The brain GIVES the ambition that balances the market (plan progression
//! A4, user 2026-11-21, measure « b »): no random draw. Every unserved
//! demand weighs the gain per 6000 ticks of serving it, the level climb
//! included (`investment`, A1); the double logarithmic compression of
//! `ambition_catalog::compress` / `need_mass` is no part of this choice.
//!
//! ```text
//! value(bot, kind) = Σ over unserved bids P :  serves(kind, P) × asked(P) × investment(P).value_6000
//!                  + alchemy       (the mage: `investment::alchemy_investment`)
//!                  + outlet        (Σ over what the kind's training burns, o:
//!                                   min(burn_6000(o), pile(o) / (H / 6000)) × price(o))
//! score(bot, kind) = value(bot, kind) / (existing holders of kind + 1),
//!                    each bid counted on its RESIDUAL capacity: unmet - sum
//!                    over the kinds k of the bots ASSIGNED IN THIS PASS of
//!                    assigned(k) x serves(k, P) x units_per_tick(P) x H
//! ```
//!
//! - demand lines (`UnservedBid`): every source of market value the brain
//!   already reads (docs/economy.md, « Rentabilité de tout le marché »): the
//!   open bids (unit price, units unmet; a bid no resting sell offer answers
//!   weighs 1, with asks resting `ASKED_SHARE`), the market's SALES (per
//!   object traded in the last day, the units a new seller places over the
//!   horizon at the current price less the asks resting under it,
//!   `market_demand::sale_lines`), the alchemy of the natures on the asks,
//!   and the OUTLET of the consumer skills (user rule: consumer trades destroy
//!   objects, piles are settled by consuming more: what one bot of the kind
//!   burns over the horizon, bounded by the pile left after the other bots
//!   assigned in the pass, at the current price of the object);
//! - `investment(P)` reads the bot's own levels in the skills of the kind
//!   (`investment::investment`, `own`): the climb of an advanced bot is
//!   short, a fresh one pays the whole climb: that is the bot's fit (its
//!   advance in the skill), nothing else;
//! - sharing, two parts. The bots ALREADY on a kind when the pass starts
//!   divide its value by their number + 1 (the user's formula: the demand
//!   is still unserved with them there, they serve it in part, and a
//!   pinned crafter does not make amulets for being a crafter). The bots
//!   ASSIGNED DURING the pass (the rebalance, a fleet assignment) cover
//!   the demand they are given: dividing by holders + 1 alone piled 28 of
//!   74 bots on the mage on the live state, because one amulet bid weighs
//!   a million a unit while one mage makes 0.3 amulets a tick and the
//!   whole bid is a few dozen units (a bot works one thing at a time: its
//!   time goes to the densest lines of its kind first, `cover`); the
//!   residual capacity (units unmet
//!   less what the assigned bots make over the horizon, each weighted by
//!   how much its kind answers the bid) falls to zero once a few bots
//!   serve it and the next bots go to the next demand. Same for the
//!   alchemy (casts of the assigned mages against the units on the asks)
//!   and the outlet (what the assigned bots burn of the pile);
//! - the best score wins, deterministic; on a tie (no demand anywhere or all
//!   of it covered: every score 0): the kind with the fewest holders, then
//!   the larger outlet, then the best skill fit (`ambition_catalog::fit`),
//!   then the name;
//! - the rebalance changes a bot only when the best kind beats keeping its
//!   own by `MIN_GAIN_TO_CHANGE` (+25 %), both evaluated alike: the own kind
//!   shared by the bots on it (itself apart), no coverage by the others
//!   assigned in the pass.
//!
//! Applies (1) to every new ambition (`pick_by_value`, `assign_by_value`)
//! and (2) once, at the first start with this code, to the whole fleet
//! (`rebalance`, the only exception to A2). Never touched, never
//! reassigned: `PROTECTED_ACCOUNTS` and every operator ambition
//! (`FleetBot::operator`: pins, market maker, alcher, overall); their
//! kinds still count as holders. Pure.

use crate::ambition_catalog::{consumption_of, fit, group_skills, serves, spec, specs};
use crate::ambition_fleet::{Assignment, FleetBot};
use crate::investment::{alchemy_investment, investment, AlchOffer, INVESTMENT_HORIZON_TICKS};
use crate::work_rate::{RateReader, WINDOW_TICKS};

/// Accounts the A4 rebalance NEVER touches (user 2026-11-21): the three
/// level-led exceptions of D3 (`ambition_fleet::LEVEL_LED_ACCOUNTS`).
/// Matched without case; never reassigned, never counted as movable.
pub const PROTECTED_ACCOUNTS: [&str; 3] = ["Zezima", "N0valyfe", "Yogosun"];

/// Weight of a bid some resting sell offer already answers, against 1 for
/// a bid no ask answers (the old need doubled the unasked: same ratio).
pub const ASKED_SHARE: f64 = 0.5;

/// A bot of the one-time rebalance changes ambition only when the new
/// kind's value beats keeping its own by more than this share (+25 %),
/// both measured the same way (user 2026-11-21: a less brutal pass).
pub const MIN_GAIN_TO_CHANGE: f64 = 0.25;

/// True for an account of `PROTECTED_ACCOUNTS`.
pub fn protected(account: &str) -> bool {
    PROTECTED_ACCOUNTS.iter().any(|name| name.eq_ignore_ascii_case(account))
}

/// One unserved bid: the object, its best unit price, the units unmet,
/// whether any sell offer rests for it.
#[derive(Debug, Clone, PartialEq)]
pub struct UnservedBid {
    pub item: String,
    pub unit_price: f64,
    pub unmet: i64,
    pub asked: bool,
}

/// Everything the value reads besides the bot (module doc).
pub struct ValueContext<'a> {
    pub bids: &'a [UnservedBid],
    pub alch_offers: &'a [AlchOffer],
    /// (object, units held by the fleet beyond the bids): the piles.
    pub surplus: &'a [(String, f64)],
    /// Market price of an object (`None`: unknown).
    pub price: &'a dyn Fn(&str) -> Option<f64>,
    /// Best XP per tick of a skill at a level (`investment::best_xp_rate`).
    pub xp_rate: &'a dyn Fn(&str, u8) -> Option<f64>,
    pub rate: RateReader<'a>,
}

/// The value of one kind for one bot, with what drives it.
#[derive(Debug, Clone, PartialEq)]
pub struct KindValue {
    pub kind: String,
    /// Bids, sales, alchemy and outlet: coins per 6000 ticks.
    pub value_6000: f64,
    /// The part of `value_6000` that is the outlet of the piles the kind's
    /// training burns (also the first tie-break).
    pub outlet_6000: f64,
    /// The object carrying the most value, with its climb in ticks.
    pub driver: Option<(String, f64)>,
}

/// A kind the A4 choice gave, with its numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    pub kind: String,
    /// The kind's value for the bot given the bots already on the market.
    pub score: f64,
    pub value_6000: f64,
    pub outlet_6000: f64,
    /// The bot keeps its own kind (the rebalance: no change beats it by
    /// `MIN_GAIN_TO_CHANGE`).
    pub kept: bool,
    /// Bots already holding this kind.
    pub holders: usize,
    pub driver: Option<String>,
    pub climb_ticks: f64,
}

impl Pick {
    /// The ambition's `why`: `value:<driver>` or `value`.
    pub fn why(&self) -> String {
        self.driver.as_ref().map(|item| format!("value:{item}")).unwrap_or_else(|| "value".to_string())
    }

    /// The numbers, for the log line.
    pub fn numbers(&self) -> String {
        format!(
            "value {:.0} per 6000 ticks{}, {} bots already on {}{}",
            self.value_6000,
            if self.kept { " (kept: no change beats it by 25 %)" } else { "" },
            self.holders,
            self.kind,
            self.driver.as_ref().map(|item| format!(", driven by {item} (climb {:.0} ticks)", self.climb_ticks)).unwrap_or_default()
        )
    }
}

/// One bot of the rebalance, before and after.
#[derive(Debug, Clone, PartialEq)]
pub struct Reassignment {
    pub account: String,
    pub from: Option<String>,
    pub pick: Pick,
}

/// Kinds a draw may give: every catalogue kind but the operator-only
/// ones (generalist, overall, alcher, market maker).
fn drawable_kinds() -> Vec<&'static str> {
    specs().iter().map(|entry| entry.kind).filter(|kind| !crate::ambition::ladderless(kind) && *kind != crate::market_maker::MARKET_MAKER_KIND).collect()
}

/// Skills a kind's groups train.
fn own_skills(kind: &str) -> Vec<&'static str> {
    spec(kind).map(|entry| entry.groups.iter().flat_map(|group| group_skills(group)).collect()).unwrap_or_default()
}

/// What does not depend on the bot: the drawable kinds and how much each
/// answers each bid (`serves`).
struct Prepared<'a, 'ctx> {
    ctx: &'a ValueContext<'ctx>,
    kinds: Vec<&'static str>,
    /// `serves[bid][kind]`, 0 for a bid with no price or no unit unmet.
    serves: Vec<Vec<f64>>,
}

impl<'a, 'ctx> Prepared<'a, 'ctx> {
    fn new(ctx: &'a ValueContext<'ctx>) -> Prepared<'a, 'ctx> {
        let kinds = drawable_kinds();
        let serves: Vec<Vec<f64>> = ctx
            .bids
            .iter()
            .map(|bid| kinds.iter().map(|kind| if bid.unit_price > 0.0 && bid.unmet > 0 { serves(kind, &bid.item) } else { 0.0 }).collect())
            .collect();
        Prepared { ctx, kinds, serves }
    }
}

/// One unserved bid a kind answers, for one bot: the figures of its
/// investment, kept to re-evaluate under any set of holders.
struct Term {
    item: String,
    bid: usize,
    /// How much the kind answers the line (`serves`).
    serves: f64,
    /// `serves` of the kind × `ASKED_SHARE` when an ask rests.
    weight: f64,
    gain_per_tick: f64,
    margin: f64,
    capacity: f64,
    units_per_tick: f64,
    climb_ticks: f64,
}

/// What a kind's terms are for one bot (`terms_of`).
struct KindTerms {
    kind_index: usize,
    terms: Vec<Term>,
    /// The alchemy opening (mage): the same figures, serving the asks.
    alchemy: Option<Term>,
    /// (object, units a holder burns per 6000 ticks, pile, price).
    outlets: Vec<(String, f64, f64, f64)>,
}

fn terms_of(bot: &FleetBot, kind_index: usize, prep: &Prepared) -> KindTerms {
    let kind = prep.kinds[kind_index];
    let ctx = prep.ctx;
    let own = own_skills(kind);
    let level_of = |skill: &str| bot.level(skill);
    let mut terms = Vec::new();
    for (index, bid) in ctx.bids.iter().enumerate() {
        let share = prep.serves[index][kind_index];
        if share <= 0.0 {
            continue;
        }
        let Some(found) = investment(&bid.item, bid.unit_price, bid.unmet as f64, &own, &level_of, ctx.price, ctx.xp_rate, ctx.rate) else { continue };
        let weight = share * if bid.asked { ASKED_SHARE } else { 1.0 };
        terms.push(Term {
            item: bid.item.clone(),
            bid: index,
            serves: share,
            weight,
            gain_per_tick: found.gain_per_tick,
            margin: found.margin,
            capacity: found.capacity,
            units_per_tick: found.units_per_tick,
            climb_ticks: found.climb_ticks,
        });
    }
    let alchemy = own.contains(&"magic").then(|| alchemy_investment(&level_of, ctx.alch_offers, ctx.price, ctx.xp_rate)).flatten().map(|found| Term {
        item: "alchemy".to_string(),
        bid: usize::MAX,
        serves: 1.0,
        weight: 1.0,
        gain_per_tick: found.gain_per_tick,
        margin: found.margin,
        capacity: found.capacity,
        units_per_tick: found.units_per_tick,
        climb_ticks: found.climb_ticks,
    });
    let outlets = consumption_of(kind)
        .into_iter()
        .filter(|(_, per_6000)| *per_6000 > 0.0)
        .map(|(item, per_6000)| {
            let pile: f64 = ctx.surplus.iter().filter(|(known, _)| *known == item).map(|(_, units)| units.max(0.0)).sum();
            let unit_price = (ctx.price)(&item).unwrap_or(0.0).max(0.0);
            (item, per_6000, pile, unit_price)
        })
        .collect();
    KindTerms { kind_index, terms, alchemy, outlets }
}

/// Value of a kind's terms given `holders[kind index]` bots already on
/// each kind. A demand the holders already cover is no longer unserved:
/// each holder of a kind answering the bid covers `serves` x the units
/// one bot makes over the horizon; only the residual capacity counts
/// (module doc).
fn evaluate(terms: &KindTerms, existing: &[usize], assigned: &[usize], covered: &[f64], prep: &Prepared) -> KindValue {
    let horizon = INVESTMENT_HORIZON_TICKS;
    let windows = horizon / WINDOW_TICKS;
    let mut total = 0.0;
    let mut outlet = 0.0;
    let mut driver: Option<(String, f64, f64)> = None;
    // The bots already on the kind when the pass starts share its value
    // (`/ (holders + 1)`: the demand is still unserved with them there, so
    // they cover it only in part); the bots assigned during the pass cover
    // the capacity they make (below).
    let share = 1.0 / (1 + existing[terms.kind_index]) as f64;
    let mut count = |term: &Term, covered: f64| {
        let residual = (term.capacity - covered).max(0.0);
        let open = (horizon - term.climb_ticks).max(0.0);
        let value = share * term.weight * (term.gain_per_tick * open).min((term.margin * residual).max(0.0)) / windows;
        total += value;
        if value > 0.0 && driver.as_ref().is_none_or(|(_, known, _)| value > *known) {
            driver = Some((term.item.clone(), value, term.climb_ticks));
        }
    };
    for term in &terms.terms {
        count(term, covered[term.bid]);
    }
    if let Some(term) = &terms.alchemy {
        let casters = prep.kinds.iter().position(|kind| own_skills(kind).contains(&"magic")).map(|index| assigned[index]).unwrap_or(0);
        count(term, casters as f64 * term.units_per_tick * horizon);
    }
    for (_, per_6000, pile, unit_price) in &terms.outlets {
        let residual = (pile - assigned[terms.kind_index] as f64 * per_6000 * windows).max(0.0);
        outlet += share * per_6000.min(residual / windows) * unit_price;
    }
    KindValue { kind: prep.kinds[terms.kind_index].to_string(), value_6000: total + outlet, outlet_6000: outlet, driver: driver.map(|(item, _, climb)| (item, climb)) }
}

fn holder_counts(prep: &Prepared, held: &[&str]) -> Vec<usize> {
    prep.kinds.iter().map(|kind| held.iter().filter(|known| *known == kind).count()).collect()
}

/// The value of `kind` for `bot` with nobody on the market yet (module
/// doc).
pub fn kind_value(bot: &FleetBot, kind: &str, ctx: &ValueContext) -> KindValue {
    let prep = Prepared::new(ctx);
    let Some(index) = prep.kinds.iter().position(|known| *known == kind) else { return KindValue { kind: kind.to_string(), value_6000: 0.0, outlet_6000: 0.0, driver: None } };
    let none = vec![0; prep.kinds.len()];
    let no_cover = vec![0.0; prep.ctx.bids.len()];
    evaluate(&terms_of(bot, index, &prep), &none, &none, &no_cover, &prep)
}

/// Best kind for `bot` given the kinds `held` by the other bots (module
/// doc); `None` only when nothing is drawable.
pub fn pick_by_value(bot: &FleetBot, held: &[&str], ctx: &ValueContext, exclude: &[&str]) -> Option<Pick> {
    let prep = Prepared::new(ctx);
    let holders = holder_counts(&prep, held);
    let none = vec![0; prep.kinds.len()];
    let no_cover = vec![0.0; prep.ctx.bids.len()];
    let values: Vec<KindValue> = (0..prep.kinds.len()).filter(|index| !exclude.contains(&prep.kinds[*index])).map(|index| evaluate(&terms_of(bot, index, &prep), &holders, &none, &no_cover, &prep)).collect();
    best_of(bot, &values, &holders, &prep)
}

fn best_of(bot: &FleetBot, values: &[KindValue], holders: &[usize], prep: &Prepared) -> Option<Pick> {
    values
        .iter()
        .map(|row| {
            let held = prep.kinds.iter().position(|kind| *kind == row.kind).map(|index| holders[index]).unwrap_or(0);
            let pick = Pick {
                kind: row.kind.clone(),
                score: row.value_6000,
                value_6000: row.value_6000,
                outlet_6000: row.outlet_6000,
                kept: false,
                holders: held,
                driver: row.driver.as_ref().map(|(item, _)| item.clone()),
                climb_ticks: row.driver.as_ref().map(|(_, ticks)| *ticks).unwrap_or(0.0),
            };
            let skill_fit = fit(&row.kind, &|skill| bot.level(skill));
            (pick, skill_fit)
        })
        .max_by(|(left, left_fit), (right, right_fit)| {
            left.score
                .total_cmp(&right.score)
                .then_with(|| right.holders.cmp(&left.holders))
                .then_with(|| left.outlet_6000.total_cmp(&right.outlet_6000))
                .then_with(|| left_fit.total_cmp(right_fit))
                .then_with(|| right.kind.cmp(&left.kind))
        })
        .map(|(pick, _)| pick)
}

/// Fleet pass for the bots WITHOUT an ambition (the first pass that meets
/// one): the level-led exceptions and market makers keep their draw
/// (`ambition_fleet::draw_kind`); every other free bot takes the best
/// score, the best (bot, kind) pair first, each assignment raising the
/// holders of its kind (the demand it covers is no longer unserved). Bots
/// already holding an ambition are never reassigned here (A2).
pub fn assign_by_value(bots: &[FleetBot], demand: &[crate::ambition_catalog::KindDemand], ctx: &ValueContext) -> Vec<Assignment> {
    let mut held: Vec<String> = bots.iter().filter_map(|bot| bot.kind.clone()).collect();
    let mut out = Vec::new();
    let mut free: Vec<&FleetBot> = bots.iter().filter(|bot| bot.kind.is_none()).collect();
    free.sort_by(|left, right| left.account.cmp(&right.account));
    for bot in free.iter().filter(|bot| crate::ambition_fleet::level_led(bot)) {
        let refs: Vec<&str> = held.iter().map(String::as_str).collect();
        let (kind, why) = crate::ambition_fleet::draw_kind(bot, &[], &crate::ambition_fleet::saturate(demand, &refs), &[]);
        held.push(kind.clone());
        out.push(Assignment { account: bot.account.clone(), kind, why });
    }
    let movable: Vec<&FleetBot> = free.into_iter().filter(|bot| !crate::ambition_fleet::level_led(bot)).collect();
    for (bot, pick) in greedy(&movable, &mut held, ctx, None) {
        out.push(Assignment { account: bot.account.clone(), kind: pick.kind.clone(), why: pick.why() });
    }
    out
}

/// A bot assigned to a kind spends its time (the horizon) on the lines it
/// answers, the densest first (`gain_per_tick` x weight): each takes the
/// ticks its residual capacity needs at the bot's output, until the time
/// is gone. A bot works one thing at a time: it never covers every line.
fn cover(terms: &KindTerms, covered: &mut [f64]) {
    let mut lines: Vec<&Term> = terms.terms.iter().filter(|term| term.units_per_tick > 0.0 && term.serves > 0.0).collect();
    lines.sort_by(|left, right| (right.gain_per_tick * right.weight).total_cmp(&(left.gain_per_tick * left.weight)));
    let mut remaining = INVESTMENT_HORIZON_TICKS;
    for term in lines {
        if remaining <= 0.0 {
            break;
        }
        let residual = (term.capacity - covered[term.bid]).max(0.0);
        let output = term.serves * term.units_per_tick;
        let used = (residual / output).min(remaining);
        covered[term.bid] += used * output;
        remaining -= used;
    }
}

/// Best-pair-first assignment of `bots` over the kinds: `held` carries the
/// kinds already held and grows with each assignment.
fn greedy<'bot>(bots: &[&'bot FleetBot], held: &mut Vec<String>, ctx: &ValueContext, keep_margin: Option<f64>) -> Vec<(&'bot FleetBot, Pick)> {
    let prep = Prepared::new(ctx);
    let terms: Vec<Vec<KindTerms>> = bots.iter().map(|bot| (0..prep.kinds.len()).map(|index| terms_of(bot, index, &prep)).collect()).collect();
    let mut left: Vec<usize> = (0..bots.len()).collect();
    let mut out = Vec::new();
    let start: Vec<&str> = held.iter().map(String::as_str).collect();
    let existing = holder_counts(&prep, &start);
    let mut assigned = vec![0usize; prep.kinds.len()];
    // Units of each line the bots assigned in this pass already cover.
    let mut covered = vec![0.0f64; prep.ctx.bids.len()];
    while !left.is_empty() {
        let total: Vec<usize> = existing.iter().zip(&assigned).map(|(before, now)| before + now).collect();
        let mut best: Option<(usize, Pick, f64)> = None;
        for &index in &left {
            let values: Vec<KindValue> = terms[index].iter().map(|row| evaluate(row, &existing, &assigned, &covered, &prep)).collect();
            let Some(mut pick) = best_of(bots[index], &values, &total, &prep) else { continue };
            // The rebalance: keep the bot's own kind unless the best one
            // beats it by `keep_margin`, both evaluated alike.
            if let (Some(margin), Some(current)) = (keep_margin, bots[index].kind.as_deref()) {
                if let Some(current_index) = prep.kinds.iter().position(|kind| *kind == current) {
                    // Keeping: the bot's own kind valued alike, shared by
                    // the bots on it (itself apart), no coverage by the
                    // others assigned in this pass.
                    let mut others = existing.clone();
                    others[current_index] = others[current_index].saturating_sub(1);
                    let none = vec![0usize; prep.kinds.len()];
                    let untouched = vec![0.0f64; prep.ctx.bids.len()];
                    let own = vec![evaluate(&terms[index][current_index], &others, &none, &untouched, &prep)];
                    if let Some(mut kept) = best_of(bots[index], &own, &total, &prep) {
                        if pick.kind != current && pick.score <= kept.score * (1.0 + margin) {
                            kept.kept = true;
                            pick = kept;
                        }
                    }
                }
            }
            let skill_fit = fit(&pick.kind, &|skill| bots[index].level(skill));
            let better = best.as_ref().is_none_or(|(known_index, known, known_fit)| {
                pick.score
                    .total_cmp(&known.score)
                    .then_with(|| known.holders.cmp(&pick.holders))
                    .then_with(|| pick.outlet_6000.total_cmp(&known.outlet_6000))
                    .then_with(|| skill_fit.total_cmp(known_fit))
                    .then_with(|| bots[*known_index].account.cmp(&bots[index].account))
                    .is_gt()
            });
            if better {
                best = Some((index, pick, skill_fit));
            }
        }
        let Some((index, pick, _)) = best else { break };
        left.retain(|other| *other != index);
        if let Some(kind_index) = prep.kinds.iter().position(|kind| *kind == pick.kind) {
            assigned[kind_index] += 1;
            cover(&terms[index][kind_index], &mut covered);
        }
        held.push(pick.kind.clone());
        out.push((bots[index], pick));
    }
    out
}

/// The one-time rebalance of the whole fleet (A4 (2), the only exception
/// to A2): every bot that is neither protected (`PROTECTED_ACCOUNTS`) nor
/// an operator (`FleetBot::operator`) is reassigned, best pair first
/// (`greedy`), the protected and operator kinds counting as holders.
/// One row per movable bot, its kind before (`from`) and the pick.
pub fn rebalance(bots: &[FleetBot], ctx: &ValueContext) -> Vec<Reassignment> {
    let fixed = |bot: &FleetBot| protected(&bot.account) || bot.operator || bot.market_maker;
    // Every bot's kind counts as a tenant (the movable ones too: the
    // sharing is the fleet's state at the start of the pass).
    let mut held: Vec<String> = bots.iter().filter_map(|bot| bot.kind.clone()).collect();
    let mut movable: Vec<&FleetBot> = bots.iter().filter(|bot| !fixed(bot)).collect();
    movable.sort_by(|left, right| left.account.cmp(&right.account));
    let mut out: Vec<Reassignment> = greedy(&movable, &mut held, ctx, Some(MIN_GAIN_TO_CHANGE)).into_iter().map(|(bot, pick)| Reassignment { account: bot.account.clone(), from: bot.kind.clone(), pick }).collect();
    out.sort_by(|left, right| left.account.cmp(&right.account));
    out
}

#[cfg(test)]
#[path = "ambition_value_tests.rs"]
mod tests;
