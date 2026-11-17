//! 100 % market execution (plan rendements R5, D15): the planner method
//! `trade:craft/<recipe>` buys every input of a recipe on the market,
//! crafts it with the recipe's own script, and leaves the product to the
//! default selling (motive `trade`). Pure: the brain hands it the money
//! guide's market rows (`guides_store::current()`, never a build on
//! demand), the quotes of the cycle, the units the bot holds and its free
//! wallet.
//!
//! Sequence of one lot (one `make_money` node):
//! 1. resolve: the method ranks with the other money methods on the
//!    guide's profit per 6000 ticks (`profit_6000`); locked, it sizes a
//!    lot (`lot_size`: capital = the free wallet, depth = the resting
//!    asks, at most one window's volume of the row: the volume the guide
//!    priced) and blocks on the inputs short (`missing:a,b`);
//! 2. the block files one `obtain` per input with the `market:buy` leg
//!    ONLY (`replan`), sized on the lot (`need_count`): the existing buy
//!    legs bid at the ask, the brain parks them until the handover;
//! 3. inputs held, the node resolves again: the lot whose inputs were
//!    bought is crafted first whatever the ranking says now (`sticky`),
//!    `--want <product> <lot>` on the recipe's script (`mapping`);
//! 4. the craft run ends on the want, or on its inputs exhausted with
//!    products made (`lot_finished`: a burnt fish never re-buys); the node
//!    is done, the default sale lists the product (motive `trade`).
//!
//! The lot is recorded in the life's memory (`TradeLot`): predicted
//! profit (guide), purchases planned, products made and craft ticks; the
//! economy report (`orchestrator::trade_report`) adds the realized coins
//! from the deals. Ironman: never (no row, no method, no launch).
//!
//! R7 extension point: `pick_offer` (today the best profit) is where the
//! weighted draw of D10/D14 will choose among the offers.

use lifescape_json::{object, str_value, Value};

use crate::expand::ReadyCtx;
use crate::guide_model::{Family, MoneyRow, Outlet, Quote};
use crate::model::{Node, NodeStatus};

/// Prefix of the method: `trade:craft/<recipe>`, the recipe a guide
/// activity (`smelt:bronze_bar`, `cook:trout`, `jewelry:gold_ring`).
pub const PREFIX: &str = "trade:craft/";
/// Lots kept in a life's memory (the economy report reads them).
pub const LOTS_CAP: usize = 20;

/// The method of a recipe.
pub fn method_of(recipe: &str) -> String {
    format!("{PREFIX}{recipe}")
}

/// The recipe of a trade method (`None` for any other method).
pub fn recipe_of(method: &str) -> Option<&str> {
    method.strip_prefix(PREFIX).filter(|recipe| !recipe.is_empty())
}

/// True for a trade method.
pub fn is_trade(method: &str) -> bool {
    recipe_of(method).is_some()
}

/// The recipe of a trade method, the method itself otherwise: what the
/// game data (way, sites, quest, activity) knows the work by.
pub fn inner(method: &str) -> &str {
    recipe_of(method).unwrap_or(method)
}

/// One input of an offer, per unit of the product.
#[derive(Debug, Clone, PartialEq)]
pub struct TradeInput {
    pub item: String,
    pub per_unit: f64,
    /// Resting asks (price, units), cheapest first (the cycle's quote).
    pub asks: Vec<(f64, f64)>,
    /// Current price of a traded object (each unit costs at least it,
    /// D15 as in the guide), 0 when never traded.
    pub floor: f64,
    /// Units the bot holds and may use.
    pub held: i64,
}

/// One market row a bot can work, read from the guide.
#[derive(Debug, Clone, PartialEq)]
pub struct TradeOffer {
    pub method: String,
    pub product: String,
    /// Guide profit per 6000 ticks of the row's time (D6, D15).
    pub profit_6000: f64,
    /// Products per 6000 ticks of the row (the volume the guide priced).
    pub product_per_6000: f64,
    /// Price the guide sells one product at.
    pub unit_sale: f64,
    pub inputs: Vec<TradeInput>,
}

impl TradeOffer {
    /// Guide profit of one product (`profit_6000` over the products of
    /// the window).
    pub fn unit_profit(&self) -> f64 {
        if self.product_per_6000 > 0.0 {
            self.profit_6000 / self.product_per_6000
        } else {
            0.0
        }
    }

    /// Products the held inputs make now.
    pub fn makeable(&self) -> i64 {
        self.inputs
            .iter()
            .map(|input| if input.per_unit > 0.0 { ((input.held as f64 + 1e-9) / input.per_unit).floor() as i64 } else { i64::MAX })
            .min()
            .unwrap_or(0)
            .max(0)
    }
}

/// Causes of the negative flows a market row buys (the guide's
/// `InputPricing::Bought` plus its bought consumables).
fn bought_cause(cause: &str) -> bool {
    cause == "input" || crate::guide_engine::BOUGHT_CAUSES.contains(&cause)
}

/// The offer of one money row, or why it cannot be traded: a market
/// row, unblocked, earning, whose product is an object (alchemy turns
/// its object into coins: nothing to craft nor sell) and whose every
/// purchase is a market one (a shop purchase has no `market:buy` leg).
pub fn offer_from_row(row: &MoneyRow, quote: &dyn Fn(&str) -> Option<Quote>, held: &dyn Fn(&str) -> i64) -> Result<TradeOffer, String> {
    if row.family != Family::Market {
        return Err(format!("{}: not a market row ({})", row.method, row.family.as_str()));
    }
    if !row.blocked.is_empty() {
        return Err(format!("{}: blocked ({})", row.method, row.blocked.join("; ")));
    }
    if row.profit_6000 <= 0.0 {
        return Err(format!("{}: no profit ({:.0} / 6000)", row.method, row.profit_6000));
    }
    let product = crate::guide_methods::own_product(&row.method).filter(|product| product != "coins").ok_or_else(|| format!("{}: no product to sell", row.method))?;
    let sold = row.flows.iter().find(|line| line.item == product && line.per_6000 > 0.0).ok_or_else(|| format!("{}: the row makes no {product}", row.method))?;
    let (product_per_6000, unit_sale) = (sold.per_6000, sold.unit_price);
    let mut inputs: Vec<TradeInput> = Vec::new();
    for line in row.flows.iter().filter(|line| line.per_6000 < 0.0 && line.item != "coins" && bought_cause(&line.cause)) {
        if line.outlet != Outlet::Market {
            return Err(format!("{}: {} is bought at a {} (no market leg)", row.method, line.item, line.outlet.as_str()));
        }
        let per_unit = -line.per_6000 / product_per_6000;
        if let Some(known) = inputs.iter_mut().find(|input| input.item == line.item) {
            known.per_unit += per_unit;
            continue;
        }
        let quoted = quote(&line.item).unwrap_or_default();
        let mut asks: Vec<(f64, f64)> = quoted.asks.iter().copied().filter(|(price, units)| *price > 0.0 && *units > 0.0).collect();
        asks.sort_by(|left, right| left.0.total_cmp(&right.0));
        let floor = if quoted.traded { quoted.price.max(0.0) } else { 0.0 };
        inputs.push(TradeInput { item: line.item.clone(), per_unit, asks, floor, held: held(&line.item).max(0) });
    }
    if inputs.is_empty() {
        return Err(format!("{}: nothing to buy", row.method));
    }
    Ok(TradeOffer { method: method_of(&row.method), product, profit_6000: row.profit_6000, product_per_6000, unit_sale, inputs })
}

/// The bare offer of a trade method from its recipe's way (inputs per
/// product, the failures counted; no guide profit, no asks): what a lot
/// whose inputs were bought needs to be crafted when its row left the
/// guide (`sticky`). `None` for another method or a recipe without way.
pub fn recipe_offer(method: &str, held: &dyn Fn(&str) -> i64) -> Option<TradeOffer> {
    let recipe = recipe_of(method)?;
    let (product, way) = crate::method_way::method_way(recipe)?;
    let out = if way.expected_out() > 0.0 { way.expected_out() } else { 1.0 };
    let inputs: Vec<TradeInput> = way
        .inputs
        .iter()
        .map(|(item, per_action)| TradeInput { item: item.clone(), per_unit: (*per_action).max(1) as f64 / out, asks: Vec::new(), floor: 0.0, held: held(item).max(0) })
        .collect();
    (!inputs.is_empty()).then(|| TradeOffer { method: method.to_string(), product, profit_6000: 0.0, product_per_6000: 0.0, unit_sale: 0.0, inputs })
}

/// True when a bot (live `level_of`, `quests_done`, Ironman or not) may
/// work the row: an Ironman never trades; levels, quests and the quest
/// points of the shops it deals with (Q7) met.
pub fn eligible(row: &MoneyRow, level_of: &dyn Fn(&str) -> u8, quests_done: &[String], ironman: bool) -> bool {
    if ironman || row.family != Family::Market {
        return false;
    }
    let levels = row.prereqs.levels.iter().all(|(skill, level)| level_of(skill) >= *level);
    let quests = row.prereqs.quests.iter().all(|quest| quests_done.contains(quest));
    let points = row.prereqs.quest_points <= lifescape_gamedata::quest_requirements::quest_points_of(quests_done);
    levels && quests && points
}

/// Offers of the market rows a bot may work, one per method (the first
/// row of a method wins: the reader orders them), best profit first.
pub fn offers(rows: &[&MoneyRow], quote: &dyn Fn(&str) -> Option<Quote>, held: &dyn Fn(&str) -> i64, level_of: &dyn Fn(&str) -> u8, quests_done: &[String], ironman: bool) -> Vec<TradeOffer> {
    let mut out: Vec<TradeOffer> = Vec::new();
    for row in rows {
        if !eligible(row, level_of, quests_done, ironman) || out.iter().any(|known| known.method == method_of(&row.method)) {
            continue;
        }
        if let Ok(offer) = offer_from_row(row, quote, held) {
            out.push(offer);
        }
    }
    out.sort_by(|left, right| right.profit_6000.total_cmp(&left.profit_6000));
    out
}

/// R7 extension point: the offer a bot works among those it may. Today
/// the best guide profit; R7 replaces it by the weighted draw (D10, D14).
pub fn pick_offer(offers: &[TradeOffer]) -> Option<&TradeOffer> {
    offers.iter().max_by(|left, right| left.profit_6000.total_cmp(&right.profit_6000))
}

/// What bounds a lot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LotBound {
    /// The free wallet.
    Capital,
    /// The resting asks of an input.
    Depth,
    /// One window's volume of the row (what the guide priced).
    Window,
}

impl LotBound {
    pub fn as_str(self) -> &'static str {
        match self {
            LotBound::Capital => "capital",
            LotBound::Depth => "depth",
            LotBound::Window => "window",
        }
    }
}

/// A sized lot: products, coins its purchases cost at the book, what
/// bounds it, units bought per input.
#[derive(Debug, Clone, PartialEq)]
pub struct LotPlan {
    pub lot: i64,
    pub cost: i64,
    pub bound: LotBound,
    pub buys: Vec<(String, i64)>,
}

/// Units of `input` a lot of `lot` products still needs bought.
fn to_buy(input: &TradeInput, lot: i64) -> i64 {
    ((lot as f64 * input.per_unit - 1e-9).ceil() as i64 - input.held).max(0)
}

/// Coins `units` cost along the asks (each unit at least the current
/// price), `None` past their depth.
fn walk_cost(input: &TradeInput, units: i64) -> Option<f64> {
    let mut left = units as f64;
    let mut coins = 0.0;
    for (price, depth) in &input.asks {
        if left <= 0.0 {
            break;
        }
        let taken = depth.min(left);
        coins += taken * price.max(input.floor);
        left -= taken;
    }
    (left <= 1e-9).then_some(coins)
}

/// Coins a lot's purchases cost, `None` past the depth of an input.
fn lot_cost(offer: &TradeOffer, lot: i64) -> Option<f64> {
    offer.inputs.iter().try_fold(0.0, |total, input| walk_cost(input, to_buy(input, lot)).map(|coins| total + coins))
}

/// The lot a bot works: as many products as one window of the row (the
/// volume the guide priced), the asks of every input (held units first)
/// and the free wallet allow. `lot` 0: nothing fits (no capital, no ask).
pub fn lot_size(offer: &TradeOffer, wallet: i64) -> LotPlan {
    let window = (offer.product_per_6000.floor() as i64).max(1);
    let depth = offer
        .inputs
        .iter()
        .map(|input| {
            let resting: f64 = input.asks.iter().map(|(_, units)| *units).sum();
            if input.per_unit > 0.0 {
                (((input.held as f64 + resting) + 1e-9) / input.per_unit).floor() as i64
            } else {
                i64::MAX
            }
        })
        .min()
        .unwrap_or(0)
        .max(0);
    let (upper, upper_bound) = if depth < window { (depth, LotBound::Depth) } else { (window, LotBound::Window) };
    let fits = |lot: i64| lot_cost(offer, lot).is_some_and(|coins| coins <= wallet.max(0) as f64);
    // The cost grows with the lot: the largest lot the wallet pays.
    let (mut low, mut high) = (0i64, upper);
    while low < high {
        let middle = low + (high - low + 1) / 2;
        if fits(middle) {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    let bound = if low == upper { upper_bound } else { LotBound::Capital };
    let cost = lot_cost(offer, low).unwrap_or(0.0).ceil() as i64;
    let buys = offer.inputs.iter().map(|input| (input.item.clone(), to_buy(input, low))).filter(|(_, units)| *units > 0).collect();
    LotPlan { lot: low, cost, bound, buys }
}

/// The offer of `method` in the context.
pub fn offer_in<'a>(ctx: &'a ReadyCtx, method: &str) -> Option<&'a TradeOffer> {
    ctx.trades.iter().find(|offer| offer.method == method)
}

/// True when a trade method can start now for this bot: its offer is
/// published, the work's tools held (its inputs and consumables are the
/// lot's purchases), not quarantined nor cooled, its quest done, and a
/// lot fits (or held inputs make one).
pub fn workable(method: &str, ctx: &ReadyCtx) -> bool {
    if ctx.ironman || ctx.sick(method).is_some() || crate::quests::missing_quest(method, None, ctx).is_some() {
        return false;
    }
    let Some(offer) = offer_in(ctx, method) else { return false };
    let tools_held = crate::tools::required_tool(method).is_none_or(|need| need.tools.iter().all(|tool| crate::tools::gear_ok(tool, ctx)));
    tools_held && (offer.makeable() >= 1 || lot_size(offer, ctx.trade_wallet).lot >= 1)
}

/// Inputs bought for the node's last lot are crafted first, whatever the
/// ranking says now (never stranded stock): the node blocked on its trade
/// (`blocked_by`) and the held inputs make a product. Locks the method
/// with the lot the held inputs make; `false` otherwise.
pub fn sticky(node: &mut Node, ctx: &ReadyCtx) -> bool {
    let Some(method) = node.blocked_by.clone().filter(|method| is_trade(method)) else { return false };
    if ctx.ironman || node.excluded.contains(&method) {
        return false;
    }
    let Some(offer) = offer_in(ctx, &method) else { return false };
    let makeable = offer.makeable();
    if makeable < 1 {
        return false;
    }
    node.method = Some(method.clone());
    node.batch = makeable;
    if !node.alternatives.iter().any(|alt| alt.method == method) {
        node.alternatives.insert(0, crate::model::Alternative { method, est_ticks: 6000, est_coins: 0 });
    }
    true
}

/// Locks the chosen trade method on its lot: held inputs that already
/// make the lot craft at once; otherwise the node blocks on the inputs
/// short (`missing:a,b`), its batch the lot the purchases size on.
pub fn lock(node: &mut Node, ctx: &ReadyCtx) {
    let Some(method) = node.method.clone().filter(|method| is_trade(method)) else { return };
    let Some(offer) = offer_in(ctx, &method) else { return };
    let plan = lot_size(offer, ctx.trade_wallet);
    let makeable = offer.makeable();
    if makeable >= plan.lot.max(1) {
        node.batch = makeable;
        return;
    }
    node.batch = plan.lot;
    let short: Vec<String> = plan.buys.iter().map(|(item, _)| item.clone()).collect();
    if !short.is_empty() {
        node.status = NodeStatus::Blocked { reason: format!("missing:{}", short.join(",")) };
    }
}

/// Units of `need` a lot of `batch` products of the trade method
/// `method` asks (the recipe's way, its failures counted: an iron bar
/// costs two ores); `None` for another method, no batch, or a need its
/// way does not consume.
pub fn input_count(method: &str, need: &str, batch: i64) -> Option<i64> {
    let recipe = recipe_of(method)?;
    if batch <= 0 {
        return None;
    }
    let (_, way) = crate::method_way::method_way(recipe)?;
    let (_, per_action) = way.inputs.iter().find(|(input, _)| input == need)?;
    let out = if way.expected_out() > 0.0 { way.expected_out() } else { 1.0 };
    Some(((batch as f64 * (*per_action).max(1) as f64 / out) - 1e-9).ceil().max(1.0) as i64)
}

/// True when `need` is an input the trade method `method` buys on the
/// market (its recipe consumes it); a tool or anything else is not.
pub fn buys_input(method: &str, need: &str) -> bool {
    recipe_of(method).and_then(crate::method_way::method_way).is_some_and(|(_, way)| way.inputs.iter().any(|(input, _)| input == need))
}

/// True when a craft run of a trade lot that stopped `blocked` on
/// `missing` finished its lot: every missing object is an input of the
/// recipe (the bought stock is spent: burns and failures ate the rest)
/// and the run made products. Such a run closes the node; it never buys
/// again (the lot was what the purchases held).
pub fn lot_finished(method: &str, missing: &[String], made: i64) -> bool {
    let Some(recipe) = recipe_of(method) else { return false };
    let Some((_, way)) = crate::method_way::method_way(recipe) else { return false };
    made > 0 && !missing.is_empty() && missing.iter().all(|need| way.inputs.iter().any(|(input, _)| input == need))
}

/// One lot of a life (memory key `trades`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TradeLot {
    pub node: String,
    pub method: String,
    pub product: String,
    pub lot: i64,
    /// Units bought per input (planned at the lock).
    pub buys: Vec<(String, i64)>,
    /// Coins those purchases cost at the book when planned.
    pub cost: i64,
    /// What bounded the lot (`capital`, `depth`, `window`; `held` when
    /// the bot's stock made it).
    pub bound: String,
    /// Guide profit of the lot (profit per product × lot, D15/D6).
    pub predicted: i64,
    /// Guide products per 6000 ticks (the craft's yield it assumed).
    pub guide_per_6000: f64,
    /// Unix seconds of the lock.
    pub opened: i64,
    /// Products the craft run made (`None`: not crafted yet).
    pub made: Option<i64>,
    /// Work ticks of the craft run.
    pub craft_ticks: i64,
}

impl TradeLot {
    /// Products made per 6000 ticks of work (the craft's yield), `None`
    /// before the craft.
    pub fn craft_yield(&self) -> Option<f64> {
        let made = self.made?;
        (self.craft_ticks > 0).then(|| made as f64 * 6000.0 / self.craft_ticks as f64)
    }
}

/// Records the lot the node just locked (idempotent: a lot already open
/// for the node, purchases under way or crafting, is kept). Keeps the
/// last `LOTS_CAP` lots.
pub fn open_lot(lots: &mut Vec<TradeLot>, node: &Node, ctx: &ReadyCtx, now: i64) -> Option<TradeLot> {
    let method = node.method.clone().filter(|method| is_trade(method))?;
    if lots.iter().any(|lot| lot.node == node.id && lot.made.is_none()) {
        return None;
    }
    let offer = offer_in(ctx, &method)?;
    let plan = lot_size(offer, ctx.trade_wallet);
    let lot = node.batch.max(1);
    // The purchases of THIS lot (a lot the held stock makes buys nothing).
    let buys: Vec<(String, i64)> = offer.inputs.iter().map(|input| (input.item.clone(), to_buy(input, lot))).filter(|(_, units)| *units > 0).collect();
    let bound = if buys.is_empty() { "held" } else { plan.bound.as_str() };
    let record = TradeLot {
        node: node.id.clone(),
        method,
        product: offer.product.clone(),
        lot,
        buys,
        cost: lot_cost(offer, lot).unwrap_or(0.0).ceil() as i64,
        bound: bound.to_string(),
        predicted: (offer.unit_profit() * lot as f64).round() as i64,
        guide_per_6000: offer.product_per_6000,
        opened: now,
        made: None,
        craft_ticks: 0,
    };
    lots.push(record.clone());
    let excess = lots.len().saturating_sub(LOTS_CAP);
    if excess > 0 {
        lots.drain(..excess);
    }
    Some(record)
}

/// Records what the craft run of the node's open lot made.
pub fn note_made(lots: &mut [TradeLot], node: &str, made: i64, craft_ticks: i64) -> bool {
    match lots.iter_mut().rev().find(|lot| lot.node == node && lot.made.is_none()) {
        Some(lot) => {
            lot.made = Some(made.max(0));
            lot.craft_ticks = craft_ticks.max(0);
            true
        }
        None => false,
    }
}

/// Products of the life's lots (the sale lists them with motive `trade`).
pub fn lot_products(lots: &[TradeLot]) -> Vec<String> {
    let mut out: Vec<String> = lots.iter().map(|lot| lot.product.clone()).collect();
    out.sort();
    out.dedup();
    out
}

/// Inputs of every open money node working (or buying for) a trade: kept
/// from the sale and the janitor until crafted.
pub fn open_inputs(life: &crate::model::Life) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for node in &life.nodes {
        if matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned) {
            continue;
        }
        for method in node.method.iter().chain(node.blocked_by.iter()).filter(|method| is_trade(method)) {
            if let Some((_, way)) = recipe_of(method).and_then(crate::method_way::method_way) {
                out.extend(way.inputs.iter().map(|(input, _)| input.clone()));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn pairs_to_value(pairs: &[(String, i64)]) -> Value {
    Value::Arr(pairs.iter().map(|(item, units)| object(vec![("item", str_value(item)), ("units", Value::Int(*units))])).collect())
}

/// Memory shape of one lot.
pub fn lot_to_value(lot: &TradeLot) -> Value {
    object(vec![
        ("node", str_value(&lot.node)),
        ("method", str_value(&lot.method)),
        ("product", str_value(&lot.product)),
        ("lot", Value::Int(lot.lot)),
        ("buys", pairs_to_value(&lot.buys)),
        ("cost", Value::Int(lot.cost)),
        ("bound", str_value(&lot.bound)),
        ("predicted", Value::Int(lot.predicted)),
        ("guide_per_6000", Value::Float(lot.guide_per_6000)),
        ("opened", Value::Int(lot.opened)),
        ("made", lot.made.map(Value::Int).unwrap_or(Value::Null)),
        ("craft_ticks", Value::Int(lot.craft_ticks)),
    ])
}

/// One lot read back (`None` for a malformed record).
pub fn lot_from_value(value: &Value) -> Option<TradeLot> {
    let text = |key: &str| value.get(key).as_str().unwrap_or("").to_string();
    Some(TradeLot {
        node: value.get("node").as_str()?.to_string(),
        method: value.get("method").as_str()?.to_string(),
        product: text("product"),
        lot: value.get("lot").as_i64().unwrap_or(0),
        buys: value
            .get("buys")
            .as_arr()
            .map(|rows| rows.iter().filter_map(|row| Some((row.get("item").as_str()?.to_string(), row.get("units").as_i64().unwrap_or(0)))).collect())
            .unwrap_or_default(),
        cost: value.get("cost").as_i64().unwrap_or(0),
        bound: text("bound"),
        predicted: value.get("predicted").as_i64().unwrap_or(0),
        guide_per_6000: value.get("guide_per_6000").as_f64().unwrap_or(0.0),
        opened: value.get("opened").as_i64().unwrap_or(0),
        made: value.get("made").as_i64(),
        craft_ticks: value.get("craft_ticks").as_i64().unwrap_or(0),
    })
}

/// Lots of a memory value (absent in older files: none).
pub fn lots_from_value(value: &Value) -> Vec<TradeLot> {
    value.as_arr().map(|rows| rows.iter().filter_map(lot_from_value).collect()).unwrap_or_default()
}

#[cfg(test)]
#[path = "trade_craft_tests.rs"]
mod tests;
