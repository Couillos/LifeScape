//! Producers respond to bids (user-approved point 7, 2026-11-14). On
//! every goal draw, the brain reads the PUBLIC book and asks: is there a
//! resting bid this bot can make the object for, and does filling it pay
//! more than what the bot was going to do? Pure: the book views, the
//! bot's levels, holdings, bans, quests, own coin rate and yields come in
//! as arguments (no other bot's state, D22).
//!
//! Cost of one unit (`unit_cost`, recursive over the recipe):
//! - buy it: the cheapest resting asks that hold the units needed;
//! - make it: a production leg the bot can work NOW (skill level met and
//!   not banned, quest done, every tool held or buyable — resting ask, or
//!   an NPC shop when D-A allows it —, not a guild-only site), its own
//!   ticks (6000 / units per 6000) plus each input at ITS unit cost;
//! - stock the bot holds costs what selling it would bring (its best
//!   public bid, else nothing) and no time;
//! - the cheaper of buy and make in coins + ticks × the bot's coin rate.
//!
//! Training (2026-11-14): a leg up to `MAX_TRAIN_LEVELS` above the
//! bot's level of a production skill counts for a bid
//! (`unit_cost_training`): its training (reach tranche method, server XP
//! table, the bot's rate, inputs at their unit cost) is a one-off cost of
//! the lot, added to the opportunity's coins and ticks; the `Produce`
//! goal blocks on the level and trains first (`missing:level:...`).
//!
//! Opportunity (`best_opportunity`): for each object with others' bids,
//! the bids that pay more than the unit cost (time included) are the
//! profitable depth; asks resting within 25 % above the best bid are
//! supply already on its way (sellers walk down into the bids) and come
//! off; the bot's own free stock comes off (it sells, no need to make);
//! the bot takes `1 / PRODUCER_SPLIT` of what remains (rounded up, at most
//! `MAX_BID_LOT`): no single producer rushes a whole book.
//!
//! Scoring rule (documented in `docs/economy.md`): the opportunity's
//! gross rate = (bid revenue − coins spent) / (production ticks + one
//! Varrock round trip). It preempts the bot's ambition only when it pays
//! at least `BID_PREEMPT` × the bot's own coin rate (a player drops its
//! plan for a bid worth twice its usual hour) and nets at least
//! `BID_MIN_PROFIT` coins after paying for the time. Among several, the
//! best gross rate wins, tilted ±15 % by a per-bot, per-item word so
//! equally good bids spread over the fleet. Spread: a bot answers an
//! object only in the windows (`BID_WINDOW_TICKS`) its word selects (one
//! in `PRODUCER_SPREAD`), so capable bots do not all draw the same bid at
//! once.

use crate::market_fit::BookView;
use crate::model::GoalKind;

/// Prefix of a bid goal's origin (`life.goal_origin`, `bid:<item>`).
pub const BID_PREFIX: &str = "bid:";
/// A bid must pay this many times the bot's own coin rate.
pub const BID_PREEMPT: f64 = 2.0;
/// Coins a bid goal must net after paying for the bot's time.
pub const BID_MIN_PROFIT: f64 = 50.0;
/// Share of the remaining profitable depth one producer takes (1/2).
pub const PRODUCER_SPLIT: i64 = 2;
/// One bot answers a given object one window in `PRODUCER_SPREAD`.
pub const PRODUCER_SPREAD: u64 = 2;
/// Spread window (ticks): the answering bots rotate every window.
pub const BID_WINDOW_TICKS: u64 = 6000;
/// At most this many units per bid goal.
pub const MAX_BID_LOT: i64 = 100;
/// Varrock round trip (buy inputs, sell the lot) charged once per goal.
pub const BID_TRIP_TICKS: f64 = 800.0;
/// Recipe depth explored (product, bar, ore).
const MAX_DEPTH: u8 = 3;
/// Asks this far above the best bid count as supply on its way.
const PENDING_SUPPLY_PCT: i64 = 125;
/// Levels a bot trains to answer a bid (live 2026-11-14: 18 bronze
/// scimitar bids up to 2961 and nobody answered: the whole fleet had
/// smithing 1, the scimitar needs 5). A player trains a few levels for a
/// bid worth it; the training time and inputs count in the opportunity.
pub const MAX_TRAIN_LEVELS: u8 = 15;
/// Skills trained for a bid: the production skills (combat, magic and
/// prayer train through their own goals).
const TRAINABLE: [&str; 8] = ["smithing", "crafting", "cooking", "mining", "fishing", "woodcutting", "firemaking", "runecraft"];

/// What the scorer reads of the bot (its own state only).
pub struct Producer<'a> {
    pub level_of: &'a dyn Fn(&str) -> u8,
    /// Free units held (pack, bank, worn; minus what its listings lock).
    pub held: &'a dyn Fn(&str) -> i64,
    /// Skills banned now (`bans::banned_skills`).
    pub avoid: &'a [String],
    pub quests_done: &'a [String],
    /// Coins one tick of its time is worth (`market_fit::coin_rate`).
    pub coin_rate: f64,
    /// Units per 6000 ticks of a production method for an object
    /// (measured rates, else the cold-start prior).
    pub units_per_6000: &'a dyn Fn(&str, &str) -> f64,
    pub ironman: bool,
    pub seed: u64,
    /// Daemon tick of the draw (selects the spread window).
    pub tick: u64,
}

/// Cost of one unit: coins spent and the bot's own ticks, with the plan
/// (`buy <item>`, `make <method>`, `held <item>`).
#[derive(Debug, Clone, PartialEq)]
pub struct UnitCost {
    pub coins: f64,
    pub ticks: f64,
    pub plan: Vec<String>,
    /// Levels the plan trains first (skill, live level, level needed):
    /// a one-off cost of the whole lot (`training_cost`), never per unit.
    pub train: Vec<(String, u8, u8)>,
}

impl UnitCost {
    /// Coins plus the bot's time at its coin rate.
    pub fn total(&self, coin_rate: f64) -> f64 {
        self.coins + self.ticks * coin_rate
    }
}

/// One bid the bot can fill at a profit.
#[derive(Debug, Clone, PartialEq)]
pub struct Opportunity {
    pub item: String,
    pub count: i64,
    pub best_bid: i64,
    /// Coins the `count` best profitable bids pay.
    pub revenue: f64,
    pub unit: UnitCost,
    /// Production ticks plus the round trip.
    pub ticks: f64,
    /// (revenue − coins) / ticks.
    pub rate: f64,
    /// revenue − coins − ticks × coin rate.
    pub profit: f64,
    /// `rate` tilted by the per-bot word (ranking only).
    pub score: f64,
}

/// Seeded word (FNV-1a of the salt, splitmix64 finalizer).
fn mix(seed: u64, salt: &str) -> u64 {
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    for byte in salt.bytes() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01B3);
    }
    let mut word = (seed ^ hash).wrapping_add(0x9E37_79B9_7F4A_7C15);
    word = (word ^ (word >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    word = (word ^ (word >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    word ^ (word >> 31)
}

/// True when this bot answers `item` in the window of `tick` (one window
/// in `PRODUCER_SPREAD`, rotating per bot and per object).
pub fn answers_now(seed: u64, item: &str, tick: u64) -> bool {
    let window = tick / BID_WINDOW_TICKS;
    mix(seed ^ window.wrapping_mul(0x9E37_79B9_7F4A_7C15), item) % PRODUCER_SPREAD == 0
}

/// Ranking tilt in [0.85, 1.15] for this bot and object.
fn tilt(seed: u64, item: &str) -> f64 {
    0.85 + (mix(seed, item) % 1000) as f64 / 1000.0 * 0.30
}

fn skill_banned(skill: &str, avoid: &[String]) -> bool {
    crate::side_goal::skill_banned(skill, avoid)
}

/// Inputs per unit made by `method` (the recipe's per-action inputs over
/// its expected output: two ores per iron bar).
fn per_unit_inputs(method: &str, inputs: &[(String, i64)]) -> Vec<(String, f64)> {
    let out = crate::method_way::method_way(method).map(|(_, way)| way.expected_out()).filter(|out| *out > 0.0).unwrap_or(1.0);
    inputs.iter().map(|(name, qty)| (name.clone(), *qty as f64 / out)).collect()
}

/// Coins to come by one tool the bot does not hold: the cheapest resting
/// ask, else an NPC shop when D-A allows it, else `None`.
fn tool_cost(tool: &str, market: &dyn Fn(&str) -> Option<BookView>) -> Option<f64> {
    if let Some(ask) = market(tool).and_then(|view| view.best_ask()) {
        return Some(ask.max(1) as f64);
    }
    if lifescape_gamedata::obtain::merchant_buy_allowed(tool) {
        return lifescape_gamedata::objs_by_name(tool).into_iter().next().map(|obj| obj.price.max(1) as f64);
    }
    None
}

/// Cost of one unit of `item` when the bot needs `units` of it (module
/// doc); `None` when it can neither buy nor make that many now.
pub fn unit_cost(item: &str, units: i64, market: &dyn Fn(&str) -> Option<BookView>, bot: &Producer) -> Option<UnitCost> {
    cost_at(item, units.max(1), market, bot, 0, false)
}

/// `unit_cost` where a production leg up to `MAX_TRAIN_LEVELS` above the
/// bot's level counts, its training listed in `UnitCost::train`.
pub fn unit_cost_training(item: &str, units: i64, market: &dyn Fn(&str) -> Option<BookView>, bot: &Producer) -> Option<UnitCost> {
    cost_at(item, units.max(1), market, bot, 0, true)
}

fn cost_at(item: &str, units: i64, market: &dyn Fn(&str) -> Option<BookView>, bot: &Producer, depth: u8, train: bool) -> Option<UnitCost> {
    let view = market(item);
    if depth > 0 && (bot.held)(item) >= units {
        let bid = view.as_ref().and_then(BookView::best_bid).unwrap_or(0).max(0) as f64;
        return Some(UnitCost { coins: bid, ticks: 0.0, plan: vec![format!("held {item}")], train: Vec::new() });
    }
    let bought = (!bot.ironman)
        .then(|| view.as_ref().and_then(|book| book.buy_cost(units)))
        .flatten()
        .map(|coins| UnitCost { coins: coins / units as f64, ticks: 0.0, plan: vec![format!("buy {item}")], train: Vec::new() });
    let made = if depth < MAX_DEPTH { make_cost(item, units, market, bot, depth, train) } else { None };
    match (bought, made) {
        (Some(buy), Some(make)) => Some(if make.total(bot.coin_rate) < buy.total(bot.coin_rate) { make } else { buy }),
        (buy, make) => buy.or(make),
    }
}

fn make_cost(item: &str, units: i64, market: &dyn Fn(&str) -> Option<BookView>, bot: &Producer, depth: u8, train: bool) -> Option<UnitCost> {
    let mut best: Option<UnitCost> = None;
    for leg in crate::obtain_legs::production_legs(item) {
        // Skilled or recipe ways only: combat drops and pickups need a
        // site and combat pick the scorer does not model.
        let Some((skill, level)) = leg.skill.clone() else { continue };
        if skill_banned(&skill, bot.avoid) {
            continue;
        }
        let live = (bot.level_of)(&skill).max(1);
        let mut training = Vec::new();
        if i64::from(live) < level {
            let gap_ok = level - i64::from(live) <= i64::from(MAX_TRAIN_LEVELS);
            if !train || !gap_ok || !TRAINABLE.contains(&skill.as_str()) {
                continue;
            }
            training.push((skill.clone(), live, u8::try_from(level).unwrap_or(99)));
        }
        if leg.quest.as_ref().is_some_and(|quest| !bot.quests_done.contains(quest)) {
            continue;
        }
        if crate::skeleton_deps::guild_only(&leg.method).is_some() {
            continue;
        }
        let mut coins = 0.0;
        let mut feasible = true;
        for tool in &leg.tools {
            if crate::tools::family_held(tool, 1, bot.held) > 0 {
                continue;
            }
            match tool_cost(tool, market) {
                // A tool serves the whole lot.
                Some(price) => coins += price / units as f64,
                None => {
                    feasible = false;
                    break;
                }
            }
        }
        if !feasible {
            continue;
        }
        let rate = (bot.units_per_6000)(&leg.method, item);
        if !rate.is_finite() || rate <= 0.0 {
            continue;
        }
        let mut ticks = 6000.0 / rate;
        let mut plan = vec![format!("make {}", leg.method)];
        for (input, per_unit) in per_unit_inputs(&leg.method, &leg.inputs) {
            let need = (per_unit * units as f64).ceil() as i64;
            let Some(sub) = cost_at(&input, need, market, bot, depth + 1, train) else {
                feasible = false;
                break;
            };
            coins += per_unit * sub.coins;
            ticks += per_unit * sub.ticks;
            plan.extend(sub.plan);
            training.extend(sub.train);
        }
        if !feasible {
            continue;
        }
        let candidate = UnitCost { coins, ticks, plan, train: merge_training(training) };
        if best.as_ref().is_none_or(|known| candidate.total(bot.coin_rate) < known.total(bot.coin_rate)) {
            best = Some(candidate);
        }
    }
    best
}

/// Cost of MAKING one unit of `item` now when the bot makes `units` of
/// it: the make path `opportunity` prices a quota with (never the buy
/// path). Shared with the seller reserve (`brain_reserve`, 2026-11-14):
/// a reserve never exceeds it, so a producer never holds the stock the
/// bids it answered would buy.
pub fn production_cost(item: &str, units: i64, market: &dyn Fn(&str) -> Option<BookView>, bot: &Producer) -> Option<UnitCost> {
    make_cost(item, units.max(1), market, bot, 0, false)
}

/// One entry per skill, the highest level needed.
fn merge_training(mut training: Vec<(String, u8, u8)>) -> Vec<(String, u8, u8)> {
    training.sort();
    let mut out: Vec<(String, u8, u8)> = Vec::new();
    for (skill, from, to) in training {
        match out.iter_mut().find(|(known, _, _)| *known == skill) {
            Some(entry) => entry.2 = entry.2.max(to),
            None => out.push((skill, from, to)),
        }
    }
    out
}

/// One-off cost of the levels a plan trains: (coins, ticks). Each skill
/// trains with its reach tranche method from the live level
/// (`tranches::reach_method`: smelting for smithing), its actions sized
/// by the server XP table (`sizing`), each action at the bot's rate for
/// that method, the inputs it burns at their unit cost. `None` when the
/// training cannot be priced (no XP data, inputs nobody sells or makes).
pub fn training_cost(train: &[(String, u8, u8)], market: &dyn Fn(&str) -> Option<BookView>, bot: &Producer) -> Option<(f64, f64)> {
    let mut coins = 0.0;
    let mut ticks = 0.0;
    for (skill, from, to) in train {
        let method = crate::tranches::reach_method(skill, *from);
        let actions = crate::sizing::tranche_actions(&method, *from, *to)?;
        let product = crate::method_way::method_way(&method).map(|(item, _)| item).unwrap_or_default();
        let rate = (bot.units_per_6000)(&method, &product);
        if !rate.is_finite() || rate <= 0.0 {
            return None;
        }
        ticks += actions as f64 * 6000.0 / rate;
        for (input, units) in crate::sizing::tranche_consumption(&method, *from, *to) {
            let sub = cost_at(&input, units.max(1), market, bot, 1, false)?;
            coins += units as f64 * sub.coins;
            ticks += units as f64 * sub.ticks;
        }
    }
    Some((coins, ticks))
}

/// The opportunity of one object's bids for this bot (module doc), or
/// `None` when nothing pays or the bot cannot make it now.
pub fn opportunity(item: &str, view: &BookView, market: &dyn Fn(&str) -> Option<BookView>, bot: &Producer) -> Option<Opportunity> {
    let best_bid = view.best_bid()?;
    let mut bids = view.bids.clone();
    bids.sort_by(|left, right| right.0.cmp(&left.0));
    let depth = view.bid_depth().min(MAX_BID_LOT * PRODUCER_SPLIT);
    let unit = cost_at(item, depth.max(1), market, bot, 0, true)?;
    let (train_coins, train_ticks) = training_cost(&unit.train, market, bot)?;
    // Buying the object to resell is the merchants' trade, not a producer's.
    if !unit.plan.first().is_some_and(|step| step.starts_with("make ")) {
        return None;
    }
    let cost = unit.total(bot.coin_rate);
    let profitable: Vec<i64> = bids
        .iter()
        .filter(|(price, _)| *price as f64 > cost)
        .flat_map(|(price, qty)| std::iter::repeat_n(*price, (*qty).max(0) as usize))
        .take(depth as usize)
        .collect();
    let ceiling = best_bid.saturating_mul(PENDING_SUPPLY_PCT) / 100;
    let pending: i64 = view.asks.iter().filter(|(price, _)| *price <= ceiling).map(|(_, qty)| (*qty).max(0)).sum();
    let open = profitable.len() as i64 - pending - (bot.held)(item).max(0);
    if open <= 0 {
        return None;
    }
    let count = ((open + PRODUCER_SPLIT - 1) / PRODUCER_SPLIT).clamp(1, MAX_BID_LOT);
    let revenue: f64 = profitable.iter().take(count as usize).map(|price| *price as f64).sum();
    let ticks = unit.ticks * count as f64 + BID_TRIP_TICKS + train_ticks;
    let spent = unit.coins * count as f64 + train_coins;
    let rate = (revenue - spent) / ticks;
    let profit = revenue - spent - ticks * bot.coin_rate;
    Some(Opportunity { item: item.to_string(), count, best_bid, revenue, unit, ticks, rate, profit, score: rate * tilt(bot.seed, item) })
}

/// True when an opportunity preempts the bot's ambition (scoring rule).
pub fn preempts(found: &Opportunity, coin_rate: f64) -> bool {
    found.rate >= BID_PREEMPT * coin_rate.max(crate::market_fit::MIN_COIN_RATE) && found.profit >= BID_MIN_PROFIT
}

/// The best bid this bot should answer now, among `books` (object →
/// public view): spread-selected, preempting, best tilted score.
pub fn best_opportunity(books: &[(String, BookView)], market: &dyn Fn(&str) -> Option<BookView>, bot: &Producer) -> Option<Opportunity> {
    if bot.ironman {
        return None;
    }
    let mut best: Option<Opportunity> = None;
    for (item, view) in books.iter().filter(|(_, view)| !view.bids.is_empty()) {
        if !answers_now(bot.seed, item, bot.tick) {
            continue;
        }
        let Some(found) = opportunity(item, view, market, bot) else { continue };
        if !preempts(&found, bot.coin_rate) {
            continue;
        }
        if best.as_ref().is_none_or(|known| found.score > known.score) {
            best = Some(found);
        }
    }
    best
}

/// The goal and origin a chosen opportunity becomes: a production quota
/// (`GoalKind::Produce`, production legs only, inputs bought or made by
/// the sub-tasks), origin `bid:<item>` (no milestone: the ambition is
/// untouched and the next draw weighs it again). The default selling
/// lists the result once the quota is done; it opens at the best bid.
pub fn bid_goal(found: &Opportunity) -> (GoalKind, String) {
    (GoalKind::Produce { item: found.item.clone(), count: found.count }, format!("{BID_PREFIX}{}", found.item))
}

#[cfg(test)]
#[path = "bid_response_tests.rs"]
mod tests;
