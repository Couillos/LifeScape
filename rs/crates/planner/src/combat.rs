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

/// True for any fight method against `foe` (`fight:melee/men`,
/// `fight:ranged/men`, `fight:magic/men` share one hunting ground).
fn fights_foe(method: &str, foe: &str) -> bool {
    method.starts_with("fight:") && method.ends_with(foe)
}

/// Minimum combat level to work one planner method without being
/// hunted on the way in. Every harvest/transform method carries one;
/// unknown methods are safe at 3 (CLEAN unless the matrix says else).
pub fn min_combat_for_method(method: &str) -> u8 {
    if method == "gather:woodcut/willow_logs" {
        15
    } else if fights_foe(method, "/men") {
        13
    } else if method == "gather:mining/coal" {
        43
    } else if method == "gather:mining/gold_ore" {
        41
    } else if fights_foe(method, "/lesser_demons") {
        // Volcano entrance transit (scorpion 14): the demons themselves
        // are the training target, the walk in is the gate.
        29
    } else if method.starts_with("smith:") {
        5
    } else {
        3
    }
}

/// Safe ground for the same method when the level falls short: the
/// same resource exists hunter-free around Lumbridge (willows south of
/// the castle with only far spiders, men with no hunter in 25 tiles).
/// None when no safe same-skill ground exists (train first instead).
pub fn safe_area_for_method(method: &str) -> Option<&'static str> {
    if method == "gather:woodcut/willow_logs" || fights_foe(method, "/men") {
        Some("lumbridge")
    } else {
        None
    }
}

/// What the brain owes a bot whose level falls short: run the same
/// method from safe ground (an access-like move first), or train
/// combat to the requirement before any trip past it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CombatGate {
    /// Strong enough: dispatch the method as planned.
    Clear { required: u8 },
    /// Same method, safe area first (Lumbridge ground, same resource).
    Relocate { required: u8, area: &'static str },
    /// No safe ground: a priority `reach_combat` task to this level
    /// runs before the dangerous one (never a trip past the level).
    TrainFirst { required: u8 },
}

/// Gate one method dispatch on the bot's current combat level.
pub fn combat_gate(current: u8, method: &str) -> CombatGate {
    let required = min_combat_for_method(method);
    if current >= required {
        CombatGate::Clear { required }
    } else if let Some(area) = safe_area_for_method(method) {
        CombatGate::Relocate { required, area }
    } else {
        CombatGate::TrainFirst { required }
    }
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

#[cfg(test)]
mod tests {
    use super::{combat_gate, combat_level, min_combat_for_method, safe_area_for_method, training_method, BaseLevels, CombatError, CombatGate};

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

    #[test]
    fn matrix_pins_every_risky_spot() {
        assert_eq!(min_combat_for_method("gather:woodcut/willow_logs"), 15, "saules Draynor");
        assert_eq!(min_combat_for_method("fight:melee/men"), 13, "hommes Varrock-Est");
        assert_eq!(min_combat_for_method("fight:ranged/men"), 13, "style keeps the ground");
        assert_eq!(min_combat_for_method("fight:magic/men"), 13, "style keeps the ground");
        assert_eq!(min_combat_for_method("gather:mining/coal"), 43, "charbon 2701,3331");
        assert_eq!(min_combat_for_method("gather:mining/gold_ore"), 41, "or 2733,3225");
        assert_eq!(min_combat_for_method("fight:melee/lesser_demons"), 29, "volcan entree");
        assert_eq!(min_combat_for_method("smith:steel_platebody"), 5, "enclume 2991,3689");
        for safe in [
            "gather:fishing/raw_trout",
            "gather:mining/iron_ore",
            "smelt:steel_bar",
            "cook:trout",
            "farm:feather",
            "quest:cook",
        ] {
            assert_eq!(min_combat_for_method(safe), 3, "{safe} sur par defaut");
        }
    }

    #[test]
    fn matrix_matches_the_2x_rule_in_the_dumps() {
        // Requirement is 2 * hunter vislevel + 1 (Npc.ts:1014).
        let hunter = |name: &str| {
            let entry = lifescape_gamedata::npcs_by_name(name).into_iter().next().unwrap_or_else(|| panic!("{name} in dumps"));
            assert!(entry.hunts_players, "{name} hunts");
            entry.vislevel
        };
        assert_eq!(min_combat_for_method("gather:woodcut/willow_logs"), 2 * hunter("young_dark_wizard") as u8 + 1);
        assert_eq!(min_combat_for_method("fight:melee/men"), 2 * hunter("mugger") as u8 + 1);
        assert_eq!(min_combat_for_method("gather:mining/coal"), 2 * hunter("brownbear") as u8 + 1);
        assert_eq!(min_combat_for_method("gather:mining/gold_ore"), 2 * hunter("poison_scorpion") as u8 + 1);
        assert_eq!(min_combat_for_method("fight:melee/lesser_demons"), 2 * hunter("scorpion") as u8 + 1);
        assert_eq!(min_combat_for_method("smith:steel_platebody"), 2 * hunter("giantspider1") as u8 + 1);
        // Fallback ground is hunter-free (Lumbridge): men with no hunter
        // in 25 tiles, willows with only far spiders around.
        let lumb_men = lifescape_gamedata::spawns::npc_spawns_near(3222, 3218, 0, 25);
        assert!(!lumb_men.is_empty(), "men stand around Lumbridge");
        for spawn in &lumb_men {
            let entry = lifescape_gamedata::npc_by_id(spawn.npc_id).expect("npc resolves");
            assert!(!entry.hunts_players, "hunter {} near Lumbridge men", entry.name.unwrap_or("?"));
        }
        let willow_near_lumbridge = lifescape_gamedata::locs_by_name("willowtree")
            .iter()
            .flat_map(|loc| loc.spawns)
            .any(|spawn| (spawn.x - 3222).abs() + (spawn.z - 3218).abs() <= 60);
        assert!(willow_near_lumbridge, "saules Lumbridge exist");
    }

    #[test]
    fn under_level_picks_the_safe_fallback_or_trains_first() {
        assert_eq!(
            combat_gate(3, "gather:woodcut/willow_logs"),
            CombatGate::Relocate { required: 15, area: "lumbridge" },
            "CB3 aux saules -> Lumbridge, pas Draynor"
        );
        assert_eq!(
            combat_gate(3, "fight:melee/men"),
            CombatGate::Relocate { required: 13, area: "lumbridge" },
            "men at Lumbridge under level"
        );
        assert_eq!(
            combat_gate(3, "gather:mining/coal"),
            CombatGate::TrainFirst { required: 43 },
            "no safe coal: combat d'abord"
        );
        assert_eq!(
            combat_gate(40, "gather:mining/gold_ore"),
            CombatGate::TrainFirst { required: 41 },
            "one short still trains"
        );
        assert_eq!(
            combat_gate(43, "gather:mining/coal"),
            CombatGate::Clear { required: 43 },
            "at level dispatches"
        );
        assert_eq!(safe_area_for_method("gather:mining/coal"), None, "coal has no safe ground");
    }

    #[test]
    fn no_trip_past_the_level_on_any_pinned_method() {
        // Guard: one below the requirement never clears, at the
        // requirement always clears (jamais d'envoi vers la mort).
        for method in [
            "gather:woodcut/willow_logs",
            "fight:melee/men",
            "fight:ranged/men",
            "fight:magic/men",
            "gather:mining/coal",
            "gather:mining/gold_ore",
            "fight:melee/lesser_demons",
            "fight:ranged/lesser_demons",
            "fight:magic/lesser_demons",
            "smith:steel_platebody",
        ] {
            let required = min_combat_for_method(method);
            assert!(
                !matches!(combat_gate(required - 1, method), CombatGate::Clear { .. }),
                "{method} below {required} never clears"
            );
            assert_eq!(
                combat_gate(required, method),
                CombatGate::Clear { required },
                "{method} at {required} clears"
            );
        }
    }

    #[test]
    fn priority_training_follows_the_goal_style_on_clean_camps() {
        assert_eq!(training_method("ranged"), Ok("fight:ranged/chickens"));
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
}
