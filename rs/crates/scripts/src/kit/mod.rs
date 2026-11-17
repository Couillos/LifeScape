//! Kit: shared behavior bricks. Every activity script is a thin
//! declaration executed by these bricks; the brain (L2+) decides *what*
//! to do, the kit knows *how*, the activity says only what is specific.
//!
//! Bricks: `cycle` (load loop, TaskSpec reader in L2), `pack` (pack reads,
//! object tables), `bank` (bank trips, booths, toll fence, transit heals),
//! `shop` (shop trade, sales, stock buys), `load` (gather load driver),
//! `bans` (per-tile ban clocks), `tools` (ranks, reattach, bank restores,
//! recovery), `tool_upgrade` (better banked axe/pickaxe swapped in),
//! `spots` (generic pool + rock/fish pools + foe acquisition),
//! `trees` (tree pool), `harvest` (click-and-watch core, message classes),
//! `woodcut`/`mine`/`fish` (skill drivers), `survival` (eat, death tally,
//! safe point, run gates), `gear` (equip best), `loot` (ground pickup
//! policy), `fight` (kill loop + hunt) + `style` (melee rotation) +
//! `strike` (one attack click + its refusals) + `ammo` (weapon refusals:
//! empty quiver, wrong ammo),
//! `observe` (per-run memory, regrow prediction), `produce`
//! (A-on-B transformation), `withdraw` (bank withdrawals, walking to a
//! far bank for known banked items), `spell_load` (spell casts stocked
//! from pack + known bank, staff counted), `fireline` (legal fire lines),
//! `uniform` (guild uniform worn before the door, `--wear`), `anvil`
//! (one smithing visit: bar on the anvil, Make 1/5/10, chain drained),
//! `press` (Make 1/5/10 choice: one press covering the inputs),
//! `janitor` (bank cleanup budget, chunks, whole-stack drops), `progress`
//! (silent-loop guard of every `cycle::run`: no output, bank churn), `mill`
//! (windmill hopper, lever and flour bin clicked from side stands), `want`
//! (the run's `--want` goal read inside a load: stands, chains, items),
//! `stock_use` (the run's `--use` goal: a money lot on the bot's own stock).

#![forbid(unsafe_code)]

pub mod ammo;
pub mod anvil;
pub mod bank;
pub mod bank_route;
pub mod bank_full;
pub mod bans;
pub mod chain_spec;
pub mod combat_bank;
pub mod cycle;
pub mod danger;
pub mod essence_mine;
pub mod fight;
pub mod fireline;
pub mod fish;
pub mod fish_gear;
pub mod foes;
pub mod gear;
pub mod hunt;
pub mod janitor;
pub mod ladder;
pub mod harvest;
pub mod load;
pub mod locked;
pub mod loot;
pub mod magic;
pub mod meals;
pub mod mill;
pub mod mine;
pub mod observe;
pub mod pack;
pub mod npc_spawn;
pub mod press;
pub mod produce;
pub mod progress;
pub mod shop;
pub mod site;
pub mod spell_load;
pub mod stock_use;
pub mod spots;
pub mod stranded;
pub mod strike;
pub mod style;
pub mod survival;
pub mod tan;
pub mod toll;
pub mod tool_upgrade;
pub mod tools;
pub mod trees;
pub mod uniform;
pub mod want;
pub mod water;
pub mod withdraw;
pub mod woodcut;
