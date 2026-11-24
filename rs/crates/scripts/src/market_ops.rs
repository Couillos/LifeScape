#![forbid(unsafe_code)]
//! Typed operations of the market protocol (lot 5.6, 2026-11-24): ONE
//! enum for the ops the daemon market speaks over `run/market.sock` and
//! in process (`MarketService::handle`), and the only place that frames
//! one (`frame`, `field`) or reads one back (`MarketOp::of`).
//!
//! The wire is unchanged: a JSON line `{"op": "<name>", ...}`. Replies
//! stay JSON values (the web and the workers read them as such); the
//! per-op payload fields are read by each handler.

use lifescape_json::{object, str_value, Value};

/// One market operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketOp {
    Quote,
    Book,
    History,
    Trades,
    List,
    Place,
    Amend,
    Cancel,
    MyOrders,
    MyDeals,
    Recent,
    DealArrived,
    DealReport,
    Pending,
    Convene,
    Sweep,
    Fund,
    Tick,
    Stats,
    SellerQuote,
}

impl MarketOp {
    pub const ALL: [MarketOp; 20] = [
        MarketOp::Quote,
        MarketOp::Book,
        MarketOp::History,
        MarketOp::Trades,
        MarketOp::List,
        MarketOp::Place,
        MarketOp::Amend,
        MarketOp::Cancel,
        MarketOp::MyOrders,
        MarketOp::MyDeals,
        MarketOp::Recent,
        MarketOp::DealArrived,
        MarketOp::DealReport,
        MarketOp::Pending,
        MarketOp::Convene,
        MarketOp::Sweep,
        MarketOp::Fund,
        MarketOp::Tick,
        MarketOp::Stats,
        MarketOp::SellerQuote,
    ];

    /// Wire name (also the `cpu_profile` label suffix `market:<name>`).
    pub fn name(self) -> &'static str {
        match self {
            MarketOp::Quote => "quote",
            MarketOp::Book => "book",
            MarketOp::History => "history",
            MarketOp::Trades => "trades",
            MarketOp::List => "list",
            MarketOp::Place => "place",
            MarketOp::Amend => "amend",
            MarketOp::Cancel => "cancel",
            MarketOp::MyOrders => "my_orders",
            MarketOp::MyDeals => "my_deals",
            MarketOp::Recent => "recent",
            MarketOp::DealArrived => "deal_arrived",
            MarketOp::DealReport => "deal_report",
            MarketOp::Pending => "pending",
            MarketOp::Convene => "convene",
            MarketOp::Sweep => "sweep",
            MarketOp::Fund => "fund",
            MarketOp::Tick => "tick",
            MarketOp::Stats => "stats",
            MarketOp::SellerQuote => "seller_quote",
        }
    }

    pub fn from_name(name: &str) -> Option<MarketOp> {
        MarketOp::ALL.into_iter().find(|op| op.name() == name)
    }

    /// The op of a request frame, or the refusal text the service has
    /// always answered (`unknown market op '<name>'`).
    pub fn of(request: &Value) -> Result<MarketOp, String> {
        let name = request.get("op").as_str().unwrap_or("");
        MarketOp::from_name(name).ok_or_else(|| format!("unknown market op '{name}'"))
    }

    /// The `op` field of a frame, to put first in a field list.
    pub fn field(self) -> (&'static str, Value) {
        ("op", str_value(self.name()))
    }

    /// A whole request frame: the op, then its fields.
    pub fn frame(self, fields: Vec<(&str, Value)>) -> Value {
        let mut all = vec![self.field()];
        all.extend(fields);
        object(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_op_round_trips_through_its_frame() {
        for op in MarketOp::ALL {
            assert_eq!(MarketOp::from_name(op.name()), Some(op), "{op:?}");
            let frame = op.frame(vec![("account", str_value("smith"))]);
            assert_eq!(MarketOp::of(&frame), Ok(op));
            assert_eq!(frame.get("account").as_str(), Some("smith"));
        }
        let names: std::collections::HashSet<&str> = MarketOp::ALL.iter().map(|op| op.name()).collect();
        assert_eq!(names.len(), MarketOp::ALL.len(), "names are unique");
    }

    #[test]
    fn an_unknown_or_missing_op_is_refused_with_the_old_text() {
        assert_eq!(MarketOp::of(&object(vec![("op", str_value("nope"))])), Err("unknown market op 'nope'".to_string()));
        assert_eq!(MarketOp::of(&object(vec![])), Err("unknown market op ''".to_string()));
    }
}
