#![forbid(unsafe_code)]
//! Static navigation pieces: collision, local search and door-aware planning.
//! Pure data and algorithms: no network, no bot, no live state.

pub mod bfs;
pub mod buffer;
pub mod collision;
pub mod costs;
pub mod custom_edge;
pub mod danger;
mod detour;
pub mod f2p;
pub mod fare;
mod grid;
#[cfg(test)]
mod grid_tests;
pub mod fast_hop;
#[cfg(test)]
mod fast_hop_tests;
mod fallback;
#[cfg(test)]
mod giants_exit_tests;
pub mod loctypes;
pub mod nav;
pub mod nav_meter;
pub mod no_route;
pub mod plan;
pub mod pocket;
#[cfg(test)]
mod plan_budget_tests;
pub mod reach;
mod reader;
pub mod region_lists;
pub mod route;
mod search;
pub mod site_reach;
pub mod site_banks;
pub mod route_cache;
pub mod route_patch;
pub mod route_path;
pub mod route_splice;
pub mod service;
pub mod service_accept;
pub mod service_codec;
mod service_fast;
#[cfg(test)]
mod service_costs_tests;
#[cfg(test)]
mod service_replay_tests;
#[cfg(test)]
mod service_replay_live;
#[cfg(test)]
mod route_cache_tests;
pub mod service_plan;
pub mod service_queries;
pub mod static_world;
pub mod tables;
pub mod toll_side;
pub mod walk_costs;
pub mod walk_rank;
