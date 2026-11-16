//! Guides model (plan rendements R4, §2): what the pure guide engine
//! reads and what it publishes. The engine (`guide_money`,
//! `guide_training`) never touches the store, the snapshot nor the
//! network: the daemon hands it a `GuideWorld` of closures over the
//! published yields (`rates_read`), the current prices (D12), the NPC
//! shops and the measured waits, so every rule is testable offline on a
//! fabricated world.
//!
//! Names are DEBUG names (`raw_shrimp`) everywhere in the engine; coins
//! are `coins` at one coin each.

use crate::guide_flows::OtherDrops;

/// Ticks of the published window.
pub const WINDOW_TICKS: f64 = 6000.0;
/// The deals' square: the Varrock fountain basin (`market_convene`: the
/// handover ring around x 3211..=3215, z 3427..=3431).
pub const VARROCK_SQUARE: (i64, i64, i64) = (3213, 3429, 0);
/// Pack slots one lot of a 100 % market run fills (a full pack).
pub const LOT_SLOTS: f64 = 28.0;
/// Deepest chain the engine builds (ore -> bar -> amulet -> strung ->
/// enchanted).
pub const MAX_CHAIN_DEPTH: usize = 4;

/// One yield the engine asks for: an activity (or planner method) at a
/// site and tool tier, for a bot at `level` in its skill.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RateAsk {
    pub method: String,
    pub style: String,
    pub site: Option<(i64, i64, i64)>,
    pub tool: String,
    pub level: u8,
}

/// Confidence of one yield (D9): measured or estimated, runs, age,
/// dispersion, failures, which key served it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Confidence {
    pub measured: bool,
    pub runs: u64,
    pub age_secs: i64,
    pub dispersion: f64,
    pub failure_rate: f64,
    /// `Exact`, `AllSites`, `AllSitesTools` or `PriorOnly`.
    pub read: String,
}

/// One answered yield, per 6000 ticks of work: signed flows (item,
/// cause, qty) and XP-bearing units (kind, subject, qty), debug names.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RateAnswer {
    pub flows: Vec<(String, String, f64)>,
    pub units: Vec<(String, String, f64)>,
    /// Market pause ticks per 6000 ticks of work (D6: counted in time).
    pub pause_per_6000: f64,
    pub confidence: Confidence,
}

impl RateAnswer {
    /// Units of `item` gained per 6000 ticks (positive flows).
    pub fn gained(&self, item: &str) -> f64 {
        self.flows.iter().filter(|(name, _, qty)| name == item && *qty > 0.0).map(|(_, _, qty)| qty).sum()
    }
}

/// A current price (D12), whether a trade backs it, and whether the
/// market can SELL it now (a fill or a resting ask): the 1-coin default
/// of a never-traded object values a sale, never a purchase.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quote {
    pub price: f64,
    pub traded: bool,
    pub buyable: bool,
}

/// One measured exact key of the estimator (the guides add a row for
/// each beside the prior's best site and tool).
#[derive(Debug, Clone, PartialEq)]
pub struct MeasuredKey {
    pub method: String,
    pub style: String,
    pub site: Option<(i64, i64, i64)>,
    pub tool: String,
    pub band: i64,
}

/// Everything the engine reads, as closures (pure for the engine).
pub struct GuideWorld<'a> {
    /// The published yield of one ask (`rates_read`: n >= 3 blended with
    /// the game prior, else the prior); `Err` names why none exists.
    pub rate: &'a dyn Fn(&RateAsk) -> Result<RateAnswer, String>,
    /// Current market price of an object (D12: median of the last five
    /// fills clamped by the book, else the book, else 1 coin).
    pub price: &'a dyn Fn(&str) -> Quote,
    /// What the best F2P shop pays for one unit (plan marché Q3).
    pub npc_sale: &'a dyn Fn(&str) -> Option<f64>,
    /// What the cheapest F2P shop asks for one unit (plan marché Q7).
    pub shop_buy: &'a dyn Fn(&str) -> Option<f64>,
    /// Measured market wait per lot (mean pause of the runs that
    /// paused), `None` when nothing was measured.
    pub market_wait_ticks: Option<f64>,
    /// Measured duration of a quest (planner key), `None` unmeasured.
    pub quest_ticks: &'a dyn Fn(&str) -> Option<f64>,
    /// Measured exact keys.
    pub measured: &'a [MeasuredKey],
}

/// Rows asked for: a skill, the level of a bot (every skill at it, its
/// quests done), a family.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GuideFilter {
    pub skill: Option<String>,
    /// `None`: each method at its own gate band (the catalogue view).
    pub level: Option<u8>,
    pub family: Option<String>,
    /// Quests the bot has done (planner keys): a row needing another is
    /// not accessible.
    pub quests_done: Vec<String>,
}

/// Money making family (§2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Family {
    /// Harvest or fight and sell what comes.
    Farm,
    /// Transform owned stock, inputs at their current sale price (D5).
    Recipe,
    /// Farm the inputs then transform them (D16).
    Chain,
    /// Buy every input, craft, sell (D15).
    Market,
}

impl Family {
    pub fn as_str(self) -> &'static str {
        match self {
            Family::Farm => "farm",
            Family::Recipe => "recipe",
            Family::Chain => "chain",
            Family::Market => "market",
        }
    }
}

/// Where a line's price comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outlet {
    /// The market's current price.
    Market,
    /// An F2P shop pays more than the market (Q3) or sells cheaper (Q7).
    Shop,
    /// Farmed upstream in the chain: time, no coins.
    Farmed,
}

impl Outlet {
    pub fn as_str(self) -> &'static str {
        match self {
            Outlet::Market => "market",
            Outlet::Shop => "shop",
            Outlet::Farmed => "farmed",
        }
    }
}

/// One priced flow of a row, per 6000 ticks of the row's time.
#[derive(Debug, Clone, PartialEq)]
pub struct FlowLine {
    pub item: String,
    pub cause: String,
    pub per_6000: f64,
    pub unit_price: f64,
    pub value_6000: f64,
    pub outlet: Outlet,
    /// The price rests on a trade (false: book or 1-coin default).
    pub traded: bool,
}

/// Everything a row needs before a bot can work it (D9).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Prereqs {
    /// (skill, level).
    pub levels: Vec<(String, u8)>,
    /// Combat level the sites ask (Wilderness included: a combat
    /// prerequisite, never a refusal).
    pub combat: u8,
    pub quests: Vec<String>,
    pub tools: Vec<String>,
    /// Coins one lot ties up (100 % market).
    pub capital: f64,
    /// False for rows an Ironman cannot work (market family).
    pub ironman: bool,
}

/// One stage of a chain (D16): what selling at this stage pays.
#[derive(Debug, Clone, PartialEq)]
pub struct Stage {
    pub method: String,
    pub product: String,
    /// Ticks to make one unit of this stage's product, upstream included.
    pub ticks_per_unit: f64,
    /// Profit per 6000 ticks when selling this stage's product.
    pub profit_6000: f64,
}

/// One money making line.
#[derive(Debug, Clone, PartialEq)]
pub struct MoneyRow {
    pub family: Family,
    pub method: String,
    pub style: String,
    pub site: Option<(i64, i64, i64)>,
    pub tool: String,
    pub band: i64,
    pub prereqs: Prereqs,
    pub flows: Vec<FlowLine>,
    pub other_drops: OtherDrops,
    pub revenue_6000: f64,
    pub cost_6000: f64,
    pub profit_6000: f64,
    /// (skill, XP per 6000 ticks of the row's time).
    pub xp_6000: Vec<(String, f64)>,
    /// Ticks added to each 6000 of work (pauses, market trips, waits).
    pub extra_ticks_per_6000: f64,
    pub stages: Vec<Stage>,
    pub confidence: Confidence,
    pub accessible: bool,
}

/// How a training row comes by its inputs (§2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Supply {
    /// Inputs farmed (time counted), no input: the work alone.
    Autonomous,
    /// Inputs bought, coins converted to time (D17).
    Market,
    /// A quest's fixed XP (D20).
    Quest,
}

impl Supply {
    pub fn as_str(self) -> &'static str {
        match self {
            Supply::Autonomous => "autonomous",
            Supply::Market => "market",
            Supply::Quest => "quest",
        }
    }
}

/// One training line.
#[derive(Debug, Clone, PartialEq)]
pub struct TrainingRow {
    pub skill: String,
    pub band: i64,
    pub method: String,
    pub style: String,
    pub site: Option<(i64, i64, i64)>,
    pub tool: String,
    pub supply: Supply,
    pub prereqs: Prereqs,
    /// XP of the trained skill per 6000 ticks of the row's own time.
    pub xp_6000: f64,
    /// Coins per 6000 ticks of the row's time (+ earned, − spent).
    pub coins_6000: f64,
    /// Coins spent per XP (negative: the training earns).
    pub coins_per_xp: f64,
    /// Ticks the spent coins cost at the best accessible money line
    /// (D17), per 6000 ticks of the row's time.
    pub converted_ticks_6000: f64,
    /// XP per 6000 ticks once the converted ticks are counted.
    pub effective_xp_6000: f64,
    /// Ticks to the end of the band at the effective rate (`None`: the
    /// quest is unmeasured).
    pub ticks_to_band_end: Option<f64>,
    /// XP of the other skills per 6000 ticks (bonus).
    pub other_xp_6000: Vec<(String, f64)>,
    /// A quest row's fixed XP (D20), 0 otherwise.
    pub quest_xp: f64,
    /// A quest row's measured duration in ticks (`None`: unmeasured).
    pub quest_ticks: Option<f64>,
    pub flows: Vec<FlowLine>,
    pub confidence: Confidence,
    pub accessible: bool,
}

/// A method the engine could not estimate (no measure, no prior).
#[derive(Debug, Clone, PartialEq)]
pub struct Unrated {
    pub method: String,
    pub reason: String,
}

/// First level of the 5-level band holding `level` (1, 6, 11...).
pub fn band_of(level: u8) -> i64 {
    let level = i64::from(level.max(1));
    ((level - 1) / 5) * 5 + 1
}
