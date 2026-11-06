//! Supervised bot scripts: one trait, one compiled registry.
//!
//! Names match the legacy Python registry exactly. Every entry runs for
//! real through the SDK; unknown names report errors, never silent stubs.

pub mod devtools;
pub mod devtools_check;
pub mod maze;
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

/// Script exit: Ok(code) like the Python `run` return, Err for failures.
pub type ScriptResult = Result<i32, ScriptError>;

#[derive(Debug)]
pub enum ScriptError {
    Sdk(Error),
    /// Logic not ported yet from the Python script of the same name.
    NotPorted(&'static str),
    Usage(String),
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScriptError::Sdk(err) => write!(formatter, "sdk: {err}"),
            ScriptError::NotPorted(name) => write!(formatter, "{name}: not yet ported"),
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

/// Placeholder until the Python logic of the same name is ported.
pub struct StubScript(pub &'static str);

impl Script for StubScript {
    fn name(&self) -> &'static str {
        self.0
    }
    fn run(&self, bot: &mut Bot, _args: &[String]) -> ScriptResult {
        bot.log(&format!("{}: not yet ported", self.0));
        Err(ScriptError::NotPorted(self.0))
    }
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
        bot.intend(&format!("teleport to {x},{z},{level}"));
        bot.cheat(&format!("tele {},{},{},{},{}", level, x >> 6, z >> 6, x & 63, z & 63))?;
        let target = bot.world.ticks + 100;
        while bot.world.ticks < target {
            bot.pump(2)?;
            if bot.position() == (level, x, z) {
                bot.log(&format!("at {:?}", bot.position()));
                return Ok(0);
            }
        }
        bot.log(&format!("teleport did not land: {:?}", bot.position()));
        Ok(1)
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
// Stub entries share StubScript; only the four real scripts above have
// their own types.

/// Every launchable script name, same set as the Python registry.
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
        "give", "hello",
    ]
}

/// Run one script by registry name. Unknown names report errors;
/// nothing falls back silently.
pub fn run(name: &str, bot: &mut Bot, args: &[String]) -> ScriptResult {
    match name {
        "hello" => Hello.run(bot, args),
        "goto" => Goto.run(bot, args),
        "walkto" => Walkto.run(bot, args),
        "debug_walk" => DebugWalk.run(bot, args),
        "fishing" => skill_fishing::run_fishing(bot, args),
        "mining" => skill_mining::run_mining(bot, args),
        "woodcut" => skill_woodcutting::run_woodcut(bot, args),
        "melee" => skill_combat::run_melee(bot, args),
        "tutorial" => tutorial::run_tutorial(bot, args),
        "quest_black_knights_fortress" => quests_b::run_quest("blackknight", bot, args),
        "quest_cooks_assistant" => quests_a::run_quest("cook", bot, args),
        "quest_demon_slayer" => quests_b::run_quest("demon", bot, args),
        "quest_dorics_quest" => quests_a::run_quest("doric", bot, args),
        "quest_dragon_slayer" => quests_b::run_quest("dragon", bot, args),
        "quest_ernest_the_chicken" => quests_a::run_quest("haunted", bot, args),
        "quest_goblin_diplomacy" => quests_a::run_quest("gobdip", bot, args),
        "quest_imp_catcher" => quests_a::run_quest("imp", bot, args),
        "quest_knights_sword" => quests_b::run_quest("squire", bot, args),
        "quest_pirates_treasure" => quests_b::run_quest("hunt", bot, args),
        "quest_prince_ali_rescue" => quests_b::run_quest("prince", bot, args),
        "quest_restless_ghost" => quests_b::run_quest("priest", bot, args),
        "quest_romeo_and_juliet" => quests_a::run_quest("romeojuliet", bot, args),
        "quest_rune_mysteries" => quests_a::run_quest("runemysteries", bot, args),
        "quest_sheep_shearer" => quests_a::run_quest("sheep", bot, args),
        "quest_shield_of_arrav" => quests_b::run_quest("blackarmgang", bot, args),
        "quest_vampire_slayer" => quests_b::run_quest("vampire", bot, args),
        "quest_witchs_potion" => quests_a::run_quest("hetty", bot, args),
        "rescue_maze" => rescues::run_rescue("maze", bot, args),
        "rescue_mime" => rescues::run_rescue("mime", bot, args),
        "rescue_cube" => rescues::run_rescue("cube", bot, args),
        "rescue_down" => rescues::run_rescue("down", bot, args),
        "rescue_greet" => rescues::run_rescue("greet", bot, args),
        "rescue_fallback" => rescues::run_rescue("fallback", bot, args),
        "check_transitions" | "check_doors" | "check_routes" | "walktour"
        | "bank_deposit" | "drop" | "trade_swap" | "quest_reset"
        | "reset_quests" | "give" => devtools::run_devtool(name, bot, args),
        _ => {
            if let Some(known) = names().into_iter().find(|known| *known == name) {
                StubScript(known).run(bot, args)
            } else {
                Err(ScriptError::Usage(format!("unknown script {name:?}")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::names;

    #[test]
    fn registry_covers_legacy_scripts() {
        // orchestration/registry.py::SCRIPTS plus hello.
        for name in [
            "fishing", "mining", "woodcut", "melee", "tutorial",
            "quest_cooks_assistant", "quest_dragon_slayer", "rescue_maze",
            "rescue_mime", "rescue_cube", "rescue_down", "rescue_greet", "rescue_fallback",
            "check_transitions", "check_doors", "check_routes", "goto",
            "walkto", "walktour", "bank_deposit", "drop", "debug_walk",
            "trade_swap", "quest_reset", "reset_quests", "give", "hello",
        ] {
            assert!(names().contains(&name), "registry misses {name}");
        }
        assert_eq!(names().len(), 43);
    }
}
