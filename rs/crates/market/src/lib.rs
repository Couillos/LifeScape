#![forbid(unsafe_code)]
//! Pure order-book market: matching, guards, deals, prices, trends, ZIP
//! strategy and offline simulator. No network, no files, no outside data.
//! Persistence lives in `lifescape-store`; the daemon socket in
//! `lifescape-orchestrator`. Everything here works on caller-supplied
//! values, so tests run with fixed seeds and no live game.

pub mod book;
pub mod deals;
pub mod error;
pub mod guards;
pub mod price;
pub mod sim;
pub mod trends;
pub mod zip;

pub use book::{Book, Fill, Order, Side};
pub use deals::{Deal, DealStatus, FailReason};
pub use error::Error;
pub use guards::{check_coverage, check_price};
pub use price::{bucketize, reference_price, vwap, Candle, PricedFill};
pub use trends::{compute_trend, TrendInput, TrendScores};
pub use zip::{MerchantCaps, ZipQuote};
