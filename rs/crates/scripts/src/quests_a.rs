//! F2P quests, batch A: cook, doric, gobdip, imp, sheep, hetty,
//! romeojuliet, runemysteries, haunted. Each `solve_*` mirrors its
//! `scripts/quests/*.py` counterpart step for step.

use lifescape_gamedata::quest_requirements;
use lifescape_sdk::Bot;

use super::quest_common as solver;
use crate::{ScriptError, ScriptResult};

fn solve_cook(bot: &mut Bot) -> Result<(), ScriptError> {
    bot.intend("quest cook: helping the cook");
    solver::talk_like(bot, "cook", &["what's wrong", "help you"], None, 400)?;
    solver::talk_plain(bot, "cook", &[], None, 400)
}

fn solve_doric(bot: &mut Bot) -> Result<(), ScriptError> {
    bot.intend("quest doric: helping doric");
    solver::talk_plain(bot, "doric", &["I wanted to use your anvils.", "Yes, I will get you materials."], None, 400)?;
    solver::talk_plain(bot, "doric", &[], None, 400)
}

fn solve_gobdip(bot: &mut Bot) -> Result<(), ScriptError> {
    bot.intend("quest gobdip: goblin diplomacy");
    solver::talk_plain(bot, "rustyanchor_bartender", &["Not very busy in here today, is it?"], None, 400)?;
    solver::talk_plain(bot, "general_wartface", &["Do you want me to pick an armour colour for you?"], None, 400)?;
    solver::talk_plain(bot, "general_wartface", &[], None, 400)?;
    solver::talk_plain(bot, "general_wartface", &[], None, 400)?;
    solver::talk_plain(bot, "general_wartface", &[], None, 400)
}

fn solve_imp(bot: &mut Bot) -> Result<(), ScriptError> {
    bot.intend("quest imp: imp catcher");
    solver::talk_plain(bot, "wizard_mizgog", &["Give me a quest!", "Give me a quest please."], None, 400)?;
    solver::talk_plain(bot, "wizard_mizgog", &[], None, 400)
}

fn solve_sheep(bot: &mut Bot) -> Result<(), ScriptError> {
    bot.intend("quest sheep: sheep shearer");
    solver::talk_plain(bot, "fred_the_farmer", &["I'm looking for a quest.", "Yes okay. I can do that."], None, 400)?;
    solver::talk_plain(bot, "fred_the_farmer", &[], None, 400)
}

fn solve_hetty(bot: &mut Bot) -> Result<(), ScriptError> {
    bot.intend("quest hetty: witch's potion");
    solver::talk_plain(bot, "hetty", &["I am in search of a quest.", "Yes help me become one with my darker side."], None, 400)?;
    solver::talk_plain(bot, "hetty", &[], None, 400)?;
    solver::operate_loc(bot, &["hettycauldron"], 1, None)
}

fn solve_romeojuliet(bot: &mut Bot) -> Result<(), ScriptError> {
    bot.intend("quest romeojuliet: carrying messages");
    solver::talk_like(bot, "romeo", &["looking for a quest", "quest", "help", "yes"], None, 400)?;
    solver::talk_like(bot, "juliet", &["romeo", "message", "yes"], None, 400)?;
    solver::talk_like(bot, "romeo", &["juliet", "message", "sad"], None, 400)?;
    solver::talk_like(bot, "father_lawrence", &["romeo", "juliet", "help", "potion"], None, 400)?;
    solver::talk_like(bot, "apothecary", &["cadava", "berries", "potion"], None, 400)?;
    solver::talk_like(bot, "apothecary", &["cadava", "potion", "berries"], None, 400)?;
    solver::talk_like(bot, "juliet", &["potion", "plan", "yes"], None, 400)?;
    solver::talk_like(bot, "romeo", &["juliet", "plan", "crypt"], None, 400)?;
    if !solver::wait_complete(bot, 300)? {
        return Err(lifescape_sdk::Error::Timeout(format!("romeo & juliet never completed at {:?}", bot.position())).into());
    }
    Ok(())
}

fn solve_runemysteries(bot: &mut Bot) -> Result<(), ScriptError> {
    bot.intend("quest runemysteries: delivering research");
    // Duke re-talk is pure cost once started; stage-gate on the live varp.
    if solver::varp(bot, "runemysteries") < 1 {
        solver::talk_like(bot, "duke_of_lumbridge", &["talisman", "help", "runes", "yes"], None, 400)?;
    }
    if solver::varp(bot, "runemysteries") < 3 {
        solver::talk_like(bot, "head_wizard", &["looking for the head wizard", "here you are", "certainly", "yes"], None, 400)?;
    }
    if solver::varp(bot, "runemysteries") < 4 {
        solver::talk_like(bot, "aubury", &["package", "research", "head_wizard", "notes"], None, 400)?;
    }
    if solver::varp(bot, "runemysteries") < 5 {
        solver::talk_like(bot, "aubury", &["notes", "head_wizard", "finished"], None, 400)?;
    } else if solver::count(bot, solver::obj_id("research_notes")?) == 0 {
        // Notes lost to a full pack: Aubury re-issues them on re-talk.
        solver::talk_like(bot, "aubury", &["notes", "head_wizard", "finished"], None, 400)?;
    }
    solver::use_item_on_npc(bot, "research_notes", "head_wizard", Some((3103, 9571)))?;
    if !solver::wait_complete(bot, 300)? {
        return Err(lifescape_sdk::Error::Timeout(format!("rune mysteries never completed at {:?}", bot.position())).into());
    }
    Ok(())
}

fn solve_haunted(bot: &mut Bot) -> Result<(), ScriptError> {
    bot.intend("quest haunted: ernest the chicken");
    let stage = solver::varp(bot, "haunted");
    if stage < 1 {
        solver::talk_like(bot, "veronica", &["quest", "help", "ernest"], None, 400)?;
    }
    if stage < 2 {
        solver::talk_like(bot, "professor_oddenstein", &["ernest", "looking", "chicken", "change", "back"], None, 400)?;
    }
    // Only the pressure gauge needs work in-world: poison the fountain's
    // piranhas with banked fish food + poison, then take the gauge.
    let gauge = solver::obj_id("pressure_gauge")?;
    if solver::count(bot, gauge) == 0 {
        let poisoned = solver::obj_id("poisoned_fish_food")?;
        if solver::count(bot, poisoned) == 0 {
            let food = solver::obj_id("fish_food")?;
            let poison = solver::obj_id("poison")?;
            if solver::count(bot, food) == 0 || solver::count(bot, poison) == 0 {
                return Err(lifescape_sdk::Error::State(format!("need fish food and poison for the fountain at {:?}", bot.position())).into());
            }
            let (com, slot) = solver::inv_slot(bot, food)?;
            let (use_com, use_slot) = solver::inv_slot(bot, poison)?;
            bot.op_held_u(food, slot, com, poison, use_slot, use_com)?;
            lifescape_sdk::waits::wait_ticks(bot, 10)?;
        }
        solver::go_to_npc(bot, "veronica", None)?;
        solver::use_item_on_loc(bot, "poisoned_fish_food", &["hauntedfountain"])?;
        solver::operate_loc(bot, &["hauntedfountain"], 1, None)?;
    }
    solver::talk_like(bot, "professor_oddenstein", &["found", "everything", "gauge", "tube", "oil"], None, 400)?;
    if !solver::wait_complete(bot, 300)? {
        return Err(lifescape_sdk::Error::Timeout(format!("ernest never completed at {:?}", bot.position())).into());
    }
    Ok(())
}

/// Run one batch-A quest by key (`cook`, `doric`, `gobdip`, `imp`,
/// `sheep`, `hetty`, `romeojuliet`, `runemysteries`, `haunted`).
pub fn run_quest(key: &str, bot: &mut Bot, _args: &[String]) -> ScriptResult {
    let Some(requirements) = quest_requirements::by_key(key) else {
        return Err(ScriptError::Usage(format!("unknown batch-A quest {key:?}")));
    };
    match key {
        "cook" => solver::run_requirements(bot, &requirements, solve_cook),
        "doric" => solver::run_requirements(bot, &requirements, solve_doric),
        "gobdip" => solver::run_requirements(bot, &requirements, solve_gobdip),
        "imp" => solver::run_requirements(bot, &requirements, solve_imp),
        "sheep" => solver::run_requirements(bot, &requirements, solve_sheep),
        "hetty" => solver::run_requirements(bot, &requirements, solve_hetty),
        "romeojuliet" => solver::run_requirements(bot, &requirements, solve_romeojuliet),
        "runemysteries" => solver::run_requirements(bot, &requirements, solve_runemysteries),
        "haunted" => solver::run_requirements(bot, &requirements, solve_haunted),
        _ => Err(ScriptError::Usage(format!("quest {key:?} lives in batch B"))),
    }
}
