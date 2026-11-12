//! Kit: shared behavior bricks. Every activity script is a thin
//! declaration executed by these bricks; the brain (L2+) decides *what*
//! to do, the kit knows *how*, the activity says only what is specific.
//!
//! Bricks: `cycle` (load loop, TaskSpec reader in L2), `pack` (pack reads,
//! object tables), `bank` (bank trips, booths, toll fence, transit heals),
//! `shop` (shop trade, sales, stock buys), `load` (gather load driver),
//! `bans` (per-tile ban clocks), `tools` (ranks, reattach, bank restores,
//! recovery), `spots` (generic pool + rock/fish pools + foe acquisition),
//! `trees` (tree pool), `harvest` (click-and-watch core, message classes),
//! `woodcut`/`mine`/`fish` (skill drivers), `survival` (eat, death tally,
//! safe point, run gates), `gear` (equip best), `loot` (ground pickup
//! policy), `fight` (kill loop + hunt) + `style` (melee rotation),
//! `observe` (per-run memory, regrow prediction), `produce`
//! (A-on-B transformation).

#![forbid(unsafe_code)]

pub mod bank;
pub mod bans;
pub mod chain_spec;
pub mod cycle;
pub mod danger;
pub mod fight;
pub mod fish;
pub mod gear;
pub mod harvest;
pub mod load;
pub mod loot;
pub mod magic;
pub mod mine;
pub mod observe;
pub mod pack;
pub mod produce;
pub mod shop;
pub mod spots;
pub mod style;
pub mod survival;
pub mod tan;
pub mod tools;
pub mod trees;
pub mod woodcut;
