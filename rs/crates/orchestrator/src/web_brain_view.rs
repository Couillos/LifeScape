#![forbid(unsafe_code)]
//! What the Lives page shows of the ambition and the tasks, as the brain
//! manages them since the progression plan (A1 to A4, 2026-11-21): how the
//! ambition was given (protected overall, operator pin, A4 rebalance with
//! its value per 6000 ticks, value draw at achievement), the revisions with
//! the retired rules named as such, every milestone with its REAL state
//! (open, waiting, done, set aside with its cooldown), the origin of the
//! current goal, the cooldowns, and the value of time (A1). Pure: the
//! marker, the log line and the investments come in as plain values
//! (`web_brain::brain_response_in` reads them); read-only, nothing here
//! writes a life.

use lifescape_json::{object, str_value, Value};
use lifescape_planner::cooldown::{split_key, COOL_NODE, COOL_TAG};
use lifescape_planner::investment::{pull_per_tick, time_value, Investment};
use lifescape_planner::{ambition_to_value, milestone_cool_key, set_aside, Ambition, HistoryEntry, Life, Milestone, MilestoneStatus, MILESTONE_MAX_BLOCKS};

/// The A4 rebalance row of the account (`run/ambition_rebalance_a4b.json`)
/// and whether the bot already applied it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanInfo {
    pub from: String,
    pub kind: String,
    pub numbers: String,
    pub applied: bool,
    /// When the plan was computed (unix seconds): a revision later than
    /// that is not the plan's.
    pub computed_secs: i64,
}

/// The last « given by value » line the daemon logged for this account.
#[derive(Debug, Clone, PartialEq)]
pub struct LoggedValue {
    pub kind: String,
    pub numbers: String,
    pub at: String,
}

/// What the training line choice of the current goal reads (A1 (a)).
#[derive(Debug, Clone, PartialEq)]
pub struct TrainingInput {
    pub skill: String,
    /// Coins one tick of the bot's work really earns (measured).
    pub own_rate: f64,
    pub pending: Vec<Investment>,
}

/// The figures of a value pick, read back from its text
/// (`Pick::numbers`: « value 9136 per 6000 ticks (kept: ...), 7 bots
/// already on miner, driven by amulet_of_power (climb 0 ticks) »).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ValueNumbers {
    pub value_6000: Option<i64>,
    pub kept: bool,
    pub holders: Option<i64>,
    pub driver: Option<String>,
    pub climb_ticks: Option<i64>,
}

fn digits_after(text: &str, marker: &str) -> Option<i64> {
    let rest = text.split(marker).nth(1)?;
    let digits: String = rest.chars().take_while(|letter| letter.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Parses `Pick::numbers` text; every figure missing stays `None`.
pub fn parse_numbers(text: &str) -> ValueNumbers {
    let holders = text.split(" already on ").next().filter(|_| text.contains(" already on ")).and_then(|before| {
        let before = before.trim_end().trim_end_matches("bots").trim_end_matches("bot").trim_end();
        let digits: String = before.chars().rev().take_while(|letter| letter.is_ascii_digit()).collect::<String>().chars().rev().collect();
        digits.parse().ok()
    });
    let driver = text.split("driven by ").nth(1).and_then(|rest| rest.split([' ', ',', ')']).next()).filter(|name| !name.is_empty()).map(str::to_string);
    ValueNumbers { value_6000: digits_after(text, "value "), kept: text.contains("(kept"), holders, driver, climb_ticks: digits_after(text, "climb ") }
}

/// Last `brain <account>: ambition <kind> given by value (<numbers>)` line
/// in the tail of the daemon log (`log`), with its clock.
pub fn logged_value_in(log: &str, account: &str, kind: &str) -> Option<LoggedValue> {
    let needle = format!("brain {account}: ambition {kind} given by value (");
    let line = log.lines().rev().find(|line| line.contains(&needle))?;
    let numbers = line.split(&needle).nth(1)?.trim_end().strip_suffix(')')?.to_string();
    let at = line.strip_prefix('[').and_then(|rest| rest.split(']').next()).unwrap_or("").to_string();
    Some(LoggedValue { kind: kind.to_string(), numbers, at })
}

/// Longest stretch of the log looked at, bytes from the end.
const LOG_TAIL_BYTES: u64 = 4_000_000;
/// Block of the backward read, and the overlap that keeps a line cut at a
/// block edge whole in the next one.
const LOG_BLOCK_BYTES: u64 = 256 * 1024;
const LOG_OVERLAP_BYTES: u64 = 4096;
/// Seconds a lookup is reused (the log grows every second; the line of an
/// ambition does not change once written).
const LOG_REUSE_SECS: u64 = 30;

/// The log read backward by blocks (`block` bytes) from its end, within
/// `LOG_TAIL_BYTES`, stopping at the first block that holds the line.
pub fn scan_log_backwards(path: &std::path::Path, account: &str, kind: &str, block: u64) -> Option<LoggedValue> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let mut end = length;
    loop {
        let start = end.saturating_sub(block);
        file.seek(SeekFrom::Start(start)).ok()?;
        let mut bytes = vec![0u8; (end - start) as usize];
        file.read_exact(&mut bytes).ok()?;
        if let Some(found) = logged_value_in(&String::from_utf8_lossy(&bytes), account, kind) {
            return Some(found);
        }
        if start == 0 || length - start >= LOG_TAIL_BYTES {
            return None;
        }
        end = start + LOG_OVERLAP_BYTES.min(block / 2).min(end - start);
    }
}

type LogCache = std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, Option<LoggedValue>)>>;

/// `scan_log_backwards` on `orchestrator.log`, reused `LOG_REUSE_SECS` per
/// (log, account, kind): a poll never rereads the log.
pub fn logged_value(run_dir: &std::path::Path, account: &str, kind: &str) -> Option<LoggedValue> {
    static CACHE: std::sync::OnceLock<LogCache> = std::sync::OnceLock::new();
    let path = run_dir.join("orchestrator.log");
    let key = format!("{}|{account}|{kind}", path.display());
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    if let Some((read_at, found)) = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&key) {
        if read_at.elapsed().as_secs() < LOG_REUSE_SECS {
            return found.clone();
        }
    }
    let found = scan_log_backwards(&path, account, kind, LOG_BLOCK_BYTES);
    cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(key, (std::time::Instant::now(), found.clone()));
    found
}

/// True when the ambition's figures are worth a look in the log: it was
/// chosen by a value draw (not the rebalance marker, a pin, a protected
/// account or an old draw).
pub fn is_value_draw(account: &str, ambition: &Ambition, plan: Option<&PlanInfo>) -> bool {
    given_view(account, ambition, plan, None).get("source").as_str() == Some("a4_draw")
}

/// A cooldown active now: its key (`method@x,z,level`, `method`,
/// `milestone:<kind>/<id>`), the ticks left and the cause.
#[derive(Debug, Clone, PartialEq)]
pub struct Cool {
    pub key: String,
    pub ticks_left: u64,
    pub cause: String,
}

/// Cooldowns active now, the longest of a key winning, sorted by key.
pub fn cooling(history: &[HistoryEntry], ticks_since: &dyn Fn(i64) -> u64) -> Vec<Cool> {
    let mut out: Vec<Cool> = Vec::new();
    for entry in history.iter().filter(|entry| entry.node == COOL_NODE) {
        let Some(rest) = entry.outcome.strip_prefix(COOL_TAG) else { continue };
        let mut parts = rest.splitn(3, '|');
        let (Some(key), Some(raw_ticks)) = (parts.next(), parts.next()) else { continue };
        let ticks = raw_ticks.parse::<u64>().unwrap_or(0);
        let left = ticks.saturating_sub(ticks_since(entry.ended));
        if key.is_empty() || left == 0 {
            continue;
        }
        let cause = parts.next().unwrap_or("").to_string();
        match out.iter_mut().find(|known| known.key == key) {
            Some(known) if known.ticks_left >= left => {}
            Some(known) => *known = Cool { key: key.to_string(), ticks_left: left, cause },
            None => out.push(Cool { key: key.to_string(), ticks_left: left, cause }),
        }
    }
    out.sort_by(|left, right| left.key.cmp(&right.key));
    out
}

/// Cooldowns that are not a milestone's (a method at a site, a method):
/// the sites and methods the brain avoids for this bot.
pub fn cooling_view(cools: &[Cool]) -> Value {
    let rows = cools
        .iter()
        .filter(|cool| !cool.key.starts_with("milestone:"))
        .map(|cool| {
            let (method, site) = split_key(&cool.key);
            object(vec![
                ("key", str_value(&cool.key)),
                ("method", str_value(method)),
                ("site", site.map(|(x, z, level)| str_value(&format!("{x},{z},{level}"))).unwrap_or(Value::Null)),
                ("ticks_left", Value::Int(cool.ticks_left as i64)),
                ("cause", str_value(&cool.cause)),
            ])
        })
        .collect();
    Value::Arr(rows)
}

/// Label of a revision `why` and whether the rule behind it is retired.
pub fn revision_text(why: &str) -> (String, bool) {
    let rule = |text: &str| (text.to_string(), true);
    match why {
        "rebalance" => ("rééquilibrage du 21/11 : le métier dont la demande non servie rapporte le plus".to_string(), false),
        "achieved" => ("ambition atteinte : nouvelle ambition choisie par la valeur du marché".to_string(), false),
        "operator:natural" => ("opérateur : retour à l'équilibre naturel (valeur du marché)".to_string(), false),
        "protected:overall" => ("compte protégé : overall pour de bon".to_string(), false),
        "market maker retired" => ("teneur de marché retiré : le métier le plus demandé".to_string(), false),
        _ if why.starts_with("operator:") => (format!("donnée par l'opérateur ({})", &why["operator:".len()..]), false),
        _ if why.starts_with("blocked:") => rule("un jalon bloqué 3 fois remplaçait l'ambition"),
        _ if why.starts_with("change:") => rule("changement facultatif d'ambition"),
        _ if why.starts_with("repair:") => rule("réparation de couverture"),
        _ if why.starts_with("review:") => rule("revue unique du besoin"),
        _ => (why.to_string(), false),
    }
}

fn numbers_view(text: &str) -> Value {
    let figures = parse_numbers(text);
    object(vec![
        ("text", str_value(text)),
        ("value_6000", figures.value_6000.map(Value::Int).unwrap_or(Value::Null)),
        ("kept", Value::Bool(figures.kept)),
        ("holders", figures.holders.map(Value::Int).unwrap_or(Value::Null)),
        ("driver", figures.driver.as_deref().map(str_value).unwrap_or(Value::Null)),
        ("climb_ticks", figures.climb_ticks.map(Value::Int).unwrap_or(Value::Null)),
    ])
}

/// How the ambition was given: `source` (protected, pin, operator,
/// a4_rebalance, a4_kept, a4_draw, legacy), a French sentence and, for a
/// value pick, its figures.
pub fn given_view(account: &str, ambition: &Ambition, plan: Option<&PlanInfo>, logged: Option<&LoggedValue>) -> Value {
    let last = ambition.revisions.last().map(|revision| revision.why.as_str());
    let value_item = ambition.why.strip_prefix("value:").unwrap_or("");
    let by_item = if value_item.is_empty() { String::new() } else { format!(", portée par {value_item}") };
    let (source, text, numbers): (&str, String, Option<Value>) = if lifescape_planner::protected(account) && ambition.kind == lifescape_planner::OVERALL {
        ("protected", "Toujours premier au classement overall (total des niveaux puis XP). Jamais tirée, jamais remplacée.".to_string(), None)
    } else if let Some(pin) = &ambition.pin {
        ("pin", format!("{} niveau {} : elle reste telle quelle jusqu'à ce niveau.", pin.skill, pin.level), None)
    } else if ambition.kind == lifescape_planner::ALCHER || ambition.kind == lifescape_planner::MARKET_MAKER_KIND || ambition.draw_rule == lifescape_planner::OPERATOR_RULE {
        ("operator", "Tenue par l'opérateur, hors équilibre naturel (commande « ambition <compte> natural » pour l'y remettre).".to_string(), None)
    } else if last == Some("rebalance") && plan.is_some_and(|row| row.kind == ambition.kind) {
        let row = plan.map(|row| numbers_view(&row.numbers));
        (
            "a4_rebalance",
            format!("Le métier dont la demande non servie rapporte le plus par 6000 ticks{by_item}."),
            row,
        )
    } else if let Some(row) = plan.filter(|row| row.applied && row.from == row.kind && row.kind == ambition.kind && ambition.revisions.last().is_none_or(|revision| revision.at_secs <= row.computed_secs)) {
        (
            "a4_kept",
            "Aucun autre métier ne la dépassait de 25 % : elle est restée telle quelle (valeur par 6000 ticks ci-dessous).".to_string(),
            Some(numbers_view(&row.numbers)),
        )
    } else if ambition.why.starts_with("value:") || last == Some("achieved") || last == Some("operator:natural") {
        let heading = match last {
            Some("operator:natural") => "Retour à l'équilibre naturel demandé par l'opérateur : valeur de la demande non servie",
            Some("achieved") => "Nouvelle ambition à l'atteinte de la précédente, choisie par la valeur de la demande non servie",
            _ => "Choisie par la valeur de la demande non servie",
        };
        let figures = logged.filter(|line| line.kind == ambition.kind).map(|line| numbers_view(&line.numbers));
        let tail = if figures.is_some() { " (chiffres du journal)." } else { ". Chiffres non retrouvés dans le journal récent." };
        ("a4_draw", format!("{heading}{by_item}{tail}"), figures)
    } else {
        ("legacy", format!("Tirée avant le rééquilibrage du 21/11 ({}). Elle ne change plus tant qu'elle n'est pas atteinte.", ambition.why), None)
    };
    object(vec![("source", str_value(source)), ("text", str_value(&text)), ("numbers", numbers.unwrap_or(Value::Null))])
}

/// State of one milestone as the brain treats it: `done`, `set_aside`
/// (blocked `MILESTONE_MAX_BLOCKS` times; resumed when nothing else is
/// open and its cooldown is over), `waiting` (cooled: gear nobody makes
/// (A3), a market shortfall), `active`, `open`.
pub fn milestone_state(kind: &str, stone: &Milestone, cools: &[Cool]) -> (&'static str, String, Option<u64>) {
    if stone.status == MilestoneStatus::Done {
        return ("done", "fait".to_string(), None);
    }
    let cool = cools.iter().find(|cool| cool.key == milestone_cool_key(kind, &stone.id));
    if set_aside(stone) {
        let text = match cool {
            Some(cool) => format!("mis de côté (bloqué {} fois) : reprise possible dans {} ticks", stone.blocked_count, cool.ticks_left),
            None => format!("mis de côté (bloqué {} fois) : repris dès qu'aucun autre jalon n'est ouvert", stone.blocked_count),
        };
        return ("set_aside", text, cool.map(|cool| cool.ticks_left));
    }
    if let Some(cool) = cool {
        let why = match cool.cause.as_str() {
            "gear nobody makes" => "équipement que personne ne fabrique (ni en stock, ni au marché)".to_string(),
            "market shortfall" => "offre d'achat restée sans vendeur".to_string(),
            other => other.to_string(),
        };
        return ("waiting", format!("en attente : {why}, nouvel essai dans {} ticks", cool.ticks_left), Some(cool.ticks_left));
    }
    match stone.status {
        MilestoneStatus::Active => ("active", "en cours".to_string(), None),
        MilestoneStatus::Blocked => ("open", format!("ouvert, bloqué {}/{}", stone.blocked_count, MILESTONE_MAX_BLOCKS), None),
        _ => ("open", "ouvert".to_string(), None),
    }
}

/// The ambition as the page shows it: the verbatim contract
/// (`ambition_to_value`) plus how it was given, the revisions explained,
/// every milestone with its real state. `Null` without an ambition.
pub fn ambition_view(account: &str, life: &Life, plan: Option<&PlanInfo>, logged: Option<&LoggedValue>, cools: &[Cool]) -> Value {
    let Some(ambition) = life.ambition.as_ref() else { return Value::Null };
    let mut view = ambition_to_value(ambition);
    let Value::Obj(map) = &mut view else { return view };
    map.insert("given".to_string(), given_view(account, ambition, plan, logged));
    map.insert("protected".to_string(), Value::Bool(lifescape_planner::protected(account)));
    map.insert("pinned".to_string(), Value::Bool(lifescape_planner::pinned(ambition)));
    let revisions = ambition
        .revisions
        .iter()
        .rev()
        .take(30)
        .map(|revision| {
            let (text, retired) = revision_text(&revision.why);
            object(vec![
                ("at_secs", Value::Int(revision.at_secs)),
                ("tick", Value::Int(revision.tick as i64)),
                ("from", str_value(&revision.from)),
                ("to", str_value(&revision.to)),
                ("why", str_value(&revision.why)),
                ("text", str_value(&text)),
                ("retired_rule", Value::Bool(retired)),
            ])
        })
        .collect();
    map.insert("revision_rows".to_string(), Value::Arr(revisions));
    map.insert("revision_total".to_string(), Value::Int(ambition.revisions.len() as i64));
    let rows: Vec<Value> = ambition
        .milestones
        .iter()
        .zip(map.get("milestones").and_then(|rows| if let Value::Arr(rows) = rows { Some(rows.clone()) } else { None }).unwrap_or_default())
        .map(|(stone, row)| {
            let (state, text, ticks_left) = milestone_state(&ambition.kind, stone, cools);
            let mut row = row;
            if let Value::Obj(fields) = &mut row {
                fields.insert("state".to_string(), str_value(state));
                fields.insert("state_text".to_string(), str_value(&text));
                fields.insert("ticks_left".to_string(), ticks_left.map(|ticks| Value::Int(ticks as i64)).unwrap_or(Value::Null));
            }
            row
        })
        .collect();
    map.insert("milestones".to_string(), Value::Arr(rows));
    view
}

/// Where the current goal comes from: a milestone (itself born of the
/// ladder, an extension or a market demand), the answer to a buy offer
/// (`bid:`), a side goal (`side:`), alchemy (`alch:`), or nothing.
pub fn goal_origin_view(life: &Life, filler: Option<&(String, String)>) -> Value {
    let raw = life.goal_origin.as_deref().unwrap_or("");
    let stone = life.ambition.as_ref().and_then(|ambition| ambition.milestones.iter().find(|stone| stone.id == raw));
    let (kind, text): (&str, String) = if let Some(rest) = raw.strip_prefix("bid:") {
        ("bid", format!("réponse à une offre d'achat : {rest}"))
    } else if let Some(rest) = raw.strip_prefix("side:") {
        ("side", format!("but d'appoint : {rest}"))
    } else if let Some(rest) = raw.strip_prefix("alch:") {
        ("alch", format!("alchimie : {rest}"))
    } else if let Some(stone) = stone {
        match stone.origin.strip_prefix("demand:") {
            Some(item) => ("demand", format!("{} ajouté pour une demande du marché ({item}) : {}", stone.id, stone.label)),
            None => ("milestone", format!("{} : {}", stone.id, stone.label)),
        }
    } else if raw.is_empty() {
        ("none", "sans origine (but libre)".to_string())
    } else {
        ("milestone", raw.to_string())
    };
    let (kind, text) = match filler {
        Some((parked, method)) => ("filler", format!("remplissage {method} en attendant le marché ({parked}) ; but sous-jacent : {text}")),
        None => (kind, text),
    };
    object(vec![("kind", str_value(kind)), ("text", str_value(&text)), ("raw", str_value(raw))])
}

/// The filler running now, as `(parked node, method)`, from the life.
pub fn filler_view(filler: Option<&(String, String)>) -> Value {
    match filler {
        Some((parked, method)) => object(vec![("parked", str_value(parked)), ("method", str_value(method))]),
        None => Value::Null,
    }
}

/// The value of time of the goal's skill (A1 (a)): the bot's measured
/// coins per tick, the pull of the pending climbs that open a lucrative
/// way (value per 6000 ticks / 6000) and the larger of both, which the
/// training line choice uses. The three best pending climbs follow.
pub fn training_view(input: Option<&TrainingInput>) -> Value {
    let Some(input) = input else { return Value::Null };
    let pull = pull_per_tick(&input.pending);
    let mut ranked: Vec<&Investment> = input.pending.iter().collect();
    ranked.sort_by(|left, right| right.value_6000.total_cmp(&left.value_6000));
    let rows = ranked
        .iter()
        .take(3)
        .map(|found| {
            let gates = found
                .gates
                .iter()
                .map(|gate| object(vec![("skill", str_value(&gate.skill)), ("level", Value::Int(i64::from(gate.level))), ("climb_ticks", Value::Int(gate.climb_ticks as i64))]))
                .collect();
            object(vec![
                ("item", str_value(&found.item)),
                ("method", str_value(&found.method)),
                ("climb_ticks", Value::Int(found.climb_ticks as i64)),
                ("gain_per_tick", Value::Float(found.gain_per_tick)),
                ("value_6000", Value::Int(found.value_6000 as i64)),
                ("gates", Value::Arr(gates)),
            ])
        })
        .collect();
    let used = time_value(input.own_rate, &input.pending);
    object(vec![
        ("skill", str_value(&input.skill)),
        ("own_rate", Value::Float(input.own_rate)),
        ("pull_per_tick", Value::Float(pull)),
        ("time_value", Value::Float(used)),
        ("lifted", Value::Bool(used > input.own_rate)),
        ("pending", Value::Arr(rows)),
    ])
}

#[cfg(test)]
#[path = "web_brain_view_tests.rs"]
mod tests;
