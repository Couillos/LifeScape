//! Produce: use-A-on-B transformation core (cook on a range, smelt in a
//! furnace, smith on an anvil...). One brick for every family: the
//! activity declares the site (station locs in preference order, or an
//! NPC), the interface adapter and the input; produce walks there,
//! answers the interface and waits out the consumption of the inputs.
//! Only SDK sends; produce never touches the protocol itself.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::survival::death_resume;
use crate::skill_common::{check_events, death_message_seen, dismiss_modal, hitpoints, obj_label, pack_total, walk_checked, WorkOutcome};
use crate::skill_gather::classify_message;
use crate::ScriptError;

/// Where the transformation happens: station locs (range, furnace,
/// anvil...) in preference order — ranges before fires, they burn less —
/// or a crafting NPC (tanner...), first visible name wins.
pub enum ProduceSite<'a> {
    Loc(&'a [&'a str]),
    Npc(&'a [&'a str]),
}

/// Interface adapter, one per family (never per activity): cooking clicks
/// straight through, choice dialogues pick one option, counters answer
/// the total, grids press one interface button (full com name, e.g.
/// `"smelting:com_19"`; the product-to-button map stays caller-side until
/// a grid activity exists).
pub enum ProduceUi<'a> {
    Direct,
    Choice { pick: &'a str },
    Count,
    Grid { button: &'a str },
}

/// One produce declaration: what the activity owns, nothing else.
pub struct Produce<'a> {
    pub verb: &'static str,
    pub site: ProduceSite<'a>,
    pub ui: ProduceUi<'a>,
}

/// One produce order: the input, how many (0 = all carried), the watched
/// products (outputs plus byproducts like burnt fish) and the reference
/// tile the station is picked nearest to (the bank).
pub struct ProduceJob<'a> {
    pub spec: &'a Produce<'a>,
    pub input: u16,
    pub input_name: &'a str,
    pub want: u32,
    pub products: Vec<u16>,
    pub near: (i32, i32),
}

/// What one job consumed and made (burnt counts as made: it still proves
/// the input went through the skill).
pub struct ProduceBatch {
    pub consumed: u32,
    pub made: u32,
}

const STATION_RADIUS: i32 = 150;
const STAND_WALK_TICKS: u64 = 800;
const ITEM_TICKS: u64 = 40;
const UI_TICKS: u64 = 30;
const ITEM_RETRIES: u32 = 3;

/// Free-to-play cook recipe consuming `name` (case-insensitive on the
/// input). `None` when no F2P cook row eats it: fail loud, never guess.
pub fn cook_recipe(name: &str) -> Option<knowledge::recipes::Recipe> {
    knowledge::recipes::using(name, Some("cook")).into_iter().find(|entry| entry.members == Some(false))
}

/// Pick the chat option containing `pick` (case-insensitive). Pure so the
/// choice adapter stays testable without a connection.
pub fn match_option(options: &[(u16, String)], pick: &str) -> Option<u16> {
    let needle = pick.to_lowercase();
    options.iter().find(|(_, text)| text.to_lowercase().contains(&needle)).map(|(com, _)| *com)
}

/// Counter answer for `have` carried and `remaining` wanted (0 = all).
/// Pure so the counter adapter stays testable without a connection.
pub fn count_answer(have: u32, remaining: u32) -> i32 {
    if have == 0 {
        return 0;
    }
    if remaining == 0 || remaining > have {
        have as i32
    } else {
        remaining as i32
    }
}

/// Resolve a grid button to its interface com (full dump name, e.g.
/// `"smelting:com_19"`). Unknown names fail loud: pressing the wrong
/// button would smelt the wrong bar.
pub fn grid_button(bot: &Bot, name: &str) -> Result<u16, ScriptError> {
    (bot.tables.com_id)(name)
        .ok_or_else(|| ScriptError::Usage(format!("unknown grid button {name:?} at {:?}", bot.position())))
}

/// Lowercase debugname of a loc id, empty when the dump does not know it.
fn station_debugname(loc: u16) -> String {
    knowledge::loc_by_id(usize::from(loc)).and_then(|entry| entry.name).unwrap_or_default().to_lowercase()
}

/// Tiles with exactly this station debugname, same level, within the
/// station radius of `near`: live stream first, one static batch second
/// (the worker paints nothing). Nearest first; exact match only, so
/// orange furniture never reads as a cooking range.
fn site_tiles(bot: &mut Bot, name: &str, near: (i32, i32)) -> Vec<(i32, i32, i32, u16)> {
    let level = bot.world.level;
    let mut cands: Vec<(i64, (i32, i32, i32, u16))> = Vec::new();
    let mut consider = |x: i32, z: i32, tile_level: i32, loc: u16| {
        if tile_level != level || station_debugname(loc) != name {
            return;
        }
        let dist = i64::from((x - near.0).abs() + (z - near.1).abs());
        if dist <= i64::from(STATION_RADIUS) {
            cands.push((dist, (x, z, tile_level, loc)));
        }
    };
    for (tile, slot) in &bot.world.locs {
        if let Some((loc, _, _)) = slot {
            consider(tile.0, tile.1, tile.2, *loc);
        }
    }
    match bot.static_locs_near(near.0, near.1, level, STATION_RADIUS) {
        Ok(rows) => {
            for row in rows {
                consider(row.x, row.z, row.level, row.loc);
            }
        }
        Err(error) => {
            bot.log(&format!("station search: static locs unavailable ({error}), live only"));
        }
    }
    cands.sort();
    cands.dedup();
    cands.into_iter().map(|(_, tile)| tile).collect()
}

/// Answer a chat choice dialogue: pick the option, page through plain
/// text, done when the modal closes. False on timeout, loud on a pick
/// that matches nothing (a wrong declaration, not a retry).
fn answer_choice(bot: &mut Bot, pick: &str) -> Result<bool, ScriptError> {
    let target = bot.world.ticks + UI_TICKS;
    while bot.world.ticks < target {
        if !bot.chat_options().is_empty() {
            if match_option(&bot.chat_options(), pick).is_none() {
                return Err(ScriptError::Usage(format!("choice {pick:?} matches no option at {:?}", bot.position())));
            }
            bot.chat_choose(pick)?;
            bot.pump(5)?;
            return Ok(true);
        }
        if bot.world.modal.is_none() {
            return Ok(true);
        }
        bot.chat_continue()?;
        bot.pump(5)?;
    }
    Ok(false)
}

enum Drain {
    Consumed { units: u32 },
    Outcome(WorkOutcome),
    BadInput,
    InputGone,
    Timeout,
    PumpFailed,
}

/// Drain one produce click: wait for the input count to drop (the server
/// deletes the input when the attempt resolves). Refusals map through the
/// shared gather vocabulary, plus the produce-only lines.
fn drain_click(bot: &mut Bot, input: u16, had: u32) -> Drain {
    let seen = bot.world.messages.len();
    let target = bot.world.ticks + ITEM_TICKS;
    while bot.world.ticks < target {
        if bot.pump(2).is_err() {
            return Drain::PumpFailed;
        }
        if pack_total(bot, input) < had {
            return Drain::Consumed { units: had - pack_total(bot, input) };
        }
        for message in bot.world.messages.get(seen..).unwrap_or(&[]) {
            if let Some(outcome) = classify_message(message) {
                return Drain::Outcome(outcome);
            }
            let lowered = message.to_lowercase();
            if lowered.contains("can't cook that") || lowered.contains("can't be cooked") {
                return Drain::BadInput;
            }
            if lowered.contains("don't have enough") {
                return Drain::InputGone;
            }
            if lowered.contains("needs a") && lowered.contains("level of") {
                return Drain::Outcome(WorkOutcome::LevelLocked);
            }
        }
    }
    Drain::Timeout
}

enum ClickedSite {
    Loc { x: i32, z: i32, loc: u16 },
    Npc { slot: u16 },
}

/// Click the station once (input on loc, or input on NPC) and answer the
/// interface for this click. `have`/`remaining` feed the counter.
fn click_once(bot: &mut Bot, job: &ProduceJob<'_>, site: &ClickedSite, have: u32, remaining: u32) -> Result<(), ScriptError> {
    let (use_com, use_slot) = crate::quests::common::inv_slot(bot, job.input)?;
    match site {
        ClickedSite::Loc { x, z, loc } => {
            bot.op_loc_u(*x, *z, *loc, job.input, u16::from(use_slot), use_com)?;
        }
        ClickedSite::Npc { slot } => {
            bot.op_npc_u(*slot, job.input, u16::from(use_slot), use_com)?;
        }
    }
    match &job.spec.ui {
        ProduceUi::Direct => Ok(()),
        ProduceUi::Choice { pick } => {
            if !answer_choice(bot, pick)? {
                return Err(ScriptError::Usage(format!("choice {pick:?} never answered at {:?}", bot.position())));
            }
            Ok(())
        }
        ProduceUi::Count => {
            // shortcut: count_dialog never clears server-side, so this
            // answers once per click on the live flag; revisit with edge
            // detection when a Count activity runs live.
            bot.pump(5)?;
            if bot.world.count_dialog {
                bot.resume_pcount(count_answer(have, remaining))?;
                bot.pump(5)?;
            }
            Ok(())
        }
        ProduceUi::Grid { button } => {
            let com = grid_button(bot, button)?;
            let target = bot.world.ticks + UI_TICKS;
            while bot.world.ticks < target && bot.world.modal.is_none() {
                bot.pump(2)?;
            }
            if bot.world.modal.is_none() {
                return Err(ScriptError::Usage(format!("grid {button:?} never opened at {:?}", bot.position())));
            }
            bot.if_button(com)?;
            bot.pump(10)?;
            if bot.world.count_dialog {
                bot.resume_pcount(count_answer(have, remaining))?;
                bot.pump(5)?;
            }
            Ok(())
        }
    }
}

/// Four orthogonal stands around a station tile, nearest first (ranges
/// are 1x2: a corner stand never reaches, like the tree footprint).
fn stands_for(x: i32, z: i32, from: (i32, i32)) -> Vec<(i32, i32)> {
    let mut stands = vec![(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)];
    stands.sort_by_key(|stand| (stand.0 - from.0).abs() + (stand.1 - from.1).abs());
    stands
}

/// True when the run died since `seen` (server line or flatlined hp).
/// Updates the index so the next call never replays a line.
fn died(bot: &Bot, seen: &mut usize) -> bool {
    let found = death_message_seen(bot, *seen) || hitpoints(bot) == 0;
    *seen = bot.world.messages.len();
    found
}

/// Work one input through clicks on an already-reached site: one click
/// per input, except counters and grids, which answer once and let the
/// server chain. Stops on the want, an empty pack or a full one. A stand
/// the server refuses reads as `NextStand` while nothing was consumed
/// there, so the caller moves on instead of failing the job.
fn work_site(bot: &mut Bot, job: &ProduceJob<'_>, site: &ClickedSite, target: u32, made_before: u32) -> Result<SiteDone, ScriptError> {
    let single_shot = matches!(job.spec.ui, ProduceUi::Count | ProduceUi::Grid { .. });
    let mut consumed = 0u32;
    let mut seen_death = bot.world.messages.len();
    let mut retries = 0u32;
    while consumed < target && pack_total(bot, job.input) > 0 {
        check_events(bot)?;
        if died(bot, &mut seen_death) {
            let mut hp = hitpoints(bot);
            death_resume(bot, 40, &mut hp, &mut seen_death)?;
            return Err(ScriptError::Usage(format!("died {} at {:?}", job.spec.verb, bot.position())));
        }
        if bot.world.modal.is_some() && matches!(job.spec.ui, ProduceUi::Direct) && !dismiss_modal(bot)? {
            break;
        }
        let have = pack_total(bot, job.input);
        if single_shot && consumed > 0 {
            break;
        }
        if click_once(bot, job, site, have, target - consumed).is_err() {
            break;
        }
        match drain_click(bot, job.input, have) {
            Drain::Consumed { units } => {
                consumed += units;
                retries = 0;
                if single_shot {
                    drain_chained(bot, job, target, &mut consumed, &mut seen_death);
                    break;
                }
            }
            Drain::Outcome(WorkOutcome::Full) => break,
            Drain::Outcome(WorkOutcome::UnreachableSpot) => {
                let made_now: u32 = job.products.iter().map(|id| pack_total(bot, *id)).sum();
                let batch = ProduceBatch { consumed, made: made_now.saturating_sub(made_before) };
                if batch.consumed > 0 {
                    return Ok(SiteDone::Done(batch));
                }
                return Ok(SiteDone::NextStand);
            }
            Drain::Outcome(WorkOutcome::LevelLocked) => {
                return Err(ScriptError::Usage(format!("{} needs a higher level at {:?}", obj_label(bot, job.input), bot.position())));
            }
            Drain::Outcome(_) => break,
            Drain::BadInput => {
                return Err(ScriptError::Usage(format!("{} cannot be produced here at {:?}", obj_label(bot, job.input), bot.position())));
            }
            Drain::InputGone => break,
            Drain::Timeout => {
                retries += 1;
                if retries > ITEM_RETRIES {
                    break;
                }
            }
            Drain::PumpFailed => {
                return Err(ScriptError::Usage(format!("{} pump failed at {:?}", job.spec.verb, bot.position())));
            }
        }
    }
    let made_now: u32 = job.products.iter().map(|id| pack_total(bot, *id)).sum();
    Ok(SiteDone::Done(ProduceBatch { consumed, made: made_now.saturating_sub(made_before) }))
}

/// After a counter/grid answer the server chains the whole batch: keep
/// draining without re-clicking, so no second dialogue opens.
fn drain_chained(bot: &mut Bot, job: &ProduceJob<'_>, target: u32, consumed: &mut u32, seen_death: &mut usize) {
    let total = pack_total(bot, job.input) + *consumed;
    let drain_target = bot.world.ticks + ITEM_TICKS * u64::from(target.saturating_sub(*consumed)) + UI_TICKS;
    while bot.world.ticks < drain_target && *consumed < target && pack_total(bot, job.input) > 0 {
        if bot.pump(2).is_err() || died(bot, seen_death) {
            break;
        }
    }
    *consumed = total.saturating_sub(pack_total(bot, job.input));
}

enum SiteDone {
    Done(ProduceBatch),
    NextStand,
}

/// Work one station tile: stands nearest-first, refusals move on. Stops
/// at the first stand that consumes anything.
fn work_loc(bot: &mut Bot, job: &ProduceJob<'_>, x: i32, z: i32, loc: u16) -> Result<ProduceBatch, ScriptError> {
    let target = if job.want == 0 { u32::MAX } else { job.want };
    let made_before: u32 = job.products.iter().map(|id| pack_total(bot, *id)).sum();
    let from = (bot.world.x, bot.world.z);
    for (stand_x, stand_z) in stands_for(x, z, from) {
        if pack_total(bot, job.input) == 0 {
            break;
        }
        match walk_checked(bot, stand_x, stand_z, STAND_WALK_TICKS) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) if crate::skill_common::is_walk_abort(&error) => return Err(error),
            Err(_) => continue,
        }
        let site = ClickedSite::Loc { x, z, loc };
        match work_site(bot, job, &site, target, made_before) {
            Ok(SiteDone::NextStand) => continue,
            Ok(SiteDone::Done(batch)) => {
                if batch.consumed > 0 {
                    return Ok(batch);
                }
            }
            Err(error) => return Err(error),
        }
    }
    let made_now: u32 = job.products.iter().map(|id| pack_total(bot, *id)).sum();
    Ok(ProduceBatch { consumed: 0, made: made_now.saturating_sub(made_before) })
}

/// Use the input on the job site until `want` is consumed (0 = all
/// carried), the input runs out, or the pack fills. No input at all is
/// `blocked` (the brain files an obtain task); a station that cooks
/// nothing is loud. Death aborts loud (`died ...`) so the caller retries
/// like the gather loops.
pub fn run_job(bot: &mut Bot, job: &ProduceJob<'_>) -> Result<ProduceBatch, ScriptError> {
    if pack_total(bot, job.input) == 0 {
        return Err(ScriptError::Blocked { missing: vec![job.input_name.to_string()] });
    }
    bot.log(&format!("{} {} near {:?}", job.spec.verb, job.input_name, job.near));
    bot.intend(&format!("{} {}", job.spec.verb, job.input_name));
    let target = if job.want == 0 { u32::MAX } else { job.want };
    let made_before: u32 = job.products.iter().map(|id| pack_total(bot, *id)).sum();
    let batch = match &job.spec.site {
        ProduceSite::Loc(names) => {
            let mut picked: Option<(i32, i32, i32, u16)> = None;
            for name in *names {
                if let Some(tile) = site_tiles(bot, name, job.near).into_iter().next() {
                    bot.log(&format!("{} on {name} at ({},{})", job.spec.verb, tile.0, tile.1));
                    picked = Some(tile);
                    break;
                }
            }
            let Some((x, z, _, loc)) = picked else {
                return Err(ScriptError::Usage(format!("no produce station {names:?} near {:?}", job.near)));
            };
            work_loc(bot, job, x, z, loc)?
        }
        ProduceSite::Npc(names) => {
            let mut slot: Option<u16> = None;
            for name in *names {
                if let Some(found) = bot.find_npc(name) {
                    slot = Some(found);
                    bot.log(&format!("{} on {name} (slot {found})", job.spec.verb));
                    break;
                }
            }
            let Some(slot) = slot else {
                return Err(ScriptError::Usage(format!("no produce NPC {names:?} near {:?}", bot.position())));
            };
            bot.approach_npc(slot, 15000)?;
            match work_site(bot, job, &ClickedSite::Npc { slot }, target, made_before)? {
                SiteDone::Done(batch) => batch,
                SiteDone::NextStand => ProduceBatch { consumed: 0, made: 0 },
            }
        }
    };
    if batch.consumed == 0 {
        return Err(ScriptError::Usage(format!("{} consumed nothing at {:?}", job.spec.verb, bot.position())));
    }
    bot.log(&format!("ok {} consumed {}, made {}", job.spec.verb, batch.consumed, batch.made));
    Ok(batch)
}

#[cfg(test)]
mod tests {
    use super::{cook_recipe, count_answer, grid_button, match_option, site_tiles, stands_for};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Shrimp cook row: raw in, shrimp out, burnt byproduct, level and xp
    /// from the dump; unknown inputs resolve to None.
    #[test]
    fn shrimp_recipe_comes_from_gamedata() {
        let recipe = cook_recipe("raw_shrimp").expect("shrimp row resolves");
        assert_eq!(recipe.kind, "cook");
        assert_eq!(recipe.level, 1);
        assert_eq!(recipe.xp, 300);
        assert!(recipe.inputs.iter().any(|(name, _)| name == "raw_shrimp"), "raw in, got: {:?}", recipe.inputs);
        assert!(recipe.outputs.iter().any(|(name, _)| name == "shrimp"), "shrimp out, got: {:?}", recipe.outputs);
        assert_eq!(recipe.burnt.as_deref(), Some("burntfish1"), "burnt byproduct, got: {:?}", recipe.burnt);
        assert_eq!(recipe.members, Some(false), "F2P row");
        assert!(cook_recipe("no_such_raw").is_none(), "unknown input never guesses");
        assert!(cook_recipe("RAW_SHRIMP").is_some(), "case-insensitive");
    }

    /// Choice matcher: substring, case-insensitive, com of the match.
    #[test]
    fn choice_matcher_picks_containing_option() {
        let options = vec![(3u16, "Tan all".to_string()), (4, "Make 5".to_string())];
        assert_eq!(match_option(&options, "tan all"), Some(3));
        assert_eq!(match_option(&options, "TAN"), Some(3), "case-insensitive");
        assert_eq!(match_option(&options, "make 10"), None, "no guess on mismatch");
        assert_eq!(match_option(&[], "tan"), None, "empty options never match");
    }

    /// Counter answers: all carried when wanted is 0, capped at carried.
    #[test]
    fn counter_answers_all_or_capped() {
        assert_eq!(count_answer(28, 0), 28, "0 means all carried");
        assert_eq!(count_answer(28, 10), 10);
        assert_eq!(count_answer(5, 10), 5, "capped at carried");
        assert_eq!(count_answer(0, 0), 0, "nothing carried answers nothing");
    }

    /// Grid buttons resolve through the component table (full dump name);
    /// unknown buttons fail loud instead of pressing blind.
    #[test]
    fn grid_buttons_resolve_or_fail_loud() {
        let bot = offline_bot();
        assert!(grid_button(&bot, "smelting:com_12").is_ok(), "make-x bronze resolves");
        assert!(grid_button(&bot, "no_such_button").is_err(), "unknown button fails loud");
    }

    /// Live station scan: a streamed range tile resolves by exact
    /// debugname; unknown stations find nothing.
    #[test]
    fn live_station_resolves_by_exact_debugname() {
        let mut bot = offline_bot();
        bot.world.level = 0;
        let range_id = (0u16..)
            .find(|id| super::station_debugname(*id) == "range")
            .expect("a range loc exists");
        bot.world.locs.insert((3200, 3200, 0), Some((range_id, 0, 0)));
        let found = site_tiles(&mut bot, "range", (3200, 3200));
        assert!(found.iter().any(|(x, z, _, _)| (*x, *z) == (3200, 3200)), "streamed range resolves, got: {found:?}");
        assert!(site_tiles(&mut bot, "no_such_station", (3200, 3200)).is_empty(), "unknown station finds nothing");
    }

    /// Stands ring the station orthogonally, nearest first.
    #[test]
    fn stands_ring_orthogonally_nearest_first() {
        let stands = stands_for(10, 10, (8, 10));
        assert_eq!(stands[0], (9, 10), "nearest stand first, got: {stands:?}");
        assert_eq!(stands.len(), 4, "four orthogonal stands, got: {stands:?}");
        assert!(!stands.contains(&(9, 9)), "no diagonal stands");
    }
}
