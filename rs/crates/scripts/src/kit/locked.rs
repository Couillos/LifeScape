//! Locked needs (plan P5, tour 3 point 3): a block on a tool or an
//! input the bot holds, but whose every unit its own market orders lock
//! in the bank, names the lock (`locked:<object>`), never a `missing`
//! the brain would buy again (live 2026-11-17, luke_hill: its only axe,
//! an iron one, listed "surplus"; woodcut blocked "missing bronze_axe",
//! the obtain closed on "holds 1 bronze_axe", four times, then
//! abandoned). The brain withdraws the listing and relaunches.
//!
//! One choke point (`tag_locked`, applied by `lifescape_scripts::run` to
//! every script's `Blocked`): a tool family (`planner::tools::
//! tool_family`, best first) reads as locked when the pack holds none of
//! it and the known bank holds only locked units.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::pack::{banked_usable, pack_total};

/// Tag of a need whose units the bot's own orders lock.
pub const LOCKED_TAG: &str = "locked:";

/// Units of one object: carried, banked (known bank view) and usable
/// from the bank (banked minus the market lock).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Units {
    pub carried: u32,
    pub banked: u32,
    pub usable: u32,
}

/// Pure: the need `name` (its family `family`, best first) as the bot
/// holds it. `locked:<member>` when no member is carried or usable from
/// the bank and some member sits banked under a lock (the best such
/// member); `name` itself otherwise.
pub fn tag_need(name: &str, family: &[String], units_of: &dyn Fn(&str) -> Units) -> String {
    if name.contains(':') {
        return name.to_string();
    }
    let rows: Vec<(&String, Units)> = family.iter().map(|member| (member, units_of(member))).collect();
    if rows.iter().any(|(_, units)| units.carried > 0 || units.usable > 0) {
        return name.to_string();
    }
    match rows.iter().find(|(_, units)| units.banked > units.usable) {
        Some((member, _)) => format!("{LOCKED_TAG}{member}"),
        None => name.to_string(),
    }
}

/// `tag_need` for every need of a block, on the live bot.
pub fn tag_locked(bot: &Bot, missing: Vec<String>) -> Vec<String> {
    let units_of = |name: &str| -> Units {
        let Some(obj) = (bot.tables.find_obj_by_name)(name) else { return Units::default() };
        let banked = (bot.tables.com_id)("bank_main:inv")
            .and_then(|com| bot.world.inv_known(com))
            .map(|slots| slots.values().filter(|(held, _)| *held == obj).map(|(_, count)| *count).sum())
            .unwrap_or(0);
        Units { carried: pack_total(bot, obj), banked, usable: banked_usable(bot, obj) }
    };
    missing.into_iter().map(|name| tag_need(&name, &lifescape_planner::tools::tool_family(&name), &units_of)).collect()
}

#[cfg(test)]
mod tests {
    use super::{tag_need, Units};

    fn family() -> Vec<String> {
        lifescape_planner::tools::tool_family("bronze_axe")
    }

    /// Live luke_hill 2026-11-17: its only axe (iron) is listed, every
    /// unit locked in the bank: the block names the lock, not a buy.
    #[test]
    fn a_listed_only_tool_blocks_locked() {
        let units = |name: &str| if name == "iron_axe" { Units { carried: 0, banked: 1, usable: 0 } } else { Units::default() };
        assert_eq!(tag_need("bronze_axe", &family(), &units), "locked:iron_axe");
    }

    /// A free unit anywhere (carried, or usable in the bank) or no unit
    /// at all keeps the plain need; tagged needs pass through.
    #[test]
    fn a_free_or_absent_tool_stays_missing() {
        let carried = |name: &str| match name {
            "iron_axe" => Units { carried: 0, banked: 1, usable: 0 },
            "bronze_axe" => Units { carried: 1, banked: 0, usable: 0 },
            _ => Units::default(),
        };
        assert_eq!(tag_need("bronze_axe", &family(), &carried), "bronze_axe");
        let usable = |name: &str| if name == "steel_axe" { Units { carried: 0, banked: 2, usable: 1 } } else { Units::default() };
        assert_eq!(tag_need("bronze_axe", &family(), &usable), "bronze_axe");
        assert_eq!(tag_need("bronze_axe", &family(), &|_| Units::default()), "bronze_axe");
        assert_eq!(tag_need("coins:60", &["coins:60".to_string()], &|_| Units { carried: 0, banked: 5, usable: 0 }), "coins:60");
        let coal = |name: &str| if name == "coal" { Units { carried: 0, banked: 9, usable: 0 } } else { Units::default() };
        assert_eq!(tag_need("coal", &["coal".to_string()], &coal), "locked:coal", "an input locked the same way");
    }
}
