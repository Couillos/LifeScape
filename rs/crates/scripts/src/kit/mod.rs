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
//! policy), `fight` (kill loop + hunt) + `style` (melee rotation) +
//! `strike` (one attack click + its refusals) + `ammo` (weapon refusals:
//! empty quiver, wrong ammo),
//! `observe` (per-run memory, regrow prediction), `produce`
//! (A-on-B transformation), `withdraw` (bank withdrawals, walking to a
//! far bank for known banked items), `fireline` (legal fire lines).

#![forbid(unsafe_code)]

pub mod ammo;
pub mod bank;
pub mod bans;
pub mod chain_spec;
pub mod cycle;
pub mod danger;
pub mod fight;
pub mod fireline;
pub mod fish;
pub mod fish_gear;
pub mod foes;
pub mod gear;
pub mod hunt;
pub mod ladder;
pub mod harvest;
pub mod load;
pub mod loot;
pub mod magic;
pub mod mine;
pub mod observe;
pub mod pack;
pub mod produce;
pub mod shop;
pub mod site;
pub mod spots;
pub mod strike;
pub mod style;
pub mod survival;
pub mod tan;
pub mod toll;
pub mod tools;
pub mod trees;
pub mod withdraw;
pub mod woodcut;
