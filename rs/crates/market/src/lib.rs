#![forbid(unsafe_code)]
//! Pure order-book market: matching, guards, deals, prices, trends, ZIP
//! strategy and offline simulator. No network, no files, no outside data.
//! Persistence lives in `lifescape-store`; the daemon socket in
//! `lifescape-orchestrator`. Everything here works on caller-supplied
//! values, so tests run with fixed seeds and no live game.

pub mod book;
pub mod concession;
pub mod deals;
pub mod discovery;
pub mod error;
pub mod guards;
pub mod maker;
pub mod outlets;
pub mod price;
pub mod sim;
pub mod sim_demand;
pub mod sim_discovery;
pub mod sim_lifecycle;
pub mod sim_maker;
pub mod sim_prd3;
pub mod trends;
pub mod zip;

pub use book::{Book, Fill, Order, Side};
pub use concession::{concession_beta, Concession, Step, Urgency, Window};
pub use deals::{Deal, DealStatus, FailReason};
pub use discovery::Anchors;
pub use error::Error;
pub use guards::{check_coverage, check_price};
pub use outlets::{market_pays, npc_sale_beats_market, npc_sale_min_unit};
pub use price::{bucketize, change_thousandths, current_price, impact_price, impact_size, median_price, vwap, Candle, LastFill, PricedFill, Reference, IMPACT_CAP, PRICE_FILLS};
pub use trends::{compute_trend, TrendInput, TrendScores};
pub use maker::{balanced_capital, common_capital_target, item_capital, lot_capital, lot_units, maker_count, partition, slice_need, quote as maker_quote, slice_capacity, Leg, MakerCaps, MakerInput, MakerQuote, Regime, MAKER_CAPS};
pub use zip::{demand_step, found_open, opening_ask, undercut_step, opening_bid, remembered_bid, seller_open, step_toward, OwnFill, ZipQuote};

#[cfg(test)]
mod found_tests;
#[cfg(test)]
mod outlier_tests;
