//! Buy from an NPC that sells through its dialogue (Karim's kebabs,
//! Baraek's furs, the bartenders' beer: `gamedata::dialogue_shops`, only
//! what the merchant rule allows): `dialogue_buy <item> --want <item> N
//! [--site x,z,level] [toll] --node <id>`. One unit per conversation;
//! the coins come from the pack (fetched from the bank first), the pack
//! banks when full. The brain picked the seller (`--site`).

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::pack::{free_slots, obj_id, pack_slots, pack_total};
use crate::kit::survival::{check_events, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};

/// Ticks one conversation may take (walk to the seller, pages, choices).
const TALK_TICKS: u64 = 120;
/// Conversations in a row that hand over nothing before the run fails.
const DRY_TALKS: u32 = 3;

/// The seller of `item` at `site` (the brain's pick), else the nearest
/// F2P one to `from`.
pub fn seller(item: &str, site: Option<(i32, i32, i32)>, from: (i32, i32)) -> Option<(&'static knowledge::dialogue_shops::DialogueShop, (i64, i64, i64))> {
    let mut best: Option<(i64, &'static knowledge::dialogue_shops::DialogueShop, (i64, i64, i64))> = None;
    for shop in knowledge::dialogue_shops::allowed(item) {
        for spawn in knowledge::dialogue_shops::sites(shop) {
            let far = match site {
                Some((x, z, level)) if i64::from(level) == spawn.2 => (spawn.0 - i64::from(x)).abs().max((spawn.1 - i64::from(z)).abs()),
                Some(_) => continue,
                None => (spawn.0 - i64::from(from.0)).abs() + (spawn.1 - i64::from(from.1)).abs(),
            };
            if site.is_some() && far > 8 {
                continue;
            }
            if best.as_ref().is_none_or(|(known, _, _)| far < *known) {
                best = Some((far, shop, spawn));
            }
        }
    }
    best.map(|(_, shop, spawn)| (shop, spawn))
}

pub fn run_dialogue_buy(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = crate::kit::cycle::parse_order("dialogue_buy", args)?;
    let usage = || ScriptError::Usage("dialogue_buy <item> [--want <item> N] [--site x,z,level] [toll] [--node id]".to_string());
    let item = spec.resource.first().cloned().ok_or_else(usage)?;
    let want = spec.targets.want_count.or(spec.targets.count).unwrap_or(1).max(1);
    let Some((shop, spawn)) = seller(&item, spec.site, (bot.world.x, bot.world.z)) else {
        return Err(ScriptError::Usage(format!("no F2P dialogue seller of {item} (merchant rule) for site {:?}", spec.site)));
    };
    if !spec.allow_shop {
        return Err(ScriptError::Failed(format!("dialogue_buy {item}: purchases are off for this task")));
    }
    crate::kit::toll::arm_fares(bot, spec.allow_paid, false)?;
    bot.assigned_site = spec.site;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let item_id = obj_id(bot, &item)?;
    let coins = obj_id(bot, "coins")?;
    let reserve = if bot.allow_paid { bot.fare_reserve } else { 0 };
    let budget = shop.cost as u32 * want + reserve;
    let carried = pack_total(bot, coins);
    if carried < budget {
        let keep: Vec<u16> = pack_slots(bot).iter().map(|(_, (held, _))| *held).collect();
        let bank = crate::kit::bank::nearest_bank(bot)?;
        crate::kit::bank::bank_trip(bot, &bank, &keep, budget - carried)?;
        if pack_total(bot, coins) < shop.cost as u32 + reserve {
            return Err(ScriptError::Blocked { missing: vec![format!("coins:{budget}")] });
        }
    }
    bot.log(&format!("dialogue_buy: {want} {item} from {} at ({},{}) for {} coins each", shop.npc, spawn.0, spawn.1, shop.cost));
    let mut bought = 0u32;
    let mut dry = 0u32;
    while bought < want {
        check_events(bot)?;
        if pack_total(bot, coins) < shop.cost as u32 + reserve {
            bot.log(&format!("dialogue_buy: out of coins after {bought} {item}"));
            return Err(ScriptError::Blocked { missing: vec![format!("coins:{}", shop.cost as u32 * (want - bought))] });
        }
        if free_slots(bot) == 0 && pack_total(bot, item_id) > 0 {
            let bank = crate::kit::bank::nearest_bank(bot)?;
            crate::kit::bank::bank_trip(bot, &bank, &[coins], 0)?;
        }
        let before = pack_total(bot, item_id);
        bot.intend(&format!("buying {item} from {}", shop.npc));
        crate::quests::common::talk_plain(bot, shop.npc, shop.answers, Some((spawn.0 as i32, spawn.1 as i32)), TALK_TICKS)?;
        let gained = pack_total(bot, item_id).saturating_sub(before);
        if gained == 0 {
            dry += 1;
            if dry >= DRY_TALKS {
                return Err(ScriptError::Failed(format!("{} handed over no {item} in {DRY_TALKS} talks at {:?}", shop.npc, bot.position())));
            }
            continue;
        }
        dry = 0;
        bought += gained;
    }
    bot.log(&format!("dialogue_buy: {bought} {item} bought"));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::seller;

    /// The brain's site names the seller; without one the nearest wins.
    #[test]
    fn the_site_names_the_seller() {
        let (karim, at) = seller("kebab", None, (3222, 3218)).expect("Karim sells kebabs");
        assert_eq!(karim.npc, "kebab_seller");
        assert!(seller("kebab", Some((at.0 as i32, at.1 as i32, at.2 as i32)), (0, 0)).is_some());
        assert!(seller("kebab", Some((3222, 3218, 0)), (0, 0)).is_none(), "no seller at Lumbridge");
        assert!(seller("bronze_bar", None, (3222, 3218)).is_none());
    }
}
