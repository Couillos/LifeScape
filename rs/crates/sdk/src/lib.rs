//! LifeScape SDK: socket, handshake, ISAAC, framed 274 opcodes, live
//! world state, tick pump and game actions. Std-only, `forbid(unsafe)`.
//!
//! Static navigation (collision, search, planning) lives in `lifescape-nav`
//! and is re-exported here so existing paths keep working.

pub mod client;
pub mod connection;
pub mod error;
pub mod isaac;
pub mod protocol;
pub mod rsa;
pub mod save;
pub mod snapshot;
pub mod tables;
pub mod waits;
pub mod words;
pub mod world;

pub use lifescape_nav::{bfs, buffer, collision, loctypes, nav, plan, reach, route, static_world};

pub use client::Bot;
pub use client::movement::TransitionRow;
pub use connection::{Connection, Frame};
pub use error::{Error, Result};
pub use tables::{NOTABLES, Tables};
pub use world::World;
