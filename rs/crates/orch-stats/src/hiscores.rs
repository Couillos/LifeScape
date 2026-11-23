#![forbid(unsafe_code)]
//! Hiscores (user 2026-11-14, page `/hiscores`): skill rankings in the
//! style of the 2004 hiscores plus two wealth rankings (coins, total
//! value at the market current price). Pure model: holders in, one
//! JSON snapshot out. Gathering, caching and the 30-minute schedule
//! live in `hiscores_service`; the endpoint only serves the snapshot.
//!
//! Rules:
//! - skills are the 19 rev-274 stats in the hiscores order (ids 0..17
//!   and 20; 18/19 are unused), level = base level from the XP (never
//!   the boosted or drained current level);
//! - overall = total level, then total XP; a skill = its XP; coins =
//!   pack + bank; value = every held object (pack + bank + worn) at the
//!   market current price, coins at 1, untradeable or unpriced at 0;
//! - ties are broken by account name, so every rank is unique.

use std::collections::BTreeMap;

use lifescape_json::{Value, object, str_value};

/// Snapshot period: at most one snapshot per half hour.
pub const INTERVAL_SECONDS: i64 = 30 * 60;
/// First snapshot after a daemon start (after the restore spike).
pub const START_DELAY_SECONDS: i64 = 240;
/// A life file written within this window makes its account a player.
pub const LIFE_ACTIVE_SECONDS: u64 = 24 * 3600;
/// Most valuable objects listed per player.
pub const TOP_ITEMS: usize = 10;
pub const COINS: i64 = 995;

/// Ranked skills in the 2004 hiscores order: (stat id, name).
pub const SKILLS: [(u8, &str); 19] = [
    (0, "attack"),
    (1, "defence"),
    (2, "strength"),
    (3, "hitpoints"),
    (4, "ranged"),
    (5, "prayer"),
    (6, "magic"),
    (7, "cooking"),
    (8, "woodcutting"),
    (9, "fletching"),
    (10, "fishing"),
    (11, "firemaking"),
    (12, "crafting"),
    (13, "smithing"),
    (14, "mining"),
    (15, "herblore"),
    (16, "agility"),
    (17, "thieving"),
    (20, "runecraft"),
];

/// Position of a stat id in `SKILLS`, None for unranked stats.
pub fn skill_slot(stat: i64) -> Option<usize> {
    SKILLS.iter().position(|(id, _)| i64::from(*id) == stat)
}

/// One player as gathered: display XP per `SKILLS` slot and every held
/// stack (pack, worn, bank; repeated ids allowed).
#[derive(Clone, Debug, PartialEq)]
pub struct Holder {
    pub account: String,
    pub iron: bool,
    /// "live" (pushed snapshot) or "save" (engine save file).
    pub source: &'static str,
    pub xp: [i64; 19],
    pub items: Vec<(i64, i64)>,
}

/// Base level of a display XP amount (server units are 10x).
pub fn base_level(xp: i64) -> i64 {
    i64::from(lifescape_gamedata::xp::level_for_xp(xp.max(0).saturating_mul(10)))
}

impl Holder {
    pub fn total_level(&self) -> i64 {
        self.xp.iter().map(|xp| base_level(*xp)).sum()
    }

    pub fn total_xp(&self) -> i64 {
        self.xp.iter().map(|xp| (*xp).max(0)).sum()
    }

    /// Coins in the pack and the bank (`items` never holds worn coins).
    pub fn coins(&self) -> i64 {
        self.items.iter().filter(|(obj, _)| *obj == COINS).map(|(_, count)| (*count).max(0)).fold(0, i64::saturating_add)
    }
}

/// One priced stack of a player's top list.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemValue {
    pub obj: i64,
    pub count: i64,
    pub price: i64,
    pub value: i64,
}

/// Total value of `items` and the `TOP_ITEMS` most valuable stacks
/// (same objects merged, a banknote under its object: user 2026-11-19,
/// one object valued the same whatever its id; worthless ones left out).
/// `price_of` gives the unit price (0 = untradeable or unpriced); it is
/// called once per object, with the object's base id.
pub fn valuation(items: &[(i64, i64)], price_of: &mut dyn FnMut(i64) -> i64) -> (i64, Vec<ItemValue>) {
    let mut merged: BTreeMap<i64, i64> = BTreeMap::new();
    for (obj, count) in items {
        if *count > 0 {
            let object = usize::try_from(*obj).map(|id| lifescape_gamedata::base_obj_id(id) as i64).unwrap_or(*obj);
            let slot = merged.entry(object).or_insert(0);
            *slot = slot.saturating_add(*count);
        }
    }
    let mut total = 0i64;
    let mut priced: Vec<ItemValue> = Vec::new();
    for (obj, count) in merged {
        let price = price_of(obj).max(0);
        let value = count.saturating_mul(price);
        total = total.saturating_add(value);
        if value > 0 {
            priced.push(ItemValue { obj, count, price, value });
        }
    }
    priced.sort_by(|left, right| right.value.cmp(&left.value).then(left.obj.cmp(&right.obj)));
    priced.truncate(TOP_ITEMS);
    (total, priced)
}

/// 1-based unique rank of each holder: `key` (of the holder's index)
/// descending, then account name ascending.
pub fn ranks<K: Ord>(holders: &[Holder], key: impl Fn(usize) -> K) -> Vec<i64> {
    let keys: Vec<K> = (0..holders.len()).map(key).collect();
    let mut order: Vec<usize> = (0..holders.len()).collect();
    order.sort_by(|left, right| keys[*right].cmp(&keys[*left]).then_with(|| holders[*left].account.cmp(&holders[*right].account)));
    let mut ranked = vec![0i64; holders.len()];
    for (position, index) in order.into_iter().enumerate() {
        ranked[index] = position as i64 + 1;
    }
    ranked
}

/// Everything `build` needs besides the holders.
pub struct Context<'ctx> {
    pub generated_at: i64,
    pub next_at: i64,
    /// Unit price of an object, None when the market is down.
    pub price_of: &'ctx mut dyn FnMut(i64) -> Option<i64>,
    pub item_name: &'ctx dyn Fn(i64) -> String,
    pub display_name: &'ctx dyn Fn(&str) -> String,
    pub errors: Vec<String>,
}

/// Who counts as a player (shown on the page, documented in
/// docs/orchestrator.md).
pub const RULE: &str = "joueurs = comptes suivis par le daemon + vies (run/lives) écrites depuis moins de 24 h, \
hors admin, bite, run/economy-excluded.txt et proof-* ; niveaux = niveau de base tiré de l'XP ; \
valeur = sac + banque + porté au prix courant du marché (pièces = 1, non échangeable ou sans prix = 0)";

/// The snapshot served by `/api/hiscores` (shape in docs/orchestrator.md):
/// players in overall order, each with `[rank, level, xp]` per skill.
pub fn build(holders: &[Holder], context: Context<'_>) -> Value {
    let Context { generated_at, next_at, price_of, item_name, display_name, mut errors } = context;
    let mut memo: BTreeMap<i64, Option<i64>> = BTreeMap::new();
    let mut market_up = true;
    let mut lookup = |obj: i64| -> i64 {
        if obj == COINS {
            return 1;
        }
        let price = *memo.entry(obj).or_insert_with(|| price_of(obj));
        match price {
            Some(price) => price,
            None => {
                market_up = false;
                0
            }
        }
    };
    let wealth: Vec<(i64, Vec<ItemValue>)> = holders.iter().map(|holder| valuation(&holder.items, &mut lookup)).collect();
    if !market_up {
        errors.push("market down: total value not ranked".to_string());
    }
    let overall = ranks(holders, |index| (holders[index].total_level(), holders[index].total_xp()));
    let skill_ranks: Vec<Vec<i64>> = (0..SKILLS.len()).map(|slot| ranks(holders, |index| holders[index].xp[slot])).collect();
    let coin_ranks = ranks(holders, |index| holders[index].coins());
    let value_ranks = ranks(holders, |index| wealth[index].0);
    let mut order: Vec<usize> = (0..holders.len()).collect();
    order.sort_by_key(|index| overall[*index]);
    let triple = |rank: i64, level: i64, xp: i64| Value::Arr(vec![Value::Int(rank), Value::Int(level), Value::Int(xp)]);
    let players: Vec<Value> = order
        .iter()
        .map(|index| {
            let holder = &holders[*index];
            let skills: Vec<Value> =
                (0..SKILLS.len()).map(|slot| triple(skill_ranks[slot][*index], base_level(holder.xp[slot]), holder.xp[slot].max(0))).collect();
            let (total, top) = &wealth[*index];
            let top_rows: Vec<Value> = top
                .iter()
                .map(|item| {
                    object(vec![
                        ("id", Value::Int(item.obj)),
                        ("name", str_value(&item_name(item.obj))),
                        ("count", Value::Int(item.count)),
                        ("price", Value::Int(item.price)),
                        ("value", Value::Int(item.value)),
                    ])
                })
                .collect();
            let value = if market_up {
                Value::Arr(vec![Value::Int(value_ranks[*index]), Value::Int(*total)])
            } else {
                Value::Null
            };
            object(vec![
                ("account", str_value(&holder.account)),
                ("name", str_value(&display_name(&holder.account))),
                ("iron", Value::Bool(holder.iron)),
                ("source", str_value(holder.source)),
                ("overall", triple(overall[*index], holder.total_level(), holder.total_xp())),
                ("skills", Value::Arr(skills)),
                ("coins", Value::Arr(vec![Value::Int(coin_ranks[*index]), Value::Int(holder.coins())])),
                ("value", value),
                ("top", Value::Arr(top_rows)),
            ])
        })
        .collect();
    object(vec![
        ("ok", Value::Bool(true)),
        ("generated_at", Value::Int(generated_at)),
        ("next_at", Value::Int(next_at)),
        ("interval_seconds", Value::Int(INTERVAL_SECONDS)),
        ("skills", Value::Arr(SKILLS.iter().map(|(_, name)| str_value(name)).collect())),
        ("market", Value::Bool(market_up)),
        ("rule", str_value(RULE)),
        ("errors", Value::Arr(errors.iter().map(|error| str_value(error)).collect())),
        ("players", Value::Arr(players)),
    ])
}

#[cfg(test)]
#[path = "hiscores_tests.rs"]
mod tests;
