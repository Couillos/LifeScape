//! Static collision pieces: flags, painted map and pack loading.
//! Re-exported at this level so map builders keep one import path.
mod flags;
mod gate_swing;
mod loc_collision;
mod map_pack;

pub use flags::*;
pub use gate_swing::*;
pub use loc_collision::*;
pub use map_pack::*;

pub use crate::bfs::CollisionSource;
