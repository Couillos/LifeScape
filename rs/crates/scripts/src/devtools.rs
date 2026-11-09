//! Staff/dev tools for supervised sessions (live checks and account setup).
//!
//! Already live elsewhere, not repeated here: `debug_walk`, `goto`,
//! `hello`, `walkto` (see `lib.rs`). Everything below runs inside the
//! orchestrator session: the worker opened it, it closes it. Ticks bound
//! every game wait; only process guards (file IO) touch the wall clock.
//!
//! Entry point is [`run_devtool`]; the registry wiring lives in `lib.rs`.
//! Tools that need the pack reader, the engine content tree or a second
//! connection cannot run in-session and report `impossible` instead.

use lifescape_gamedata::{loc_by_id, objs_by_name};
use lifescape_sdk::waits::wait_ticks;
use lifescape_sdk::{Bot, Error};

use crate::{ScriptError, ScriptResult};

/// Every devtool name this module answers for.
pub const DEVTOOL_NAMES: &[&str] = &[
    "bank_deposit",
    "capture",
    "check_doors",
    "check_routes",
    "check_transitions",
    "diff_routes",
    "drop",
    "dump_bank_stands",
    "dump_transitions",
    "give",
    "make_accounts",
    "parallel_login",
    "probe_transitions",
    "quest_reset",
    "reset_quests",
    "trade_swap",
    "walktour",
];

/// Run one devtool by name. Unknown names and impossible tools are
/// errors, never silent fallbacks.
pub fn run_devtool(name: &str, bot: &mut Bot, args: &[String]) -> ScriptResult {
    match name {
        "bank_deposit" => dev_bank_deposit(bot, args),
        "capture" => dev_capture(bot, args),
        "check_doors" => crate::devtools_check::dev_check_doors(bot, args),
        "check_routes" => crate::devtools_check::dev_check_routes(bot, args),
        "check_transitions" => crate::devtools_check::dev_check_transitions(bot, args),
        "diff_routes" => Err(impossible(
            "diff_routes needs the node engine (tools/lifescape-routes.mts) and the packed collision maps, \
             neither is reachable from a supervised Rust session",
        )),
        "drop" => dev_drop(bot, args),
        "dump_bank_stands" => crate::devtools_check::dev_dump_bank_stands(bot, args),
        "dump_transitions" => Err(impossible(
            "dump_transitions runs outside sessions: orchestrator dump-transitions <sortie>",
        )),
        "give" => dev_give(bot, args),
        "make_accounts" => Err(impossible(
            "make_accounts writes sqlite accounts, hashes via docker/node bcrypt and logs in fresh sessions; \
             a supervised script owns no DB handle and must not spawn logins",
        )),
        "parallel_login" => Err(impossible(
            "parallel_login opens N fresh connections with distinct credentials; the session hides \
             host/port/pack/key and owns exactly one connection, so this must stay a standalone runner",
        )),
        "probe_transitions" => crate::devtools_probe::dev_probe_transitions(bot, args),
        "quest_reset" => dev_quest_reset(bot, args),
        "reset_quests" => dev_reset_quests(bot, args),
        "trade_swap" => dev_trade_swap(bot, args),
        "walktour" => dev_walktour(bot, args),
        _ => Err(ScriptError::Usage(format!("unknown devtool {name:?}"))),
    }
}

pub(crate) fn impossible(why: &str) -> ScriptError {
    ScriptError::Usage(format!("impossible: {why}"))
}

// -- shared helpers ----------------------------------------------------------

/// Staff cheats only exist on the admin account; anywhere else they are
/// refused instead of silently skipped.
pub(crate) fn require_admin(bot: &Bot) -> Result<(), ScriptError> {
    if bot.username == "admin" {
        Ok(())
    } else {
        Err(ScriptError::Usage(format!(
            "forbidden: staff cheat on non-staff account {:?} (admin only)",
            bot.username
        )))
    }
}

pub(crate) fn obj_id_exact(name: &str) -> Result<u16, ScriptError> {
    objs_by_name(name).into_iter().next().map(|entry| entry.obj_id as u16).ok_or_else(|| {
        ScriptError::Usage(format!("unknown item {name:?}"))
    })
}

/// Staff teleport: `::tele` lands exactly, then pump until the position
/// matches. True when the landing stuck.
pub(crate) fn cheat_teleport(bot: &mut Bot, level: i32, x: i32, z: i32, timeout: u64) -> Result<bool, ScriptError> {
    bot.cheat(&format!("tele {level},{},{},{},{}", x >> 6, z >> 6, x & 63, z & 63))?;
    let deadline = bot.world.ticks + timeout;
    while bot.world.ticks < deadline {
        bot.pump(2)?;
        if bot.position() == (level, x, z) {
            return Ok(true);
        }
    }
    Ok(bot.position() == (level, x, z))
}

/// 1-based "Open" op of a loc type, like `doors.open_op`; None when the
/// loc has no Open op (stairs, containers, decoration).
pub(crate) fn open_op(loc_id: u16) -> Option<u8> {
    loc_by_id(usize::from(loc_id)).and_then(|entry| {
        entry.ops.iter().position(|name| name.as_deref().is_some_and(|label| label.to_lowercase() == "open"))
    })
    .map(|index| index as u8 + 1)
}

/// Orthogonal neighbours of a footprint, closest to the bot first.
pub(crate) fn sides_of(footprint: &[[i64; 2]], bot_x: i32, bot_z: i32) -> Vec<(i32, i32)> {
    let mut sides = Vec::new();
    for tile in footprint {
        for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let side = (tile[0] as i32 + dx, tile[1] as i32 + dz);
            if !footprint.contains(&[side.0 as i64, side.1 as i64]) && !sides.contains(&side) {
                sides.push(side);
            }
        }
    }
    sides.sort_by_key(|side| (side.0 - bot_x).abs() + (side.1 - bot_z).abs());
    sides
}

/// Pump until the player is on `expected` (within one tile), tick-bounded.
pub(crate) fn await_landing(bot: &mut Bot, expected: (i32, i32, i32), timeout: u64) -> Result<bool, ScriptError> {
    let deadline = bot.world.ticks + timeout;
    while bot.world.ticks < deadline {
        bot.pump(2)?;
        let (level, x, z) = bot.position();
        if level == expected.0 && (x - expected.1).abs() + (z - expected.2).abs() <= 1 {
            return Ok(true);
        }
    }
    let (level, x, z) = bot.position();
    Ok(level == expected.0 && (x - expected.1).abs() + (z - expected.2).abs() <= 1)
}

pub(crate) fn save_report(path: &str, body: &str) -> Result<(), ScriptError> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|err| ScriptError::Sdk(Error::Io(err.to_string())))?;
        }
    }
    std::fs::write(path, body).map_err(|err| ScriptError::Sdk(Error::Io(err.to_string())))
}

pub(crate) fn escape_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if (other as u32) < 0x20 => out.push_str("?"),
            other => out.push(other),
        }
    }
    out
}

/// `--only X --limit N --skip M`. Anything else is refused
/// instead of silently ignored (`--trace` needs the private connection
/// trace, `--run`/`--fresh` have no Rust-SDK equivalent yet).
#[derive(Debug, Default)]
pub(crate) struct Filter {
    pub(crate) only: Option<String>,
    pub(crate) limit: usize,
    pub(crate) skip: usize,
}

pub(crate) fn parse_filter(args: &[String]) -> Result<Filter, ScriptError> {
    let mut filter = Filter::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--only" => {
                index += 1;
                filter.only = args.get(index).cloned();
            }
            "--limit" => {
                index += 1;
                filter.limit = args.get(index).and_then(|text| text.parse().ok()).unwrap_or(0);
            }
            "--skip" => {
                index += 1;
                filter.skip = args.get(index).and_then(|text| text.parse().ok()).unwrap_or(0);
            }
            other => return Err(ScriptError::Usage(format!("unsupported devtool flag {other:?} (want --only/--limit/--skip)"))),
        }
        index += 1;
    }
    if filter.only.is_none() && args.iter().any(|arg| arg == "--only" && filter.only.is_none()) {
        return Err(ScriptError::Usage("--only needs a value".to_string()));
    }
    Ok(filter)
}

// -- walktour ----------------------------------------------------------------
// Varrock / Lumbridge / Falador triangle with the SDK pathfinder.

const TOUR_STOPS: &[(&str, i32, i32)] = &[("Lumbridge", 3222, 3218), ("Falador", 2965, 3378), ("Varrock", 3205, 3429)];

fn dev_walktour(bot: &mut Bot, _args: &[String]) -> ScriptResult {
    for (name, x, z) in TOUR_STOPS {
        bot.intend(&format!("walktour leg to {name}"));
        bot.log(&format!("leg {:?} -> {name} ({x},{z})", bot.position()));
        bot.walk_to(*x, *z, 0, 9000)?;
        let (level, bx, bz) = bot.position();
        bot.log(&format!("arrived {name} at ({level},{bx},{bz})"));
        if (bx, bz) != (*x, *z) {
            bot.log(&format!("MISS {name}: ({bx},{bz}) instead of ({x},{z})"));
            return Ok(1);
        }
    }
    bot.log("tour complete");
    Ok(0)
}

// -- bank_deposit ------------------------------------------------------------

fn dev_bank_deposit(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let mut keep = Vec::new();
    for name in args {
        match obj_id_exact(name) {
            Ok(id) => keep.push(id),
            Err(_) => bot.log(&format!("unknown keep item {name:?}, not kept")),
        }
    }
    bot.intend("depositing backpack");
    let (here_x, here_z, here_level) = (bot.world.x, bot.world.z, bot.world.level);
    let mut booths: Vec<(i32, i32, u16)> = Vec::new();
    for entry in bot.static_locs_near(here_x, here_z, here_level, 150)? {
        if (bot.tables.loc_name)(entry.loc).as_deref() == Some("bankbooth") {
            booths.push((entry.x, entry.z, entry.loc));
        }
    }
    booths.sort_by_key(|booth| (booth.0 - bot.world.x).abs() + (booth.1 - bot.world.z).abs());
    let (booth_x, booth_z, booth_loc) = match booths.first() {
        Some(booth) => *booth,
        None => {
            bot.log("no bank booth in live view");
            return Ok(1);
        }
    };
    if !bot.open_bank(booth_x, booth_z, booth_loc, 100)? {
        bot.log(&format!("bank did not open at {booth_x},{booth_z}"));
        return Ok(1);
    }
    let Some(side_com) = (bot.tables.com_id)("bank_side:inv") else {
        return Err(ScriptError::Usage("bank_side:inv missing from component table".to_string()));
    };
    if !bot.bank_deposit_all(side_com, &keep, 80)? {
        bot.log("deposit did not clear the backpack");
        for text in bot.world.messages.iter().rev().take(5) {
            bot.log(&format!("game msg: {:?}", text.chars().take(100).collect::<String>()));
        }
    }
    bot.log(&format!("backpack now holds {} slots", bot.backpack().1.len()));
    Ok(0)
}

// -- drop / give -------------------------------------------------------------

fn dev_drop(bot: &mut Bot, args: &[String]) -> ScriptResult {
    if args.is_empty() {
        return Err(ScriptError::Usage("drop <item> [count]".to_string()));
    }
    let obj_id = obj_id_exact(&args[0])?;
    let count: u32 = args.get(1).and_then(|text| text.parse().ok()).unwrap_or(1).max(1);
    bot.intend(&format!("dropping {} x{count}", args[0]));
    let mut dropped = 0;
    while dropped < count {
        if !bot.drop(obj_id, 750)? {
            break;
        }
        wait_ticks(bot, 3)?;
        dropped += 1;
    }
    bot.log(&format!("dropped {} x{dropped}", args[0]));
    Ok(0)
}

fn dev_give(bot: &mut Bot, args: &[String]) -> ScriptResult {
    require_admin(bot)?;
    if args.is_empty() {
        return Err(ScriptError::Usage("give <item> [count]".to_string()));
    }
    let count: u32 = args.get(1).and_then(|text| text.parse().ok()).unwrap_or(1).max(1);
    bot.intend(&format!("give {} x{count}", args[0]));
    let obj_id = obj_id_exact(&args[0])?;
    let before = bot.inventory().get(&obj_id).copied().unwrap_or(0);
    bot.cheat(&format!("give {} {count}", args[0]))?;
    let deadline = bot.world.ticks + 120;
    while bot.world.ticks < deadline {
        bot.pump(2)?;
        if bot.inventory().get(&obj_id).copied().unwrap_or(0) >= before + count {
            bot.log(&format!("have {} x{}", args[0], bot.inventory().get(&obj_id).copied().unwrap_or(0)));
            return Ok(0);
        }
    }
    bot.log(&format!("give failed: {:?}", bot.inventory()));
    Ok(0)
}

// -- quest_reset / reset_quests -----------------------------------------------

/// Stage varps zeroed by the server `reset_all_quests` proc (F2P subset),
/// plus qp so quest-point gates are earned honestly by the test order.
const QUEST_VARPS: &[&str] = &[
    "spy", "cookquest", "demonstart", "doricquest", "dragon_goblin", "dragon_ned_hired", "dragon_oracle",
    "dragon_shield", "dragon_wall", "dragonquest", "dragonquestvar", "ernestlever", "haunted_manor_fountain_poisoned",
    "haunted", "goblinquest", "squire", "imp", "hunt", "hunt_store_employed", "prince_keystatus", "princequest",
    "prieststart", "rjquest", "runemysteries", "sheep", "blackarmgang", "phoenixgang", "vampire", "hetty", "qp",
];

fn dev_quest_reset(bot: &mut Bot, _args: &[String]) -> ScriptResult {
    require_admin(bot)?;
    bot.intend("resetting F2P quest stages");
    for name in QUEST_VARPS {
        let seen = bot.world.messages.len();
        bot.cheat(&format!("setvar {name} 0"))?;
        let deadline = bot.world.ticks + 200;
        let mut confirmed = false;
        while bot.world.ticks < deadline {
            bot.pump(2)?;
            if bot.world.messages[seen..].iter().any(|line| line.to_lowercase().contains(&format!("set {name}:"))) {
                confirmed = true;
                break;
            }
        }
        bot.log(&format!("{name} {}", if confirmed { "ok" } else { "NOT-CONFIRMED" }));
    }
    Ok(0)
}

fn dev_reset_quests(bot: &mut Bot, _args: &[String]) -> ScriptResult {
    require_admin(bot)?;
    bot.intend("resetting quests");
    bot.cheat("~resetquests")?;
    wait_ticks(bot, 1000)?;
    bot.log(&format!("reset sent, at {:?}", bot.position()));
    Ok(0)
}

// -- trade_swap ---------------------------------------------------------------
// Bilateral swap: both sides run this mirrored, each side drives its own
// window (offer, accept twice) and verifies its own result.

fn dev_trade_swap(bot: &mut Bot, args: &[String]) -> ScriptResult {
    if args.len() < 3 {
        return Err(ScriptError::Usage("trade_swap <partner> <give_item> <give_n> [want_item] [want_n]".to_string()));
    }
    let partner = args[0].clone();
    let give = if args[1] == "-" { None } else { Some(obj_id_exact(&args[1])?) };
    let give_n: u32 = args[2].parse().map_err(|_| ScriptError::Usage("give_n must be a number".to_string()))?;
    let want = if args.len() > 3 && args[3] != "-" { Some(obj_id_exact(&args[3])?) } else { None };
    let want_n: u32 = args.get(4).and_then(|text| text.parse().ok()).unwrap_or(1);
    bot.intend(&format!("trade with {partner}"));
    if let Some(id) = give {
        if bot.inventory().get(&id).copied().unwrap_or(0) < give_n {
            bot.log(&format!("no {} x{give_n} to give", args[1]));
            return Ok(1);
        }
    }
    let deadline = bot.world.ticks + 6000;
    while bot.world.ticks < deadline && bot.trade_modal().is_none() {
        if !bot.trade_request(&partner, 600)? {
            continue;
        }
    }
    if bot.trade_modal().is_none() {
        bot.log(&format!("no trade window with {partner}"));
        return Ok(1);
    }
    bot.log(&format!("trade window open with {partner}"));
    while bot.world.ticks < deadline && bot.trade_modal() == Some("main") {
        if let Some(id) = give {
            if !bot.trade_offer(id, give_n, 120)? {
                bot.log("offer failed");
                return Ok(1);
            }
        }
        if let Some(id) = want {
            if bot.trade_partner_offer().get(&id).copied().unwrap_or(0) < want_n {
                bot.pump(10)?;
                continue;
            }
        }
        bot.trade_accept(120)?;
        bot.pump(5)?;
    }
    if bot.trade_modal() != Some("confirm") {
        bot.log("no confirm screen");
        bot.trade_close()?;
        return Ok(1);
    }
    bot.log("confirm screen, accepting");
    while bot.world.ticks < deadline && bot.trade_modal().is_some() {
        bot.trade_accept(120)?;
        bot.pump(5)?;
    }
    bot.pump(10)?;
    let have = want.map(|id| bot.inventory().get(&id).copied().unwrap_or(0)).unwrap_or(0);
    let kept = give.map(|id| bot.inventory().get(&id).copied().unwrap_or(0)).unwrap_or(0);
    bot.log(&format!("after swap: want={have} kept-give={kept}"));
    if want.is_some_and(|_| have < want_n) {
        bot.log(&format!("swap failed: no {}", args[3]));
        return Ok(1);
    }
    Ok(0)
}

// -- capture ------------------------------------------------------------------

fn dev_capture(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let ticks: u32 = args.first().and_then(|text| text.parse().ok()).unwrap_or(200);
    let outfile = args.get(1).cloned().unwrap_or_else(|| format!("capture_{}.json", bot.username));
    bot.intend("capturing frames");
    let mut frames = Vec::new();
    for frame in bot.pump(ticks)? {
        let mut hex = String::with_capacity(frame.body.len() * 2);
        for byte in &frame.body {
            hex.push_str(&format!("{byte:02x}"));
        }
        frames.push(format!(
            "{{\"op\":{},\"name\":\"{}\",\"hex\":\"{hex}\"}}",
            frame.opcode,
            escape_json(&frame.name)
        ));
    }
    let body = format!("[{}]", frames.join(","));
    save_report(&outfile, &body)?;
    bot.log(&format!("wrote {} frames to {outfile}", frames.len()));
    Ok(0)
}
#[cfg(test)]
mod tests {
    use super::{escape_json, parse_filter, sides_of};

    #[test]
    fn filter_flags() {
        let args = ["--only".to_string(), "lum".to_string(), "--limit".to_string(), "3".to_string(), "--skip".to_string(), "1".to_string()];
        let filter = parse_filter(&args).expect("flags");
        assert_eq!(filter.only.as_deref(), Some("lum"));
        assert_eq!(filter.limit, 3);
        assert_eq!(filter.skip, 1);
    }

    #[test]
    fn unknown_flag_is_refused() {
        assert!(parse_filter(&["--trace".to_string(), "x".to_string()]).is_err());
    }

    #[test]
    fn sides_skip_footprint_tiles() {
        let sides = sides_of(&[[10, 10], [11, 10]], 0, 0);
        assert!(!sides.contains(&(10, 10)));
        assert!(!sides.contains(&(11, 10)));
        assert!(sides.contains(&(10, 9)));
        assert!(sides.contains(&(12, 10)));
    }

    #[test]
    fn json_escape_round_meaning() {
        assert_eq!(escape_json("a\"b"), "a\\\"b");
    }
}
