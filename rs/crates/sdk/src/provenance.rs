//! Item-flow provenance and skill views for task reports.
//!
//! The session diffs holdings before/after a run; the labels below say
//! where each delta came from. `classify_take` is the take-side rule the
//! whole codebase shares: a pickup made while skill XP lands is the skill
//! product (`produced`); a pickup of an object that appeared at a targeted
//! NPC kill is `loot`; any other ground pickup is a static spawn
//! (`ground_spawn`). Shop, trade, consume, death and loss flows are labelled
//! at their own call sites (the session maps net coin coupling for buys and
//! sells). Pure, no network, no tables.
//!
//! Frozen contract (PRD 2, D45): the ten values below are the only
//! provenance strings the session, the rates service and the front may
//! emit or read. `death` (lost to death) and `unattributed` (unexplained
//! gap, never a yield) are first-class; the PRD 1 draft value `picked`
//! is retired — no session ever labelled it, takes read `ground_spawn`.
//! Rates counts only farm flows (`produced`, `loot`, `ground_spawn`) as
//! yields; `market:*` and `shop:*` methods never enter the rates (D44).

#![forbid(unsafe_code)]

/// Where one reported item flow came from. Every report line carries one.
/// Frozen set (D45): add a variant only with a PRD amendment, never
/// silently — the rates service, the store and the front match on these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    /// Skill action product (XP landed with the gain).
    Produced,
    /// NPC drop (object appeared at a targeted kill).
    Loot,
    /// Static ground spawn (`spawns_obj` tile).
    GroundSpawn,
    /// Bought from an NPC shop.
    ShopBought,
    /// Sold to an NPC shop.
    ShopSold,
    /// Player-to-player trade flow (market, later lots).
    Traded,
    /// Eaten, buried, burned or otherwise used up.
    Consumed,
    /// Dropped, tolled away or otherwise lost alive.
    Lost,
    /// Lost to death (kept apart from `Lost` so death costs stay visible).
    Death,
    /// Unexplained gap: labelled at the report, never a yield (D45).
    Unattributed,
}

impl Provenance {
    pub fn as_str(self) -> &'static str {
        match self {
            Provenance::Produced => "produced",
            Provenance::Loot => "loot",
            Provenance::GroundSpawn => "ground_spawn",
            Provenance::ShopBought => "shop_bought",
            Provenance::ShopSold => "shop_sold",
            Provenance::Traded => "traded",
            Provenance::Consumed => "consumed",
            Provenance::Lost => "lost",
            Provenance::Death => "death",
            Provenance::Unattributed => "unattributed",
        }
    }

    pub fn parse(name: &str) -> Option<Provenance> {
        match name {
            "produced" => Some(Provenance::Produced),
            "loot" => Some(Provenance::Loot),
            "ground_spawn" => Some(Provenance::GroundSpawn),
            "shop_bought" => Some(Provenance::ShopBought),
            "shop_sold" => Some(Provenance::ShopSold),
            "traded" => Some(Provenance::Traded),
            "consumed" => Some(Provenance::Consumed),
            "lost" => Some(Provenance::Lost),
            "death" => Some(Provenance::Death),
            "unattributed" => Some(Provenance::Unattributed),
            _ => None,
        }
    }

    pub fn all() -> [&'static str; 10] {
        [
            "produced",
            "loot",
            "ground_spawn",
            "shop_bought",
            "shop_sold",
            "traded",
            "consumed",
            "lost",
            "death",
            "unattributed",
        ]
    }
}

/// Take-side rule, in precedence order: concomitant skill XP wins over a
/// tracked NPC drop, which wins over a static spawn tile. A take with no
/// other signal is still a ground pickup, never an error.
pub fn classify_take(xp_gained: bool, on_spawn_tile: bool, npc_drop: bool) -> Provenance {
    if xp_gained {
        Provenance::Produced
    } else if npc_drop {
        Provenance::Loot
    } else if on_spawn_tile {
        Provenance::GroundSpawn
    } else {
        Provenance::GroundSpawn
    }
}

/// Server stat order (`PlayerStat.ts`): attack 0 .. runecraft 20.
pub fn skill_name(stat: u8) -> &'static str {
    match stat {
        0 => "attack",
        1 => "defence",
        2 => "strength",
        3 => "hitpoints",
        4 => "ranged",
        5 => "prayer",
        6 => "magic",
        7 => "cooking",
        8 => "woodcutting",
        9 => "fletching",
        10 => "fishing",
        11 => "firemaking",
        12 => "crafting",
        13 => "smithing",
        14 => "mining",
        15 => "herblore",
        16 => "agility",
        17 => "thieving",
        18 => "stat18",
        19 => "stat19",
        20 => "runecraft",
        _ => "unknown",
    }
}

/// Lowercase skill name back to its stat id, for task targets.
pub fn skill_id(name: &str) -> Option<u8> {
    match name {
        "attack" => Some(0),
        "defence" => Some(1),
        "strength" => Some(2),
        "hitpoints" => Some(3),
        "ranged" => Some(4),
        "prayer" => Some(5),
        "magic" => Some(6),
        "cooking" => Some(7),
        "woodcutting" => Some(8),
        "fletching" => Some(9),
        "fishing" => Some(10),
        "firemaking" => Some(11),
        "crafting" => Some(12),
        "smithing" => Some(13),
        "mining" => Some(14),
        "herblore" => Some(15),
        "agility" => Some(16),
        "thieving" => Some(17),
        "stat18" => Some(18),
        "stat19" => Some(19),
        "runecraft" => Some(20),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_take, skill_id, skill_name, Provenance};

    /// Take precedence: skill XP first, tracked drop next, spawn or bare
    /// ground last. Every combination labels, nothing errors.
    #[test]
    fn take_labels_xp_then_drop_then_spawn() {
        assert_eq!(classify_take(true, true, true), Provenance::Produced);
        assert_eq!(classify_take(true, false, false), Provenance::Produced);
        assert_eq!(classify_take(false, false, true), Provenance::Loot);
        assert_eq!(classify_take(false, true, false), Provenance::GroundSpawn);
        assert_eq!(classify_take(false, false, false), Provenance::GroundSpawn);
    }

    /// Provenance names round-trip; the set is exactly the report contract.
    #[test]
    fn provenance_names_round_trip() {
        assert_eq!(Provenance::all().len(), 10);
        for name in Provenance::all() {
            assert_eq!(Provenance::parse(name).map(|provenance| provenance.as_str()), Some(name));
        }
        assert_eq!(Provenance::parse("dropped"), None);
        // `picked` is retired (D45): never emitted, never parsed back.
        assert_eq!(Provenance::parse("picked"), None);
        assert_eq!(Provenance::Death.as_str(), "death");
        assert_eq!(Provenance::Unattributed.as_str(), "unattributed");
    }

    /// Skill table follows the server order (spot-checked against live
    /// stat ids the kit already hard-codes).
    #[test]
    fn skill_table_matches_live_ids() {
        assert_eq!((skill_name(8), skill_name(10), skill_name(14)), ("woodcutting", "fishing", "mining"));
        assert_eq!((skill_name(0), skill_name(3)), ("attack", "hitpoints"));
        assert_eq!(skill_name(99), "unknown");
        assert_eq!(skill_id("woodcutting"), Some(8));
        assert_eq!(skill_id("mining"), Some(14));
        assert_eq!(skill_id("nope"), None);
    }
}
