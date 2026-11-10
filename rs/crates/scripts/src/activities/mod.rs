//! Activity scripts: one thin runnable per skill (CLI + loads loop).
//! Everything they call lives in `kit/` (drivers, pools, tools) or
//! `legacy/` (pre-brain life decisions).

#![forbid(unsafe_code)]

pub mod cook;
pub mod fishing;
pub mod market;
pub mod market_cleanup;
pub mod market_flip;
pub mod market_deliver;
pub mod market_shop;
pub mod melee;
pub mod mining;
pub mod woodcut;
