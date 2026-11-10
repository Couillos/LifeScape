//! Live transition prober (`probe_transitions [filtre]`).
//!
//! Admin-only dev tool: teleports onto the first reach-valid planner
//! stand of every row of `transitions.json` (or the filtered subset),
//! runs the trapdoor pre-open when one is listed, clicks the climb op,
//! then watches for the arrival, an open choice dialogue, stillness or
//! a wrong landing. Bounds mirror the `travel_to` executor (ride 41,
//! pre-open 17: measured live, never round).
//!
//! One failed row never stops the run: infra errors are logged, the
//! loop teleports to the next stand in every case, and filtered runs
//! merge into the existing verified file instead of wiping untested
//! rows. The planner reads that file back (§5.2: verified rides lose
//! the +50 penalty, failed ones are excluded).

use lifescape_json::{self as json, Value};
use lifescape_sdk::plan::Tile;
use lifescape_sdk::{Bot, Error};

use crate::devtools::{cheat_teleport, open_op, require_admin, save_report};
use crate::{ScriptError, ScriptResult};

/// Verified file, relative to the project root like the other devtool
/// paths (the session worker runs with the root as its working dir).
const VERIFIED_PATH: &str = "gamedata/data/transitions_verified.json";
/// Cheat landing settle: the teleport is instant, the stream lags.
const TELEPORT_TIMEOUT: u64 = 250;
/// Trapdoor lid watch, same bound as the executor pre-open.
const PRE_OPEN_WAIT: u64 = 17;
/// Ride landing watch, same bound as the executor ride.
const RIDE_WAIT: u64 = 41;
/// Lumbridge spawn: the safe tile every probe ends on.
const HOME: (i32, i32, i32) = (0, 3222, 3218);

/// One probed ride, in the planner's file shape plus the choice text.
struct Entry {
    loc: u16,
    from: Tile,
    result: &'static str,
    arrived: Tile,
    ticks: u64,
    message: String,
    choice: String,
    date: String,
}

/// Filter view of a transition: locs and table tiles only.
struct RowView {
    loc: u16,
    from: Tile,
}

/// Transport and operator stops abort the run; anything else is one
/// skipped row and the loop teleports on.
fn is_fatal(err: &ScriptError) -> bool {
    matches!(
        err,
        ScriptError::Sdk(Error::Interrupted(_) | Error::Io(_) | Error::Login(_))
    )
}

/// Region stream catch-up after a teleport: cross-region jumps deliver
/// `REBUILD_NORMAL` with the fresh locs; same-region hops just burn the
/// bound. Lid reads below need post-teleport data, never the
/// pre-teleport cache (which wrongly skipped two pre-opens mid-run).
const STREAM_SETTLE: u64 = 13;

/// True when a death message arrived since the click: the respawn, not
/// the transition, explains the landing. Those rows leave no verdict.
fn died_since(messages: &[String], since: usize) -> bool {
    messages.get(since..).unwrap_or(&[]).iter().any(|line| line.to_lowercase().contains("oh dear you are dead"))
}

fn await_stream(bot: &mut Bot) -> Result<(), ScriptError> {
    for _ in 0..STREAM_SETTLE {
        let frames = bot.pump(1)?;
        if frames.iter().any(|frame| frame.name == "REBUILD_NORMAL") {
            break;
        }
    }
    Ok(())
}
/// Last game message that arrived since the click: the ride's own
/// answer, never a stale line from an earlier row.
fn new_message(bot: &Bot, since: usize) -> String {
    bot.world.messages.get(since..).unwrap_or(&[]).last().cloned().unwrap_or_default()
}

fn arrived_tile(bot: &Bot) -> Tile {
    let (level, x, z) = bot.position();
    Tile { x, z, level }
}

/// Today's UTC date through civil-from-days (std-only, no chrono).
fn civil_date() -> String {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|span| span.as_secs() / 86400)
        .unwrap_or(0) as i64;
    civil_date_of(days)
}

fn civil_date_of(days: i64) -> String {
    let shifted = days + 719468;
    let era = shifted.div_euclid(146097);
    let day_of_era = shifted - era * 146097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_part + 2) / 5 + 1) as u32;
    let month = (if month_part < 10 { month_part + 3 } else { month_part - 9 }) as u32;
    if month <= 2 {
        year += 1;
    }
    format!("{year:04}-{month:02}-{day:02}")
}

fn tile_value(tile: Tile) -> Value {
    Value::Arr(vec![
        Value::Int(tile.level as i64),
        Value::Int(tile.x as i64),
        Value::Int(tile.z as i64),
    ])
}

fn entry_value(entry: &Entry) -> Value {
    json::object(vec![
        ("arrived", tile_value(entry.arrived)),
        ("choice", json::str_value(&entry.choice)),
        ("date", json::str_value(&entry.date)),
        ("from", tile_value(entry.from)),
        ("loc", Value::Int(entry.loc as i64)),
        ("message", json::str_value(&entry.message)),
        ("result", json::str_value(entry.result)),
        ("ticks", Value::Int(entry.ticks as i64)),
    ])
}

fn entry_key(value: &Value) -> Option<(u16, i32, i32, i32)> {
    let loc = value.get("loc").as_i64()? as u16;
    let from = value.get("from").as_arr()?;
    if from.len() != 3 {
        return None;
    }
    Some((loc, from[0].as_i64()? as i32, from[1].as_i64()? as i32, from[2].as_i64()? as i32))
}

/// Merge new entries into the existing file body: tested rows are
/// replaced, skipped rows lose any earlier verdict, untested rows
/// survive, unparseable rows are kept as-is.
fn merge_verified(existing: Option<&str>, fresh: &[Entry], remove: &[(u16, i32, i32, i32)]) -> Result<String, String> {
    let mut kept: Vec<Value> = Vec::new();
    let mut order: Vec<(u16, i32, i32, i32)> = Vec::new();
    let mut table: std::collections::HashMap<(u16, i32, i32, i32), Value> = std::collections::HashMap::new();
    if let Some(body) = existing {
        let parsed = json::parse(body).map_err(|err| format!("bad {VERIFIED_PATH}: {err}"))?;
        let rows = parsed.as_arr().ok_or_else(|| format!("bad {VERIFIED_PATH}: not an array"))?;
        for row in rows {
            match entry_key(row) {
                Some(key) => {
                    if !table.contains_key(&key) {
                        order.push(key);
                    }
                    table.insert(key, row.clone());
                }
                None => kept.push(row.clone()),
            }
        }
    }
    for entry in fresh {
        let key = (entry.loc, entry.from.level, entry.from.x, entry.from.z);
        if !table.contains_key(&key) {
            order.push(key);
        }
        table.insert(key, entry_value(entry));
    }
    for key in remove {
        table.remove(key);
        order.retain(|known| known != key);
    }
    order.sort();
    let mut out: Vec<Value> = order.into_iter().filter_map(|key| table.remove(&key)).collect();
    out.append(&mut kept);
    Ok(json::stringify_pretty(&Value::Arr(out)))
}

/// Ride one transition from its stand: pre-open the lid when one is
/// listed, click the climb op on the footprint tile nearest the stand,
/// then watch for the arrival like the executor does (same dialogue,
/// jump and stillness rules, same bounds).
fn probe_row(bot: &mut Bot, view: RowView, stand: Tile, expect: Tile, open_to: Option<u16>, climb_op: u8, climb_loc: u16, footprint: &[(i32, i32)], date: &str) -> Result<Entry, ScriptError> {
    if !cheat_teleport(bot, stand.level, stand.x, stand.z, TELEPORT_TIMEOUT)? {
        return Err(ScriptError::Usage(format!(
            "teleport to stand ({},{},{}) failed",
            stand.x, stand.z, stand.level
        )));
    }
    await_stream(bot)?;
    if let Some(open_to) = open_to {
        let lid = if view.loc != 0 { view.loc } else { climb_loc };
        if bot.world.loc_at(view.from.x, view.from.z, view.from.level).map(|loc| loc.0) != Some(open_to) {
            match open_op(lid) {
                Some(op) => {
                    bot.op_loc(op, view.from.x, view.from.z, lid)?;
                    let watched = bot.world.ticks + PRE_OPEN_WAIT;
                    while bot.world.ticks < watched {
                        bot.pump(1)?;
                        if bot.world.loc_at(view.from.x, view.from.z, view.from.level).map(|loc| loc.0) == Some(open_to) {
                            break;
                        }
                    }
                }
                None => bot.log(&format!("no open op for loc {lid}, trying the climb anyway")),
            }
        }
    }
    let (start_level, start_x, start_z) = bot.position();
    let seen_messages = bot.world.messages.len();
    let (click_x, click_z) = footprint
        .iter()
        .min_by_key(|(tile_x, tile_z)| (tile_x - stand.x).abs() + (tile_z - stand.z).abs())
        .copied()
        .unwrap_or((view.from.x, view.from.z));
    bot.op_loc(climb_op, click_x, click_z, climb_loc)?;
    let started = bot.world.ticks;
    let watched = started + RIDE_WAIT;
    let entry = |bot: &Bot, result: &'static str, choice: String| Entry {
        loc: view.loc,
        from: view.from,
        result,
        arrived: arrived_tile(bot),
        ticks: bot.world.ticks.saturating_sub(started),
        message: new_message(bot, seen_messages),
        choice,
        date: date.to_string(),
    };
    loop {
        if bot.world.ticks >= watched {
            break;
        }
        bot.pump(1)?;
        let options = bot.chat_options();
        if !options.is_empty() {
            let choice = options.iter().map(|(_, text)| text.clone()).collect::<Vec<_>>().join(" / ");
            return Ok(entry(bot, "needs_choice", choice));
        }
        let (level, x, z) = bot.position();
        if level == expect.level && x == expect.x && z == expect.z {
            return Ok(entry(bot, "ok", String::new()));
        }
        let jumped = level != start_level || (x - start_x).abs().max((z - start_z).abs()) > 2;
        if jumped {
            if died_since(&bot.world.messages, seen_messages) {
                return Err(ScriptError::Usage("died mid-probe (respawned), no verdict".to_string()));
            }
            return Ok(entry(bot, "wrong_destination", String::new()));
        }
    }
    let (level, x, z) = bot.position();
    if died_since(&bot.world.messages, seen_messages) {
        return Err(ScriptError::Usage("died mid-probe (respawned), no verdict".to_string()));
    }
    if (level, x, z) == (start_level, start_x, start_z) {
        Ok(entry(bot, "no_move", String::new()))
    } else {
        Ok(entry(bot, "wrong_destination", String::new()))
    }
}

pub(crate) fn dev_probe_transitions(bot: &mut Bot, args: &[String]) -> ScriptResult {
    require_admin(bot)?;
    if args.len() > 1 {
        return Err(ScriptError::Usage("probe_transitions [filtre]".to_string()));
    }
    let filter = args.first().map(String::as_str);
    // Stands and expects come from the service bulk op (same prober
    // options: paid doors open, no tags held); the planner rides the
    // same first stand, so one verdict still covers the whole row.
    struct Target {
        name: String,
        loc: u16,
        climb_loc: u16,
        from: Tile,
        stand: Tile,
        expect: Tile,
        open_to: Option<u16>,
        climb_op: u8,
        footprint: Vec<(i32, i32)>,
    }
    let mut targets: Vec<Target> = Vec::new();
    let mut skipped = 0u32;
    for found in bot.probe_targets(filter)? {
        let (Some(stand), Some(expect)) = (found.stand, found.expect) else {
            bot.log(&format!("skip {}: no reach-valid stand", found.view.name));
            skipped += 1;
            continue;
        };
        targets.push(Target {
            name: found.view.name,
            loc: found.view.loc,
            climb_loc: found.view.climb_loc,
            from: found.view.from,
            stand,
            expect,
            open_to: found.view.open_to,
            climb_op: found.view.climb_op,
            footprint: found.view.footprint,
        });
    }
    bot.intend("probing transitions");
    bot.log(&format!(
        "probing {} transitions{}",
        targets.len(),
        filter.map(|text| format!(" (filtre {text:?})")).unwrap_or_default()
    ));
    let date = civil_date();
    let mut entries: Vec<Entry> = Vec::new();
    let mut remove: Vec<(u16, i32, i32, i32)> = Vec::new();
    for (position, target) in targets.iter().enumerate() {
        bot.log(&format!(
            "{:4}/{} {} loc={} climb={} op={} stand=({},{},{}) expect=({},{},{})",
            position + 1,
            targets.len(),
            target.name,
            target.loc,
            target.climb_loc,
            target.climb_op,
            target.stand.x,
            target.stand.z,
            target.stand.level,
            target.expect.x,
            target.expect.z,
            target.expect.level,
        ));
        let view = RowView { loc: target.loc, from: target.from };
        match probe_row(
            bot,
            view,
            target.stand,
            target.expect,
            target.open_to,
            target.climb_op,
            target.climb_loc,
            &target.footprint,
            &date,
        ) {
            Ok(entry) => {
                bot.log(&format!(
                    "  -> {} arrived=({},{},{}) en {} ticks{}",
                    entry.result,
                    entry.arrived.x,
                    entry.arrived.z,
                    entry.arrived.level,
                    entry.ticks,
                    if entry.message.is_empty() { String::new() } else { format!(" msg={:?}", entry.message) },
                ));
                entries.push(entry);
            }
            Err(err) => {
                if is_fatal(&err) {
                    let _ = cheat_teleport(bot, HOME.0, HOME.1, HOME.2, TELEPORT_TIMEOUT);
                    return Err(err);
                }
                bot.log(&format!("  -> skip: {err}"));
                remove.push((target.loc, target.from.level, target.from.x, target.from.z));
                skipped += 1;
            }
        }
        // Safety: head to the next stand (or home) in every case, so a
        // dead row never leaves admin without an exit. A missed hop is
        // logged only: the next probe re-teleports anyway.
        let next = targets.get(position + 1).map(|found| (found.stand.level, found.stand.x, found.stand.z)).unwrap_or(HOME);
        match cheat_teleport(bot, next.0, next.1, next.2, TELEPORT_TIMEOUT) {
            Ok(true) => {}
            Ok(false) => bot.log("safety hop did not settle, next probe re-teleports"),
            Err(err) => {
                if is_fatal(&err) {
                    return Err(err);
                }
                bot.log(&format!("safety hop failed ({err}), next probe re-teleports"));
            }
        }
    }
    let _ = cheat_teleport(bot, HOME.0, HOME.1, HOME.2, TELEPORT_TIMEOUT);
    let mut counts = std::collections::HashMap::new();
    for entry in &entries {
        *counts.entry(entry.result).or_insert(0u32) += 1;
    }
    bot.log(&format!(
        "probed={} ok={} wrong_destination={} no_move={} needs_choice={} skipped={}",
        entries.len(),
        counts.get("ok").copied().unwrap_or(0),
        counts.get("wrong_destination").copied().unwrap_or(0),
        counts.get("no_move").copied().unwrap_or(0),
        counts.get("needs_choice").copied().unwrap_or(0),
        skipped,
    ));
    if entries.is_empty() {
        bot.log("nothing probed, verified file untouched");
        return Ok(0);
    }
    let existing = std::fs::read_to_string(VERIFIED_PATH).ok();
    let body = merge_verified(existing.as_deref(), &entries, &remove)
        .map_err(|err| ScriptError::Sdk(Error::Io(format!("merge {VERIFIED_PATH}: {err}"))))?;
    save_report(VERIFIED_PATH, &body)?;
    bot.log(&format!("wrote {VERIFIED_PATH} ({} probed rows merged)", entries.len()));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::{civil_date_of, died_since, merge_verified, Entry};
    use lifescape_sdk::plan::Tile;

    #[test]
    fn civil_epoch_is_1970_01_01() {
        assert_eq!(civil_date_of(0), "1970-01-01");
    }

    fn entry(result: &'static str) -> Entry {
        Entry {
            loc: 5,
            from: Tile { x: 1, z: 2, level: 0 },
            result,
            arrived: Tile { x: 3, z: 4, level: 0 },
            ticks: 7,
            message: "m".to_string(),
            choice: String::new(),
            date: "2026-11-06".to_string(),
        }
    }

    #[test]
    fn death_is_detected_since_the_click() {
        let messages = vec!["Welcome to RuneScape.".to_string(), "Oh dear you are dead!".to_string()];
        assert!(died_since(&messages, 1));
        assert!(!died_since(&messages, 2));
        assert!(died_since(&messages, 0));
    }

    #[test]
    fn merge_replaces_tested_and_keeps_others() {
        let old = r#"[{"loc":5,"from":[0,1,2],"result":"no_move","arrived":[0,1,2],"ticks":41,"message":"","choice":"","date":"2026-11-01"},{"loc":9,"from":[0,7,7],"result":"ok","arrived":[0,8,8],"ticks":3,"message":"","choice":"","date":"2026-11-01"}]"#;
        let body = merge_verified(Some(old), &[entry("ok")], &[]).expect("merge");
        let parsed = lifescape_json::parse(&body).expect("parses");
        let rows = parsed.as_arr().expect("array");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get("result").as_str(), Some("ok"));
        assert_eq!(rows[0].get("date").as_str(), Some("2026-11-06"));
        assert_eq!(rows[1].get("loc").as_i64(), Some(9));
    }

    #[test]
    fn merge_starts_from_empty_without_file() {
        let body = merge_verified(None, &[entry("needs_choice")], &[]).expect("merge");
        let parsed = lifescape_json::parse(&body).expect("parses");
        let rows = parsed.as_arr().expect("array");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get("from").as_arr().map(Vec::len), Some(3));
    }

    #[test]
    fn merge_drops_skipped_verdicts() {
        let old = r#"[{"loc":5,"from":[0,1,2],"result":"wrong_destination","arrived":[0,9,9],"ticks":22,"message":"Oh dear you are dead!","choice":"","date":"2026-11-06"}]"#;
        let body = merge_verified(Some(old), &[], &[(5, 0, 1, 2)]).expect("merge");
        let parsed = lifescape_json::parse(&body).expect("parses");
        assert!(parsed.as_arr().expect("array").is_empty());
    }
    /// The planner must read back exactly what the prober writes: same
    /// `from` order, same result words, same verdict effects.
    #[test]
    fn verified_round_trips_through_planner_tables() {
        let dir = std::env::temp_dir().join(format!("probe_verify_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tmp");
        std::fs::write(
            dir.join("transitions.json"),
            r#"[{"name":"lad","from":[0,1,2],"to":[0,3,4],"loc":5,"climb_loc":5,"climb_op":1},{"name":"trap","from":[0,7,7],"to":[0,7,8],"loc":6,"climb_loc":6,"climb_op":1}]"#,
        )
        .expect("rows");
        std::fs::write(dir.join("nav_overrides.json"), "{}").expect("overrides");
        let ok = Entry {
            loc: 5,
            from: Tile { x: 1, z: 2, level: 0 },
            result: "ok",
            arrived: Tile { x: 3, z: 4, level: 0 },
            ticks: 7,
            message: String::new(),
            choice: String::new(),
            date: "2026-11-06".to_string(),
        };
        let bad = Entry {
            loc: 6,
            from: Tile { x: 7, z: 7, level: 0 },
            result: "wrong_destination",
            arrived: Tile { x: 7, z: 8, level: 0 },
            ticks: 9,
            message: String::new(),
            choice: String::new(),
            date: "2026-11-06".to_string(),
        };
        let body = merge_verified(None, &[ok, bad], &[]).expect("merge");
        std::fs::write(dir.join("transitions_verified.json"), &body).expect("verified");
        let tables = lifescape_nav::tables::Tables::load(&dir).expect("tables");
        assert_eq!(tables.verdict(5, Tile { x: 1, z: 2, level: 0 }), Some(lifescape_nav::tables::Verdict::Ok));
        assert!(tables.verdict(5, Tile { x: 1, z: 2, level: 0 }).is_some_and(|seen| seen.verified() && !seen.excludes()));
        assert_eq!(
            tables.verdict(6, Tile { x: 7, z: 7, level: 0 }),
            Some(lifescape_nav::tables::Verdict::WrongDestination)
        );
        assert!(tables.verdict(6, Tile { x: 7, z: 7, level: 0 }).is_some_and(|seen| seen.excludes()));
        assert!(lifescape_nav::tables::Verdict::NeedsChoice.verified());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn refuses_non_admin() {
        let mut bot = lifescape_sdk::Bot::new("127.0.0.1", 43594, ".", ".");
        bot.username = "t01".to_string();
        let err = super::dev_probe_transitions(&mut bot, &[]).unwrap_err();
        assert!(err.to_string().contains("admin"), "want admin refusal, got {err}");
    }
}
