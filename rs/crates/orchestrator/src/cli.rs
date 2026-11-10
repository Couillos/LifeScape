#![forbid(unsafe_code)]
//! Orchestrator CLI client: sessions stay open, scripts load/unload in place.
//!
//! Sessions are persistent: open one per account, then load/unload scripts
//! and call SDK methods inside it without reconnecting. Every command
//! auto-starts the daemon when needed.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::Command;
use std::time::Duration;

use crate::daemon::WORKER_TIMEOUT;
use lifescape_json::{self as json, Value, object, str_value};
use crate::paths;

fn send_command(request: &Value, timeout: Duration) -> Result<Value, String> {
    let stream = UnixStream::connect(paths::socket_path())
        .map_err(|error| format!("session unreachable: {error}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| error.to_string())?;
    let mut buf = json::stringify(request);
    buf.push('\n');
    (&stream)
        .write_all(buf.as_bytes())
        .map_err(|error| format!("session unreachable: {error}"))?;
    let mut line = String::new();
    BufReader::new(&stream)
        .read_line(&mut line)
        .map_err(|error| format!("session unreachable: {error}"))?;
    if line.trim().is_empty() {
        return Ok(object(vec![]));
    }
    json::parse(&line).map_err(|error| format!("bad daemon reply: {error}"))
}

fn ensure_daemon() -> bool {
    if send_command(
        &object(vec![("cmd", str_value("status"))]),
        Duration::from_secs(2),
    )
    .is_ok()
    {
        return true;
    }
    paths::ensure_dirs();
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths::daemon_log());
    let Ok(log) = log else { return false };
    let out = log
        .try_clone()
        .map(std::process::Stdio::from)
        .unwrap_or_else(|_| std::process::Stdio::null());
    let _ = Command::new(std::env::current_exe().unwrap_or_else(|_| "orchestrator".into()))
        .arg("daemon")
        .current_dir(paths::root())
        .stdin(std::process::Stdio::null())
        .stdout(out)
        .stderr(log)
        .spawn();
    let deadline = crate::jobs::now_secs() + 5.0;
    while crate::jobs::now_secs() < deadline {
        if send_command(
            &object(vec![("cmd", str_value("status"))]),
            Duration::from_secs(2),
        )
        .is_ok()
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

fn val_str(value: &Value) -> String {
    match value {
        Value::Null => "-".to_string(),
        Value::Str(text) => text.clone(),
        Value::Int(num) => num.to_string(),
        Value::Float(num) => num.to_string(),
        Value::Bool(flag) => flag.to_string(),
        other => json::stringify(other),
    }
}

fn format_status(jobs: &Value) -> String {
    let jobs = jobs.as_arr().cloned().unwrap_or_default();
    if jobs.is_empty() {
        return "no session".to_string();
    }
    let mut rows = Vec::new();
    for job in &jobs {
        let mut place = "-".to_string();
        if let Some(pos) = job.get("position").as_arr() {
            if pos.len() > 2 {
                place = format!("{},{}", val_str(&pos[1]), val_str(&pos[2]));
            }
        }
        if let Some(zone) = job.get("zone").as_str() {
            if !zone.is_empty() {
                place += &format!(" {zone}");
            }
        }
        rows.push(format!(
            "{:<14} {:<14} {:<13} hp={:<4} {:<10} {}",
            job.get("account").as_str().unwrap_or("?"),
            job.get("script").as_str().unwrap_or("-"),
            job.get("status").as_str().unwrap_or("?"),
            job.get("hitpoints").as_i64().map(|num| num.to_string()).as_deref().unwrap_or("-"),
            place,
            job.get("intention").as_str().unwrap_or(""),
        ));
    }
    rows.join("\n")
}

/// One operator-readable line over the resource gauge: cumulative counts
/// with per-second averages since daemon start. A `-` marks a counter
/// missing from the reply.
fn format_res_stats(stats: &Value) -> String {
    let int = |key: &str| stats.get(key).as_i64().map(|n| n.to_string()).unwrap_or_else(|| "-".to_string());
    let rate = |key: &str| stats.get(key).as_f64().map(|n| format!("{n:.2}")).unwrap_or_else(|| "-".to_string());
    let uptime = stats.get("uptime_secs").as_f64().map(|s| format!("{s:.0}s")).unwrap_or_else(|| "-".to_string());
    format!(
        "res-stats up={} conn={} ({} /s) pushes={} ({} /s, {} B at {} B/s) lights={} ({} /s) plans={} ({} /s) polls={} ({} /s) files={} ({} /s) reconnects={} refused={} disk={}Go low={}",
        uptime,
        int("connections"),
        rate("connections_per_s"),
        int("pushes"),
        rate("pushes_per_s"),
        int("push_bytes"),
        rate("push_bytes_per_s"),
        int("lights"),
        rate("lights_per_s"),
        int("plan_submits"),
        rate("plan_submits_per_s"),
        int("plan_polls"),
        rate("plan_polls_per_s"),
        int("files_written"),
        rate("files_per_s"),
        int("push_reconnects"),
        int("admission_refused"),
        int("disk_free_go"),
        stats.get("disk_low").as_bool().map(|low| low.to_string()).unwrap_or_else(|| "-".to_string()),
    )
}

/// One operator-readable line over the pathfinding gauge (lot 4): cache,
/// queue, refusals (= typed `nav-down` client-side, never a local plan),
/// batch reads, RAM. Full JSON stays available; this line is the quick
/// health glance. A `-` marks a counter missing from `stats_json`.
fn format_nav_stats(stats: &Value) -> String {
    let num = |key: &str| stats.get(key).as_i64().map(|n| n.to_string()).unwrap_or_else(|| "-".to_string());
    let age = match stats.get("l1_oldest_age_ms").as_i64() {
        Some(ms) => format!("{ms}ms"),
        None => "empty".to_string(),
    };
    let rss_mb = stats
        .get("rss_bytes")
        .as_i64()
        .map(|bytes| format!("{}Mo", bytes / (1024 * 1024)))
        .unwrap_or_else(|| "-".to_string());
    format!(
        "nav-stats v={} hits={} trunc={} joins={} miss={} l1={}/{}B age={} q={} inflight={} refused(cap/full/stale)={}/{}/{} unknown={} batch(door_defs/tiles_flags/static_locs_near/in_f2p/toll_fence/wants/diff/debug/transitions_list/probe_targets/doors_near)={}/{}/{}/{}/{}/{}/{}/{}/{}/{}/{} ram(warn/shed)={}/{} rss={} painted={}",
        stats.get("version").as_str().unwrap_or("-"),
        num("hits"),
        num("trunc_hits"),
        num("joins"),
        num("misses"),
        num("l1_len"),
        num("l1_bytes"),
        age,
        num("queue_depth"),
        num("inflight"),
        num("refused_cap"),
        num("refused_full"),
        num("refused_stale"),
        num("unknown"),
        num("door_defs"),
        num("tiles_flags"),
        num("static_locs_near"),
        num("in_f2p"),
        num("toll_fence"),
        num("wants"),
        num("diff"),
        num("debug"),
        num("transitions_list"),
        num("probe_targets"),
        num("doors_near"),
        num("ram_warn"),
        num("ram_shed"),
        rss_mb,
        num("painted_squares"),
    )
}

fn format_ascii(state: &Value, radius: i64) -> Vec<String> {
    let pos = state.get("position").as_arr().cloned().unwrap_or_default();
    let here = (
        pos.get(1).and_then(|v| v.as_i64()).unwrap_or(0),
        pos.get(2).and_then(|v| v.as_i64()).unwrap_or(0),
    );
    let mut blocked = std::collections::HashSet::new();
    let mut doors = std::collections::HashSet::new();
    for entry in state.get("locs").as_arr().cloned().unwrap_or_default() {
        let (Some(x), Some(z)) = (entry.get("x").as_i64(), entry.get("z").as_i64()) else {
            continue;
        };
        if entry.get("block").as_bool().unwrap_or(false) {
            blocked.insert((x, z));
        } else if entry.get("act").as_bool().unwrap_or(false) {
            doors.insert((x, z));
        }
    }
    let mut route = std::collections::HashSet::new();
    for step in state.get("route").as_arr().cloned().unwrap_or_default() {
        if let Some(pair) = step.as_arr() {
            if pair.len() >= 2 {
                if let (Some(x), Some(z)) = (pair[0].as_i64(), pair[1].as_i64()) {
                    route.insert((x, z));
                }
            }
        }
    }
    let target = state.get("route_target").as_arr().cloned().unwrap_or_default();
    let target_tile = if target.len() >= 2 {
        match (target[target.len() - 2].as_i64(), target[target.len() - 1].as_i64()) {
            (Some(x), Some(z)) => Some((x, z)),
            _ => None,
        }
    } else {
        None
    };
    let mut marks = HashMap::new();
    for entry in state.get("objs").as_arr().cloned().unwrap_or_default() {
        if let (Some(x), Some(z)) = (entry.get("x").as_i64(), entry.get("z").as_i64()) {
            marks.entry((x, z)).or_insert('o');
        }
    }
    if let Some(tile) = target_tile {
        marks.entry(tile).or_insert('X');
    }
    for step in &route {
        marks.entry(*step).or_insert('*');
    }
    for tile in &doors {
        marks.entry(*tile).or_insert('D');
    }
    for tile in &blocked {
        marks.entry(*tile).or_insert('#');
    }
    for entry in state.get("players").as_arr().cloned().unwrap_or_default() {
        if let (Some(x), Some(z)) = (entry.get("x").as_i64(), entry.get("z").as_i64()) {
            marks.insert((x, z), 'P');
        }
    }
    for entry in state.get("npcs").as_arr().cloned().unwrap_or_default() {
        if let (Some(x), Some(z)) = (entry.get("x").as_i64(), entry.get("z").as_i64()) {
            marks.insert((x, z), 'N');
        }
    }
    marks.insert(here, '@');
    let mut rows = vec![format!(
        "map @=({},{}) N=npc P=player *=route X=target o=obj D=door #=wall",
        here.0, here.1
    )];
    for dz in (-radius..=radius).rev() {
        let mut row = String::new();
        for dx in -radius..=radius {
            row.push(*marks.get(&(here.0 + dx, here.1 + dz)).unwrap_or(&'.'));
        }
        rows.push(row);
    }
    rows
}

/// UTC clock: std has no localtime (the dashboard shows UTC too).
fn clock_hhmmss(epoch_secs: f64) -> String {
    let secs = epoch_secs as u64;
    format!("{:02}:{:02}:{:02}", (secs / 3600) % 24, (secs / 60) % 60, secs % 60)
}

fn label(entry: &Value, key: &str) -> String {
    entry
        .get(key)
        .as_str()
        .map(str::to_string)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| val_str(entry.get("id")))
}

fn format_seen(state: &Value) -> String {
    if state.as_obj().is_none() {
        return "no state".to_string();
    }
    let pos = state.get("position").as_arr().cloned().unwrap_or_default();
    let here = (
        pos.first().and_then(|v| v.as_i64()).unwrap_or(0),
        pos.get(1).and_then(|v| v.as_i64()).unwrap_or(0),
        pos.get(2).and_then(|v| v.as_i64()).unwrap_or(0),
    );
    let dist = |entry: &Value| {
        (entry.get("x").as_i64().unwrap_or(here.1) - here.1).abs()
            + (entry.get("z").as_i64().unwrap_or(here.2) - here.2).abs()
    };
    let mut lines = vec![
        format!(
            "{} hp={} pos=({},{},{}) {} :: {}",
            state.get("account").as_str().unwrap_or("?"),
            val_str(state.get("hitpoints")),
            here.0,
            here.1,
            here.2,
            state.get("status").as_str().unwrap_or("?"),
            state.get("intention").as_str().unwrap_or("-"),
        ),
        format!(
            "zone {} ({})",
            state.get("zone").as_str().unwrap_or("-"),
            state.get("zone_dist").as_str().unwrap_or("-"),
        ),
    ];
    if !state.get("ticks").is_null() || !state.get("updated").is_null() {
        let mut stamp = String::new();
        if let Some(updated) = state.get("updated").as_f64() {
            if updated > 0.0 {
                stamp = format!(" @ {}", clock_hhmmss(updated));
            }
        }
        lines.push(format!("tick {}{}", val_str(state.get("ticks")), stamp));
    }
    lines.extend(format_ascii(state, 12));
    let mut npcs = state.get("npcs").as_arr().cloned().unwrap_or_default();
    npcs.sort_by_key(|entry| dist(entry));
    for entry in &npcs {
        let name = entry
            .get("name")
            .as_str()
            .map(str::to_string)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| val_str(entry.get("type")));
        lines.push(format!(
            "npc  {:<24} ({},{}) d={}",
            name,
            val_str(entry.get("x")),
            val_str(entry.get("z")),
            dist(entry),
        ));
    }
    let mut locs = state.get("locs").as_arr().cloned().unwrap_or_default();
    locs.sort_by_key(|entry| dist(entry));
    for entry in &locs {
        let mut flags = String::new();
        if entry.get("act").as_bool().unwrap_or(false) {
            flags.push_str("act");
        }
        if entry.get("block").as_bool().unwrap_or(false) {
            flags.push_str("block");
        }
        lines.push(format!(
            "loc  {:<24} ({},{}) d={} {}",
            label(entry, "name"),
            val_str(entry.get("x")),
            val_str(entry.get("z")),
            dist(entry),
            flags,
        ));
    }
    let mut objs = state.get("objs").as_arr().cloned().unwrap_or_default();
    objs.sort_by_key(|entry| dist(entry));
    for entry in &objs {
        lines.push(format!(
            "obj  {:<24} x{:<3} ({},{}) d={}",
            label(entry, "name"),
            val_str(entry.get("count")),
            val_str(entry.get("x")),
            val_str(entry.get("z")),
            dist(entry),
        ));
    }
    let mut players = state.get("players").as_arr().cloned().unwrap_or_default();
    players.sort_by_key(|entry| dist(entry));
    for entry in &players {
        lines.push(format!(
            "player {:<22} ({},{}) d={}",
            entry.get("name").as_str().unwrap_or("?"),
            val_str(entry.get("x")),
            val_str(entry.get("z")),
            dist(entry),
        ));
    }
    let inv = state.get("inv").as_arr().cloned().unwrap_or_default();
    let carried: i64 = inv.iter().filter_map(|entry| entry.get("count").as_i64()).sum();
    lines.push(format!("Inventory ({carried})"));
    for entry in &inv {
        lines.push(format!("inv  {:<24} x{}", label(entry, "name"), val_str(entry.get("count")),));
    }
    for entry in state.get("worn").as_arr().cloned().unwrap_or_default() {
        lines.push(format!("worn {:<24} x{}", label(&entry, "name"), val_str(entry.get("count")),));
    }
    if !state.get("modal").is_null() || state.get("modal_name").as_str().is_some() {
        lines.push(format!(
            "modal {} {}",
            state.get("modal_name").as_str().unwrap_or("-"),
            json::stringify(state.get("modal")),
        ));
    }
    let chat = state.get("chat").as_arr().cloned().unwrap_or_default();
    for line in chat.iter().rev().take(8).rev() {
        let text = json::stringify(line);
        lines.push(format!("chat {}", text.chars().take(160).collect::<String>()));
    }
    let messages = state.get("messages").as_arr().cloned().unwrap_or_default();
    let ticks = state.get("message_ticks").as_arr().cloned().unwrap_or_default();
    let start = messages.len().saturating_sub(6);
    for (index, line) in messages.iter().skip(start).enumerate() {
        let tag = ticks
            .get(start + index)
            .map(|tick| format!("t={}", val_str(tick)))
            .unwrap_or_else(|| "-".to_string());
        let text = json::stringify(line);
        lines.push(format!("msg [{tag}] {}", text.chars().take(160).collect::<String>()));
    }
    if !state.get("route_target").is_null() {
        lines.push(format!("route_target {}", json::stringify(state.get("route_target"))));
    }
    if !state.get("last_event").is_null() {
        lines.push(format!("last_event {}", json::stringify(state.get("last_event"))));
    }
    if !state.get("last_error").is_null() {
        lines.push(format!("last_error {}", json::stringify(state.get("last_error"))));
    }
    lines.join("\n")
}

/// Quote-aware split for the shell: JSON stays whole, else raw strings.
fn split_shell(line: &str) -> Result<Vec<String>, String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut in_word = false;
    let mut chars = line.chars();
    while let Some(ch) = chars.next() {
        if let Some(open) = quote {
            if ch == open {
                quote = None;
            } else if ch == '\\' {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            } else {
                current.push(ch);
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
            in_word = true;
        } else if ch.is_whitespace() {
            if in_word {
                parts.push(std::mem::take(&mut current));
                in_word = false;
            }
        } else if ch == '\\' {
            if let Some(next) = chars.next() {
                current.push(next);
                in_word = true;
            }
        } else {
            current.push(ch);
            in_word = true;
        }
    }
    if quote.is_some() {
        return Err("unterminated quote".to_string());
    }
    if in_word {
        parts.push(current);
    }
    Ok(parts)
}

fn shell(account: &str) -> i32 {
    let reply = match send_command(
        &object(vec![("cmd", str_value("methods")), ("account", str_value(account))]),
        Duration::from_secs(10),
    ) {
        Ok(reply) => reply,
        Err(error) => {
            eprintln!("session unreachable: {error}");
            return 1;
        }
    };
    if reply.get("ok").as_bool() != Some(true) {
        eprintln!(
            "{}",
            reply.get("error").as_str().unwrap_or(&format!("no session, open {account} first"))
        );
        return 1;
    }
    let reads: Vec<String> = reply
        .get("reads")
        .as_arr()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    let writes: Vec<String> = reply
        .get("writes")
        .as_arr()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    println!(
        "session {account}: {} reads (snapshot) + {} writes (safe-point). help lists them.",
        reads.len(),
        writes.len()
    );
    let stdin = std::io::stdin();
    loop {
        print!("{account}> ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        let parts = match split_shell(line.trim()) {
            Ok(parts) => parts,
            Err(error) => {
                println!("parse error: {error}");
                continue;
            }
        };
        if parts.is_empty() {
            continue;
        }
        if parts[0] == "exit" {
            break;
        }
        if parts[0] == "help" {
            println!("reads:  {}", reads.join(" "));
            println!("writes: {}", writes.join(" "));
            println!("also:   unload, exit");
            continue;
        }
        if parts[0] == "unload" {
            let reply = send_command(
                &object(vec![("cmd", str_value("unload")), ("account", str_value(account))]),
                Duration::from_secs(WORKER_TIMEOUT),
            )
            .unwrap_or_else(|error| object(vec![("ok", Value::Bool(false)), ("error", str_value(&error))]));
            println!("{}", json::stringify_pretty(&reply));
            continue;
        }
        let args: Vec<Value> = parts[1..].iter().map(|arg| str_value(arg)).collect();
        let reply = send_command(
            &object(vec![
                ("cmd", str_value("call")),
                ("account", str_value(account)),
                ("method", str_value(&parts[0])),
                ("args", Value::Arr(args)),
            ]),
            Duration::from_secs(WORKER_TIMEOUT),
        )
        .unwrap_or_else(|error| object(vec![("ok", Value::Bool(false)), ("error", str_value(&error))]));
        println!("{}", json::stringify_pretty(&reply));
        let Ok(state) = send_command(
            &object(vec![("cmd", str_value("show")), ("account", str_value(account))]),
            Duration::from_secs(10),
        ) else {
            continue;
        };
        let state = state.get("state");
        println!(
            "{} hp={} pos={} :: {}",
            state.get("status").as_str().unwrap_or("?"),
            val_str(state.get("hitpoints")),
            json::stringify(state.get("position")),
            state.get("intention").as_str().unwrap_or("-"),
        );
    }
    0
}

/// True when a session worker answers on the account socket. A plain
/// connect is enough: a stale socket file refuses, a live worker
/// accepts. Split by path so tests bind a temp socket, never run/.
fn session_worker_alive(account: &str) -> bool {
    let (sock_path, _) = crate::paths::session_paths(account);
    worker_socket_alive(&sock_path)
}

fn worker_socket_alive(sock_path: &std::path::Path) -> bool {
    UnixStream::connect(sock_path).is_ok()
}

/// Direct run without daemon nor session worker: login, run one script,
/// logout. Same Bot setup as a session for the local part (live tables,
/// map index) plus the daemon plan handle when the pathfinding service
/// answers: without it every `travel_to` is `nav-down` and the run
/// refuses to start rather than silently diverging on straight legs.
/// Nothing is published (no heartbeat, no logs, no spectate). For fast
/// live tests only. Refuses when a session worker is alive on the
/// account: two live logins fight over the name ("already logged in"
/// storms).
fn run_direct(args: &[String]) -> i32 {
    if args.len() < 2 {
        eprintln!("usage: orchestrator direct <account> <script> [args...]");
        return 2;
    }
    let account = &args[0];
    if session_worker_alive(account) {
        eprintln!("direct: session worker alive on {account}; unload it first");
        return 2;
    }
    if UnixStream::connect(crate::plan_service::socket_path()).is_err() {
        eprintln!(
            "direct: pathfinding service down (no {}); start the daemon first — travel needs it, no local plan",
            crate::plan_service::socket_path().display()
        );
        return 1;
    }
    let script = &args[1];
    let script_args = args[2..].to_vec();
    if !lifescape_scripts::names().iter().any(|name| name == script) {
        eprintln!("direct: unknown script {script:?}");
        return 2;
    }
    let engine = std::env::var("LIFESCAPE_ENGINE_DIR").ok().filter(|dir| !dir.is_empty()).unwrap_or_else(|| {
        let sibling = crate::paths::root().join("../lostcity/engine");
        if sibling.is_dir() {
            sibling.to_string_lossy().to_string()
        } else {
            "../lostcity/engine".to_string()
        }
    });
    let host = std::env::var("LIFESCAPE_SERVER_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port: u16 = std::env::var("LIFESCAPE_SERVER_PORT").ok().and_then(|raw| raw.parse().ok()).unwrap_or(43594);
    let mut bot = lifescape_sdk::Bot::new(&host, port, &format!("{engine}/data/pack"), &format!("{engine}/data/config/private.pem"));
    lifescape_scripts::tables_live::attach_live_tables(&mut bot);
    match crate::staticmap::build_pack(&engine) {
        Ok(pack) => bot.maps = Some(pack),
        Err(error) => eprintln!("direct: static locs missing ({error}); straight legs only"),
    }
    {
        let data_dir = crate::paths::root().join("gamedata").join("data");
        let engine_path = std::path::Path::new(&engine);
        let zip = engine_path.join("data").join("pack").join(".cache").join("maps-server.zip");
        let f2p = engine_path.join("../content").join("maps").join("free2play.csv");
        bot.plan_client =
            Some(lifescape_nav::service::PlanClient::new(&crate::plan_service::socket_path(), account));
        bot.plan_data_version = Some(crate::plan_service::version_for(&data_dir, &zip, &f2p));
    }
    if let Err(error) = bot.login(account, None) {
        eprintln!("direct: login {account} failed: {error}");
        return 1;
    }
    println!("direct: {account} logged in at {:?} t={}", bot.position(), bot.world.ticks);
    let code = match lifescape_scripts::run(script, &mut bot, &script_args) {
        Ok(code) => {
            println!("direct: {script} exit {code} at {:?} t={}", bot.position(), bot.world.ticks);
            code
        }
        Err(error) => {
            eprintln!("direct: {script} failed at {:?} t={}: {error}", bot.position(), bot.world.ticks);
            1
        }
    };
    bot.close();
    code
}

pub fn cli(argv: &[String]) -> i32 {
    if argv.len() < 2 {
        println!("usage: orchestrator open|run|call|shell|status|show|see|logs|create-user|unload|pause|resume|hold|close|shutdown|reload-nav|nav-stats|res-stats|bench-l2r|daemon|direct|map|route|replay-nav|dump-transitions ...");
        return 2;
    }
    if argv[1].as_str() == "dump-transitions" {
        let Some(output) = argv.get(2) else {
            eprintln!("usage: orchestrator dump-transitions <sortie>");
            return 2;
        };
        return match crate::dump_transitions::dump_transitions(std::path::Path::new(output)) {
            Ok(_) => 0,
            Err(error) => {
                eprintln!("dump-transitions: {error}");
                1
            }
        };
    }
    if argv[1].as_str() == "map" {
        return crate::mapcli::map_cli(&argv[2..].to_vec());
    }
    if argv[1].as_str() == "route" {
        return crate::routecmd::route_cmd(&argv[2..].to_vec());
    }
    if argv[1].as_str() == "replay-nav" {
        return crate::replaynav::replay_nav_cmd(&argv[2..].to_vec());
    }
    if argv[1].as_str() == "direct" {
        return run_direct(&argv[2..].to_vec());
    }
    if !ensure_daemon() {
        eprintln!("orchestrator failed to start");
        return 1;
    }
    let command = argv[1].as_str();
    let account = argv.get(2).cloned().unwrap_or_default();
    let send = |request: Value| {
        send_command(&request, Duration::from_secs(WORKER_TIMEOUT)).unwrap_or_else(|error| {
            object(vec![("ok", Value::Bool(false)), ("error", str_value(&error))])
        })
    };
    match command {
        "create-user" => {
            let mut iron = false;
            let mut no_tutorial = false;
            let mut goal: Option<String> = None;
            // argv[2] already holds the account (or the first flag when
            // the pseudo is generated).
            let explicit = !account.starts_with("--") && !account.is_empty();
            let mut index = if explicit { 3 } else { 2 };
            while index < argv.len() {
                match argv[index].as_str() {
                    "--iron" => {
                        iron = true;
                    }
                    "--no-tutorial" => {
                        no_tutorial = true;
                    }
                    "--goal" => {
                        goal = argv.get(index + 1).cloned();
                        index += 1;
                    }
                    other => {
                        eprintln!("create-user: unknown flag {other:?} (usage: create-user [pseudo] [--iron] [--goal kind] [--no-tutorial])");
                        return 2;
                    }
                }
                index += 1;
            }
            let name = match crate::brain::create_user(if explicit { Some(account) } else { None }, iron, goal) {
                Ok(name) => name,
                Err(error) => {
                    eprintln!("create-user: {error}");
                    return 1;
                }
            };
            println!("created {name}");
            if no_tutorial {
                return 0;
            }
            let reply = send(object(vec![("cmd", str_value("open")), ("account", str_value(&name))]));
            println!("{}", json::stringify_pretty(&reply));
            if reply.get("ok").as_bool() != Some(true) {
                return 1;
            }
            let run = send(object(vec![
                ("cmd", str_value("run")),
                ("account", str_value(&name)),
                ("script", str_value("tutorial")),
                ("args", Value::Arr(Vec::new())),
            ]));
            println!("{}", json::stringify_pretty(&run));
            if run.get("ok").as_bool() == Some(true) { 0 } else { 1 }
        }
        "open" => {
            let reply = send(object(vec![("cmd", str_value("open")), ("account", str_value(&account))]));
            println!("{}", json::stringify_pretty(&reply));
            if reply.get("ok").as_bool() == Some(true) { 0 } else { 1 }
        }
        "close" => {
            let reply = send(object(vec![("cmd", str_value("close")), ("account", str_value(&account))]));
            println!("{}", json::stringify_pretty(&reply));
            if reply.get("ok").as_bool() == Some(true) { 0 } else { 1 }
        }
        "run" => {
            let script = argv.get(3).cloned().unwrap_or_default();
            let args = Value::Arr(argv.get(4..).unwrap_or(&[]).iter().map(|a| str_value(a)).collect());
            let reply = send(object(vec![
                ("cmd", str_value("run")),
                ("account", str_value(&account)),
                ("script", str_value(&script)),
                ("args", args),
            ]));
            println!("{}", json::stringify_pretty(&reply));
            if reply.get("ok").as_bool() == Some(true) { 0 } else { 1 }
        }
        "unload" | "pause" | "resume" | "hold" => {
            let reply = send(object(vec![("cmd", str_value(command)), ("account", str_value(&account))]));
            println!("{}", json::stringify_pretty(&reply));
            if reply.get("ok").as_bool() == Some(true) { 0 } else { 1 }
        }
        "call" => {
            let method = argv.get(3).cloned().unwrap_or_default();
            let args = Value::Arr(argv.get(4..).unwrap_or(&[]).iter().map(|a| str_value(a)).collect());
            let reply = send(object(vec![
                ("cmd", str_value("call")),
                ("account", str_value(&account)),
                ("method", str_value(&method)),
                ("args", args),
            ]));
            println!("{}", json::stringify_pretty(&reply));
            if reply.get("ok").as_bool() == Some(true) { 0 } else { 1 }
        }
        "shell" => shell(&account),
        "status" => {
            let reply = send(object(vec![("cmd", str_value("status")), ("account", str_value(&account))]));
            println!("{}", format_status(reply.get("jobs")));
            if reply.get("ok").as_bool() == Some(true) { 0 } else { 1 }
        }
        "show" => {
            let reply = send(object(vec![("cmd", str_value("show")), ("account", str_value(&account))]));
            println!("{}", json::stringify_pretty(reply.get("state")));
            0
        }
        "see" => {
            let reply = send(object(vec![("cmd", str_value("show")), ("account", str_value(&account))]));
            println!("{}", format_seen(reply.get("state")));
            0
        }
        "logs" => {
            let lines = argv.get(3).and_then(|raw| raw.parse::<i64>().ok()).unwrap_or(40);
            let reply = send(object(vec![
                ("cmd", str_value("logs")),
                ("account", str_value(&account)),
                ("lines", Value::Int(lines)),
            ]));
            for line in reply.get("lines").as_arr().cloned().unwrap_or_default() {
                println!("{}", line.as_str().unwrap_or(""));
            }
            0
        }
        "shutdown" => {
            // Fans out over every session (5s close + 8s kill grace each):
            // needs headroom past the per-command worker timeout.
            let reply = send_command(&object(vec![("cmd", str_value("shutdown"))]), Duration::from_secs(100))
                .unwrap_or_else(|error| object(vec![("ok", Value::Bool(false)), ("error", str_value(&error))]));
            println!("{}", json::stringify_pretty(&reply));
            if reply.get("ok").as_bool() == Some(true) { 0 } else { 1 }
        }
        "reload-nav" => {
            let reply = send(object(vec![("cmd", str_value("reload-nav"))]));
            println!("{}", json::stringify_pretty(&reply));
            if reply.get("ok").as_bool() == Some(true) { 0 } else { 1 }
        }
        "nav-stats" => {
            let reply = send(object(vec![("cmd", str_value("nav-stats"))]));
            if reply.get("ok").as_bool() == Some(true) {
                println!("{}", format_nav_stats(&reply));
            } else {
                println!("{}", json::stringify_pretty(&reply));
            }
            if reply.get("ok").as_bool() == Some(true) { 0 } else { 1 }
        }
        "res-stats" => {
            let reply = send(object(vec![("cmd", str_value("res-stats"))]));
            if reply.get("ok").as_bool() == Some(true) {
                println!("{}", format_res_stats(&reply));
            } else {
                println!("{}", json::stringify_pretty(&reply));
            }
            if reply.get("ok").as_bool() == Some(true) { 0 } else { 1 }
        }
        "bench-l2r" => {
            let workers = argv.get(2).and_then(|raw| raw.parse::<usize>().ok()).unwrap_or(300);
            let secs = argv.get(3).and_then(|raw| raw.parse::<u64>().ok()).unwrap_or(15);
            let config = crate::bench_offline::BenchConfig { workers, secs, legs: 12, pace_ms: 700 };
            match crate::bench_offline::run_bench(&config) {
                Ok(report) => {
                    println!("{}", report.summary());
                    0
                }
                Err(error) => {
                    eprintln!("bench-l2r: {error}");
                    1
                }
            }
        }
        _ => {
            eprintln!("unknown command '{command}'");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::worker_socket_alive;
    use std::os::unix::net::UnixListener;

    fn temp_sock(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("lifescape-test-{}-{name}", std::process::id()))
    }

    /// Spectate header: total carried items in parentheses, then one
    /// line per stack (`Inventory (28)` + `inv ... x27` + `inv ... x1`).
    #[test]
    fn inventory_header_counts_items() {
        use super::format_seen;
        use lifescape_json::{object, str_value, Value};
        let state = object(vec![
            ("account", str_value("t01")),
            ("hitpoints", Value::Int(19)),
            ("position", Value::Arr(vec![Value::Int(0), Value::Int(1), Value::Int(2)])),
            ("status", str_value("running")),
            ("intention", str_value("-")),
            ("zone", str_value("-")),
            ("zone_dist", str_value("-")),
            (
                "inv",
                Value::Arr(vec![
                    object(vec![("name", str_value("Yew logs")), ("count", Value::Int(27))]),
                    object(vec![("name", str_value("Steel axe")), ("count", Value::Int(1))]),
                ]),
            ),
        ]);
        let seen = format_seen(&state);
        assert!(seen.lines().any(|line| line == "Inventory (28)"), "header sums stacks, got:\n{seen}");
        assert_eq!(seen.lines().filter(|line| line.starts_with("inv ")).count(), 2, "one line per stack");
        let empty = object(vec![
            ("account", str_value("t01")),
            ("position", Value::Arr(vec![Value::Int(0), Value::Int(1), Value::Int(2)])),
        ]);
        assert!(
            format_seen(&empty).lines().any(|line| line == "Inventory (0)"),
            "empty pack still headers",
        );
    }

    /// Single-session guard: a bound socket reads alive, a missing or
    /// stale path reads dead — `direct` refuses only the live worker.
    #[test]
    fn worker_socket_guard() {
        let missing = temp_sock("missing.sock");
        let _ = std::fs::remove_file(&missing);
        assert!(!worker_socket_alive(&missing), "no socket means no worker");

        let stale = temp_sock("stale.sock");
        std::fs::write(&stale, b"stale").unwrap();
        assert!(!worker_socket_alive(&stale), "stale file is not a worker");
        std::fs::remove_file(&stale).unwrap();

        let live = temp_sock("live.sock");
        let _ = std::fs::remove_file(&live);
        let _listener = UnixListener::bind(&live).unwrap();
        assert!(worker_socket_alive(&live), "bound socket means a live worker");
    }
}
