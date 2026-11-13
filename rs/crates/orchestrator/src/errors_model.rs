#![forbid(unsafe_code)]
//! Error rows for the errors page, built from each source's own shape
//! (pure: no IO, zone lookup injected). Sources: `task` (script error
//! events and failed/blocked task reports), `nav` (failure dossiers),
//! `deal` (failed market deals), `brain` (abandons, stuck graphs, bans,
//! read from the daemon's own log lines).
//!
//! `signature` groups similar errors: numbers, coordinates, bracketed
//! lists, the account name and the trailing game chat are stripped.

use lifescape_json::Value;
use lifescape_store::{ErrorRecord, FailedDeal, TaskOutcome};

/// Raw message cap (the dossier keeps the full text).
pub const MESSAGE_CAP: usize = 300;
/// Signature cap: long enough to tell causes apart, short enough to group.
pub const SIGNATURE_CAP: usize = 160;
/// Report folded onto a script error row seen at most this long before.
pub const FOLD_WINDOW_SECS: i64 = 120;

/// Zone name for a tile (x, z); empty when unnamed.
pub type ZoneOf<'a> = &'a dyn Fn(i64, i64) -> String;

/// Production zone lookup over the shipped world map labels.
pub fn map_zone(pos_x: i64, pos_z: i64) -> String {
    crate::zones::zone_name(pos_x as i32, pos_z as i32).map(|(name, _)| name).unwrap_or_default()
}

/// Cut `text` to at most `cap` bytes on a char boundary, with an ellipsis.
pub fn clip(text: &str, cap: usize) -> String {
    if text.len() <= cap {
        return text.to_string();
    }
    let mut end = cap;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Grouping key for an error message: game chat tail dropped
/// (`; last game: …`), the account name replaced by `<acct>`, bracketed
/// lists collapsed to `[…]`, every number to `#` (`(3200, 3201, 0)` and
/// `(3200,3201,0)` both read `(#,#,#)`), whitespace collapsed, capped.
pub fn signature(message: &str, account: &str) -> String {
    let mut text = match message.find("; last game:") {
        Some(cut) => message[..cut].to_string(),
        None => message.to_string(),
    };
    if account.len() >= 3 {
        text = text.replace(account, "<acct>");
    }
    let mut out = String::with_capacity(text.len());
    let mut depth = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(current) = chars.next() {
        if current == '[' {
            if depth == 0 {
                out.push_str("[…]");
            }
            depth += 1;
            continue;
        }
        if depth > 0 {
            if current == ']' {
                depth -= 1;
            }
            continue;
        }
        let negative = current == '-' && chars.peek().is_some_and(char::is_ascii_digit);
        if current.is_ascii_digit() || negative {
            let mut run = String::new();
            if current.is_ascii_digit() {
                run.push(current);
            }
            while let Some(next) = chars.peek().copied().filter(|next| next.is_ascii_digit() || *next == '.') {
                run.push(next);
                chars.next();
            }
            // Hex literal (`0x2a0`): one number, not `#x#a#`.
            if run == "0" && chars.peek() == Some(&'x') {
                chars.next();
                while chars.peek().is_some_and(char::is_ascii_hexdigit) {
                    chars.next();
                }
            }
            if out.ends_with("#, ") {
                out.pop();
            }
            out.push('#');
            continue;
        }
        if current.is_whitespace() {
            if !out.ends_with(' ') && !out.is_empty() {
                out.push(' ');
            }
            continue;
        }
        out.push(current);
    }
    clip(out.trim_end(), SIGNATURE_CAP)
}

/// Script error type from its display text (`ScriptError` prefixes).
pub fn script_kind(message: &str) -> &'static str {
    if message.starts_with("failed:") {
        "Failed"
    } else if message.starts_with("usage:") {
        "Usage"
    } else if message.starts_with("blocked") {
        "Blocked"
    } else {
        "Sdk"
    }
}

/// Node id from task args (`--node n36`), empty when absent.
pub fn node_of_args(args: &[String]) -> String {
    args.iter()
        .position(|arg| arg == "--node")
        .and_then(|index| args.get(index + 1))
        .cloned()
        .unwrap_or_default()
}

/// `[level, x, z]` (worker frames) to (x, z, level).
pub fn position_of(at: &Value) -> Option<(i64, i64, i64)> {
    let parts = at.as_arr()?;
    if parts.len() != 3 {
        return None;
    }
    Some((parts[1].as_i64()?, parts[2].as_i64()?, parts[0].as_i64()?))
}

fn place(row: &mut ErrorRecord, position: Option<(i64, i64, i64)>, zone_of: ZoneOf<'_>) {
    row.position = position;
    row.zone = position.map(|(pos_x, pos_z, _)| zone_of(pos_x, pos_z)).unwrap_or_default();
}

/// Row for a pushed `event` frame; only `script_error` events count.
pub fn from_event(account: &str, frame: &Value, clock: i64, zone_of: ZoneOf<'_>) -> Option<ErrorRecord> {
    if frame.get("kind").as_str() != Some("script_error") {
        return None;
    }
    let message = frame.get("message").as_str().unwrap_or("").to_string();
    let args: Vec<String> = frame
        .get("args")
        .as_arr()
        .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let mut row = ErrorRecord {
        clock,
        source: "task".to_string(),
        kind: script_kind(&message).to_string(),
        account: account.to_string(),
        script: frame.get("script").as_str().unwrap_or("").to_string(),
        node_id: node_of_args(&args),
        args: args.join(" "),
        signature: signature(&message, account),
        message: clip(&message, MESSAGE_CAP),
        ..ErrorRecord::default()
    };
    place(&mut row, position_of(frame.get("at")), zone_of);
    Some(row)
}

/// For a failed or blocked task report: the outcome to fold onto the
/// script error row of the same failure, and the row to record when
/// none exists (exit codes and blocks push no event).
pub fn from_report(
    account: &str,
    report: &Value,
    position: Option<(i64, i64, i64)>,
    clock: i64,
    zone_of: ZoneOf<'_>,
) -> Option<(TaskOutcome, ErrorRecord)> {
    let stop = report.get("stop").as_str().unwrap_or("");
    if stop != "failed" && stop != "blocked" {
        return None;
    }
    let text = |key: &str| report.get(key).as_str().unwrap_or("").to_string();
    let activity = text("activity");
    let script = if text("script").is_empty() { activity.clone() } else { text("script") };
    let method = if text("method").is_empty() { activity } else { text("method") };
    let error = report.get("error");
    let missing: Vec<String> = report
        .get("missing")
        .as_arr()
        .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let message = match error.get("message").as_str() {
        Some(found) if !found.is_empty() => found.to_string(),
        _ if !missing.is_empty() => format!("blocked, missing: {}", missing.join(", ")),
        _ => format!("task {stop}"),
    };
    let kind = match error.get("type").as_str() {
        Some(found) if !found.is_empty() => found.to_string(),
        _ if stop == "blocked" => "Blocked".to_string(),
        _ => "failed".to_string(),
    };
    let node_id = text("node_id");
    let outcome = TaskOutcome {
        account: account.to_string(),
        node_id: node_id.clone(),
        method: method.clone(),
        stop: stop.to_string(),
        kind: kind.clone(),
        message: clip(&message, MESSAGE_CAP),
        since: clock - FOLD_WINDOW_SECS,
    };
    let mut row = ErrorRecord {
        clock,
        source: "task".to_string(),
        kind,
        account: account.to_string(),
        script,
        node_id,
        method,
        stop: stop.to_string(),
        signature: signature(&message, account),
        message: clip(&message, MESSAGE_CAP),
        ..ErrorRecord::default()
    };
    if !missing.is_empty() {
        row.detail = format!("missing {}", missing.join(", "));
    }
    place(&mut row, position, zone_of);
    Some((outcome, row))
}

fn tile_of(value: &Value) -> Option<(i64, i64, i64)> {
    Some((value.get("x").as_i64()?, value.get("z").as_i64()?, value.get("level").as_i64().unwrap_or(0)))
}

/// Row for one nav failure dossier (`run/failures/<file>`, written at
/// `mtime`). `from` is the position, the destination goes to `detail`.
pub fn from_dossier(file_name: &str, mtime: i64, dossier: &Value, zone_of: ZoneOf<'_>) -> Option<ErrorRecord> {
    let error = dossier.get("error");
    let kind = error.get("kind").as_str().filter(|kind| !kind.is_empty())?.to_string();
    let account = dossier.get("account").as_str().unwrap_or("").to_string();
    let observed = error.get("observed").as_str().unwrap_or("");
    let message = if observed.is_empty() { error.get("expected").as_str().unwrap_or("") } else { observed };
    let mut row = ErrorRecord {
        clock: mtime,
        source: "nav".to_string(),
        kind,
        signature: signature(message, &account),
        message: clip(message, MESSAGE_CAP),
        account,
        dossier: file_name.to_string(),
        source_ref: Some(format!("nav:{file_name}:{mtime}")),
        ..ErrorRecord::default()
    };
    if let Some((dest_x, dest_z, dest_level)) = tile_of(dossier.get("dest")) {
        let dest_zone = zone_of(dest_x, dest_z);
        row.detail = format!("dest ({dest_x},{dest_z},{dest_level}) {dest_zone}").trim_end().to_string();
    }
    place(&mut row, tile_of(dossier.get("from")), zone_of);
    Some(row)
}

/// Row for one failed deal: buyer as account, seller as peer, meeting
/// point as position; grouped by reason and item.
pub fn from_deal(deal: &FailedDeal, zone_of: ZoneOf<'_>) -> ErrorRecord {
    let reason = if deal.reason.is_empty() { "unknown".to_string() } else { deal.reason.replace(' ', "_") };
    let item = if deal.item_name.is_empty() { format!("item {}", deal.item_id) } else { deal.item_name.clone() };
    let mut row = ErrorRecord {
        clock: deal.clock,
        source: "deal".to_string(),
        kind: reason.clone(),
        account: deal.buyer.clone(),
        peer: deal.seller.clone(),
        script: "market".to_string(),
        signature: format!("deal {reason}: {item}"),
        message: format!(
            "deal {} {}: {} x {item} @ {} gp (buyer {}, seller {})",
            deal.deal_id, deal.reason, deal.quantity, deal.price, deal.buyer, deal.seller
        ),
        detail: format!("{} x {item} @ {} gp", deal.quantity, deal.price),
        source_ref: Some(format!("deal:{}", deal.deal_id)),
        ..ErrorRecord::default()
    };
    place(&mut row, Some(deal.meeting), zone_of);
    row
}

/// Row for a daemon log line from the brain (without the clock prefix):
/// node abandons, stuck graphs and goals, skill bans, revive caps.
/// Position and zone are filled by the caller (the job's live state).
pub fn from_brain_line(line: &str, clock: i64) -> Option<ErrorRecord> {
    let rest = line.strip_prefix("brain ")?;
    let (account, rest) = rest.split_once(": ")?;
    if account.is_empty() || account.contains(' ') {
        return None;
    }
    let mut row = ErrorRecord {
        clock,
        source: "brain".to_string(),
        account: account.to_string(),
        message: clip(rest, MESSAGE_CAP),
        ..ErrorRecord::default()
    };
    if rest.starts_with("graph stuck") {
        row.kind = "graph_stuck".to_string();
        row.signature = "graph stuck, nothing left to run".to_string();
        return Some(row);
    }
    if rest.starts_with("goal stuck") {
        row.kind = "goal_stuck".to_string();
        row.signature = signature(rest, account);
        return Some(row);
    }
    if rest.starts_with("revive cap reached") {
        row.kind = "revive_cap".to_string();
        row.signature = signature(rest, account);
        return Some(row);
    }
    if let Some(skill_part) = rest.strip_prefix("skill ") {
        let (skill, after) = skill_part.split_once(' ')?;
        if !after.starts_with("banned") {
            return None;
        }
        row.kind = "skill_banned".to_string();
        row.script = skill.to_string();
        let cause = after.split_once(": ").map(|(_, cause)| cause).unwrap_or(after);
        row.signature = format!("skill {skill} banned: {}", signature(cause, account));
        return Some(row);
    }
    let marker = " -> abandoned (";
    let cut = rest.find(marker)?;
    let head = &rest[..cut];
    let (node, outcome) = head.split_once(' ')?;
    if !node.starts_with('n') {
        return None;
    }
    let tail = &rest[cut + marker.len()..];
    let cause = tail.split_once(") ").map(|(_, cause)| cause).unwrap_or("");
    let cause = if cause.is_empty() { outcome } else { cause };
    row.kind = "abandoned".to_string();
    row.node_id = node.to_string();
    row.signature = signature(cause, account);
    row.message = clip(cause, MESSAGE_CAP);
    Some(row)
}

/// UTC `YYYY-MM-DD HH:MM:SS` for a unix clock (civil-from-days).
pub fn utc_text(clock: i64) -> String {
    let days = clock.div_euclid(86_400);
    let secs = clock.rem_euclid(86_400);
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60
    )
}

#[cfg(test)]
#[path = "errors_model_tests.rs"]
mod tests;
