#![forbid(unsafe_code)]
//! Static navigation pieces: collision, local search and door-aware planning.
//! Pure data and algorithms: no network, no bot, no live state.

pub mod bfs;
pub mod buffer;
pub mod collision;
pub mod nav;
pub mod reach;
pub mod route;
