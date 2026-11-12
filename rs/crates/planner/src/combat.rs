//! Combat prerequisites for the brain (cerveau combat): how strong a
//! bot must be before a task sends it somewhere, and what to do when it
//! is not. Pure: caller-supplied levels in, decisions out. No network,
//! no files, no JSON (the gate rides in planner types, never in a file).
//!
//! Server truth: `Player.ts:1371` (combat level from base levels) and
//! `Npc.ts:1014` (outside the Wilderness a hunter skips players above
//! twice its level, so a spot is safe at `2 * vislevel + 1`).
//!
//! Spot matrix (dump-verified, see tests): Draynor willows 15 (young
//! dark wizard 7), Varrock-East men 13 (mugger 6), coal 2701,3331 43
//! (brown bear 21), gold 2733,3225 41 (poison scorpion 20), volcano
//! entrance 29 (scorpion 14, transit to the lesser demons), anvil
//! 2991,3689 5 (giant spider 2). Anything else is safe at 3.
//! shortcut: the matrix pins each method to its script's primary spot;
//! if a script reroutes elsewhere, the pin goes stale (bans + hurt-flee
//! stay the live safety net).

/// Base combat levels (server `baseLevels`: trained levels, never the
/// boosted current HP the client shows in a fight).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BaseLevels {
    pub attack: u8,
    pub strength: u8,
    pub defence: u8,
    pub hitpoints: u8,
    pub ranged: u8,
    pub prayer: u8,
    pub magic: u8,
}

impl BaseLevels {
    /// A fresh account: everything 1, hitpoints 10 (server start).
    pub fn fresh() -> Self {
        BaseLevels {
            attack: 1,
            strength: 1,
            defence: 1,
            hitpoints: 10,
            ranged: 1,
            prayer: 1,
            magic: 1,
        }
    }
}

/// Combat level from base levels (`Player.ts:1371`): quarter of
/// defence + hitpoints + floored half prayer, plus the best of the
/// melee / ranged / magic parts, floored. Fresh accounts read 3.
pub fn combat_level(levels: &BaseLevels) -> u8 {
    let base = 0.25 * f64::from(levels.defence + levels.hitpoints + levels.prayer / 2);
    let melee = 0.325 * f64::from(levels.attack + levels.strength);
    let ranged = 0.325 * f64::from(levels.ranged / 2 + levels.ranged);
    let magic = 0.325 * f64::from(levels.magic / 2 + levels.magic);
    (base + melee.max(ranged).max(magic)) as u8
}

/// Site family and resource a method works (`None`: no site in the data,
/// e.g. smithing at any anvil).
fn method_sites(method: &str) -> Option<(&'static str, String)> {
    let (kind, resource) = if let Some(fish) = method.strip_prefix("gather:fishing/") {
        ("fishing", fish)
    } else if let Some(ore) = method.strip_prefix("gather:mining/") {
        ("mining", ore)
    } else if let Some(logs) = method.strip_prefix("gather:woodcut/") {
        ("woodcut", logs)
    } else if let Some(rest) = method.strip_prefix("fight:") {
        ("fight", rest.rsplit('/').next().unwrap_or(rest))
    } else if let Some(item) = method.strip_prefix("farm:") {
        if item.starts_with("raw_") {
            ("fishing", item)
        } else if item.ends_with("_ore") || item == "coal" || item == "clay" {
            ("mining", item)
        } else if item.ends_with("logs") {
            ("woodcut", item)
        } else {
            return None;
        }
    } else {
        return None;
    };
    Some((kind, resource.to_string()))
}

/// Minimum combat level to work one planner method without being hunted
/// on the way in, computed from the game data (no zone written by hand):
/// the least demanding site of the method, a site counting its bank
/// (`gamedata::danger::method_requirement`). `NEVER` when no site is
/// workable at any level; 3 for methods without sites.
pub fn min_combat_for_method(method: &str) -> u8 {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, u8>>> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    if let Some(known) = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(method) {
        return *known;
    }
    let computed = match method_sites(method) {
        Some((kind, resource)) => lifescape_gamedata::danger::method_requirement(kind, &resource).unwrap_or(3),
        None if method.starts_with("smith:") => 5,
        None => 3,
    };
    cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(method.to_string(), computed);
    computed
}

/// Typed failure of the combat brain: only unknown training styles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CombatError {
    /// No training method for this goal style.
    UnknownStyle(String),
}

impl std::fmt::Display for CombatError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CombatError::UnknownStyle(style) => write!(formatter, "no combat training for style {style}"),
        }
    }
}

/// Priority training method for `reach_combat`: the goal's own style
/// when it fights (ranged/magic), else melee — always on chickens,
/// the weakest CLEAN camp (combat spot `safe`, minimum 1).
pub fn training_method(style: &str) -> Result<&'static str, CombatError> {
    match style {
        "ranged" => Ok("fight:ranged/chickens"),
        "magic" => Ok("fight:magic/chickens"),
        "melee" | "attack" | "strength" | "defence" | "hitpoints" | "prayer" => Ok("fight:melee/chickens"),
        _ => Err(CombatError::UnknownStyle(style.to_string())),
    }
}

/// Combat style the goal trains under: its own style when it fights,
/// else melee (prayer buries where melee fights, producers fight
/// melee). Feeds `training_method` for the priority `reach_combat`.
pub fn goal_combat_style(goal: &crate::model::GoalKind) -> &str {
    match goal {
        crate::model::GoalKind::EquipBest { style } if style == "ranged" || style == "magic" => style,
        crate::model::GoalKind::MaxSkills { skills, .. } => {
            if skills.iter().any(|skill| skill == "ranged") {
                "ranged"
            } else if skills.iter().any(|skill| skill == "magic") {
                "magic"
            } else {
                "melee"
            }
        }
        _ => "melee",
    }
}

/// Where the brain sends one method for a bot: a workable site, the
/// combat level the least demanding site needs (a sub-task to train),
/// or nothing to choose (the method works anywhere).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SitePick {
    /// No site family for this method (smithing, quests...).
    Anywhere,
    /// The nearest site this bot works safely (x, z, level).
    At((i64, i64, i64)),
    /// No site is safe at this combat level: train to this level first
    /// (`NEVER`: no level ever makes one safe, the method is unworkable).
    Needs(u8),
}

/// (site, requirement) list of one method, computed once from the data.
fn method_site_table(method: &str) -> Option<std::sync::Arc<Vec<((i64, i64, i64), u8)>>> {
    type Table = std::collections::HashMap<String, Option<std::sync::Arc<Vec<((i64, i64, i64), u8)>>>>;
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Table>> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    if let Some(known) = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(method) {
        return known.clone();
    }
    let table = method_sites(method).map(|(kind, resource)| {
        std::sync::Arc::new(
            lifescape_gamedata::danger::sites(kind, &resource)
                .into_iter()
                .map(|(x, z, level)| ((x, z, level), lifescape_gamedata::danger::site_requirement(x, z, level)))
                .collect::<Vec<_>>(),
        )
    });
    let table = table.filter(|rows| !rows.is_empty());
    cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(method.to_string(), table.clone());
    table
}

/// The brain's site choice for one method: among the sites this combat
/// level works safely (site and bank), the nearest to the bot; else the
/// level the least demanding site needs.
pub fn pick_site(method: &str, combat: u8, position: Option<(i64, i64, i64)>) -> SitePick {
    let Some(table) = method_site_table(method) else {
        return SitePick::Anywhere;
    };
    let distance = |site: &(i64, i64, i64)| match position {
        Some((x, z, level)) => (site.0 - x).abs() + (site.1 - z).abs() + if site.2 == level { 0 } else { 1000 },
        None => 0,
    };
    match table.iter().filter(|(_, required)| *required <= combat).min_by_key(|(site, _)| distance(site)) {
        Some((site, _)) => SitePick::At(*site),
        None => SitePick::Needs(table.iter().map(|(_, required)| *required).min().unwrap_or(lifescape_gamedata::danger::NEVER)),
    }
}

#[cfg(test)]
mod tests {
    use super::{combat_level, goal_combat_style, min_combat_for_method, pick_site, training_method, BaseLevels, CombatError, SitePick};
    use crate::model::GoalKind;

    fn levels(attack: u8, strength: u8, defence: u8, hitpoints: u8, ranged: u8, prayer: u8, magic: u8) -> BaseLevels {
        BaseLevels { attack, strength, defence, hitpoints, ranged, prayer, magic }
    }

    #[test]
    fn fresh_accounts_read_three() {
        assert_eq!(combat_level(&BaseLevels::fresh()), 3, "neuf = 3");
    }

    #[test]
    fn formula_matches_known_builds() {
        // Melee 40s: base 20 + melee 26 = 46.
        assert_eq!(combat_level(&levels(40, 40, 40, 40, 1, 1, 1)), 46);
        // Ranger 40: base 2.75 + range 19.5 = 22.25 -> 22.
        assert_eq!(combat_level(&levels(1, 1, 1, 10, 40, 1, 1)), 22);
        // 60/60/60 + 43 prayer (classic 74): base 35.25 + melee 39.
        assert_eq!(combat_level(&levels(60, 60, 60, 60, 1, 43, 1)), 74);
        // Maxed: base 61.75 + melee 64.35 -> 126.
        assert_eq!(combat_level(&levels(99, 99, 99, 99, 99, 99, 99)), 126);
    }

    /// Requirements are computed from the data (no zone in the code): the
    /// least demanding site of each method, its bank included.
    #[test]
    fn requirements_come_from_the_sites() {
        assert_eq!(min_combat_for_method("gather:fishing/raw_shrimp"), 15, "only Draynor water, under the level-7 wizards");
        assert_eq!(min_combat_for_method("farm:raw_shrimp"), 15, "the obtain leg fishes the same water");
        assert_eq!(min_combat_for_method("gather:woodcut/willow_logs"), 3, "some willows sit outside every hunter");
        assert_eq!(min_combat_for_method("fight:melee/chickens"), 3);
        assert_eq!(min_combat_for_method("gather:fishing/raw_lobster"), lifescape_gamedata::danger::NEVER, "no bank within reach of Karamja water");
        assert_eq!(min_combat_for_method("smith:steel_platebody"), 5);
        assert_eq!(min_combat_for_method("quest:cook"), 3);
    }

    /// The brain sends a fresh bot to a site it works safely, and asks for
    /// training when no site is safe yet.
    #[test]
    fn site_pick_orients_or_requires_training() {
        let lumbridge = Some((3222, 3218, 0));
        match pick_site("gather:woodcut/willow_logs", 3, lumbridge) {
            SitePick::At((x, z, level)) => {
                assert!(lifescape_gamedata::danger::site_requirement(x, z, level) <= 3, "picked a site a CB3 cannot work: ({x},{z})");
            }
            other => panic!("willows are open to CB3, got {other:?}"),
        }
        assert_eq!(pick_site("gather:fishing/raw_shrimp", 3, lumbridge), SitePick::Needs(15));
        assert!(matches!(pick_site("gather:fishing/raw_shrimp", 15, lumbridge), SitePick::At(_)));
        assert_eq!(pick_site("smith:bronze_bar", 3, lumbridge), SitePick::Anywhere);
    }

    #[test]
    fn priority_training_follows_the_goal_style_on_clean_camps() {        assert_eq!(training_method("ranged"), Ok("fight:ranged/chickens"));
        assert_eq!(training_method("magic"), Ok("fight:magic/chickens"));
        for style in ["melee", "attack", "strength", "prayer"] {
            assert_eq!(training_method(style), Ok("fight:melee/chickens"), "{style} trains melee");
        }
        assert_eq!(
            training_method("sailing"),
            Err(CombatError::UnknownStyle("sailing".to_string())),
            "unknown styles fail typed"
        );
        // Every training method executes (mapping knows it) on the
        // weakest CLEAN camp (chickens: safe, minimum 1).
        let chickens = lifescape_gamedata::combat_spots::by_key("chickens").expect("chickens");
        assert_eq!(chickens.danger, "safe");
        assert_eq!(chickens.min_level, 1);
        let probe = crate::model::Node::new(
            "n9",
            crate::model::NodeKind::ObtainItems { item: "feather".to_string(), count: 1 },
            Vec::new(),
            1,
        );
        for style in ["melee", "ranged", "magic"] {
            let method = training_method(style).expect("style trains");
            assert!(crate::mapping::map_method(method, &probe, None).is_some(), "{method} executes");
        }
    }

    #[test]
    fn goal_style_feeds_the_priority_training() {
        assert_eq!(goal_combat_style(&GoalKind::EquipBest { style: "ranged".to_string() }), "ranged");
        assert_eq!(goal_combat_style(&GoalKind::EquipBest { style: "magic".to_string() }), "magic");
        assert_eq!(goal_combat_style(&GoalKind::EquipBest { style: "melee".to_string() }), "melee");
        assert_eq!(
            goal_combat_style(&GoalKind::MaxSkills { skills: vec!["attack".to_string(), "ranged".to_string()], level: 40 }),
            "ranged",
            "ranged wins the mix"
        );
        assert_eq!(
            goal_combat_style(&GoalKind::MaxSkills { skills: vec!["magic".to_string()], level: 40 }),
            "magic"
        );
        assert_eq!(goal_combat_style(&GoalKind::MaxSkills { skills: vec!["fishing".to_string()], level: 40 }), "melee");
        assert_eq!(goal_combat_style(&GoalKind::MakeMoney), "melee", "producers fight melee");
        // Every goal style trains (jamais d'envoi vers la mort).
        for goal in [
            GoalKind::MakeMoney,
            GoalKind::MaxSkills { skills: vec!["ranged".to_string()], level: 40 },
            GoalKind::EquipBest { style: "magic".to_string() },
        ] {
            assert!(training_method(goal_combat_style(&goal)).is_ok(), "{goal:?} trains");
        }
    }

}
