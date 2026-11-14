#![forbid(unsafe_code)]
//! Static navigation pieces: collision, local search and door-aware planning.
//! Pure data and algorithms: no network, no bot, no live state.

pub mod bfs;
pub mod buffer;
pub mod collision;
pub mod costs;
pub mod custom_edge;
pub mod f2p;
mod fallback;
pub mod loctypes;
pub mod nav;
pub mod no_route;
pub mod plan;
pub mod pocket;
#[cfg(test)]
mod plan_budget_tests;
pub mod reach;
pub mod region_lists;
pub mod route;
pub mod route_cache;
pub mod route_patch;
pub mod route_path;
pub mod route_splice;
pub mod service;
pub mod service_accept;
#[cfg(test)]
mod service_costs_tests;
#[cfg(test)]
mod service_replay_tests;
#[cfg(test)]
mod route_cache_tests;
pub mod service_plan;
pub mod service_queries;
pub mod static_world;
pub mod tables;
pub mod toll_side;
pub mod walk_costs;
pub mod walk_rank;
