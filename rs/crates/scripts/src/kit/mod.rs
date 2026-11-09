//! Kit: shared behavior bricks. Every activity script is a thin
//! declaration executed by these bricks; the brain (L2+) decides *what*
//! to do, the kit knows *how*, the activity says only what is specific.
//!
//! Bricks: `cycle` (load loop, TaskSpec reader in L2), `tools`
//! (ranks, reattach, bank restores, recovery), `spots` (generic pool +
//! rock/fish pools + foe acquisition), `trees` (tree pool), `harvest`
//! (click-and-watch core), `woodcut`/`mine`/`fish` (skill drivers),
//! `survival` (eat, death tally, safe point), `gear` (equip best),
//! `loot` (ground pickup policy), `fight` (kill loop + hunt) + `style`
//! (melee rotation), `observe` (per-run memory).

#![forbid(unsafe_code)]

pub mod cycle;
pub mod fight;
pub mod fish;
pub mod gear;
pub mod harvest;
pub mod loot;
pub mod mine;
pub mod observe;
pub mod produce;
pub mod spots;
pub mod style;
pub mod survival;
pub mod tools;
pub mod trees;
pub mod woodcut;
