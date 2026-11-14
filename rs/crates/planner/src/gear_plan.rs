//! Equipment a combat bot wants (user 2026-11-14: "les bots qui font de
//! la mêlée devraient vouloir le meilleur matériel pour leur combat
//! lvl"). Replaces the one-tier-at-a-time ladder for the CHOICE of
//! pieces (live 2026-11-14: level 40-58 fighters wearing a bronze med
//! helm and an iron dagger all bid on a BRONZE scimitar, one slot at a
//! time, never a platebody, boots or an amulet).
//!
//! Per slot (weapon, body, legs, helm, shield, boots, gloves, amulet,
//! cape), the bot wants the best F2P piece its levels let it wear for
//! its combat style (`gamedata::gear::score`: melee bonuses + strength,
//! range for bows and leather, magic for staves and robes), better than
//! what it owns, tradeable and obtainable without an unfinished quest.
//! It wants what it can AFFORD: every slot starts at its best piece; while
//! the expected prices add up to more than the gear budget, the dearest
//! slot steps down to its next cheaper piece (or drops out). Expected
//! price = the current market price once traded, else the high alchemy
//! value (nobody sells under it: the natural floor), at least 1.
//! Each want's ceiling is its share of the budget (expected × budget /
//! total): bids open at the current price and climb with patience.
//!
//! Weapons: one-handers for melee and magic (a shield fills the off
//! hand), bows for ranged (two-handed: no shield). Ammunition and runes
//! are consumables sized by the tranches, never gear.
//!
//! Pure: levels, holdings, budget and prices come in as arguments; the
//! candidate list per style is built once (gamedata only).

use std::sync::OnceLock;

use lifescape_gamedata::equipment::{all_wearables, Wearable};
use lifescape_gamedata::gear::{score, wieldable, Style, BODY, CAPE, FEET, HANDS, HEAD, LEGS, NECK, SHIELD, WEAPON};

/// Slots a bot equips, in purchase priority (weapon first).
pub const PLAN_SLOTS: [i64; 9] = [WEAPON, BODY, LEGS, HEAD, SHIELD, FEET, HANDS, NECK, CAPE];

/// One wearable a style may want.
#[derive(Debug, Clone)]
pub struct GearPiece {
    pub wearpos: i64,
    pub item: String,
    pub score: i64,
    entry: Wearable,
}

/// One piece the bot wants now, with its expected price and bid ceiling.
#[derive(Debug, Clone, PartialEq)]
pub struct GearWant {
    pub wearpos: i64,
    pub item: String,
    /// Style score gained over the best owned piece of the slot.
    pub gain: i64,
    pub expected: i64,
    pub ceiling: i64,
}

fn is_tool(name: &str) -> bool {
    name.ends_with("_pickaxe") || (name.ends_with("_axe") && !name.contains("battleaxe"))
}

/// Some F2P way yields it without a quest (quest rewards stay out: the
/// market never carries them for a bot that has not done the quest).
fn quest_free(name: &str) -> bool {
    lifescape_gamedata::obtain::ways(name).iter().any(|way| way.quest.is_none() && way.quest_hints().is_empty())
}

fn style_index(style: Style) -> usize {
    match style {
        Style::Melee => 0,
        Style::Ranged => 1,
        Style::Magic => 2,
    }
}

fn build(style: Style) -> Vec<GearPiece> {
    let mut out = Vec::new();
    for entry in all_wearables() {
        if !PLAN_SLOTS.contains(&entry.wearpos) || is_tool(&entry.name) {
            continue;
        }
        if style == Style::Ranged && entry.wearpos == SHIELD {
            continue;
        }
        // Melee and magic hold a shield: one-handed weapons only.
        if entry.wearpos == WEAPON && style != Style::Ranged && entry.wearpos2 == SHIELD {
            continue;
        }
        let value = score(&entry, style);
        if value <= 0 {
            continue;
        }
        let tradeable_f2p = lifescape_gamedata::objs_by_name(&entry.name).into_iter().next().is_some_and(|obj| obj.tradeable && obj.members == Some(false));
        if !tradeable_f2p || !lifescape_gamedata::obtain::obtainable_f2p(&entry.name) || !quest_free(&entry.name) {
            continue;
        }
        out.push(GearPiece { wearpos: entry.wearpos, item: entry.name.clone(), score: value, entry });
    }
    out.sort_by(|left, right| right.score.cmp(&left.score).then_with(|| left.item.cmp(&right.item)));
    out
}

/// Every piece `style` may want, best score first (built once).
pub fn candidates(style: Style) -> &'static [GearPiece] {
    static TABLES: OnceLock<[Vec<GearPiece>; 3]> = OnceLock::new();
    let tables = TABLES.get_or_init(|| [build(Style::Melee), build(Style::Ranged), build(Style::Magic)]);
    &tables[style_index(style)]
}

/// Style score of an owned piece in `wearpos` the bot may wear now.
fn owned_score(style: Style, wearpos: i64, owned: &[String], levels: &[(&str, i64)], quests_done: &[&str]) -> i64 {
    owned
        .iter()
        .filter_map(|name| lifescape_gamedata::equipment::by_name(name))
        .filter(|entry| entry.wearpos == wearpos && wieldable(entry, levels, quests_done))
        .map(|entry| score(&entry, style))
        .max()
        .unwrap_or(0)
}

/// What the bot wants this pass (module doc). `owned`: pack, bank and
/// worn debug names; `budget`: coins the gear may take (a share of the
/// free wallet); `price_of`: expected price of one object.
pub fn plan_gear(style: Style, levels: &[(&str, i64)], quests_done: &[&str], owned: &[String], budget: i64, price_of: &dyn Fn(&str) -> i64) -> Vec<GearWant> {
    if budget < 1 {
        return Vec::new();
    }
    // Per slot: upgrades the bot may wear now, best first, each strictly
    // cheaper than every better one (a dearer, weaker piece is never a
    // step down).
    let mut slots: Vec<(i64, i64, Vec<(String, i64, i64)>)> = Vec::new();
    for wearpos in PLAN_SLOTS {
        if style == Style::Ranged && wearpos == SHIELD {
            continue;
        }
        let have = owned_score(style, wearpos, owned, levels, quests_done);
        let mut options: Vec<(String, i64, i64)> = Vec::new();
        for piece in candidates(style).iter().filter(|piece| piece.wearpos == wearpos && piece.score > have) {
            if !wieldable(&piece.entry, levels, quests_done) || owned.contains(&piece.item) {
                continue;
            }
            let price = price_of(&piece.item).max(1);
            if options.last().is_none_or(|(_, _, cheapest)| price < *cheapest) {
                options.push((piece.item.clone(), piece.score, price));
            }
        }
        if !options.is_empty() {
            slots.push((wearpos, have, options));
        }
    }
    // Every slot at its best piece; the dearest steps down while the sum
    // passes the budget.
    let mut picks: Vec<Option<usize>> = vec![Some(0); slots.len()];
    loop {
        let total: i64 = picks.iter().zip(&slots).filter_map(|(pick, slot)| pick.map(|at| slot.2[at].2)).sum();
        if total <= budget {
            break;
        }
        let dearest = picks
            .iter()
            .enumerate()
            .filter_map(|(index, pick)| pick.map(|at| (index, slots[index].2[at].2)))
            .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.cmp(&left.0)))
            .map(|(index, _)| index);
        let Some(index) = dearest else { break };
        picks[index] = picks[index].and_then(|at| (at + 1 < slots[index].2.len()).then_some(at + 1));
    }
    let total: i64 = picks.iter().zip(&slots).filter_map(|(pick, slot)| pick.map(|at| slot.2[at].2)).sum();
    if total < 1 {
        return Vec::new();
    }
    let mut wants = Vec::new();
    for (pick, (wearpos, have, options)) in picks.iter().zip(&slots) {
        let Some(at) = pick else { continue };
        let (item, value, expected) = &options[*at];
        let share = (i128::from(*expected) * i128::from(budget) / i128::from(total)) as i64;
        wants.push(GearWant { wearpos: *wearpos, item: item.clone(), gain: value - have, expected: *expected, ceiling: share.max(*expected) });
    }
    wants
}

/// Owned wearables the bot keeps (never listed for sale): for every
/// style and slot, the best piece it may wear now and any better one it
/// cannot wear yet. Lower pieces are surplus once a better one is owned.
pub fn kept_gear(levels: &[(&str, i64)], quests_done: &[&str], owned: &[String]) -> Vec<String> {
    let entries: Vec<Wearable> = owned.iter().filter(|name| !is_tool(name)).filter_map(|name| lifescape_gamedata::equipment::by_name(name)).collect();
    let mut keep = Vec::new();
    for style in [Style::Melee, Style::Ranged, Style::Magic] {
        for wearpos in PLAN_SLOTS {
            let best_now = entries.iter().filter(|entry| entry.wearpos == wearpos && wieldable(entry, levels, quests_done)).map(|entry| score(entry, style)).max();
            for entry in entries.iter().filter(|entry| entry.wearpos == wearpos) {
                let value = score(entry, style);
                if value > 0 && best_now.is_none_or(|best| value >= best) {
                    keep.push(entry.name.clone());
                }
            }
        }
    }
    keep.sort();
    keep.dedup();
    keep
}

/// An owned piece the bot may wear that beats what it wears in that slot
/// for `style`: bought or delivered, not worn yet.
pub fn unworn_upgrade(style: Style, levels: &[(&str, i64)], quests_done: &[&str], owned: &[String], worn: &[String]) -> Option<String> {
    let worn_entries: Vec<Wearable> = worn.iter().filter_map(|name| lifescape_gamedata::equipment::by_name(name)).collect();
    let mut best: Option<(usize, i64, String)> = None;
    for name in owned.iter().filter(|name| !worn.contains(name) && !is_tool(name)) {
        let Some(entry) = lifescape_gamedata::equipment::by_name(name) else { continue };
        let Some(priority) = PLAN_SLOTS.iter().position(|slot| *slot == entry.wearpos) else { continue };
        if (style == Style::Ranged && entry.wearpos == SHIELD) || !wieldable(&entry, levels, quests_done) {
            continue;
        }
        if entry.wearpos == WEAPON && style != Style::Ranged && entry.wearpos2 == SHIELD {
            continue;
        }
        let value = score(&entry, style);
        let current = worn_entries.iter().filter(|worn| worn.wearpos == entry.wearpos).map(|worn| score(worn, style)).max().unwrap_or(0);
        if value > current.max(0) && best.as_ref().is_none_or(|(at, gain, _)| priority < *at || (priority == *at && value - current > *gain)) {
            best = Some((priority, value - current, name.clone()));
        }
    }
    best.map(|(_, _, name)| name)
}

#[cfg(test)]
#[path = "gear_plan_tests.rs"]
mod tests;
