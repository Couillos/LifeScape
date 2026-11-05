//! LifeScape SDK: socket, handshake, ISAAC, framed 274 opcodes, live
//! world state, tick pump and game actions. Std-only, `forbid(unsafe)`.

pub mod bfs;
pub mod buffer;
pub mod client;
pub mod collision;
pub mod connection;
pub mod error;
pub mod isaac;
pub mod nav;
pub mod protocol;
pub mod reach;
pub mod route;
pub mod rsa;
pub mod save;
pub mod snapshot;
pub mod tables;
pub mod waits;
pub mod words;
pub mod world;

pub use client::Bot;
pub use client::movement::TransitionRow;
pub use connection::{Connection, Frame};
pub use error::{Error, Result};
pub use tables::{NOTABLES, Tables};
pub use world::World;
