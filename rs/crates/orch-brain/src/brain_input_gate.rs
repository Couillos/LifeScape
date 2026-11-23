#![forbid(unsafe_code)]
//! Input gate of the production methods (smelt, smith, cut, string,
//! pottery, spin, jewelry), asked by the launch pass before a worker
//! starts. The script already stops `blocked` on the inputs one action
//! lacks (pack + known bank, `withdraw::preflight_blocked`), but only
//! after a worker was spawned and an error row written: the brain knows
//! the same holdings (pack, bank, worn, free of the bot's own listings),
//! so the shortfall files its obtain sub-tasks at once, no launch.
//! Live 2026-11-22: 585 smelt, 108 smith, 72 cut_gems, 35 stringing,
//! 24 pottery blocks in four hours, most of them 3 ticks long.

use lifescape_planner::method_way::method_way;

/// Method families whose script consumes recipe inputs it never buys.
const GATED_FAMILIES: [&str; 18] = [
    "smelt", "smith", "cut", "string", "pottery", "spin", "jewelry", "curse", "teleport", "bananas", "enchant", "superheat", "telegrab", "fill", "runecraft", "craft", "fight", "cook",
];

/// Staves and the rune each pays (a held staff stands for its element).
use lifescape_scripts::kit::magic::STAVES;

/// What one action of `method` consumes and the gate checks: the recipe
/// way (production, spells, fills) or the cast way of a magic fight.
/// Raw-food cooking is gated too (`cook/<raw>`); other families, whose
/// scripts assemble or buy their own inputs, are not.
fn gate_inputs(method: &str) -> Vec<(String, i64)> {
    let family = method.split([':', '/']).next().unwrap_or("");
    let gated = GATED_FAMILIES.contains(&family) || method.starts_with("cook/");
    if !gated {
        return Vec::new();
    }
    let way = method_way(method).or_else(|| lifescape_planner::spell_methods::cast_method_way(method));
    let Some((_, way)) = way else { return Vec::new() };
    let mut inputs = way.inputs;
    // The dump lists thread among the carried tools, yet leatherwork
    // consumes one per five pieces and blocks when none is left.
    if way.tools.iter().any(|tool| tool == CONSUMED_TOOL) {
        inputs.push((CONSUMED_TOOL.to_string(), 1));
    }
    inputs
}

/// Units of `name` one action of `method` consumes (at least 1): what a
/// listed input must still be held for, to be worth withdrawing.
pub fn input_each(method: &str, name: &str) -> i64 {
    gate_inputs(method).iter().find(|(input, _)| input == name).map_or(1, |(_, each)| (*each).max(1))
}

/// A carried tool the leather script uses up (`leatherwork::THREAD`).
const CONSUMED_TOOL: &str = lifescape_scripts::activities::leatherwork::THREAD;

/// The inputs of one action of `method` that `free` does not cover.
/// Empty for an ungated method, or while the bank was never seen
/// (`bank_known` false: a banked input must not read as missing).
pub fn short_inputs(method: &str, bank_known: bool, free: &dyn Fn(&str) -> i64) -> Vec<String> {
    if !bank_known {
        return Vec::new();
    }
    // A held staff pays its rune: the spell scripts never count it.
    let paid_by_staff = |rune: &str| STAVES.iter().any(|(staff, paid)| *paid == rune && free(staff) > 0);
    let inputs: Vec<(String, i64)> = gate_inputs(method).into_iter().filter(|(name, _)| !paid_by_staff(name)).collect();
    // The script's own definition of "enough to start" (`smelt` runs it
    // as its preflight): one shared rule, never a second copy.
    lifescape_scripts::kit::withdraw::preflight_short(&inputs, free, bank_known)
}

/// What the launch pass does with a production method.
#[derive(Debug, PartialEq, Eq)]
pub enum Gate {
    /// Every input is held: launch.
    Clear,
    /// Inputs held but listed on the market by the bot itself: the
    /// listings were withdrawn, launch.
    Released(Vec<String>),
    /// Inputs the bot does not hold anywhere: file them as a block.
    Short(Vec<String>),
}

/// The gate on live holdings: `free` counts what the bot holds free of
/// its own market locks (read fresh at each call), `total` every unit
/// physically held (a listing or a deal can lock units the bot no longer
/// has), `listed` what its resting sells hold, `release` withdraws
/// listings and returns how many closed. A released input launches only
/// when, once the listings are gone, it reads free: a lock a deal in
/// flight keeps (or units sold away) files the plain need instead of a
/// run its script blocks `locked:` in seconds (live 2026-11-22:
/// kate_ellis `locked:uncut_emerald`, jim_ellis `locked:steel_bar`).
pub fn verdict(
    method: &str,
    bank_known: bool,
    free: &dyn Fn(&str) -> i64,
    total: &dyn Fn(&str) -> i64,
    listed: &dyn Fn(&str) -> i64,
    release: &mut dyn FnMut(&[String]) -> usize,
) -> Gate {
    let short = short_inputs(method, bank_known, free);
    if short.is_empty() {
        return Gate::Clear;
    }
    let inputs = gate_inputs(method);
    let each_of = |name: &str| inputs.iter().find(|(input, _)| input == name).map_or(1, |(_, each)| (*each).max(1));
    let (listed_short, plain): (Vec<String>, Vec<String>) = short.into_iter().partition(|name| listed(name) > 0 && total(name) >= each_of(name));
    if !plain.is_empty() {
        return Gate::Short(plain);
    }
    if release(&listed_short) > 0 {
        let still_short = short_inputs(method, bank_known, free);
        if still_short.is_empty() {
            Gate::Released(listed_short)
        } else {
            Gate::Short(still_short)
        }
    } else {
        // A listing in a deal in flight stays locked: the plain need.
        Gate::Short(listed_short)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live 2026-11-22: curse, magic, enchant, fill and leatherwork
    /// launched without runes, jugs or thread (about 100 blocks in 6 h).
    #[test]
    fn spells_fills_and_crafts_gate_on_their_consumables() {
        let nothing = |_: &str| 0;
        let curse = short_inputs("curse:curse/cows", true, &nothing);
        assert!(curse.contains(&"waterrune".to_string()) && curse.contains(&"earthrune".to_string()), "{curse:?}");
        let fight = short_inputs("fight:magic/fire_blast/cows", true, &nothing);
        assert!(fight.contains(&"deathrune".to_string()), "{fight:?}");
        assert!(short_inputs("enchant:strung_sapphire_amulet", true, &nothing).contains(&"cosmicrune".to_string()));
        assert_eq!(short_inputs("fill:jug_empty", true, &nothing), vec!["jug_empty".to_string()]);
        assert!(short_inputs("cook/raw_beef", true, &nothing).contains(&"raw_beef".to_string()));
        assert!(short_inputs("craft:leather_chaps", true, &nothing).contains(&"thread".to_string()));
        assert!(short_inputs("curse:curse/cows", true, &|_| 99).is_empty());
        assert!(short_inputs("curse:curse/cows", false, &nothing).is_empty());
    }

    /// A held staff pays its element: no air or water rune needed.
    #[test]
    fn a_held_staff_covers_its_rune() {
        let held = |name: &str| if name == "staff_of_water" || name == "earthrune" || name == "bodyrune" { 9 } else { 0 };
        assert!(short_inputs("curse:curse/cows", true, &held).is_empty());
    }

    /// Live 2026-11-22 (unastorm `cook:swordfish`, arthur_75
    /// `string:unstrung_emerald_amulet`): the product-named cook and the
    /// stringing launched with the raw fish or amulet missing.
    #[test]
    fn product_named_cook_and_string_methods_gate() {
        let nothing = |_: &str| 0;
        assert!(short_inputs("cook:swordfish", true, &nothing).contains(&"raw_swordfish".to_string()));
        assert!(short_inputs("cook:lobster", true, &nothing).contains(&"raw_lobster".to_string()));
        assert!(short_inputs("string:unstrung_emerald_amulet", true, &nothing).contains(&"unstrung_emerald_amulet".to_string()));
    }

    #[test]
    fn an_ungated_or_unseen_bank_never_gates() {
        assert!(short_inputs("gather:mining/coal", true, &|_| 0).is_empty());
        assert!(short_inputs("smelt:iron_bar", false, &|_| 0).is_empty());
    }

    #[test]
    fn a_smelt_without_ore_names_the_short_input_only() {
        let held = |name: &str| if name == "coal" { 5 } else { 0 };
        assert_eq!(short_inputs("smelt:iron_bar", true, &held), vec!["iron_ore".to_string()]);
        let steel = |name: &str| if name == "iron_ore" { 1 } else { 0 };
        assert_eq!(short_inputs("smelt:steel_bar", true, &steel), vec!["coal".to_string()]);
        assert!(short_inputs("smelt:steel_bar", true, &|_| 99).is_empty());
    }

    #[test]
    fn a_listed_input_releases_instead_of_blocking() {
        let free_after = std::cell::Cell::new(false);
        let free = |name: &str| if free_after.get() && name == "iron_ore" { 30 } else { 0 };
        let total = |name: &str| if name == "iron_ore" { 30 } else { 0 };
        let listed = |name: &str| if name == "iron_ore" { 30 } else { 0 };
        let mut released: Vec<String> = Vec::new();
        let gate = verdict("smelt:iron_bar", true, &free, &total, &listed, &mut |items| {
            released.extend(items.iter().cloned());
            free_after.set(true);
            1
        });
        assert_eq!(gate, Gate::Released(vec!["iron_ore".to_string()]));
        assert_eq!(released, vec!["iron_ore".to_string()]);
        // Nothing listed: a plain shortfall, nothing withdrawn.
        let gate = verdict("smelt:iron_bar", true, &|_| 0, &|_| 0, &|_| 0, &mut |_| panic!("no listing to release"));
        assert_eq!(gate, Gate::Short(vec!["iron_ore".to_string()]));
        // A listing a deal holds frees nothing: the plain need.
        let gate = verdict("smelt:iron_bar", true, &|_| 0, &total, &listed, &mut |_| 0);
        assert_eq!(gate, Gate::Short(vec!["iron_ore".to_string()]));
    }

    /// Live 2026-11-22 (kate_ellis `locked:uncut_emerald`): the listing
    /// closed but a deal in flight still locks the units, which read
    /// held after the release: no launch.
    #[test]
    fn a_release_that_leaves_the_lock_files_the_need() {
        let total = |name: &str| if name == "iron_ore" { 2 } else { 0 };
        let listed = |name: &str| if name == "iron_ore" { 2 } else { 0 };
        let gate = verdict("smelt:iron_bar", true, &|_| 0, &total, &listed, &mut |_| 1);
        assert_eq!(gate, Gate::Short(vec!["iron_ore".to_string()]));
    }

    /// Live 2026-11-22 (kate_ellis `missing uncut_sapphire`): units a
    /// listing promises but the bot no longer holds are not releasable.
    #[test]
    fn a_listing_without_the_units_behind_it_is_a_plain_need() {
        let listed = |name: &str| if name == "iron_ore" { 5 } else { 0 };
        let gate = verdict("smelt:iron_bar", true, &|_| 0, &|_| 0, &listed, &mut |_| panic!("nothing physical to release"));
        assert_eq!(gate, Gate::Short(vec!["iron_ore".to_string()]));
    }
}
