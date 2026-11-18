#![forbid(unsafe_code)]
//! Report parsing and method/provenance predicates of the rates (split
//! from `rates_service`, rule 4).

use lifescape_json::Value;

use super::LEVEL_BAND;

/// Provenances that count as farm output. `picked` is retired (D45): the
/// SDK no longer emits or parses it, so it never rates.
pub fn is_farm_provenance(provenance: &str) -> bool {
    matches!(provenance, "produced" | "loot" | "ground_spawn")
}

/// Trading methods (D44): never a yield, never XP. Bare `market` is
/// the market script (the session reports the script name: a maker's
/// post, a buy, a sell, a cleanup); `market:*`
/// and `shop:*` are the planner's finer legs. Bare `shop` is reserved
/// for the symmetric script name. Coin P&L of these methods feeds the
/// trading book instead of the rate buckets.
/// Activities that never feed the rates: the tutorial's kit, XP and
/// drops say nothing about any activity a bot can choose (user rule).
pub fn is_unrated_method(method: &str) -> bool {
    method.eq_ignore_ascii_case("tutorial")
}

pub fn is_trading_method(method: &str) -> bool {
    method == "market" || method == "shop" || method.starts_with("market:") || method.starts_with("shop:")
}

/// Provenances that count as an engagement input cost (bait, food).
pub fn is_input_provenance(provenance: &str) -> bool {
    matches!(provenance, "consumed" | "alch_low" | "alch_high")
}

/// First level of the band holding `level`: 1-5 -> 1, 6-10 -> 6.
/// Levels below 1 clamp to the first band.
pub fn level_band(level: i64) -> i64 {
    if level <= 1 {
        return 1;
    }
    ((level - 1) / LEVEL_BAND) * LEVEL_BAND + 1
}

/// One parsed engagement, straight from the pushed report JSON. `ticks`
/// is the wall budget, `pause_ticks` the market-pause slice inside it;
/// rates divide by their difference (work ticks).
#[derive(Clone, Debug)]
pub struct ParsedReport {
    pub account: String,
    pub node_id: String,
    pub method: String,
    pub script: String,
    pub wall_ticks: i64,
    pub pause_ticks: i64,
    pub stop: String,
    pub level: i64,
    pub band: i64,
    pub items: Vec<ParsedItem>,
    pub xp: Vec<ParsedXp>,
}

/// One reported item flow.
#[derive(Clone, Debug)]
pub struct ParsedItem {
    pub name: String,
    pub provenance: String,
    pub quantity: i64,
}

/// One reported XP gain.
#[derive(Clone, Debug)]
pub struct ParsedXp {
    pub skill: String,
    pub amount: i64,
}

fn text_field(report: &Value, key: &str) -> String {
    report.get(key).as_str().unwrap_or("").to_string()
}

/// Main-skill level at engagement start: the level beside the first XP
/// skill when present, else the first band level seen, else 1.
fn context_level(report: &Value) -> i64 {
    let xp_skill_stat = report
        .get("xp")
        .as_arr()
        .and_then(|rows| rows.first())
        .and_then(|row| row.get("stat").as_i64());
    let bands = report.get("levels_before");
    let level_for = |stat: i64| {
        bands.as_arr().and_then(|rows| {
            rows.iter().find(|row| row.get("stat").as_i64() == Some(stat)).and_then(|row| row.get("level").as_i64())
        })
    };
    if let Some(stat) = xp_skill_stat {
        if let Some(level) = level_for(stat) {
            return level.max(1);
        }
    }
    if let Some(rows) = bands.as_arr() {
        if let Some(row) = rows.first() {
            return row.get("level").as_i64().unwrap_or(1).max(1);
        }
    }
    1
}

/// Parses the pushed report JSON into store + snapshot rows. `method`
/// falls back to `activity` (today's contract reports carry the activity
/// only; finer `gather:skill/resource` methods land with the planner).
/// Empty account or method is a caller bug, reported loudly.
pub fn parse_report(report: &Value) -> Result<ParsedReport, String> {
    let account = text_field(report, "account");
    if account.is_empty() {
        return Err("rates: report account is empty".to_string());
    }
    let activity = text_field(report, "activity");
    let explicit = text_field(report, "method");
    let method = if explicit.is_empty() { activity.clone() } else { explicit };
    if method.is_empty() {
        return Err("rates: report method/activity is empty".to_string());
    }
    let script = {
        let named = text_field(report, "script");
        if named.is_empty() { activity.clone() } else { named }
    };
    let wall_ticks = report.get("ticks").as_i64().unwrap_or(0);
    let pause_ticks = report.get("market_pause_ticks").as_i64().unwrap_or(0).max(0);
    let mut items = Vec::new();
    if let Some(rows) = report.get("items").as_arr() {
        for row in rows {
            let provenance = row.get("provenance").as_str().unwrap_or("").to_string();
            if provenance.is_empty() {
                return Err(format!("rates: item '{}' has no provenance", row.get("name").as_str().unwrap_or("?")));
            }
            if lifescape_sdk::provenance::Provenance::parse(&provenance).is_none() {
                return Err(format!(
                    "rates: item '{}' has unknown provenance '{provenance}'",
                    row.get("name").as_str().unwrap_or("?")
                ));
            }
            items.push(ParsedItem {
                name: row.get("name").as_str().unwrap_or("").to_string(),
                provenance,
                quantity: row.get("qty").as_i64().unwrap_or(0),
            });
        }
    }
    let mut xp = Vec::new();
    if let Some(rows) = report.get("xp").as_arr() {
        for row in rows {
            let skill = row.get("skill").as_str().unwrap_or("").to_string();
            let amount = row.get("xp").as_i64().unwrap_or(0);
            if skill.is_empty() || amount == 0 {
                continue;
            }
            xp.push(ParsedXp { skill, amount });
        }
    }
    Ok(ParsedReport {
        account,
        node_id: text_field(report, "node_id"),
        method,
        script,
        wall_ticks,
        pause_ticks,
        stop: text_field(report, "stop"),
        level: context_level(report),
        band: level_band(context_level(report)),
        items,
        xp,
    })
}
