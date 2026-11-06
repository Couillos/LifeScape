//! Supervised bot scripts: one trait, one compiled registry.
//!
//! Every script the orchestrator can launch. Every entry runs for
//! real through the SDK; unknown names report errors, never silent stubs.

pub mod devtools;
pub mod devtools_check;
pub mod devtools_probe;
pub mod maze;
pub mod maze_doors;
pub mod skill_common;
pub mod skill_trade;
pub mod skill_walk;
pub mod skill_fishing;
pub mod skill_combat;
pub mod skill_mining;
pub mod skill_woodcutting;
pub mod quest_common;
pub mod quests_a;
pub mod quests_b;
pub mod randomevent;
pub mod randomevent_full;
pub mod rescues;
pub mod tables_live;
pub mod tutorial;

use lifescape_sdk::{Bot, Error};

pub use tables_live::{LIVETABLES, attach_live_tables};

/// A supervised script: the session owns the connection, the script only
/// names the destination. Args come from the orchestrator (`run t01 goto
/// 3200 3227`), never from the script itself.
pub trait Script {
    fn name(&self) -> &'static str;
    fn run(&self, bot: &mut Bot, args: &[String]) -> ScriptResult;
}

/// Script exit: Ok(code) on success, Err for failures.
pub type ScriptResult = Result<i32, ScriptError>;

#[derive(Debug)]
pub enum ScriptError {
    Sdk(Error),
    Usage(String),
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScriptError::Sdk(err) => write!(formatter, "sdk: {err}"),
            ScriptError::Usage(text) => write!(formatter, "usage: {text}"),
        }
    }
}

impl std::error::Error for ScriptError {}

impl From<Error> for ScriptError {
    fn from(err: Error) -> ScriptError {
        ScriptError::Sdk(err)
    }
}

/// Real planner context for offline tests: static world plus tables from
/// the repo dumps (same layout as the nav golden tests). Offline walks
/// then plan for real and fail at the dead socket, like live walks with
/// no connection would.
#[cfg(test)]
pub(crate) fn wire_nav(bot: &mut Bot) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
    let data = root.join("LifeScape/gamedata/data");
    let gametypes = std::fs::read_to_string(data.join("gametypes.json")).expect("gametypes");
    let overrides = std::fs::read_to_string(data.join("nav_overrides.json")).expect("overrides");
    let defs = lifescape_nav::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
    let world = lifescape_nav::static_world::StaticWorld::open(&zip, defs).expect("static world");
    let zones = lifescape_nav::f2p::load_zones(&root.join("lostcity/content/maps/free2play.csv")).expect("f2p");
    world.set_f2p(zones);
    let tables = lifescape_nav::tables::Tables::load(&data).expect("tables");
    bot.nav_world = Some(world);
    bot.nav_tables = Some(std::sync::Arc::new(tables));
}

// -- real scripts --------------------------------------------------------

struct Hello;
impl Script for Hello {
    fn name(&self) -> &'static str {
        "hello"
    }
    fn run(&self, bot: &mut Bot, _args: &[String]) -> ScriptResult {
        bot.intend("saying hello");
        bot.say("hello lifescape")?;
        bot.idle()?;
        for frame in bot.pump(25)? {
            if frame.name == "MESSAGE_GAME" {
                bot.log(&format!("game message: {}", String::from_utf8_lossy(&frame.body)));
            } else {
                bot.log(&format!("recv {} ({}) len={}", frame.name, frame.opcode, frame.body.len()));
            }
        }
        Ok(0)
    }
}

fn parse_target(args: &[String]) -> Result<(i32, i32, i32), ScriptError> {
    if args.len() < 2 {
        return Err(ScriptError::Usage("goto|walkto <x> <z> [level]".to_string()));
    }
    let parse = |slot: usize| args.get(slot).and_then(|text| text.parse::<i32>().ok()).ok_or_else(|| ScriptError::Usage("goto|walkto <x> <z> [level]".to_string()));
    Ok((parse(0)?, parse(1)?, args.get(2).and_then(|text| text.parse::<i32>().ok()).unwrap_or(0)))
}

struct Goto;
impl Script for Goto {
    fn name(&self) -> &'static str {
        "goto"
    }
    fn run(&self, bot: &mut Bot, args: &[String]) -> ScriptResult {
        let (x, z, level) = parse_target(args)?;
        bot.log(&format!("goto ({x},{z},{level}) from {:?}", bot.position()));
        bot.travel_to(lifescape_sdk::plan::Tile { x, z, level })?;
        bot.log(&format!("arrived {:?}", bot.position()));
        Ok(0)
    }
}

struct Walkto;
impl Script for Walkto {
    fn name(&self) -> &'static str {
        "walkto"
    }
    fn run(&self, bot: &mut Bot, args: &[String]) -> ScriptResult {
        let (x, z, level) = parse_target(args)?;
        bot.log(&format!("from {:?} to {x},{z},{level}", bot.position()));
        bot.walk_to(x, z, level, 9000)?;
        bot.log(&format!("arrived {:?} (target {x},{z})", bot.position()));
        Ok(0)
    }
}

struct DebugWalk;
impl Script for DebugWalk {
    fn name(&self) -> &'static str {
        "debug_walk"
    }
    fn run(&self, bot: &mut Bot, args: &[String]) -> ScriptResult {
        let (x, z, level) = parse_target(args)?;
        for line in bot.debug_walk(x, z, level).lines {
            bot.log(&format!("{line}"));
        }
        Ok(0)
    }
}

// -- registry ---------------------------------------------------------------

/// Every launchable script name.
pub fn names() -> Vec<&'static str> {
    vec![
        "fishing", "mining", "woodcut", "melee", "tutorial",
        "quest_black_knights_fortress", "quest_cooks_assistant", "quest_demon_slayer",
        "quest_dorics_quest", "quest_dragon_slayer", "quest_ernest_the_chicken",
        "quest_goblin_diplomacy", "quest_imp_catcher", "quest_knights_sword",
        "quest_pirates_treasure", "quest_prince_ali_rescue", "quest_restless_ghost",
        "quest_romeo_and_juliet", "quest_rune_mysteries", "quest_sheep_shearer",
        "quest_shield_of_arrav", "quest_vampire_slayer", "quest_witchs_potion",
        "rescue_maze", "rescue_mime", "rescue_cube", "rescue_down", "rescue_greet", "rescue_fallback",
        "check_transitions", "check_doors", "check_routes", "goto", "walkto", "walktour",
        "bank_deposit", "drop", "debug_walk", "trade_swap", "quest_reset", "reset_quests",
        "give", "hello", "probe_transitions",
    ]
}

// Dispatch table: every name in `names()` resolves here, unknown names do
// not. Wrapping keeps one match: the table below is the dispatch.

type Runner = fn(&mut Bot, &[String]) -> ScriptResult;

fn runner(name: &str) -> Option<Runner> {
    match name {
        "hello" => Some(|bot, args| Hello.run(bot, args)),
        "goto" => Some(|bot, args| Goto.run(bot, args)),
        "walkto" => Some(|bot, args| Walkto.run(bot, args)),
        "debug_walk" => Some(|bot, args| DebugWalk.run(bot, args)),
        "fishing" => Some(|bot, args| skill_fishing::run_fishing(bot, args)),
        "mining" => Some(|bot, args| skill_mining::run_mining(bot, args)),
        "woodcut" => Some(|bot, args| skill_woodcutting::run_woodcut(bot, args)),
        "melee" => Some(|bot, args| skill_combat::run_melee(bot, args)),
        "tutorial" => Some(|bot, args| tutorial::run_tutorial(bot, args)),
        "quest_black_knights_fortress" => Some(|bot, args| quests_b::run_quest("blackknight", bot, args)),
        "quest_cooks_assistant" => Some(|bot, args| quests_a::run_quest("cook", bot, args)),
        "quest_demon_slayer" => Some(|bot, args| quests_b::run_quest("demon", bot, args)),
        "quest_dorics_quest" => Some(|bot, args| quests_a::run_quest("doric", bot, args)),
        "quest_dragon_slayer" => Some(|bot, args| quests_b::run_quest("dragon", bot, args)),
        "quest_ernest_the_chicken" => Some(|bot, args| quests_a::run_quest("haunted", bot, args)),
        "quest_goblin_diplomacy" => Some(|bot, args| quests_a::run_quest("gobdip", bot, args)),
        "quest_imp_catcher" => Some(|bot, args| quests_a::run_quest("imp", bot, args)),
        "quest_knights_sword" => Some(|bot, args| quests_b::run_quest("squire", bot, args)),
        "quest_pirates_treasure" => Some(|bot, args| quests_b::run_quest("hunt", bot, args)),
        "quest_prince_ali_rescue" => Some(|bot, args| quests_b::run_quest("prince", bot, args)),
        "quest_restless_ghost" => Some(|bot, args| quests_b::run_quest("priest", bot, args)),
        "quest_romeo_and_juliet" => Some(|bot, args| quests_a::run_quest("romeojuliet", bot, args)),
        "quest_rune_mysteries" => Some(|bot, args| quests_a::run_quest("runemysteries", bot, args)),
        "quest_sheep_shearer" => Some(|bot, args| quests_a::run_quest("sheep", bot, args)),
        "quest_shield_of_arrav" => Some(|bot, args| quests_b::run_quest("blackarmgang", bot, args)),
        "quest_vampire_slayer" => Some(|bot, args| quests_b::run_quest("vampire", bot, args)),
        "quest_witchs_potion" => Some(|bot, args| quests_a::run_quest("hetty", bot, args)),
        "rescue_maze" => Some(|bot, args| rescues::run_rescue("maze", bot, args)),
        "rescue_mime" => Some(|bot, args| rescues::run_rescue("mime", bot, args)),
        "rescue_cube" => Some(|bot, args| rescues::run_rescue("cube", bot, args)),
        "rescue_down" => Some(|bot, args| rescues::run_rescue("down", bot, args)),
        "rescue_greet" => Some(|bot, args| rescues::run_rescue("greet", bot, args)),
        "rescue_fallback" => Some(|bot, args| rescues::run_rescue("fallback", bot, args)),
        "check_transitions" => Some(|bot, args| devtools::run_devtool("check_transitions", bot, args)),
        "probe_transitions" => Some(|bot, args| devtools::run_devtool("probe_transitions", bot, args)),
        "check_doors" => Some(|bot, args| devtools::run_devtool("check_doors", bot, args)),
        "check_routes" => Some(|bot, args| devtools::run_devtool("check_routes", bot, args)),
        "walktour" => Some(|bot, args| devtools::run_devtool("walktour", bot, args)),
        "bank_deposit" => Some(|bot, args| devtools::run_devtool("bank_deposit", bot, args)),
        "drop" => Some(|bot, args| devtools::run_devtool("drop", bot, args)),
        "trade_swap" => Some(|bot, args| devtools::run_devtool("trade_swap", bot, args)),
        "quest_reset" => Some(|bot, args| devtools::run_devtool("quest_reset", bot, args)),
        "reset_quests" => Some(|bot, args| devtools::run_devtool("reset_quests", bot, args)),
        "give" => Some(|bot, args| devtools::run_devtool("give", bot, args)),
        _ => None,
    }
}

/// Run one script by registry name. Unknown names report errors;
/// nothing falls back silently.
pub fn run(name: &str, bot: &mut Bot, args: &[String]) -> ScriptResult {
    match runner(name) {
        Some(execute) => execute(bot, args),
        None => Err(ScriptError::Usage(format!("unknown script {name:?}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::names;

    #[test]
    fn registry_covers_legacy_scripts() {
        // all supervised entry points, checked against names().
        for name in [
            "fishing", "mining", "woodcut", "melee", "tutorial",
            "quest_cooks_assistant", "quest_dragon_slayer", "rescue_maze",
            "rescue_mime", "rescue_cube", "rescue_down", "rescue_greet", "rescue_fallback",
            "check_transitions", "check_doors", "check_routes", "goto",
            "walkto", "walktour", "bank_deposit", "drop", "debug_walk",
            "trade_swap", "quest_reset", "reset_quests", "give", "hello",
            "probe_transitions",
        ] {
            assert!(names().contains(&name), "registry misses {name}");
        }
        assert_eq!(names().len(), 44);
    }

    #[test]
    fn every_name_dispatches() {
        for name in names() {
            assert!(super::runner(name).is_some(), "registry name {name} has no runner");
        }
        assert!(super::runner("nope").is_none(), "unknown names must not dispatch");
    }
}
