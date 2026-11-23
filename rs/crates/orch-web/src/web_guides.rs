//! Guides API (plan rendements R4): `/api/guides/money` and
//! `/api/guides/training`, the pure engine of `lifescape_planner::guide_*`
//! fed with the public data only (D22):
//! - yields: the live estimator (`rates_read::estimate_using`, n >= 3
//!   blended with the guides' game prior, else the prior alone), names
//!   turned into debug names for the engine;
//! - prices: the current price of each traded object (`rates_price`,
//!   D12) and the units its last five fills moved (the price holds for
//!   them only, user 2026-11-17); a never-traded object has none, only its resting book (the
//!   engine counts it for the units it holds, `quote_of`); the best F2P
//!   shop sale at a line's volume (Q3, `NPC_SALE_VOLUME`) and what the
//!   F2P shops sell in one window (Q7, finite stock, decision 2026-11-16
//!   §2: base stock unit by unit at the server price, then the window's
//!   restock, `gamedata::shop_buy`);
//! - the market wait per lot: mean pause of the contract-2 runs that
//!   paused for the market; a quest's duration: mean work of its
//!   finished contract-2 runs.
//!
//! Filters (query string): `skill`, `level` (a bot with every skill at
//! it; empty: each method at its own gate), `family`, `quests` (done,
//! comma separated), `held` (objects a bot holds, `item:units,...`,
//! debug names: a recipe on stock is evaluated on them). Profile of the
//! feasibility (D22, `guide_feasible`): `level`, `levels`
//! (`skill:level,...`, overriding `level`), `quests`, `held` and `coins`
//! (free coins; empty: a lot's capital unjudged). Every row carries its
//! `conditions` (structured list) and its `feasibility` for that profile
//! (`feasible` | `after_prereqs` | `infeasible`, pending and reasons);
//! nothing is ever blocked. No profile (none of those keys): no line is
//! judged, `feasibility` null on every row and `judged` false (never a
//! judgement for a made-up bot: each bot judges itself, D22). No request
//! builds a guide: the API reads the set the `guides` thread publishes
//! (`guides_store`, filters applied by reading); `build_guide` builds one
//! guide directly for the offline check (`rates-rebuild --guide`).

#![forbid(unsafe_code)]

use lifescape_json::{object, str_value, Value};
use lifescape_planner::guide_model::GuideFilter;

pub use crate::guides_model::GuideKind;
use crate::guides_model::{filter_of, money_value, training_value, unrated_value};

/// One guide read from a published set as the API serves it, with the
/// set's age, cost and what the read approximates.
pub fn read_guide(set: &crate::guides_store::GuideSet, kind: GuideKind, filter: &GuideFilter) -> Value {
    // D22 Q1 (R7): a profile given, the lines it can work come first.
    let profiled = filter.has_profile();
    let read = lifescape_planner::guide_feasible::with_filter_bot(filter, |bot| match kind {
        GuideKind::Money => set.money(filter).map(|(mut rows, unrated)| {
            if profiled {
                lifescape_planner::guide_feasible::feasible_first(&mut rows, bot);
            }
            (rows.iter().map(|row| money_value(row, profiled.then_some(bot))).collect::<Vec<Value>>(), unrated)
        }),
        GuideKind::Training => {
            let (mut rows, unrated) = set.training(filter);
            if profiled {
                lifescape_planner::guide_feasible::feasible_first(&mut rows, bot);
            }
            Ok((rows.iter().map(|row| training_value(row, profiled.then_some(bot))).collect::<Vec<Value>>(), unrated))
        }
    });
    let (rows, unrated) = match read {
        Ok(found) => found,
        Err(error) => return crate::web::err_json(error),
    };
    let view_level = match kind {
        GuideKind::Money => set.money_view(filter.level).level,
        GuideKind::Training => set.training_view(filter.level).level,
    };
    // What the read does not rebuild (`guide_read`): said, never hidden.
    let mut approximate: Vec<Value> = Vec::new();
    if let (Some(level), Some(built)) = (filter.level, view_level) {
        if level != built {
            approximate.push(str_value(&format!("rates of level {built}, the band's last level (activities above {level} left out, accessibility at {level})")));
        }
    }
    if !filter.quests_done.is_empty() {
        approximate.push(str_value(match kind {
            GuideKind::Money => "chain lines keep the farm-or-buy choices of a bot without quests (its D17 outside the chains)",
            GuideKind::Training => "D17 re-read from the money view with the quests (its chain lines as for a bot without quests)",
        }));
    }
    object(vec![
        ("ok", Value::Bool(true)),
        ("count", Value::Int(rows.len() as i64)),
        ("judged", Value::Bool(profiled)),
        ("rows", Value::Arr(rows)),
        ("unrated", Value::Arr(unrated.iter().map(unrated_value).collect())),
        ("market_wait_ticks", set.market_wait_ticks.map(Value::Float).unwrap_or(Value::Null)),
        ("view_level", view_level.map(|level| Value::Int(i64::from(level))).unwrap_or(Value::Null)),
        ("approximate", Value::Arr(approximate)),
        ("built_at_ms", Value::Int(set.built_unix_ms)),
        ("age_ms", Value::Int(set.age().as_millis() as i64)),
        ("cycle_ms", Value::Int(set.cycle_ms as i64)),
        ("cycle_cpu_ms", set.cycle_cpu_ms.map(|cpu| Value::Int(cpu as i64)).unwrap_or(Value::Null)),
        ("refresh_s", Value::Int(crate::guides_store::REFRESH_PERIOD.as_secs() as i64)),
    ])
}

/// HTTP entry: the guide of `kind` for the query string, read from the
/// last published set (never built here); pending before the first.
pub fn guides_api(kind: GuideKind, query: &str) -> Value {
    let filter = filter_of(&crate::web::parse_query(query));
    match crate::guides_store::current() {
        Some(set) => read_guide(&set, kind, &filter),
        None => object(vec![("ok", Value::Bool(false)), ("pending", Value::Bool(true)), ("error", str_value("guides: first build running"))]),
    }
}

#[cfg(test)]
pub(crate) use crate::guides_model::*;

#[cfg(test)]
#[path = "web_guides_tests.rs"]
mod tests;
