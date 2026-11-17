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

/// A current price (D12: median of the last five fills clamped by the
/// book) and whether a trade backs it, the median, how many fills it
/// rests on and the units they moved, and the resting book (non-dormant
/// levels, best first, (price, units)). The current price holds for the
/// units its fills moved only (`fill_units`, user 2026-11-17), the book
/// for the units it holds (bids for a sale, asks for a purchase), never
/// for a window's volume: an object without a fill has NO current price
/// (`price` then carries no value), and a clamp by the book moves the
/// price of the clamping orders' units only.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Quote {
    pub price: f64,
    pub traded: bool,
    /// Median of the last fills (0: none, or unknown: `price` stands).
    pub median: f64,
    /// Fills the median rests on (at most five).
    pub fills: usize,
    /// Units those fills moved (Σ quantities of the fills the median
    /// rests on, the last `FULL_FILLS` whatever their age: "recently"):
    /// the current price holds for these units only, whatever the number
    /// of fills (user 2026-11-17; 2026-11-16 for fewer than five).
    pub fill_units: f64,
    pub bids: Vec<(f64, f64)>,
    pub asks: Vec<(f64, f64)>,
}

/// Fills a current price needs to rest on to be called "traded" rather
/// than "thin" (D12 reads the last five). A confidence flag only: the
/// units the price holds for are `Quote::fill_units`.
pub const FULL_FILLS: usize = 5;

/// What a flow's unit price rests on (the guide's price confidence).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceBasis {
    /// The current price of a traded object (D12).
    Traded,
    /// The current price of an object traded fewer than `FULL_FILLS`
    /// times (D12 still: a median of what exists, flagged), or one that
    /// carries most of a volume running past the units its fills moved.
    Thin,
    /// A server shop's price (Q3 sale, Q7 purchase).
    Shop,
    /// Never traded: every unit at resting orders of the book.
    Book,
    /// Never traded: the book's depth, the rest at a shop.
    BookShop,
    /// Never traded: the book's depth, the rest worth nothing (no
    /// outlet) or not for sale (purchase unavailable).
    BookNone,
    /// Never traded, no book, no shop: worth nothing, or not for sale.
    Nothing,
    /// Coins, or farmed upstream (time, no price).
    Own,
}

impl PriceBasis {
    pub fn as_str(self) -> &'static str {
        match self {
            PriceBasis::Traded => "traded",
            PriceBasis::Thin => "thin",
            PriceBasis::Shop => "shop",
            PriceBasis::Book => "book",
            PriceBasis::BookShop => "book+shop",
            PriceBasis::BookNone => "book+none",
            PriceBasis::Nothing => "none",
            PriceBasis::Own => "own",
        }
    }

    /// Weakness of the basis: 0 a full trade history, a shop or no price
    /// needed (coins, farmed); 1 a thin trade history; 2 the book of a
    /// never-traded object or nothing.
    pub fn weakness(self) -> u8 {
        match self {
            PriceBasis::Traded | PriceBasis::Shop | PriceBasis::Own => 0,
            PriceBasis::Thin => 1,
            PriceBasis::Book | PriceBasis::BookShop | PriceBasis::BookNone | PriceBasis::Nothing => 2,
        }
    }
}

/// Price confidence of a row, its weakest flow: "traded" (every price
/// rests on five fills, a shop or none is needed), "thin" (one rests on
/// fewer fills), "book/npc" (one rests on the book of a never-traded
/// object, a shop's floor beyond it, or nothing).
pub fn price_confidence(flows: &[FlowLine]) -> &'static str {
    match flows.iter().map(|line| line.basis.weakness()).max().unwrap_or(0) {
        0 => "traded",
        1 => "thin",
        _ => "book/npc",
    }
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

/// What a bot needs to reach an F2P shop (user 2026-11-16, Q7: a shop
/// behind a condition is open to every bot that fulfils it, the
/// condition a prerequisite of the row trading there): an F2P quest done
/// (planner key: Oziach after `dragon`, the only quest gating an F2P
/// shop in 274) and quest points held (the Champions' Guild door, 32).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShopNeed {
    pub quest: Option<&'static str>,
    pub quest_points: i64,
}

impl ShopNeed {
    /// Both conditions (the stricter quest points; a quest kept).
    pub fn and(self, other: ShopNeed) -> ShopNeed {
        ShopNeed { quest: self.quest.or(other.quest), quest_points: self.quest_points.max(other.quest_points) }
    }

    /// True for a shop every bot reaches.
    pub fn is_open(self) -> bool {
        self.quest.is_none() && self.quest_points == 0
    }
}

/// Everything the engine reads, as closures (pure for the engine).
pub struct GuideWorld<'a> {
    /// The published yield of one ask (`rates_read`: n >= 3 blended with
    /// the game prior, else the prior); `Err` names why none exists.
    pub rate: &'a dyn Fn(&RateAsk) -> Result<RateAnswer, String>,
    /// Current market price of an object (D12: median of the last five
    /// fills clamped by the book, else the book, else 1 coin).
    pub price: &'a dyn Fn(&str) -> Quote,
    /// What the best reachable F2P shop pays for one unit (plan marché
    /// Q3), with what reaching it needs (Q7).
    pub npc_sale: &'a dyn Fn(&str) -> Option<(f64, ShopNeed)>,
    /// What F2P shops sell of an object in ONE window (6000 ticks):
    /// (unit price, units, condition) steps cheapest first, each
    /// reachable shop's base stock unit by unit at the server price for
    /// the stock left, then the window's restock (plan marché Q7,
    /// decision 2026-11-16 §2, `gamedata::shop_buy`); empty when no shop
    /// sells it.
    pub shop_buy: &'a dyn Fn(&str) -> Vec<(f64, f64, ShopNeed)>,
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
    /// feasible after that quest only (`guide_feasible`).
    pub quests_done: Vec<String>,
    /// Objects the bot holds (debug name, units): a recipe on stock is
    /// evaluated on that stock (R7), and the profile's feasibility
    /// (`guide_feasible`) reads it for stock, tools and market needs.
    /// Empty: the general guide, where nobody is assumed to hold anything.
    pub held: Vec<(String, f64)>,
    /// Per-skill levels of the profile (feasibility only, D22): they
    /// override `level` for the skills named; the view stays `level`'s.
    pub levels: Vec<(String, u8)>,
    /// Free coins of the profile (feasibility only, D22): `None` leaves a
    /// lot's capital unjudged (no wallet given).
    pub coins: Option<f64>,
}

impl GuideFilter {
    /// True when the filter describes a bot (D22 Q1, R7): a level, levels,
    /// quests, a stock or a wallet; its feasible lines then come first.
    pub fn has_profile(&self) -> bool {
        self.level.is_some() || !self.levels.is_empty() || !self.quests_done.is_empty() || !self.held.is_empty() || self.coins.is_some()
    }
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
    /// The price rests on a trade or a shop (false: the book of a
    /// never-traded object, or no price at all).
    pub traded: bool,
    pub basis: PriceBasis,
}

/// Availability of one object a row buys (D22): what the market and the
/// F2P shops held of it when the set was built, against what one lot
/// needs. Information only: each bot judges it at decision time
/// (`guide_feasible::feasible_for`, its own stock counted).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MarketNeed {
    pub item: String,
    /// `input` (100 % market) or a consumable cause (rune, ammo, bait...).
    pub cause: String,
    /// Units bought per 6000 ticks of the row.
    pub per_6000: f64,
    /// Units one lot needs (`guide_engine::need_lots`: a pack of the
    /// non-stackable objects the work moves, else the window).
    pub per_lot: f64,
    /// Units resting on the asks at the build.
    pub asks: f64,
    /// Units the object's last fills moved ("recently", the D12 window):
    /// shown, never counted as a supply (D21).
    pub recent: f64,
    /// Units the F2P shops sell in one window (stock + restock).
    pub shops: f64,
    /// False when nothing prices the object (never traded, no ask, no
    /// shop): the row's economy is unknown (`MoneyRow::priced`).
    pub priced: bool,
}

/// An object a row consumes that no activity makes at its level and
/// nothing prices (autonomous training: no autonomous source; a chain:
/// neither farmed nor priced): impossible for every bot today.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourceGap {
    pub item: String,
    pub reason: String,
}

/// Everything a row needs before a bot can work it (D9, D22): information
/// carried by the row, judged per bot by `guide_feasible::feasible_for`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Prereqs {
    /// (skill, level).
    pub levels: Vec<(String, u8)>,
    /// Combat level the sites ask (Wilderness included: a combat
    /// prerequisite, never a refusal).
    pub combat: u8,
    pub quests: Vec<String>,
    /// Quest points a shop the row trades with asks at its door
    /// (Champions' Guild: 32; Q7), 0 for none.
    pub quest_points: i64,
    pub tools: Vec<String>,
    /// Coins one lot ties up (100 % market).
    pub capital: f64,
    /// False for rows an Ironman cannot work (market family).
    pub ironman: bool,
    /// Stock one lot of a recipe on owned stock transforms (debug name,
    /// units): only a bot holding it can work the row (D5).
    pub stock: Vec<(String, f64)>,
    /// Every object the row buys, with its availability (D22).
    pub market: Vec<MarketNeed>,
    /// Objects nothing makes nor prices (D22: impossible today).
    pub sources: Vec<SourceGap>,
}

/// How a row comes by one input or consumable (decision 2026-11-16 §1,
/// §3): farmed by an activity (its ticks counted) or bought.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Farm,
    Buy,
}

impl Route {
    pub fn as_str(self) -> &'static str {
        match self {
            Route::Farm => "farm",
            Route::Buy => "buy",
        }
    }
}

/// The way chosen for one input or consumable of a chain or an
/// autonomous line, with what each way costs per unit of the item
/// (coins; farming: its ticks at the bot's D17 price less the other
/// flows of the source, `None` when the way is closed or unpriced).
#[derive(Debug, Clone, PartialEq)]
pub struct InputRoute {
    pub item: String,
    pub cause: String,
    pub route: Route,
    /// The farming activity (empty when bought).
    pub source: String,
    /// Units of the item per unit of the consuming stage's product
    /// (chains) or per 6000 ticks of the row (training).
    pub units: f64,
    pub farm_ticks_per_unit: Option<f64>,
    pub farm_coins_per_unit: Option<f64>,
    pub buy_coins_per_unit: Option<f64>,
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
    /// The way each input of a chain came by (farmed or bought).
    pub routes: Vec<InputRoute>,
    pub confidence: Confidence,
    /// "traded" or "book/npc" (`price_confidence`).
    pub price_confidence: &'static str,
    /// False when an object the row buys has no price at all (never
    /// traded, no ask, no shop: `MarketNeed::priced`) or one it consumes
    /// has no source (`Prereqs::sources`): revenue, cost and profit are
    /// then 0 and mean nothing, the row ranks after the priced ones (D22:
    /// never "blocked", its conditions say why).
    pub priced: bool,
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
    /// The bot's own stock transformed (R7: the money guide's recipe row
    /// read as training, inputs at their sale price, D5): only for a
    /// reader holding one lot of the stock (`GuideFilter::held`).
    Stock,
}

impl Supply {
    pub fn as_str(self) -> &'static str {
        match self {
            Supply::Autonomous => "autonomous",
            Supply::Market => "market",
            Supply::Quest => "quest",
            Supply::Stock => "stock",
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
    /// The way each input and consumable of an autonomous line came by.
    pub routes: Vec<InputRoute>,
    pub confidence: Confidence,
    /// "traded" or "book/npc" (`price_confidence`).
    pub price_confidence: &'static str,
    /// False as `MoneyRow::priced`: coins and the effective rate then
    /// mean nothing (0), the row ranks after the priced ones.
    pub priced: bool,
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
