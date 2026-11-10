//! Activity scripts: one thin runnable per skill (CLI + loads loop).
//! Everything they call lives in `kit/` (drivers, pools, tools) or
//! `legacy/` (pre-brain life decisions).

#![forbid(unsafe_code)]

pub mod alchemy;
pub mod bake_bread;
pub mod bury_loot;
pub mod cook;
pub mod cook_meals;
pub mod cut_gems;
pub mod enchant_teleport;
pub mod firemaking;
pub mod fishing;
pub mod ground_pickup;
pub mod leatherwork;
pub mod magic_style;
pub mod market;
pub mod milk;
pub mod market_cleanup;
pub mod market_flip;
pub mod market_deliver;
pub mod market_shop;
pub mod melee;
pub mod mining;
pub mod ranged_style;
pub mod runecraft;
pub mod shear;
pub mod smelt;
pub mod smith;
pub mod stringing;
pub mod superheat;
pub mod telegrab_wine;
pub mod woodcut;
// TEMP-P34-VERIFY: removed before hand-off.
pub mod dye;
pub mod pottery;
pub mod smelt_jewelry;
pub mod spin;
