#![forbid(unsafe_code)]
//! Static navigation pieces: collision, local search and door-aware planning.
//! Pure data and algorithms: no network, no bot, no live state.

pub mod bfs;
pub mod buffer;
pub mod collision;
pub mod costs;
pub mod f2p;
pub mod loctypes;
pub mod nav;
pub mod plan;
pub mod reach;
pub mod route;
pub mod service;
pub mod service_plan;
pub mod service_queries;
pub mod static_world;
pub mod tables;
pub mod walk_costs;
pub mod walk_rank;
