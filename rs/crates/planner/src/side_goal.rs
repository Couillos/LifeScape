//! Side goals (user rule "aucun bot inactif", decision D-SIDE in
//! `docs/decisions.md`, 2026-11-13). When every open milestone of a bot's
//! ambition trains a banned skill, the bot works a TEMPORARY goal instead
//! of idling until the ban runs out; the ambition and its milestones are
//! never touched:
//! 1. the heaviest paying demand (`DemandItem::margin` > 0) the bot can
//!    make now with skills that are not banned (levels met, quota not
//!    already held) → `GoalKind::Produce`, origin `side:demand:<item>`;
//! 2. else a money goal (`GoalKind::MakeMoney`, origin `side:money`) whose
//!    resolution is steered off the banned skills (`steer_off_bans`).
//!
//! The origin names no milestone, so the close folds into nothing and the
//! next draw goes back to the ambition as soon as a milestone is drawable
//! again. Nothing possible: `None`, the caller keeps its typed reason.
//! Pure: levels, bans, demand and holdings come in as arguments.

use crate::ambition_catalog::{group_of_skill, method_skill, DemandItem};
use crate::combat::{pick_site_paid, Fare, SitePick};
use crate::expand::{method_feasible, ReadyCtx};
use crate::model::{GoalKind, Node};

/// Prefix of a side goal's origin (`life.goal_origin`).
pub const SIDE_PREFIX: &str = "side:";
/// Skills the money methods train (`resolve_obtain::money_methods`:
/// gathering tiers, sheep shearing, melee spots).
const MONEY_SKILLS: [&str; 5] = ["fishing", "woodcutting", "mining", "crafting", "attack"];

/// True when `skill` is banned now: the skill itself, or any melee stat
/// for a melee stat (melee is ONE group: a fight trains them all).
pub fn skill_banned(skill: &str, avoid: &[String]) -> bool {
    let group = group_of_skill(skill);
    avoid.iter().any(|banned| banned == skill || (group == Some("melee") && group_of_skill(banned) == Some("melee")))
}

/// True when a method trains a banned skill (`market:flip`, unknown
/// methods: never).
pub fn method_banned(method: &str, avoid: &[String]) -> bool {
    method_skill(method).is_some_and(|skill| skill_banned(skill, avoid))
}

/// The side goal for a bot whose open milestones are all banned:
/// `(goal, origin)`, or `None` when nothing is workable. `ironman` skips
/// the market demand (Iron never sells).
pub fn side_goal(level_of: &dyn Fn(&str) -> u8, avoid: &[String], demand: &[DemandItem], held_of: &dyn Fn(&str) -> i64, ironman: bool) -> Option<(GoalKind, String)> {
    if !ironman {
        let mut best: Option<&DemandItem> = None;
        for wanted in demand.iter().filter(|entry| entry.unmet > 0 && entry.weight > 0.0 && entry.margin.is_some_and(|margin| margin > 0.0)) {
            if held_of(&wanted.item) >= wanted.unmet.clamp(5, 100) || !workable(&wanted.item, level_of, avoid) {
                continue;
            }
            if best.is_none_or(|known| wanted.weight > known.weight) {
                best = Some(wanted);
            }
        }
        if let Some(wanted) = best {
            let goal = GoalKind::Produce { item: wanted.item.clone(), count: wanted.unmet.clamp(5, 100) };
            return Some((goal, format!("{SIDE_PREFIX}demand:{}", wanted.item)));
        }
    }
    let money_open = !ironman || MONEY_SKILLS.iter().any(|skill| !skill_banned(skill, avoid));
    money_open.then(|| (GoalKind::MakeMoney, format!("{SIDE_PREFIX}money")))
}

/// The bot can make `item` now: its quota's ban skill (first production
/// leg's skill, `goals::goal_skills`) is free and some production leg's
/// skill gate is met by an unbanned skill (or has none).
fn workable(item: &str, level_of: &dyn Fn(&str) -> u8, avoid: &[String]) -> bool {
    let goal_skills = crate::goals::goal_skills(&GoalKind::Produce { item: item.to_string(), count: 1 });
    if goal_skills.iter().any(|skill| skill_banned(skill, avoid)) {
        return false;
    }
    let legs = crate::obtain_legs::production_legs(item);
    let gated: Vec<&(String, i64)> = legs.iter().filter_map(|leg| leg.skill.as_ref()).collect();
    if gated.is_empty() {
        return !legs.is_empty();
    }
    // A skilled way exists: the bot must meet one (a stray drop leg of
    // a level-15 ore is no side job for a level-1 miner).
    gated.iter().any(|(skill, level)| !skill_banned(skill, avoid) && i64::from(level_of(skill).max(1)) >= *level)
}

/// A resolved money (or coin) node whose method trains a banned skill
/// moves to its first ranked alternative that does not, is workable with
/// the bot's tools and has a safe site. `Ok(false)`: nothing to steer;
/// `Ok(true)`: steered; `Err`: every workable method is banned (the brain
/// waits, typed, never launches through a ban).
pub fn steer_off_bans(node: &mut Node, ctx: &ReadyCtx, avoid: &[String]) -> Result<bool, String> {
    let Some(locked) = node.method.clone() else { return Ok(false) };
    if avoid.is_empty() || !method_banned(&locked, avoid) {
        return Ok(false);
    }
    let candidates: Vec<String> = node.alternatives.iter().map(|alt| alt.method.clone()).collect();
    for method in candidates {
        if method_banned(&method, avoid) || !method_feasible(&method, ctx) {
            continue;
        }
        match pick_site_paid(&method, ctx.combat, ctx.position, Fare::of(ctx)) {
            SitePick::Anywhere => {
                node.method = Some(method);
                node.site = None;
                node.toll = false;
                return Ok(true);
            }
            SitePick::At(site) => {
                node.method = Some(method);
                node.site = Some(site);
                node.toll = false;
                return Ok(true);
            }
            SitePick::Tolled(site) => {
                node.method = Some(method);
                node.site = Some(site);
                node.toll = true;
                return Ok(true);
            }
            SitePick::Fare(_) | SitePick::Needs(_) => continue,
        }
    }
    Err(format!("every workable money method trains a banned skill ({})", avoid.join(",")))
}

#[cfg(test)]
#[path = "side_goal_tests.rs"]
mod tests;
