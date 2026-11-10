//! Frozen F2P activity catalogue (PRD 2, D56/D64, M10).
//!
//! Reference: `prd/CATALOGUE_activites_F2P_274.md`. Every F2P activity of
//! revision 274 is one row here; the planner (waves 2-3) reads these rows,
//! never hard-coded coordinates (D57) or member flags. P2.1 fills the rows
//! from the generated gamedata (M10b: station positions, recipes, NPC
//! services, combat spots, XP per action); this module only freezes the
//! shape those rows must parse into, plus JSON so a dump can feed it.
//!
//! Glossary: `Arch` is the catalogue §5 column (R decor harvest, P NPC
//! harvest, S ground collect, T station, I inventory, C combat, M utility
//! magic, Q quest). `Epoch` is the D64 priority: P1 (2004-2005 guides,
//! preferred) over P2 (possible in 274, used when it unlocks or serves).

#![forbid(unsafe_code)]

use lifescape_json::{object, str_value, Value};

/// Activity archetype: the catalogue §5 `Arch.` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    /// Decor harvest (tree, rock, fishing spot…).
    Decor,
    /// NPC harvest (shearing, milking, telegrab wine…).
    Npc,
    /// Ground collect (coin heaps, tools, spawns…).
    Ground,
    /// Station transform (furnace, anvil, range, wheel, altar…).
    Station,
    /// Inventory transform (firemaking, burying, leather, alchemy…).
    Inventory,
    /// Combat style (melee, ranged, combat magic…).
    Combat,
    /// Utility magic (teleport, superheat, enchant…).
    UtilityMagic,
    /// Quest shortcut (XP or unlock the brain plans, D59).
    Quest,
}

impl Arch {
    pub fn code(self) -> &'static str {
        match self {
            Arch::Decor => "R",
            Arch::Npc => "P",
            Arch::Ground => "S",
            Arch::Station => "T",
            Arch::Inventory => "I",
            Arch::Combat => "C",
            Arch::UtilityMagic => "M",
            Arch::Quest => "Q",
        }
    }

    pub fn parse(code: &str) -> Option<Arch> {
        match code {
            "R" => Some(Arch::Decor),
            "P" => Some(Arch::Npc),
            "S" => Some(Arch::Ground),
            "T" => Some(Arch::Station),
            "I" => Some(Arch::Inventory),
            "C" => Some(Arch::Combat),
            "M" => Some(Arch::UtilityMagic),
            "Q" => Some(Arch::Quest),
            _ => None,
        }
    }
}

/// Era priority (D64): P1 first, P2 when it unlocks or serves a goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Epoch {
    /// Attested by 2004-2005 guides: the brain prefers it.
    P1,
    /// Possible in 274, used when it serves a goal or unblocks.
    P2,
}

impl Epoch {
    pub fn as_str(self) -> &'static str {
        match self {
            Epoch::P1 => "P1",
            Epoch::P2 => "P2",
        }
    }

    pub fn parse(raw: &str) -> Option<Epoch> {
        match raw {
            "P1" => Some(Epoch::P1),
            "P2" => Some(Epoch::P2),
            _ => None,
        }
    }
}

/// Access prerequisites of one activity: tools, quests, area access.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Prereqs {
    pub tools: Vec<String>,
    pub quests: Vec<String>,
    pub access: Vec<String>,
}

/// One item stack (input consumed or product made).
#[derive(Debug, Clone, PartialEq)]
pub struct Stack {
    pub item: String,
    pub count: i64,
}

/// One catalogue row (PRD 2 §4.2): fixed per activity, the brain fills the
/// objective (XP, level, quantity) per node. `station` is the post (furnace,
/// anvil, range…) for `T` rows, `None` otherwise; `level` is 0 when no
/// skill level is required (quests, services).
#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    pub id: String,
    pub skill: String,
    pub level: u8,
    pub arch: Arch,
    pub epoch: Epoch,
    pub prereqs: Prereqs,
    pub inputs: Vec<Stack>,
    pub products: Vec<Stack>,
    pub station: Option<String>,
    pub xp_per_action: f64,
}

fn stacks_value(stacks: &[Stack]) -> Value {
    Value::Arr(
        stacks
            .iter()
            .map(|stack| object(vec![("item", str_value(&stack.item)), ("count", Value::Int(stack.count))]))
            .collect(),
    )
}

fn stacks_from(value: &Value) -> Option<Vec<Stack>> {
    value
        .as_arr()
        .map(|rows| {
            rows.iter()
                .map(|row| Stack {
                    item: row.get("item").as_str().unwrap_or("").to_string(),
                    count: row.get("count").as_i64().unwrap_or(0),
                })
                .collect()
        })
}

/// Catalogue JSON: `{"activities": [<activity>…]}`.
pub fn catalog_to_value(activities: &[Activity]) -> Value {
    object(vec![(
        "activities",
        Value::Arr(activities.iter().map(activity_to_value).collect()),
    )])
}

pub fn catalog_from_value(value: &Value) -> Option<Vec<Activity>> {
    value
        .get("activities")
        .as_arr()
        .map(|rows| rows.iter().filter_map(activity_from_value).collect())
}

pub fn activity_to_value(activity: &Activity) -> Value {
    object(vec![
        ("id", str_value(&activity.id)),
        ("skill", str_value(&activity.skill)),
        ("level", Value::Int(i64::from(activity.level))),
        ("arch", str_value(activity.arch.code())),
        ("epoch", str_value(activity.epoch.as_str())),
        (
            "prereqs",
            object(vec![
                (
                    "tools",
                    Value::Arr(activity.prereqs.tools.iter().map(|tool| str_value(tool)).collect()),
                ),
                (
                    "quests",
                    Value::Arr(activity.prereqs.quests.iter().map(|quest| str_value(quest)).collect()),
                ),
                (
                    "access",
                    Value::Arr(activity.prereqs.access.iter().map(|zone| str_value(zone)).collect()),
                ),
            ]),
        ),
        ("inputs", stacks_value(&activity.inputs)),
        ("products", stacks_value(&activity.products)),
        (
            "station",
            activity.station.as_deref().map(str_value).unwrap_or(Value::Null),
        ),
        ("xp_per_action", Value::Float(activity.xp_per_action)),
    ])
}

pub fn activity_from_value(value: &Value) -> Option<Activity> {
    let prereqs = value.get("prereqs");
    let strings = |key: &str| {
        prereqs
            .get(key)
            .as_arr()
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| row.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    Some(Activity {
        id: value.get("id").as_str().unwrap_or("").to_string(),
        skill: value.get("skill").as_str().unwrap_or("").to_string(),
        level: value.get("level").as_i64().unwrap_or(-1).clamp(0, 99) as u8,
        arch: Arch::parse(value.get("arch").as_str().unwrap_or(""))?,
        epoch: Epoch::parse(value.get("epoch").as_str().unwrap_or(""))?,
        prereqs: Prereqs {
            tools: strings("tools"),
            quests: strings("quests"),
            access: strings("access"),
        },
        inputs: stacks_from(value.get("inputs"))?,
        products: stacks_from(value.get("products"))?,
        station: value.get("station").as_str().map(str::to_string),
        xp_per_action: value.get("xp_per_action").as_f64().unwrap_or(0.0),
    })
}

/// Shape check before the planner trusts a row: identifiers present,
/// skill level in range, station exactly on `T` rows, XP non-negative.
/// Pure interface guard; P2.1 calls it on every generated row.
pub fn validate(activity: &Activity) -> Result<(), String> {
    if activity.id.is_empty() {
        return Err("activity id is empty".to_string());
    }
    if activity.skill.is_empty() {
        return Err(format!("activity {} has no skill", activity.id));
    }
    if activity.level > 99 {
        return Err(format!("activity {} level {} out of range", activity.id, activity.level));
    }
    if activity.arch == Arch::Station && activity.station.as_deref().unwrap_or("").is_empty() {
        return Err(format!("station activity {} names no station", activity.id));
    }
    if activity.arch != Arch::Station && activity.arch != Arch::Inventory && activity.station.is_some() {
        return Err(format!("non-transform activity {} names a station", activity.id));
    }
    if activity.xp_per_action < 0.0 {
        return Err(format!("activity {} has negative xp_per_action", activity.id));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{activity_from_value, activity_to_value, catalog_from_value, catalog_to_value, validate, Activity, Arch, Epoch, Prereqs, Stack};

    fn iron_bar() -> Activity {
        Activity {
            id: "smelting/iron".to_string(),
            skill: "smithing".to_string(),
            level: 15,
            arch: Arch::Station,
            epoch: Epoch::P1,
            prereqs: Prereqs {
                tools: Vec::new(),
                quests: Vec::new(),
                access: vec!["al_kharid".to_string()],
            },
            inputs: vec![Stack { item: "iron_ore".to_string(), count: 1 }],
            products: vec![Stack { item: "iron_bar".to_string(), count: 1 }],
            station: Some("furnace".to_string()),
            xp_per_action: 12.5,
        }
    }

    /// Catalogue rows round-trip through JSON: the M10b dump feeds this.
    #[test]
    fn catalogue_rows_round_trip() {
        let rows = vec![iron_bar()];
        let back = catalog_from_value(&catalog_to_value(&rows)).expect("catalogue parses back");
        assert_eq!(back, rows);
        let one = activity_from_value(&activity_to_value(&iron_bar())).expect("row parses back");
        assert_eq!(one, iron_bar());
        assert_eq!(validate(&iron_bar()), Ok(()));
    }

    /// Arch codes cover exactly the eight catalogue archetypes.
    #[test]
    fn arch_codes_cover_the_catalogue() {
        for (code, arch) in [
            ("R", Arch::Decor),
            ("P", Arch::Npc),
            ("S", Arch::Ground),
            ("T", Arch::Station),
            ("I", Arch::Inventory),
            ("C", Arch::Combat),
            ("M", Arch::UtilityMagic),
            ("Q", Arch::Quest),
        ] {
            assert_eq!(Arch::parse(code), Some(arch));
            assert_eq!(arch.code(), code);
        }
        assert_eq!(Arch::parse("X"), None);
    }

    /// Era priority is P1/P2 only: the brain prefers P1 (D64).
    #[test]
    fn epoch_is_p1_or_p2() {
        assert_eq!(Epoch::parse("P1"), Some(Epoch::P1));
        assert_eq!(Epoch::parse("P2"), Some(Epoch::P2));
        assert_eq!(Epoch::parse("P3"), None);
        assert_eq!((Epoch::P1.as_str(), Epoch::P2.as_str()), ("P1", "P2"));
    }

    /// The guard names the offending row: station rows need a station,
    /// other rows must not name one.
    #[test]
    fn validate_names_offending_rows() {
        assert!(validate(&iron_bar()).is_ok());
        let mut stationless = iron_bar();
        stationless.station = None;
        assert!(validate(&stationless).is_err(), "T without station fails loud");
        let mut stray = iron_bar();
        stray.arch = Arch::Combat;
        assert!(validate(&stray).is_err(), "combat with a station fails loud");
        let mut idless = iron_bar();
        idless.id.clear();
        assert!(validate(&idless).is_err(), "empty id fails loud");
    }
}
