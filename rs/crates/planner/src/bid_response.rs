//! Producers answer the market (user-approved point 7, 2026-11-14,
//! extended the same day to the whole market: « le brain doit être
//! capable d'évaluer la rentabilité en fonction de toutes les dépendances
//! et des rates, pour tout le marché »). On every goal draw, the brain
//! reads the PUBLIC book and asks, for every object with market value
//! (others' bids, a current price with volume, the bot's own alchemy
//! floor): what would a lot cost this bot to make, with every dependency,
//! and what will the market actually take of it? Pure: the book views,
//! the bot's levels, holdings, bans, quests, own coin rate, yields,
//! position and plans come in as arguments (no other bot's state, D22;
//! public claims are on the board).
//!
//! Cost of one unit (`unit_cost`, recursive over the recipe, `MAX_DEPTH`):
//! - buy it: the cheapest resting asks that hold the units needed (one
//!   trip from the fountain to where it is used);
//! - make it: a production leg the bot can work NOW (skill level met and
//!   not banned, quest done, every tool held or buyable — resting ask, or
//!   an NPC shop when D-A allows it —, a site safe and open for its
//!   combat level, not a guild-only one), its own ticks (6000 / units per
//!   6000: learned rates, else the prior), each input at ITS unit cost;
//!   one-off for the lot: the walk from its site to where the output goes
//!   (the next step, the fountain for the product), toll and boat fares,
//!   the tools it buys (`market_value::leg_site`);
//! - stock the bot holds costs what selling it would bring (its best
//!   public bid, else nothing) and no time;
//! - the cheaper of buy and make for the units needed, coins + ticks ×
//!   the bot's coin rate, one-off costs spread over the units.
//!
//! Training (2026-11-14): a leg up to `MAX_TRAIN_LEVELS` above the bot's
//! level of a production skill counts (`unit_cost_training`). Training is
//! an investment, not a cost of one lot: its net cost (time and inputs
//! minus what the products made while training sell for, saturation
//! applied) is charged to the lot only for its share
//! (`market_value::training_share`: nothing when the bot's ambition
//! climbs that skill anyway, one lot in `TRAINING_AMORTIZE_LOTS`
//! otherwise). The `Produce` goal blocks on the level and trains first.
//!
//! Opportunity (`opportunity`): the lot the market absorbs at the unit
//! cost (`market_value::absorbed_lot`: bids above cost, sales at the
//! current price over a day, the bot's alchemy floor; minus supply on its
//! way, public claims and the bot's own stock; its `PRODUCER_SPLIT`
//! share), the one-off trip of the lot (walks, the delivery at the
//! fountain, `HANDOVER_TICKS`), never a fixed round trip per answer.
//!
//! Scoring rule (documented in `docs/economy.md`): the opportunity's
//! gross rate = (revenue − coins spent) / ticks of the lot. It preempts
//! the bot's ambition only when it pays at least `BID_PREEMPT` × the
//! bot's own coin rate and nets at least `BID_MIN_PROFIT` coins after
//! paying for the time. Among several, the best gross rate wins, tilted
//! ±15 % by a per-bot, per-item word so equally good lots spread over the
//! fleet. Spread: a bot weighs an object only in the windows
//! (`BID_WINDOW_TICKS`) its word selects (one in `PRODUCER_SPREAD`).

use crate::market_fit::BookView;
use crate::market_value::{absorbed_lot, leg_site, training_share, walk_ticks, HANDOVER_TICKS, VARROCK_FOUNTAIN};
use crate::model::GoalKind;

/// Prefix of a bid goal's origin (`life.goal_origin`, `bid:<item>`).
pub const BID_PREFIX: &str = "bid:";
/// A bid must pay this many times the bot's own coin rate: a real
/// player switches as soon as a lot pays clearly better than its best
/// alternative (2.0 before 2026-11-14: no lot ever beat twice the best
/// realizable rate, nobody forged the demanded gear).
pub const BID_PREEMPT: f64 = 1.25;
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
/// Recipe depth explored (strung amulet, amulet, bar, ore).
const MAX_DEPTH: u8 = 4;
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
    /// Combat level (safe sites) and where the bot stands now.
    pub combat: u8,
    pub position: Option<(i64, i64, i64)>,
    /// Prince Ali Rescue done: the Al Kharid toll is free.
    pub free_pass: bool,
    /// Level the bot's own ambition climbs a skill to (0: none): training
    /// up to it is no cost of a lot.
    pub planned_level: &'a dyn Fn(&str) -> u8,
    /// Coins one alchemy cast of an object nets the bot, runes paid
    /// (`alchemy::Caster::net`; the casting time is charged here at the
    /// coin rate), `None` when it does not cast.
    pub alch_net: &'a dyn Fn(&str) -> Option<f64>,
    /// The planner's rate reader (`plan_read::PlanRead::rate`, R7): the
    /// brain's MEASURED yields (n >= 3 blended with the game prior, else
    /// the prior), `work_rate::prior_rate` offline. The training a lot
    /// carries is sized on it (actions, inputs burnt).
    pub rate: crate::work_rate::RateReader<'a>,
}

/// No plan: the default of a bot whose ambition climbs nothing.
pub fn no_plan(_: &str) -> u8 {
    0
}

/// No alchemy: the default of a bot that does not cast.
pub fn no_alch(_: &str) -> Option<f64> {
    None
}

/// Cost of one unit: coins spent and the bot's own ticks, with the plan
/// (`buy <item>`, `make <method>`, `held <item>`), plus what the lot pays
/// once whatever its size (walks between sites, fares, tools bought).
#[derive(Debug, Clone, PartialEq)]
pub struct UnitCost {
    pub coins: f64,
    pub ticks: f64,
    pub plan: Vec<String>,
    /// Levels the plan trains first (skill, live level, level needed):
    /// a one-off cost of the whole lot (`training_charge`), never per unit.
    pub train: Vec<(String, u8, u8)>,
    /// One-off coins of the lot: fares, tools bought.
    pub fixed_coins: f64,
    /// One-off ticks of the lot: walks between the sites of its steps
    /// down to where the output goes.
    pub fixed_ticks: f64,
    /// Walk from the fountain to where bought inputs are used (the
    /// purchases come in one handover: the longest counts).
    pub buy_trip: f64,
    /// Where the work of the lot starts (the deepest made step).
    pub start: Option<(i64, i64, i64)>,
}

impl UnitCost {
    fn bare(coins: f64, plan: String) -> Self {
        UnitCost { coins, ticks: 0.0, plan: vec![plan], train: Vec::new(), fixed_coins: 0.0, fixed_ticks: 0.0, buy_trip: 0.0, start: None }
    }

    /// Coins plus the bot's time at its coin rate, per unit.
    pub fn total(&self, coin_rate: f64) -> f64 {
        self.coins + self.ticks * coin_rate
    }

    /// One-off coins of the lot plus its one-off ticks at the coin rate.
    pub fn fixed_total(&self, coin_rate: f64) -> f64 {
        self.fixed_coins + (self.fixed_ticks + self.buy_trip) * coin_rate
    }

    /// Per unit, the one-off costs spread over `units`.
    pub fn total_for(&self, units: i64, coin_rate: f64) -> f64 {
        self.total(coin_rate) + self.fixed_total(coin_rate) / units.max(1) as f64
    }
}

/// One lot the bot can make and sell at a profit.
#[derive(Debug, Clone, PartialEq)]
pub struct Opportunity {
    pub item: String,
    pub count: i64,
    /// Best resting bid of another account (0: none, sold at the current
    /// price or alched).
    pub best_bid: i64,
    /// Coins the lot brings (`market_value::absorbed_lot`).
    pub revenue: f64,
    /// Units sold into bids, at the current price, alched.
    pub from_bids: i64,
    pub from_sales: i64,
    pub from_alch: i64,
    pub unit: UnitCost,
    /// Production ticks, the lot's walks and handover, the training
    /// share.
    pub ticks: f64,
    /// Coins spent: inputs, fares, tools, the training share.
    pub spent: f64,
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
pub fn per_unit_inputs(method: &str, inputs: &[(String, i64)]) -> Vec<(String, f64)> {
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

/// Cost of one unit of `item` when the bot needs `units` of it at the
/// fountain (module doc); `None` when it can neither buy nor make that
/// many now.
pub fn unit_cost(item: &str, units: i64, market: &dyn Fn(&str) -> Option<BookView>, bot: &Producer) -> Option<UnitCost> {
    cost_at(item, units.max(1), market, bot, Step { depth: 0, train: false, dest: VARROCK_FOUNTAIN })
}

/// `unit_cost` where a production leg up to `MAX_TRAIN_LEVELS` above the
/// bot's level counts, its training listed in `UnitCost::train`.
pub fn unit_cost_training(item: &str, units: i64, market: &dyn Fn(&str) -> Option<BookView>, bot: &Producer) -> Option<UnitCost> {
    cost_at(item, units.max(1), market, bot, Step { depth: 0, train: true, dest: VARROCK_FOUNTAIN })
}

/// Where the recursion stands: its depth, whether training counts, where
/// the output goes.
#[derive(Debug, Clone, Copy)]
struct Step {
    depth: u8,
    train: bool,
    dest: (i64, i64, i64),
}

fn cost_at(item: &str, units: i64, market: &dyn Fn(&str) -> Option<BookView>, bot: &Producer, step: Step) -> Option<UnitCost> {
    let view = market(item);
    if step.depth > 0 && (bot.held)(item) >= units {
        let bid = view.as_ref().and_then(BookView::best_bid).unwrap_or(0).max(0) as f64;
        return Some(UnitCost::bare(bid, format!("held {item}")));
    }
    let bought = (!bot.ironman).then(|| view.as_ref().and_then(|book| book.buy_cost(units))).flatten().map(|coins| {
        let mut unit = UnitCost::bare(coins / units as f64, format!("buy {item}"));
        unit.buy_trip = walk_ticks(VARROCK_FOUNTAIN, step.dest);
        unit
    });
    let made = if step.depth < MAX_DEPTH { make_cost(item, units, market, bot, step) } else { None };
    match (bought, made) {
        (Some(buy), Some(make)) => Some(if make.total_for(units, bot.coin_rate) < buy.total_for(units, bot.coin_rate) { make } else { buy }),
        (buy, make) => buy.or(make),
    }
}

fn make_cost(item: &str, units: i64, market: &dyn Fn(&str) -> Option<BookView>, bot: &Producer, step: Step) -> Option<UnitCost> {
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
            if !step.train || !gap_ok || !TRAINABLE.contains(&skill.as_str()) {
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
        let rate = (bot.units_per_6000)(&leg.method, item);
        if !rate.is_finite() || rate <= 0.0 {
            continue;
        }
        // Where the bot works it, seen from where the output goes.
        let Some(site) = leg_site(&leg.method, step.dest, bot.combat, bot.free_pass) else { continue };
        let mut candidate = UnitCost {
            coins: 0.0,
            ticks: 6000.0 / rate,
            plan: vec![format!("make {}", leg.method)],
            train: Vec::new(),
            fixed_coins: site.fare,
            fixed_ticks: walk_ticks(site.site, step.dest),
            buy_trip: 0.0,
            start: None,
        };
        let mut feasible = true;
        for tool in &leg.tools {
            if crate::tools::family_held(tool, 1, bot.held) > 0 {
                continue;
            }
            match tool_cost(tool, market) {
                // A tool serves the whole lot: bought once at the fountain.
                Some(price) => {
                    candidate.fixed_coins += price;
                    candidate.buy_trip = candidate.buy_trip.max(walk_ticks(VARROCK_FOUNTAIN, site.site));
                }
                None => {
                    feasible = false;
                    break;
                }
            }
        }
        if !feasible {
            continue;
        }
        let deeper = Step { depth: step.depth + 1, train: step.train, dest: site.site };
        for (input, per_unit) in per_unit_inputs(&leg.method, &leg.inputs) {
            let need = (per_unit * units as f64).ceil() as i64;
            let Some(sub) = cost_at(&input, need, market, bot, deeper) else {
                feasible = false;
                break;
            };
            candidate.coins += per_unit * sub.coins;
            candidate.ticks += per_unit * sub.ticks;
            candidate.fixed_coins += sub.fixed_coins;
            candidate.fixed_ticks += sub.fixed_ticks;
            candidate.buy_trip = candidate.buy_trip.max(sub.buy_trip);
            candidate.start = candidate.start.or(sub.start);
            candidate.plan.extend(sub.plan);
            training.extend(sub.train);
        }
        if !feasible {
            continue;
        }
        candidate.start = candidate.start.or(Some(site.site));
        candidate.train = merge_training(training);
        if best.as_ref().is_none_or(|known| candidate.total_for(units, bot.coin_rate) < known.total_for(units, bot.coin_rate)) {
            best = Some(candidate);
        }
    }
    best
}

/// Cost of MAKING one unit of `item` now when the bot makes `units` of
/// it: the make path `opportunity` prices a lot with (never the buy
/// path). Shared with the seller reserve (`brain_reserve`, 2026-11-14):
/// a reserve never exceeds it, so a producer never holds the stock the
/// bids it answered would buy.
pub fn production_cost(item: &str, units: i64, market: &dyn Fn(&str) -> Option<BookView>, bot: &Producer) -> Option<UnitCost> {
    make_cost(item, units.max(1), market, bot, Step { depth: 0, train: false, dest: VARROCK_FOUNTAIN })
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

/// One trained skill, priced.
#[derive(Debug, Clone, PartialEq)]
pub struct TrainingRow {
    pub skill: String,
    pub to: u8,
    pub coins: f64,
    pub ticks: f64,
    /// What the products made while training sell for (current price or
    /// best bid, times the share the book absorbs of them).
    pub products_value: f64,
}

/// Gross cost of the levels a plan trains, per skill. Each skill trains
/// with its reach tranche method from the live level
/// (`tranches::reach_method`: the static anvil product for smithing, its
/// bars at their unit cost), its actions sized by the server XP table
/// (`sizing`), each action at the bot's rate for that method, the inputs
/// it burns at their unit cost. `None` when the training cannot be priced
/// (no XP data, inputs nobody sells or makes).
pub fn training_rows(train: &[(String, u8, u8)], market: &dyn Fn(&str) -> Option<BookView>, bot: &Producer) -> Option<Vec<TrainingRow>> {
    let mut out = Vec::new();
    for (skill, from, to) in train {
        let method = crate::tranches::reach_method(skill, *from);
        let actions = crate::sizing::tranche_actions_with(bot.rate, &method, *from, *to)?;
        let product = crate::method_way::method_way(&method).map(|(item, _)| item).unwrap_or_default();
        let rate = (bot.units_per_6000)(&method, &product);
        if !rate.is_finite() || rate <= 0.0 {
            return None;
        }
        let mut row = TrainingRow { skill: skill.clone(), to: *to, coins: 0.0, ticks: actions as f64 * 6000.0 / rate, products_value: 0.0 };
        let shop = Step { depth: 1, train: false, dest: VARROCK_FOUNTAIN };
        for (input, units) in crate::sizing::tranche_consumption_with(bot.rate, &method, *from, *to) {
            let sub = cost_at(&input, units.max(1), market, bot, shop)?;
            row.coins += units as f64 * sub.coins;
            row.ticks += units as f64 * sub.ticks;
        }
        if !product.is_empty() && !bot.ironman {
            let book = market(&product);
            let price = book.as_ref().and_then(|view| view.current.or(view.best_bid())).unwrap_or(0).max(0) as f64;
            row.products_value = price * actions as f64 * crate::market_fit::saturation_factor(book.as_ref(), actions as f64);
        }
        out.push(row);
    }
    Some(out)
}

/// Gross one-off cost of the levels a plan trains: (coins, ticks).
pub fn training_cost(train: &[(String, u8, u8)], market: &dyn Fn(&str) -> Option<BookView>, bot: &Producer) -> Option<(f64, f64)> {
    training_rows(train, market, bot).map(|rows| rows.iter().fold((0.0, 0.0), |sum, row| (sum.0 + row.coins, sum.1 + row.ticks)))
}

/// What a lot carries of its training: (coins, ticks). The net cost of
/// each trained skill (time at the coin rate and inputs, minus what its
/// products sell for, never below zero) times its share
/// (`market_value::training_share`), split between coins and ticks in
/// the proportion of the gross cost.
pub fn training_charge(train: &[(String, u8, u8)], market: &dyn Fn(&str) -> Option<BookView>, bot: &Producer) -> Option<(f64, f64)> {
    let mut charged = (0.0, 0.0);
    for row in training_rows(train, market, bot)? {
        let gross = row.coins.max(0.0) + row.ticks * bot.coin_rate;
        if gross <= 0.0 {
            continue;
        }
        let net = (gross - row.products_value).max(0.0);
        let share = training_share((bot.planned_level)(&row.skill), row.to) * net / gross;
        charged.0 += row.coins.max(0.0) * share;
        charged.1 += row.ticks * share;
    }
    Some(charged)
}

/// The lot of one object worth making for this bot (module doc), or
/// `None` when nothing pays or the bot cannot make it now.
pub fn opportunity(item: &str, view: &BookView, market: &dyn Fn(&str) -> Option<BookView>, bot: &Producer) -> Option<Opportunity> {
    let alch = (bot.alch_net)(item).map(|net| net - crate::alchemy::CAST_TICKS * bot.coin_rate);
    let sales = view.current.map(|_| view.volume_24h.max(0) * crate::market_value::SALES_SHARE_PCT / 100).unwrap_or(0);
    if view.bids.is_empty() && sales <= 0 && alch.is_none() {
        return None;
    }
    let depth = (view.bid_depth() + sales).max(alch.map(|_| crate::market_value::ALCH_LOT).unwrap_or(0)).clamp(1, MAX_BID_LOT * PRODUCER_SPLIT);
    let unit = cost_at(item, depth, market, bot, Step { depth: 0, train: true, dest: VARROCK_FOUNTAIN })?;
    // Buying the object to resell is the merchants' trade, not a producer's.
    if !unit.plan.first().is_some_and(|step| step.starts_with("make ")) {
        return None;
    }
    let lot = absorbed_lot(view, unit.total_for(depth, bot.coin_rate), (bot.held)(item), alch, PRODUCER_SPLIT, MAX_BID_LOT)?;
    let (train_coins, train_ticks) = training_charge(&unit.train, market, bot)?;
    let reach = match (bot.position, unit.start) {
        (Some(from), Some(start)) => walk_ticks(from, start),
        _ => 0.0,
    };
    let ticks = unit.ticks * lot.count as f64 + unit.fixed_ticks + unit.buy_trip + reach + HANDOVER_TICKS + train_ticks;
    let spent = unit.coins * lot.count as f64 + unit.fixed_coins + train_coins;
    let rate = (lot.revenue - spent) / ticks;
    let profit = lot.revenue - spent - ticks * bot.coin_rate;
    Some(Opportunity {
        item: item.to_string(),
        count: lot.count,
        best_bid: view.best_bid().unwrap_or(0),
        revenue: lot.revenue,
        from_bids: lot.from_bids,
        from_sales: lot.from_sales,
        from_alch: lot.from_alch,
        unit,
        ticks,
        spent,
        rate,
        profit,
        score: rate * tilt(bot.seed, item),
    })
}

/// Why `item` is or is not a lot for this bot, stage by stage (the
/// `orchestrator bid-why` diagnostic): the same steps as `opportunity`
/// and `ranked_opportunities`, each one named with its numbers.
pub fn explain(item: &str, view: &BookView, market: &dyn Fn(&str) -> Option<BookView>, bot: &Producer) -> Vec<String> {
    let mut out = Vec::new();
    out.push(format!("spread window: {}", if answers_now(bot.seed, item, bot.tick) { "this bot weighs it now" } else { "another bot's turn (spread)" }));
    let legs = crate::obtain_legs::production_legs(item);
    if legs.is_empty() {
        out.push("no production leg: a producer never answers it".to_string());
        return out;
    }
    let alch = (bot.alch_net)(item).map(|net| net - crate::alchemy::CAST_TICKS * bot.coin_rate);
    let sales = view.current.map(|_| view.volume_24h.max(0) * crate::market_value::SALES_SHARE_PCT / 100).unwrap_or(0);
    out.push(format!("demand: best bid {:?}, bid depth {}, sales share {sales}, alch net {alch:?}", view.best_bid(), view.bid_depth()));
    if view.bids.is_empty() && sales <= 0 && alch.is_none() {
        out.push("no bid, no sales, no alchemy: nothing to answer".to_string());
        return out;
    }
    let depth = (view.bid_depth() + sales).max(alch.map(|_| crate::market_value::ALCH_LOT).unwrap_or(0)).clamp(1, MAX_BID_LOT * PRODUCER_SPLIT);
    let Some(unit) = cost_at(item, depth, market, bot, Step { depth: 0, train: true, dest: VARROCK_FOUNTAIN }) else {
        out.push(format!("no unit cost for {depth} units: no leg this bot can work (levels, tools, quests, bans, sites)"));
        return out;
    };
    out.push(format!("unit: {:.0} coins + {:.0} ticks, plan {}, train {:?}", unit.coins, unit.ticks, unit.plan.join(" < "), unit.train));
    out.extend(explain_make(item, depth, market, bot));
    if !unit.plan.first().is_some_and(|step| step.starts_with("make ")) {
        out.push("cheapest is buying it: a merchant's trade, not a producer's".to_string());
        return out;
    }
    let Some(lot) = absorbed_lot(view, unit.total_for(depth, bot.coin_rate), (bot.held)(item), alch, PRODUCER_SPLIT, MAX_BID_LOT) else {
        out.push(format!("no lot the book absorbs at a unit cost of {:.0} (bids under the cost)", unit.total_for(depth, bot.coin_rate)));
        return out;
    };
    let Some((train_coins, train_ticks)) = training_charge(&unit.train, market, bot) else {
        out.push("training charge unknown".to_string());
        return out;
    };
    match opportunity(item, view, market, bot) {
        Some(found) => {
            let floor = BID_PREEMPT * bot.coin_rate.max(crate::market_fit::MIN_COIN_RATE);
            out.push(format!(
                "lot {} (revenue {:.0}, training {train_coins:.0} coins + {train_ticks:.0} ticks): {:.2} coins/tick vs floor {floor:.2} (own {:.2} x {BID_PREEMPT}), profit {:.0} vs {BID_MIN_PROFIT}",
                lot.count, lot.revenue, found.rate, bot.coin_rate, found.profit
            ));
            out.push(if preempts(&found, bot.coin_rate) { "PREEMPTS: the bot answers it".to_string() } else { "below the floor: the ambition goes first".to_string() });
        }
        None => out.push("opportunity refused".to_string()),
    }
    out
}

/// Each production leg of `item` for this bot: why it is out, or its
/// unit cost (the `bid-why` make side).
fn explain_make(item: &str, units: i64, market: &dyn Fn(&str) -> Option<BookView>, bot: &Producer) -> Vec<String> {
    let mut out = Vec::new();
    let step = Step { depth: 0, train: true, dest: VARROCK_FOUNTAIN };
    for leg in crate::obtain_legs::production_legs(item) {
        let reason = match leg.skill.clone() {
            None => Some("no skill (drop or pickup)".to_string()),
            Some((skill, level)) if skill_banned(&skill, bot.avoid) => Some(format!("{skill} banned (level {level})")),
            Some((skill, level)) if i64::from((bot.level_of)(&skill).max(1)) + i64::from(MAX_TRAIN_LEVELS) < level => Some(format!("{skill} {level} beyond training reach")),
            _ if leg.quest.as_ref().is_some_and(|quest| !bot.quests_done.contains(quest)) => Some(format!("quest {:?} missing", leg.quest)),
            _ if crate::skeleton_deps::guild_only(&leg.method).is_some() => Some("guild only".to_string()),
            _ if !(bot.units_per_6000)(&leg.method, item).is_finite() || (bot.units_per_6000)(&leg.method, item) <= 0.0 => Some("no rate".to_string()),
            _ if leg_site(&leg.method, step.dest, bot.combat, bot.free_pass).is_none() => Some(format!("no site at combat {}", bot.combat)),
            _ => {
                let missing: Vec<String> = per_unit_inputs(&leg.method, &leg.inputs)
                    .into_iter()
                    .filter(|(input, per)| cost_at(input, (per * units as f64).ceil() as i64, market, bot, Step { depth: 1, train: true, dest: step.dest }).is_none())
                    .map(|(input, _)| input)
                    .collect();
                (!missing.is_empty()).then(|| format!("inputs with no cost: {}", missing.join(",")))
            }
        };
        match reason {
            Some(why) => out.push(format!("  make {}: out ({why})", leg.method)),
            None => out.push(format!("  make {}: workable", leg.method)),
        }
    }
    match make_cost(item, units, market, bot, step) {
        Some(made) => out.push(format!("  make cost: {:.0} coins + {:.1} ticks per unit (+{:.0} fixed coins, {:.0} fixed ticks), total {:.0} at the coin rate", made.coins, made.ticks, made.fixed_coins, made.fixed_ticks, made.total_for(units, bot.coin_rate))),
        None => out.push("  make cost: none".to_string()),
    }
    out
}

/// True when an opportunity preempts the bot's ambition (scoring rule).
pub fn preempts(found: &Opportunity, coin_rate: f64) -> bool {
    found.rate >= BID_PREEMPT * coin_rate.max(crate::market_fit::MIN_COIN_RATE) && found.profit >= BID_MIN_PROFIT
}

/// Every lot this bot would make now among `books` (object → public
/// view), spread-selected and preempting, best tilted score first.
pub fn ranked_opportunities(books: &[(String, BookView)], market: &dyn Fn(&str) -> Option<BookView>, bot: &Producer) -> Vec<Opportunity> {
    if bot.ironman {
        return Vec::new();
    }
    let mut out: Vec<Opportunity> = books
        .iter()
        .filter(|(item, _)| answers_now(bot.seed, item, bot.tick))
        .filter(|(item, _)| !crate::obtain_legs::production_legs(item).is_empty())
        .filter_map(|(item, view)| opportunity(item, view, market, bot))
        .filter(|found| preempts(found, bot.coin_rate))
        .collect();
    out.sort_by(|left, right| right.score.total_cmp(&left.score).then_with(|| left.item.cmp(&right.item)));
    out
}

/// The best lot this bot should make now (`ranked_opportunities`).
pub fn best_opportunity(books: &[(String, BookView)], market: &dyn Fn(&str) -> Option<BookView>, bot: &Producer) -> Option<Opportunity> {
    ranked_opportunities(books, market, bot).into_iter().next()
}

/// The goal and origin a chosen opportunity becomes: a production quota
/// (`GoalKind::Produce`, production legs only, inputs bought or made by
/// the sub-tasks), origin `bid:<item>` (no milestone: the ambition is
/// untouched and the next draw weighs it again). The default selling
/// lists the result once the quota is done; it opens at the best bid or
/// the current price.
pub fn bid_goal(found: &Opportunity) -> (GoalKind, String) {
    (GoalKind::Produce { item: found.item.clone(), count: found.count }, format!("{BID_PREFIX}{}", found.item))
}

#[cfg(test)]
#[path = "bid_response_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "bid_response_market_tests.rs"]
mod market_tests;
