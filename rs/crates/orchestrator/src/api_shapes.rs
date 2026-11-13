//! Frozen front API shapes (PRD 2, §6).
//!
//! The JSON the four dashboard pages consume, frozen so the wave-1 front
//! packets (P1.3 market, P1.4 rates, P1.5 lives/holdings) build against one
//! contract. Each shape carries its producer mapping and its example JSON;
//! deltas against current producers are marked DELTA for the wave-1
//! packet that closes them. Pure types plus `to_value`, no web serving.
//!
//! - Market §6.2: 24 h selected at load (D42), OHLC candles (D43) with a
//!   right volume scale, legend, tooltip (period, O/H/L/C, volume,
//!   execution count), fixed-slot time axis.
//! - Rates §6.3: no yield or XP for `market:*`/`shop:*` methods (D44, the
//!   merchant table carries their profit), provenance at the source with
//!   `unattributed` for the unexplained gap (D45, `picked` retired),
//!   every table sortable (D46, shared column keys below).
//! - Lives §6.4: full-width table, one account per row.
//! - Holdings §6.5: 24 h / 6 months curve, `res=1h|1d` (D42, D48).

#![forbid(unsafe_code)]

use lifescape_json::{object, str_value, Value};

// -- market (§6.2) -----------------------------------------------------------

/// Candle resolution: hourly (24 h view) or daily (6-month view).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandleRes {
    Hour1,
    Day1,
}

impl CandleRes {
    pub fn as_str(self) -> &'static str {
        match self {
            CandleRes::Hour1 => "1h",
            CandleRes::Day1 => "1d",
        }
    }

    /// DELTA (P1.3 serves it): unknown resolutions read as hourly, like
    /// the current `op_history`; the front only ever sends `1h` or `1d`.
    pub fn parse(raw: &str) -> CandleRes {
        match raw {
            "1d" => CandleRes::Day1,
            _ => CandleRes::Hour1,
        }
    }
}

/// One OHLCV candle: `{t,o,h,l,c,v,vb,vs,n}`. `n` is the execution count
/// in the window (tooltip §6.2). DELTA (P1.3): current rows carry no `n`;
/// the store counts it from the executions of the window. `vb`/`vs`: buy
/// and sell volume by taker side; `v - vb - vs` = unknown side (settled
/// before the split).
#[derive(Debug, Clone, PartialEq)]
pub struct Candle {
    pub t: i64,
    pub o: i64,
    pub h: i64,
    pub l: i64,
    pub c: i64,
    pub v: i64,
    pub vb: i64,
    pub vs: i64,
    pub n: i64,
}

impl Candle {
    pub fn to_value(&self) -> Value {
        object(vec![
            ("t", Value::Int(self.t)),
            ("o", Value::Int(self.o)),
            ("h", Value::Int(self.h)),
            ("l", Value::Int(self.l)),
            ("c", Value::Int(self.c)),
            ("v", Value::Int(self.v)),
            ("vb", Value::Int(self.vb)),
            ("vs", Value::Int(self.vs)),
            ("n", Value::Int(self.n)),
        ])
    }
}

/// One settled execution under the graph: `{q,p,t,s}` (matches
/// `op_trades`); `s` is the taker side, `"buy"`, `"sell"` or null.
#[derive(Debug, Clone, PartialEq)]
pub struct Exec {
    pub q: i64,
    pub p: i64,
    pub t: i64,
    pub s: Option<String>,
}

impl Exec {
    pub fn to_value(&self) -> Value {
        object(vec![
            ("q", Value::Int(self.q)),
            ("p", Value::Int(self.p)),
            ("t", Value::Int(self.t)),
            ("s", self.s.as_deref().map(str_value).unwrap_or(Value::Null)),
        ])
    }
}

/// `GET /api/market/history?item=&res=1h|1d&from=&to=`: `{ok,item,res,
/// candles,trades}` (matches `op_history`; candles gain `n`, see above).
#[derive(Debug, Clone, PartialEq)]
pub struct MarketHistory {
    pub item: i64,
    pub res: CandleRes,
    pub candles: Vec<Candle>,
    pub trades: Vec<Exec>,
}

impl MarketHistory {
    pub fn to_value(&self) -> Value {
        object(vec![
            ("ok", Value::Bool(true)),
            ("item", Value::Int(self.item)),
            ("res", str_value(self.res.as_str())),
            (
                "candles",
                Value::Arr(self.candles.iter().map(Candle::to_value).collect()),
            ),
            (
                "trades",
                Value::Arr(self.trades.iter().map(Exec::to_value).collect()),
            ),
        ])
    }
}

/// One market list row: `{item,name,price,change24h,volume24h}` (matches
/// `op_list`; `change24h` is thousandths, `volume24h` is summed over all
/// forms of the named good).
#[derive(Debug, Clone, PartialEq)]
pub struct MarketListRow {
    pub item: i64,
    pub name: String,
    pub price: i64,
    pub change24h: i64,
    pub volume24h: i64,
}

impl MarketListRow {
    pub fn to_value(&self) -> Value {
        object(vec![
            ("item", Value::Int(self.item)),
            ("name", str_value(&self.name)),
            ("price", Value::Int(self.price)),
            ("change24h", Value::Int(self.change24h)),
            ("volume24h", Value::Int(self.volume24h)),
        ])
    }
}

// -- rates (§6.3) -------------------------------------------------------------

/// Sortable columns (D46), shared with the front `table.js` (P1.4).
pub const YIELD_COLUMNS: &[&str] = &["method", "item", "band", "count", "mean", "stddev", "ema", "prior"];
pub const XP_COLUMNS: &[&str] = &["method", "skill", "band", "count", "mean", "stddev", "ema", "prior"];
pub const TRADING_COLUMNS: &[&str] = &["item", "profit_per_6000", "deals_settled", "fill_rate", "stock_value"];

/// One published yield row per 6000 ticks (matches `rates_payload`
/// `rates` entries; `market:*`/`shop:*` methods never appear here, D44).
#[derive(Debug, Clone, PartialEq)]
pub struct YieldRowShape {
    pub method: String,
    pub item: String,
    pub band: i64,
    pub count: usize,
    pub mean: f64,
    pub stddev: f64,
    pub ema: f64,
    pub prior: bool,
}

impl YieldRowShape {
    pub fn to_value(&self) -> Value {
        object(vec![
            ("method", str_value(&self.method)),
            ("item", str_value(&self.item)),
            ("band", Value::Int(self.band)),
            ("count", Value::Int(self.count as i64)),
            ("mean", Value::Float(self.mean)),
            ("stddev", Value::Float(self.stddev)),
            ("ema", Value::Float(self.ema)),
            ("prior", Value::Bool(self.prior)),
        ])
    }
}

/// One published XP row per 6000 ticks (matches `rates_payload` `xp`).
#[derive(Debug, Clone, PartialEq)]
pub struct XpRowShape {
    pub method: String,
    pub skill: String,
    pub band: i64,
    pub count: usize,
    pub mean: f64,
    pub stddev: f64,
    pub ema: f64,
    pub prior: bool,
}

impl XpRowShape {
    pub fn to_value(&self) -> Value {
        object(vec![
            ("method", str_value(&self.method)),
            ("skill", str_value(&self.skill)),
            ("band", Value::Int(self.band)),
            ("count", Value::Int(self.count as i64)),
            ("mean", Value::Float(self.mean)),
            ("stddev", Value::Float(self.stddev)),
            ("ema", Value::Float(self.ema)),
            ("prior", Value::Bool(self.prior)),
        ])
    }
}

/// One profitability row (matches `rates_payload` `profit` entries).
#[derive(Debug, Clone, PartialEq)]
pub struct ProfitRowShape {
    pub method: String,
    pub band: i64,
    pub revenue: f64,
    pub input_cost: f64,
    pub friction: f64,
    pub trend_bonus: f64,
    pub profit: f64,
    pub price_prior: bool,
}

impl ProfitRowShape {
    pub fn to_value(&self) -> Value {
        object(vec![
            ("method", str_value(&self.method)),
            ("band", Value::Int(self.band)),
            ("revenue", Value::Float(self.revenue)),
            ("input_cost", Value::Float(self.input_cost)),
            ("friction", Value::Float(self.friction)),
            ("trend_bonus", Value::Float(self.trend_bonus)),
            ("profit", Value::Float(self.profit)),
            ("price_prior", Value::Bool(self.price_prior)),
        ])
    }
}

/// One Trading-table row (D44): flip profit per 6000 ticks, settled deals,
/// execution rate, stock value. DELTA (P1.4): new table, served beside
/// `rates`; merchant profit lives here, never in the yield rows.
#[derive(Debug, Clone, PartialEq)]
pub struct TradingRow {
    pub item: String,
    pub profit_per_6000: f64,
    pub deals_settled: i64,
    pub fill_rate: f64,
    pub stock_value: i64,
}

impl TradingRow {
    pub fn to_value(&self) -> Value {
        object(vec![
            ("item", str_value(&self.item)),
            ("profit_per_6000", Value::Float(self.profit_per_6000)),
            ("deals_settled", Value::Int(self.deals_settled)),
            ("fill_rate", Value::Float(self.fill_rate)),
            ("stock_value", Value::Int(self.stock_value)),
        ])
    }
}

// -- lives (§6.4) --------------------------------------------------------------

/// Lives columns (D46), in display order: Compte · But · Profil · Tâche
/// active · Chaîne en cours · Engagement · Actives · Prêtes · En attente ·
/// Bloquées · Échouées · Faites · Dernier changement.
pub const LIVES_COLUMNS: &[&str] = &[
    "account",
    "goal",
    "profile",
    "active_task",
    "chain",
    "engagement",
    "active",
    "ready",
    "pending",
    "blocked",
    "failed",
    "done",
    "updated",
];

/// One lives row: one account per line, flat status counts (sortable,
/// D46). DELTA (P1.5): the current index serves `{account, goal, active,
/// ironman, nodes:{status:count}}`; `profile`, `active_task`, `chain`,
/// `engagement`, flat counts and `updated` are added by P1.5.
#[derive(Debug, Clone, PartialEq)]
pub struct LivesRow {
    pub account: String,
    pub goal: Value,
    pub profile: Value,
    pub active_task: Option<String>,
    pub chain: Vec<String>,
    pub engagement: Option<Value>,
    pub active: i64,
    pub ready: i64,
    pub pending: i64,
    pub blocked: i64,
    pub failed: i64,
    pub done: i64,
    pub updated: i64,
}

impl LivesRow {
    pub fn to_value(&self) -> Value {
        object(vec![
            ("account", str_value(&self.account)),
            ("goal", self.goal.clone()),
            ("profile", self.profile.clone()),
            (
                "active_task",
                self.active_task.as_deref().map(str_value).unwrap_or(Value::Null),
            ),
            (
                "chain",
                Value::Arr(self.chain.iter().map(|link| str_value(link)).collect()),
            ),
            (
                "engagement",
                self.engagement.clone().unwrap_or(Value::Null),
            ),
            ("active", Value::Int(self.active)),
            ("ready", Value::Int(self.ready)),
            ("pending", Value::Int(self.pending)),
            ("blocked", Value::Int(self.blocked)),
            ("failed", Value::Int(self.failed)),
            ("done", Value::Int(self.done)),
            ("updated", Value::Int(self.updated)),
        ])
    }
}

// -- holdings (§6.5) ----------------------------------------------------------

/// Holdings resolution: hourly (24 h view, 8 days kept) or daily
/// (6-month view, 400 days kept). Default is hourly (D42).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldingsRes {
    Hour1,
    Day1,
}

impl HoldingsRes {
    pub fn as_str(self) -> &'static str {
        match self {
            HoldingsRes::Hour1 => "1h",
            HoldingsRes::Day1 => "1d",
        }
    }

    pub fn parse(raw: &str) -> Option<HoldingsRes> {
        match raw {
            "1h" => Some(HoldingsRes::Hour1),
            "1d" => Some(HoldingsRes::Day1),
            _ => None,
        }
    }
}

/// `GET /api/items/history?res=1h|1d`: one curve, `{ok,item,res,
/// points:[{t,v}]}`. Daily values are the last measure of the day.
/// DELTA (P1.5): new endpoint over `holdings_hourly`/`holdings_daily`;
/// the current 30-minute sampler shape is replaced, imported once.
#[derive(Debug, Clone, PartialEq)]
pub struct HoldingsHistory {
    pub item: String,
    pub res: HoldingsRes,
    pub points: Vec<(i64, i64)>,
}

impl HoldingsHistory {
    pub fn to_value(&self) -> Value {
        object(vec![
            ("ok", Value::Bool(true)),
            ("item", str_value(&self.item)),
            ("res", str_value(self.res.as_str())),
            (
                "points",
                Value::Arr(
                    self.points
                        .iter()
                        .map(|(taken_at, value)| {
                            object(vec![("t", Value::Int(*taken_at)), ("v", Value::Int(*value))])
                        })
                        .collect(),
                ),
            ),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::{Candle, CandleRes, Exec, HoldingsHistory, HoldingsRes, LivesRow, MarketHistory, MarketListRow, ProfitRowShape, TradingRow, XpRowShape, YieldRowShape, LIVES_COLUMNS, TRADING_COLUMNS, XP_COLUMNS, YIELD_COLUMNS};
    use lifescape_json::{object, str_value, Value};

    fn keys(value: &Value) -> Vec<String> {
        match value {
            Value::Obj(map) => {
                let mut keys: Vec<String> = map.keys().cloned().collect();
                keys.sort();
                keys
            }
            _ => Vec::new(),
        }
    }

    /// Market history keys are frozen: the front reads these and nothing
    /// else. Candles carry the execution count (§6.2 tooltip).
    #[test]
    fn market_history_keys_are_frozen() {
        let history = MarketHistory {
            item: 7,
            res: CandleRes::Hour1,
            candles: vec![Candle { t: 1000, o: 5, h: 9, l: 4, c: 8, v: 120, vb: 70, vs: 40, n: 11 }],
            trades: vec![Exec { q: 10, p: 8, t: 1999, s: Some("buy".to_string()) }],
        };
        let value = history.to_value();
        assert_eq!(value.get("res").as_str(), Some("1h"));
        assert_eq!(keys(&value.get("candles").as_arr().expect("candles")[0]), ["c", "h", "l", "n", "o", "t", "v", "vb", "vs"]);
        assert_eq!(keys(&value.get("trades").as_arr().expect("trades")[0]), ["p", "q", "s", "t"]);
        assert_eq!(CandleRes::parse("1d"), CandleRes::Day1);
        assert_eq!(CandleRes::parse("unknown"), CandleRes::Hour1, "unknown reads hourly, like op_history");
    }

    /// Market list rows match `op_list`: id, display name, reference
    /// price, 24 h change (thousandths), 24 h volume.
    #[test]
    fn market_list_keys_are_frozen() {
        let row = MarketListRow {
            item: 7,
            name: "Feather".to_string(),
            price: 6,
            change24h: 120,
            volume24h: 4000,
        };
        let value = row.to_value();
        assert_eq!(
            keys(&value),
            ["change24h", "item", "name", "price", "volume24h"]
        );
        assert_eq!(value.get("name").as_str(), Some("Feather"));
    }

    /// Rates keys are frozen and sortable columns name exactly them.
    #[test]
    fn rates_keys_match_sortable_columns() {
        let yld = YieldRowShape {
            method: "gather:shrimp".to_string(),
            item: "Shrimp".to_string(),
            band: 1,
            count: 4,
            mean: 120.0,
            stddev: 5.0,
            ema: 118.0,
            prior: false,
        };
        let xp = XpRowShape {
            method: "gather:shrimp".to_string(),
            skill: "fishing".to_string(),
            band: 1,
            count: 4,
            mean: 300.0,
            stddev: 0.0,
            ema: 300.0,
            prior: true,
        };
        let profit = ProfitRowShape {
            method: "gather:shrimp".to_string(),
            band: 1,
            revenue: 720.0,
            input_cost: 0.0,
            friction: 3.0,
            trend_bonus: 0.0,
            profit: 717.0,
            price_prior: false,
        };
        let trading = TradingRow {
            item: "Feather".to_string(),
            profit_per_6000: 210.0,
            deals_settled: 6,
            fill_rate: 0.75,
            stock_value: 12000,
        };
        let mut ykeys = keys(&yld.to_value());
        ykeys.sort();
        let mut want: Vec<String> = YIELD_COLUMNS.iter().map(|key| key.to_string()).collect();
        want.sort();
        assert_eq!(ykeys, want);
        let mut xkeys = keys(&xp.to_value());
        xkeys.sort();
        let mut xwant: Vec<String> = XP_COLUMNS.iter().map(|key| key.to_string()).collect();
        xwant.sort();
        assert_eq!(xkeys, xwant);
        let mut tkeys = keys(&trading.to_value());
        tkeys.sort();
        let mut twant: Vec<String> = TRADING_COLUMNS.iter().map(|key| key.to_string()).collect();
        twant.sort();
        assert_eq!(tkeys, twant);
        assert_eq!(profit.to_value().get("profit").as_f64(), Some(717.0));
    }

    /// Lives rows are flat and countable: one account per line, every
    /// §6.4 column present, `updated` last.
    #[test]
    fn lives_row_covers_every_column() {
        let row = LivesRow {
            account: "t01".to_string(),
            goal: object(vec![("kind", str_value("max_skills"))]),
            profile: object(vec![("ironman", Value::Bool(false))]),
            active_task: Some("n9".to_string()),
            chain: vec!["mining/iron".to_string(), "smelting/iron".to_string()],
            engagement: None,
            active: 1,
            ready: 2,
            pending: 3,
            blocked: 0,
            failed: 1,
            done: 5,
            updated: 1762675200,
        };
        let value = row.to_value();
        let mut got = keys(&value);
        got.sort();
        let mut want: Vec<String> = LIVES_COLUMNS.iter().map(|key| key.to_string()).collect();
        want.sort();
        assert_eq!(got, want);
        assert_eq!(value.get("chain").as_arr().map(|rows| rows.len()), Some(2), "chain in progress rides along");
    }

    /// Holdings history: `res` is `1h` or `1d` only, points are time
    /// ordered pairs. Anything else is rejected loudly.
    #[test]
    fn holdings_history_freezes_res() {
        let history = HoldingsHistory {
            item: "Coins".to_string(),
            res: HoldingsRes::Day1,
            points: vec![(1762600000, 900), (1762686400, 1200)],
        };
        let value = history.to_value();
        assert_eq!(value.get("res").as_str(), Some("1d"));
        assert_eq!(value.get("points").as_arr().map(|rows| rows.len()), Some(2));
        assert_eq!(HoldingsRes::parse("1h"), Some(HoldingsRes::Hour1));
        assert_eq!(HoldingsRes::parse("30m"), None);
    }
}
