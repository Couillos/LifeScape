//! Random-event rescues: maze, mime, cube, down, greet, fallback, plus the host
//! greeting (mage, genie, dwarf talk paths ask nothing: approach, talk,
//! take the reward instead of the fail-teleport or, ignored, the dwarf's
//! stones).
//!
//! Each rescue returns 0 when no random event is pending afterwards (and
//! reports the handoff otherwise), 1 when the bound is exhausted.

use lifescape_sdk::{Bot, Error};
use lifescape_sdk::waits::wait_ticks;

use super::quest_common as kit;
use super::randomevent;
use super::randomevent_full::climb_out;
use crate::{ScriptError, ScriptResult};

fn verify(bot: &Bot, what: &str) -> ScriptResult {
    match randomevent::event_at(bot) {
        None => {
            bot.log(&format!("{what} rescue done, free at {:?}", bot.position()));
            Ok(0)
        }
        Some(event) => {
            bot.log(&randomevent::event_line(bot, event, false));
            bot.log(&format!("{what} rescue handed off at {:?}", bot.position()));
            Ok(1)
        }
    }
}

fn rescue_maze(bot: &mut Bot) -> ScriptResult {
    bot.pump(10)?;
    bot.intend("rescue: maze");
    bot.log(&format!("start {:?}", bot.position()));
    if !randomevent::in_maze(bot) {
        bot.log("not in maze, nothing to do");
        return verify(bot, "maze");
    }
    if super::maze::solve(bot)? {
        return verify(bot, "maze");
    }
    // No maze timeout to wait out (verified live: the server does not
    // release an unsolved maze). Fail loud immediately with position.
    if randomevent::in_maze(bot) {
        return Err(ScriptError::Usage(format!(
            "maze stuck at {:?}: centre unreachable",
            bot.position()
        )));
    }
    verify(bot, "maze")
}

const MIME_MODAL_INTERFACE: u16 = 6543;
const EMOTE_COM_BASE: u16 = 6546;

fn seq_to_emote(seq: u16) -> Option<u16> {
    // seq.pack ids for the emote interface.
    match seq {
        860 => Some(0), // emote_cry
        857 => Some(1), // emote_think
        861 => Some(2), // emote_laugh
        866 => Some(3), // emote_dance
        1130 => Some(4), // emote_climbing_rope
        1129 => Some(5), // emote_mime_lean
        1128 => Some(6), // emote_glass_wall
        1131 => Some(7), // emote_glass_box
        _ => None,
    }
}

fn mime_anim(bot: &Bot) -> Option<u16> {
    let slot = bot.find_npc("macro_mime")?;
    bot.world.npcs.get(&slot).and_then(|npc| npc.anim)
}

fn rescue_mime(bot: &mut Bot) -> ScriptResult {
    bot.pump(10)?;
    bot.intend("rescue: mime");
    bot.log(&format!("start {:?}", bot.position()));
    if !randomevent::in_mime(bot) {
        bot.log("not in mime, nothing to do");
        return verify(bot, "mime");
    }
    // Every cycle the mime plays one of 8 emotes (NPC anim mask), then the
    // emotes interface opens: copy the captured emote, 4 correct in a row
    // teleports back. A wrong click only resets progress.
    let mut last_anim = mime_anim(bot);
    let mut captured: Option<u16> = None;
    let deadline = bot.world.ticks + 2000;
    while randomevent::in_mime(bot) && bot.world.ticks < deadline {
        bot.pump(1)?;
        let anim = mime_anim(bot);
        if anim != last_anim {
            last_anim = anim;
            if let Some(seq) = anim {
                if seq_to_emote(seq).is_some() {
                    captured = Some(seq);
                }
            }
        }
        let open = bot.world.modal.as_ref().is_some_and(|modal| modal.layer == "chat" && modal.interface_id == MIME_MODAL_INTERFACE);
        if open {
            if let Some(seq) = captured {
                if let Some(emote) = seq_to_emote(seq) {
                    bot.log(&format!("copying emote {emote} (seq {seq})"));
                    bot.if_button(EMOTE_COM_BASE + emote)?;
                    captured = None;
                    bot.pump(2)?;
                }
            }
        }
    }
    if randomevent::in_mime(bot) {
        bot.log("mime still active");
        return Ok(1);
    }
    bot.log(&format!("mime solved, back at {:?}", bot.position()));
    verify(bot, "mime")
}

const CUBE_SHAPES: [&str; 5] = ["Triangle", "Square", "Circle", "Star", "Halfmoon"];
const CUBE_COLOURS: [&str; 3] = ["Red", "Blue", "Yellow"];

fn cube_shape_colour(obj_id: u16) -> Option<(usize, usize)> {
    if (3063..=3091).contains(&obj_id) && (obj_id - 3063) % 2 == 0 {
        let slot = (obj_id - 3063) / 2;
        return Some(((slot / 3) as usize, (slot % 3) as usize));
    }
    None
}

fn cube_answer(question: &str, shown: &[(usize, usize)]) -> Option<usize> {
    let asked = question.trim().trim_end_matches('?');
    let lower = asked.to_lowercase();
    if let Some(want) = lower.strip_prefix("what colour is the ") {
        let want = want.trim();
        return shown.iter().position(|(shape, _)| CUBE_SHAPES[*shape].to_lowercase() == want);
    }
    if let Some(want) = lower.strip_prefix("which shape is ") {
        let want = want.trim();
        return shown.iter().position(|(_, colour)| CUBE_COLOURS[*colour].to_lowercase() == want);
    }
    None
}

fn cube_com(name: &str) -> Result<u16, ScriptError> {
    lifescape_gamedata::com_id(name).ok_or_else(|| ScriptError::Sdk(Error::State(format!("{name} missing from the component table"))))
}

fn solve_strange_box(bot: &mut Bot, max_rounds: u32) -> Result<bool, ScriptError> {
    let cube = kit::obj_id("macro_cube")?;
    let question_com = cube_com("macro_cube:question")?;
    let button_coms = [cube_com("macro_cube:answer_button1")?, cube_com("macro_cube:answer_button2")?, cube_com("macro_cube:answer_button3")?];
    let model_coms = [cube_com("macro_cube:center_shape_model")?, cube_com("macro_cube:side_shape_model")?, cube_com("macro_cube:top_shape_model")?];
    for _ in 0..max_rounds {
        if bot.inventory().get(&cube).copied().unwrap_or(0) == 0 {
            return Ok(true);
        }
        // The cube may sit in a container the backpack view has not picked
        // yet (server splits stacks every 16 ticks): pump and retry, never
        // report success on a lookup miss.
        let Ok((inv_com, slot)) = kit::inv_slot(bot, cube) else {
            bot.pump(2)?;
            continue;
        };
        bot.op_held(1, cube, slot, inv_com)?;
        // Quiz UI lands in a tick or two; the replicator adds one every
        // 150, so poll tight instead of sitting out a 60-tick wait.
        let target = bot.world.ticks + 12;
        let mut question: Option<String> = None;
        let mut models: Vec<Option<u16>> = vec![None; 3];
        while bot.world.ticks < target {
            bot.pump(1)?;
            let mut texts = bot.chat_texts();
            texts.sort();
            question = texts.into_iter().find(|(com, _)| *com == question_com).map(|(_, text)| text);
            models = model_coms.iter().map(|com| bot.world.modal_models.get(com).copied()).collect();
            if question.is_some() && models.iter().all(|model| model.is_some()) {
                break;
            }
        }
        let (Some(asked), [Some(first), Some(second), Some(third)]) = (question.clone(), [models[0], models[1], models[2]]) else {
            bot.log(&format!("cube quiz waiting: question={question:?} models={models:?}"));
            continue;
        };
        let shown: Vec<(usize, usize)> = [first, second, third].iter().filter_map(|held| cube_shape_colour(*held)).collect();
        if shown.len() != 3 {
            continue;
        }
        let Some(pick) = cube_answer(&asked, &shown) else {
            bot.log(&format!("cube quiz unparsed: {:?}", &asked[..asked.len().min(60)]));
            continue;
        };
        bot.log(&format!("cube quiz: {:?} -> button {}", &asked[..asked.len().min(60)], pick + 1));
        bot.if_button(button_coms[pick])?;
        bot.pump(2)?;
        for _ in 0..6 {
            if bot.world.modal.is_none() {
                break;
            }
            bot.chat_continue()?;
            bot.pump(1)?;
        }
    }
    Ok(bot.inventory().get(&cube).copied().unwrap_or(0) == 0)
}

fn rescue_cube(bot: &mut Bot) -> ScriptResult {
    bot.intend("rescue: cube");
    // The server replicates the cube every 16 ticks while any remain, so
    // solve_strange_box keeps going until the pack is clear; the round cap
    // only guards against a quiz the bot cannot parse.
    let solved = solve_strange_box(bot, 400)?;
    let cube = kit::obj_id("macro_cube")?;
    let left = bot.inventory().get(&cube).copied().unwrap_or(0);
    if solved && left == 0 {
        bot.log("cube: cleared");
        Ok(0)
    } else {
        bot.log(&format!("cube: {}, {left} left", if solved { "solved" } else { "unfinished" }));
        Ok(1)
    }
}

fn rescue_down(bot: &mut Bot) -> ScriptResult {
    bot.pump(10)?;
    bot.intend("rescue: tower");
    bot.log(&format!("start {:?}", bot.position()));
    if !climb_out(bot)? {
        return Ok(1);
    }
    // climb_out succeeds on level 0 without moving, but the fail spot is
    // positional: standing on it re-reports the event forever. Step off.
    if randomevent::in_fail_teleport(bot) {
        let (level, here_x, here_z) = bot.position();
        let mut off = false;
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
            let (next_x, next_z) = (here_x + dx, here_z + dz);
            if randomevent::FAIL_TELEPORTS.contains(&(level, next_x, next_z)) {
                continue;
            }
            // Dead socket, not a bad step: typed instead of pacing steps.
            match bot.step_to(next_x, next_z, 30) {
                Err(error)
                    if matches!(
                        error,
                        lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)
                    ) =>
                {
                    return Err(error.into())
                }
                Err(_) | Ok(false) => continue,
                Ok(true) => {}
            }
            if !randomevent::in_fail_teleport(bot) {
                off = true;
                break;
            }
        }
        if !off {
            bot.log(&format!("tower: stuck on the fail spot at {:?}", bot.position()));
            return Ok(1);
        }
    }
    verify(bot, "tower")
}

fn rescue_fallback(bot: &mut Bot) -> ScriptResult {
    bot.intend("rescue: fallback");
    if bot.relogin().is_err() {
        bot.log("fallback relogin failed");
        return Ok(1);
    }
    wait_ticks(bot, 5)?;
    // Bounded pump loop: the shared event check (greet/survive/climb)
    // lives in randomevent_full; here just wait the zone out.
    let target = bot.world.ticks + 1500;
    while bot.world.ticks < target {
        bot.pump(5)?;
        if randomevent::event_at(bot).is_none() && bot.world.level == 0 {
            bot.log(&format!("fallback clear at {:?}", bot.position()));
            return Ok(0);
        }
    }
    bot.log(&format!("fallback exhausted at {:?}", bot.position()));
    Ok(1)
}

/// Greet every visible event host (approach + talk + reward dialog),
/// then wait bounded for the goodbye despawn. Rounds are short on
/// purpose: one long approach would wedge the worker past every timeout
/// above it, and the observer re-poses anyway. The idle observer poses
/// this for hosts the adjacent-only `greet_near` cannot reach (diagonal,
/// two tiles out); running scripts keep their own `greet()` fast path.
/// Returns 0 when no host is left staring, 1 when one lingers.
fn rescue_greet(bot: &mut Bot) -> ScriptResult {
    bot.intend("rescue: greet");
    for _ in 0..6 {
        super::randomevent_full::greet(bot, 150)?;
        let target = bot.world.ticks + 30;
        while bot.world.ticks < target
            && super::randomevent_full::EVENT_HOSTS.iter().any(|name| bot.find_npc(name).is_some())
        {
            bot.pump(3)?;
        }
        if super::randomevent_full::EVENT_HOSTS
            .iter()
            .all(|name| bot.find_npc(name).is_none())
        {
            bot.log(&format!("greet rescue done, free at {:?}", bot.position()));
            return Ok(0);
        }
    }
    bot.log(&format!("greet rescue handed off at {:?}", bot.position()));
    Ok(1)
}

/// Run one rescue by key (`maze`, `mime`, `cube`, `down`, `greet`, `fallback`).
pub fn run_rescue(key: &str, bot: &mut Bot, _args: &[String]) -> ScriptResult {
    match key {
        "maze" => rescue_maze(bot),
        "mime" => rescue_mime(bot),
        "cube" => rescue_cube(bot),
        "down" => rescue_down(bot),
        "greet" => rescue_greet(bot),
        "fallback" => rescue_fallback(bot),
        _ => Err(ScriptError::Usage(format!("unknown rescue {key:?} (want maze|mime|cube|down|greet|fallback)"))),
    }
}
