#![forbid(unsafe_code)]
//! Upgrade wishlist (PRD3 lot 5): the gear and tools a bot wants next.
//!
//! From the bot's base levels (XP truth) the gear resolver
//! (`lifescape_gamedata::gear`) names the best F2P set for the goal's
//! combat style plus the best pickaxe and axe of its gathering tiers.
//! Whatever the bot does not own yet, and that beats what it owns in the
//! same slot, is a wish. Wishes never block the current goal: the brain
//! keeps a standing market bid for the top wishes and the sales pass
//! never lists a wished object. Once owned, the kit's gear pass wears it
//! at the next bank stop.
//!
//! Since 2026-11-13 melee (and non-combat) goals climb the equipment
//! ladder instead (`brain_gear`: one tier at a time, as graph steps):
//! their wishes are the tools only. Ceilings are wallet shares that rise
//! as coins come in: `TOOL_SHARE_PCT` (25 %) per tool, `GEAR_SHARE_PCT`
//! (50 %) split across ranged/magic gear wishes. A resting wish bid keeps
//! its walk (`market_reprice::keep_bid`): only its ceiling moves.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use lifescape_gamedata::gear::{best_gear, best_tool, score, Style};
use lifescape_planner::gear_ladder::{wallet_ceiling, GEAR_SHARE_PCT, TOOL_SHARE_PCT};
use lifescape_json::Value;
use lifescape_planner::{Life, NodeKind, NodeStatus};

use crate::jobs::Job;

/// Wall seconds between two wishlist bid passes for one account.
pub const WISH_EVERY_SECS: f64 = 600.0;
/// Wishes bid at once (the wallet is shared with the graph's needs).
pub const WISH_BIDS: usize = 2;

/// Objects of open obtain nodes (graph needs, never sold either).
pub fn wanted_names(life: &Life) -> Vec<String> {
    let mut out = Vec::new();
    for node in &life.nodes {
        if matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned) {
            continue;
        }
        if let NodeKind::ObtainItems { item, .. } = &node.kind {
            out.push(item.clone());
        }
    }
    out.sort();
    out.dedup();
    out
}

fn style_of(life: &Life) -> Style {
    Style::parse(lifescape_planner::goal_combat_style(&life.goal))
}

/// One wish: the object, its slot (`pickaxe`/`axe` for tools) and the
/// score gain over the best owned piece of that slot.
#[derive(Debug, Clone, PartialEq)]
pub struct Wish {
    pub item: String,
    pub slot: String,
    pub gain: i64,
}

/// Pure wishlist: best set for `style` at `levels`, minus what `owned`
/// (debug names: pack, bank, worn) already covers as well or better.
pub fn wishes(style: Style, levels: &[(&str, i64)], owned: &[String], tools: &[&str]) -> Vec<Wish> {
    let mut out = Vec::new();
    for pick in best_gear(style, levels, &[]) {
        if owned.iter().any(|name| name == &pick.item) {
            continue;
        }
        let owned_best = owned
            .iter()
            .filter_map(|name| lifescape_gamedata::equipment::by_name(name))
            .filter(|entry| entry.wearpos == pick.slot && lifescape_gamedata::gear::wieldable(entry, levels, &[]))
            .map(|entry| score(&entry, style))
            .max()
            .unwrap_or(0);
        if pick.score > owned_best {
            out.push(Wish { item: pick.item, slot: format!("slot{}", pick.slot), gain: pick.score - owned_best });
        }
    }
    for family in tools {
        let Some(best) = best_tool(family, levels) else { continue };
        if owned.iter().any(|name| name == &best) {
            continue;
        }
        let best_tier = lifescape_gamedata::equipment::by_name(&best).map(|entry| entry.tier).unwrap_or(0);
        let owned_tier = owned
            .iter()
            .filter(|name| name.ends_with(&format!("_{family}")) && !name.contains("battleaxe"))
            .filter_map(|name| lifescape_gamedata::equipment::by_name(name))
            .map(|entry| entry.tier)
            .max();
        // A tool is wished only as an upgrade of a family the bot uses.
        if let Some(owned_tier) = owned_tier {
            if best_tier > owned_tier {
                out.push(Wish { item: best, slot: family.to_string(), gain: (best_tier - owned_tier) * 10 });
            }
        }
    }
    out.sort_by(|left, right| right.gain.cmp(&left.gain).then_with(|| left.item.cmp(&right.item)));
    out
}

fn live_of(job: &Job) -> Value {
    job.inner.try_lock().ok().and_then(|inner| inner.live.clone()).unwrap_or(Value::Null)
}

/// The bot's current wishes (empty without a live snapshot). Melee and
/// non-combat goals wish tools only: their gear climbs the ladder
/// (`brain_gear`).
pub fn wishlist(job: &Job, life: &Life) -> Vec<Wish> {
    let live = live_of(job);
    let levels_owned = crate::brain_market::live_levels(&live);
    if levels_owned.is_empty() {
        return Vec::new();
    }
    let levels: Vec<(&str, i64)> = levels_owned.iter().map(|(name, level)| (name.as_str(), i64::from(*level))).collect();
    let mut owned: Vec<String> = crate::brain_plan::live_stacks(job).into_iter().map(|(name, _)| name).collect();
    owned.extend(crate::brain_plan::worn_names(job));
    let mut list = wishes(style_of(life), &levels, &owned, &["pickaxe", "axe"]);
    if crate::brain_gear::ladder_goal(life) {
        list.retain(is_tool_wish);
    }
    list
}

fn is_tool_wish(wish: &Wish) -> bool {
    wish.slot == "pickaxe" || wish.slot == "axe"
}

/// Per-unit ceiling of one wish bid: a tool takes `TOOL_SHARE_PCT` of the
/// wallet, gear wishes split `GEAR_SHARE_PCT` between them. Rises with
/// the wallet.
pub fn wish_ceiling(wish: &Wish, coins: i64, gear_wishes: usize) -> i64 {
    if is_tool_wish(wish) {
        wallet_ceiling(coins, TOOL_SHARE_PCT)
    } else {
        wallet_ceiling(coins, GEAR_SHARE_PCT) / gear_wishes.max(1) as i64
    }
}

fn last_pass() -> &'static Mutex<HashMap<String, f64>> {
    static TABLE: OnceLock<Mutex<HashMap<String, f64>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// One wishlist pass for an idle, non-Iron bot: keeps a standing bid for
/// each top wish, its ceiling the wallet share of the moment (a resting
/// bid keeps its price walk). Returns the bids placed or kept.
pub fn maybe_bid_wishes(job: &Job, life: &Life) -> usize {
    if life.profile.ironman {
        return 0;
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs_f64()).unwrap_or(0.0);
    {
        let mut table = last_pass().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if table.get(&job.account).is_some_and(|last| now - last < WISH_EVERY_SECS) {
            return 0;
        }
        table.insert(job.account.clone(), now);
    }
    let Some(service) = crate::market_service::shared() else { return 0 };
    let wishes = wishlist(job, life);
    let coins = crate::brain_plan::holdings_count(job, "coins");
    let top: Vec<Wish> = wishes.into_iter().filter(|wish| crate::brain_sales::tradeable(&wish.item)).take(WISH_BIDS).collect();
    let gear_wishes = top.iter().filter(|wish| !is_tool_wish(wish)).count();
    let mut placed = 0;
    for wish in &top {
        let Some(id) = lifescape_gamedata::objs_by_name(&wish.item).into_iter().next().map(|obj| obj.obj_id as i64) else { continue };
        let ceiling = wish_ceiling(wish, coins, gear_wishes);
        if ceiling < 1 {
            continue;
        }
        match crate::market_reprice::keep_bid(&service, &job.account, id, 1, ceiling) {
            Ok(crate::market_reprice::StandingBid::Placed { price, .. }) => {
                placed += 1;
                crate::logln!("brain {}: wish {} ({}) bid @ {price} ceiling {ceiling}", job.account, wish.item, wish.slot);
            }
            Ok(crate::market_reprice::StandingBid::Kept { .. }) => placed += 1,
            Err(error) => crate::logln!("brain {}: wish {} bid refused: {error}", job.account, wish.item),
        }
    }
    placed
}

#[cfg(test)]
mod tests {
    use super::wishes;
    use lifescape_gamedata::gear::Style;

    #[test]
    fn wishes_name_upgrades_only() {
        let levels = [("attack", 30), ("defence", 30), ("strength", 30), ("mining", 41), ("woodcutting", 1)];
        let owned = vec!["bronze_sword".to_string(), "bronze_pickaxe".to_string()];
        let list = wishes(Style::Melee, &levels, &owned, &["pickaxe", "axe"]);
        let names: Vec<&str> = list.iter().map(|wish| wish.item.as_str()).collect();
        assert!(names.iter().any(|name| name.starts_with("adamant_") || name.starts_with("mithril_")), "attack 30 wants mithril/adamant: {names:?}");
        assert!(names.contains(&"rune_pickaxe"), "mining 41 wants rune: {names:?}");
        assert!(!names.iter().any(|name| name.ends_with("_axe") && !name.contains("battle")), "no axe owned: no axe wish");
        let geared: Vec<String> = list.iter().map(|wish| wish.item.clone()).chain(owned.clone()).collect();
        assert!(wishes(Style::Melee, &levels, &geared, &["pickaxe", "axe"]).is_empty(), "owning the set ends the wishes");
    }

    /// Wish ceilings are wallet shares: they rise as coins come in.
    #[test]
    fn wish_ceilings_are_wallet_shares() {
        use super::{wish_ceiling, Wish};
        let tool = Wish { item: "rune_pickaxe".to_string(), slot: "pickaxe".to_string(), gain: 10 };
        let bow = Wish { item: "oak_shortbow".to_string(), slot: "slot3".to_string(), gain: 10 };
        assert_eq!(wish_ceiling(&tool, 200, 0), 50, "a tool: 25 % of the wallet");
        assert_eq!(wish_ceiling(&tool, 800, 0), 200, "more coins, higher ceiling");
        assert_eq!(wish_ceiling(&bow, 200, 2), 50, "gear wishes split half the wallet");
        assert_eq!(wish_ceiling(&bow, 3, 1), 1);
    }
}
