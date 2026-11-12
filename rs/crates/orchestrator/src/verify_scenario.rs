//! `verify` scenario files: JSON shape, validation, discovery.
//!
//! One file per scenario in `scenarios/` (`id.json`), read with
//! `lifescape-json`. The runner (`verify_cmd`) assigns accounts, runs the
//! setup cheats and checks the asserts; this module only knows the shape.

#![forbid(unsafe_code)]

use lifescape_json::{self as json, Value};

// -- scenario format ---------------------------------------------------------

pub struct ScenarioRun {
    pub script: String,
    pub args: Vec<String>,
    pub budget_ticks: i64,
}

pub enum SetupOp {
    TeleAdmin { level: i64, x: i64, z: i64 },
    Teleother,
    Setvarother { var: String, value: i64 },
    BankAll { keep: Vec<String> },
}

pub enum Assert {
    Position { level: i64, x: i64, z: i64, radius: i64 },
    Inventory { item: String, min: i64 },
    InventoryDelta { item: String, min_delta: i64 },
    Xp { skill: i64, name: String, min_gain: i64 },
    Message { contains: String },
    Npc { name: String, max_dist: i64 },
}

pub struct Scenario {
    pub id: String,
    pub accounts: Vec<String>,
    pub setup: Vec<SetupOp>,
    pub run: ScenarioRun,
    pub assert: Vec<Assert>,
    pub timeout_s: f64,
}

pub fn need_str(value: &Value, field: &str) -> Result<String, String> {
    value.as_str().map(str::to_string).ok_or_else(|| format!("{field} must be a string"))
}

pub fn need_i64(value: &Value, field: &str) -> Result<i64, String> {
    value.as_i64().ok_or_else(|| format!("{field} must be a number"))
}

pub fn need_f64(value: &Value, field: &str) -> Result<f64, String> {
    value.as_f64().ok_or_else(|| format!("{field} must be a number"))
}

/// Engine stat order (`PlayerStat.ts`): COOKING 7, WOODCUTTING 8, FISHING 10…
pub fn skill_id(name: &str) -> Option<i64> {
    match name.to_ascii_lowercase().as_str() {
        "attack" => Some(0),
        "defence" | "defense" => Some(1),
        "strength" => Some(2),
        "hitpoints" | "hp" => Some(3),
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
        "runecraft" => Some(20),
        _ => None,
    }
}

pub fn parse_setup(value: &Value) -> Result<SetupOp, String> {
    match value.get("op").as_str().unwrap_or("") {
        "tele_admin" => Ok(SetupOp::TeleAdmin {
            level: need_i64(value.get("level"), "setup.tele_admin.level")?,
            x: need_i64(value.get("x"), "setup.tele_admin.x")?,
            z: need_i64(value.get("z"), "setup.tele_admin.z")?,
        }),
        "teleother" => Ok(SetupOp::Teleother),
        // Rule 13: a scenario never hands cheated goods to its account;
        // the run uses what the account holds (banked, the script withdraws).
        "giveother" => Err(crate::economy_guard::cheat_refusal("giveother").unwrap_or_default()),
        "setvarother" => Ok(SetupOp::Setvarother {
            var: need_str(value.get("var"), "setup.setvarother.var")?,
            value: need_i64(value.get("value"), "setup.setvarother.value")?,
        }),
        "bank_all" => Ok(SetupOp::BankAll {
            keep: value
                .get("keep")
                .as_arr()
                .map(|items| items.iter().map(|entry| need_str(entry, "setup.bank_all.keep[]")).collect::<Result<Vec<_>, _>>())
                .transpose()?
                .unwrap_or_default(),
        }),
        other => Err(format!("unknown setup op {other:?} (want tele_admin|teleother|setvarother|bank_all)")),
    }
}

pub fn parse_assert(value: &Value) -> Result<Assert, String> {
    match value.get("type").as_str().unwrap_or("") {
        "position" => Ok(Assert::Position {
            level: need_i64(value.get("level"), "assert.position.level")?,
            x: need_i64(value.get("x"), "assert.position.x")?,
            z: need_i64(value.get("z"), "assert.position.z")?,
            radius: value.get("radius").as_i64().unwrap_or(3),
        }),
        "inventory" => Ok(Assert::Inventory {
            item: need_str(value.get("item"), "assert.inventory.item")?,
            min: need_i64(value.get("min"), "assert.inventory.min")?,
        }),
        "inventory_delta" => Ok(Assert::InventoryDelta {
            item: need_str(value.get("item"), "assert.inventory_delta.item")?,
            min_delta: need_i64(value.get("min_delta"), "assert.inventory_delta.min_delta")?,
        }),
        "xp" => {
            let name = need_str(value.get("skill"), "assert.xp.skill")?;
            let skill = skill_id(&name).ok_or_else(|| format!("unknown skill {name:?}"))?;
            Ok(Assert::Xp { skill, name, min_gain: need_i64(value.get("min_gain"), "assert.xp.min_gain")? })
        }
        "message" => Ok(Assert::Message { contains: need_str(value.get("contains"), "assert.message.contains")? }),
        "npc" => Ok(Assert::Npc {
            name: need_str(value.get("name"), "assert.npc.name")?,
            max_dist: value.get("max_dist").as_i64().unwrap_or(12),
        }),
        other => Err(format!("unknown assert type {other:?} (want position|inventory|inventory_delta|xp|message|npc)")),
    }
}

pub fn parse_scenario(text: &str) -> Result<Scenario, String> {
    let value = json::parse(text).map_err(|error| format!("bad scenario JSON: {error}"))?;
    let id = need_str(value.get("id"), "id")?;
    let accounts: Vec<String> = value
        .get("accounts")
        .as_arr()
        .ok_or_else(|| "accounts must be an array".to_string())?
        .iter()
        .map(|entry| need_str(entry, "accounts[]"))
        .collect::<Result<_, _>>()?;
    if accounts.is_empty() {
        return Err("accounts must not be empty".to_string());
    }
    if accounts.iter().any(|name| name == "bite") {
        return Err("bite is forbidden as a test account".to_string());
    }
    let setup: Vec<SetupOp> = value
        .get("setup")
        .as_arr()
        .ok_or_else(|| "setup must be an array".to_string())?
        .iter()
        .map(parse_setup)
        .collect::<Result<_, _>>()?;
    let run = value.get("run");
    let scenario_run = ScenarioRun {
        script: need_str(run.get("script"), "run.script")?,
        args: run
            .get("args")
            .as_arr()
            .ok_or_else(|| "run.args must be an array".to_string())?
            .iter()
            .map(|entry| need_str(entry, "run.args[]"))
            .collect::<Result<_, _>>()?,
        budget_ticks: need_i64(run.get("budget_ticks"), "run.budget_ticks")?,
    };
    if scenario_run.budget_ticks <= 0 {
        return Err("run.budget_ticks must be positive".to_string());
    }
    let assert: Vec<Assert> = value
        .get("assert")
        .as_arr()
        .ok_or_else(|| "assert must be an array".to_string())?
        .iter()
        .map(parse_assert)
        .collect::<Result<_, _>>()?;
    if assert.is_empty() {
        return Err("assert must not be empty".to_string());
    }
    Ok(Scenario {
        id,
        accounts,
        setup,
        run: scenario_run,
        assert,
        timeout_s: need_f64(value.get("timeout_s"), "timeout_s")?,
    })
}

pub fn scenarios_dir() -> std::path::PathBuf {
    crate::paths::root().join("scenarios")
}

pub fn load_scenario(id: &str) -> Result<Scenario, String> {
    if id.contains('/') || id.contains('\\') || id.contains('.') {
        return Err(format!("bad scenario id {id:?}"));
    }
    let path = scenarios_dir().join(format!("{id}.json"));
    let text = std::fs::read_to_string(&path).map_err(|_| format!("unknown scenario {id:?}"))?;
    parse_scenario(&text).map_err(|error| format!("scenario {id}: {error}"))
}

pub fn list_scenarios() -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    let entries = std::fs::read_dir(scenarios_dir()).map_err(|_| "no scenarios/ directory".to_string())?;
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(id) = name.strip_suffix(".json") {
            if !id.is_empty() {
                ids.push(id.to_string());
            }
        }
    }
    ids.sort();
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every scenario file in the repo parses and names a known script.
    #[test]
    fn shipped_scenarios_parse() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scenarios");
        let mut count = 0;
        let mut names: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("scenarios dir") {
            let path = entry.expect("entry").path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let scenario = parse_scenario(&std::fs::read_to_string(&path).expect("reads")).expect("parses");
            assert!(lifescape_scripts::names().contains(&scenario.run.script.as_str()), "known script");
            // Smoke budget (W4): 2000 ticks max, sauf scénarios longs déclarés.
            let long = [("tutorial_fresh", "tutoriel complet, pas un smoke")];
            if let Some((_, why)) = long.iter().find(|(id, _)| *id == scenario.id) {
                assert!(scenario.run.budget_ticks > 2000, "{why}");
            } else {
                assert!(scenario.run.budget_ticks <= 2000, "smoke budget (W4)");
            }
            count += 1;
            names.push(scenario.id);
        }
        for want in ["walk_lumbridge", "harvest_grain", "cook_shrimp", "pickup_falador_pickaxe", "tutorial_fresh"] {
            assert!(names.contains(&want.to_string()), "ships {want}");
        }
        assert_eq!(count, 5, "W2 (3) + pickup + tutorial");
    }

    /// Unknown ids, ops and asserts fail loud with a typed message.
    #[test]
    fn bad_inputs_fail_loud() {
        assert!(load_scenario("../bite").is_err(), "path escape refused");
        assert!(parse_scenario(r#"{"id": "x"}"#).is_err(), "truncated fails");
        assert!(parse_scenario("{\"id\":\"x\",\"accounts\":[\"bite\"],\"setup\":[],\"run\":{\"script\":\"walkto\",\"args\":[],\"budget_ticks\":1},\"assert\":[{\"type\":\"position\",\"level\":0,\"x\":0,\"z\":0}],\"timeout_s\":1.0}").is_err(), "bite refused");
        assert!(parse_setup(&json::parse(r#"{"op": "fly"}"#).unwrap()).is_err(), "unknown op");
        assert!(parse_assert(&json::parse(r#"{"type": "loot"}"#).unwrap()).is_err(), "unknown assert");
    }

    /// W4 budgets: 2-3x the W5/W2 p95 (walk ~50t, cook ~1000t, harvest
    /// ~1500t), clipped by the smoke cap 2000 (pack-bounded worst case),
    /// each with a wall ceiling (`timeout_s`).
    #[test]
    fn w4_budgets_follow_p95_bands() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scenarios");
        let scenario = |id: &str| {
            let text = std::fs::read_to_string(dir.join(format!("{id}.json"))).expect("scenario reads");
            parse_scenario(&text).expect("parses")
        };
        let walk = scenario("walk_lumbridge");
        assert_eq!(walk.run.budget_ticks, 150, "3x walk p95 (~50t)");
        let cook = scenario("cook_shrimp");
        assert_eq!(cook.run.budget_ticks, 1700, "pack-bounded cook, under smoke cap");
        let harvest = scenario("harvest_grain");
        assert_eq!(harvest.run.budget_ticks, 1700, "pack-bounded harvest, under smoke cap");
        for scenario in [walk, cook, harvest] {
            assert!(scenario.run.budget_ticks <= 2000, "smoke cap");
            assert!(scenario.timeout_s >= 60.0, "wall ceiling present");
        }
    }
}
