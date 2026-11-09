//! Bootstrap: the life decisions woodcut used to make for itself —
//! selling the pack to fund the axe, hunting goblins for 16 coins,
//! buying upgrades. Moved verbatim during the kit transition so the
//! activity stays autonomous in live tests; the brain replaces all of
//! this in L7 (obtain_items / make_money tasks), then this module goes
//! away. Nothing here is shared kit: no other activity may call it.

#![forbid(unsafe_code)]

use std::collections::HashSet;

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::skill_common::{
    bank_trip, buy_stock, nearest_bank, obj_id, obj_price, pack_ids, pack_total, sell_for_target,
};
use crate::skill_gather::reattach_tool;
use crate::skill_woodcutting::{AXE_SPEC, HANDLE_NAME};
use crate::ScriptError;

/// Broke and axeless: kill goblins east of Lumbridge for coin drops
/// until the pack funds the axe. Bounded rounds, each a few kills; the hunt
/// loot stays in the pack for the shared buy below, but anything older
/// (banked logs from a failed sale, shop junk) is stashed first so combat
/// never rides with a dead stock. No gain stops early. True when the pack
/// now affords the axe.
/// Budget: goblin coin drops average ~1.6/kill (goblin.rs2 drop table, 27
/// coin rolls per 128), so 30 kills expect ~47 coins and P(<16) is ~2%.
/// True when the pack holds anything but axes and coins: shop stock or
/// loot from another job that must not ride into combat (death drops it,
/// and it clogs the slots the fight needs).
pub(crate) fn combat_pack_dirty(bot: &Bot, coins: u16) -> bool {
    let axes = AXE_SPEC.known_ids(bot);
    pack_ids(bot).iter().any(|id| *id != coins && !axes.contains(id))
}

pub(crate) fn hunt_goblins(bot: &mut Bot, coins: u16) -> Result<bool, ScriptError> {
    // Combat pack is combat loot only: logs or shop junk riding along die
    // with the bot (or clog the pack), and a failed sale above is exactly
    // how they get there. Stash everything but axes and coins first, so a
    // missed sale banks its stock instead of marching it into the hunt.
    if combat_pack_dirty(bot, coins) {
        let mut keep = AXE_SPEC.known_ids(bot);
        keep.insert(coins);
        let bank = nearest_bank(bot)?;
        bank_trip(bot, &bank, &keep.into_iter().collect::<Vec<_>>(), 0)?;
    }
    for _ in 0..5 {
        if pack_total(bot, coins) >= 16 {
            return Ok(true);
        }
        let before = pack_total(bot, coins);
        crate::skill_combat::run_melee(
            bot,
            &["goblins".to_string(), "--kills".to_string(), "6".to_string(), "--no-bank".to_string()],
        )?;
        if pack_total(bot, coins) <= before {
            break;
        }
    }
    Ok(pack_total(bot, coins) >= 16)
}

/// Pack ids the bootstrap may sell for the axe: everything carried that
/// is neither an axe, coins, the unbankable genie lamp, nor the random-
/// event cube. Ground loot (daggers, spare logs) funds the axe instead
/// of forcing a bank trip just to dump it; event items are never sale
/// stock (selling the cube would strand its rescue).
pub(crate) fn bootstrap_products(bot: &Bot, coins: u16) -> Vec<u16> {
    let axes = AXE_SPEC.known_ids(bot);
    let lamp = (bot.tables.find_obj_by_name)("macro_genilamp");
    let cube = (bot.tables.find_obj_by_name)("macro_cube");
    pack_ids(bot)
        .into_iter()
        .filter(|id| *id != coins && !axes.contains(id) && Some(*id) != lamp && Some(*id) != cube)
        .collect()
}

/// One axe buy attempt with bronze fallback: tries the affordable cands
/// best-first, one attempt each, downgrading on any shop failure (empty
/// live stock, short coins). Conn/death/interrupt still abort; anything
/// else logs and tries the next cheaper axe. Returns the bought name or
/// None when nothing affordable succeeded. Never loops: one pass, no retry.
pub(crate) fn buy_axe_upgrade(bot: &mut Bot, cands: &[String]) -> Result<Option<String>, ScriptError> {
    let have = pack_total(bot, obj_id(bot, "coins")?);
    for cand in cands {
        let price = obj_price(cand);
        if price <= 0 || have < price as u32 {
            bot.log(&format!("skip {cand}: costs {price}, have {have}"));
            continue;
        }
        match buy_stock(bot, cand, 1) {
            Ok(bought) if bought > 0 => {
                bot.log(&format!("ok upgraded to {cand}"));
                return Ok(Some(cand.clone()));
            }
            Ok(_) => {
                bot.log(&format!("{cand} trade failed at {:?}, trying cheaper", bot.position()));
            }
            Err(error) if crate::skill_common::is_death_abort(&error) => return Err(error),
            Err(error) if crate::skill_common::is_conn_abort(&error) => return Err(error),
            Err(error) if matches!(error, ScriptError::Sdk(lifescape_sdk::Error::Interrupted(_))) => return Err(error),
            Err(error) => {
                bot.log(&format!("{cand} buy failed ({error}), trying cheaper"));
            }
        }
    }
    Ok(None)
}

/// True when the pack alone can pay the upgrade right now: buy
/// immediately, never hoard coins through the chop.
pub(crate) fn can_buy_now(have_coins: u32, cheapest: i64, cands: &[String]) -> bool {
    !cands.is_empty() && cheapest > 0 && have_coins >= cheapest as u32
}

/// Sell the pack (logs and ground-loot junk alike) at the nearby general
/// store and buy the bronze axe when funded. True when the axe is now
/// held. The hunt loot only exists AFTER the hunt, so the caller runs this
/// both before and after: selling late beats losing it to the bank trek
/// (junk stranded, unsold) or to death (dropped).
pub(crate) fn sell_pack_and_buy(bot: &mut Bot, bank: &mut knowledge::banks::Bank, coins: u16, label: &str) -> Result<bool, ScriptError> {
    let funded = bootstrap_products(bot, coins);
    let have = pack_total(bot, coins);
    if !funded.is_empty() && have < 16 {
        let names: Vec<String> =
            funded.iter().map(|id| (bot.tables.obj_name)(*id).unwrap_or_else(|| format!("obj{id}"))).collect();
        bot.log(&format!("{label}: selling {names:?} for {have} coins at {:?}", bot.position()));
        match sell_for_target(bot, &funded, coins, 16 - have) {
            Ok(_) => {}
            Err(error) if crate::skill_common::is_death_abort(&error) => return Err(error),
            Err(err) => bot.log(&format!("{label} sale failed: {err}, trying the bank")),
        }
    }
    if pack_total(bot, coins) >= 16 {
        match buy_stock(bot, "bronze_axe", 1) {
            Ok(bought) if bought > 0 => {
                bot.log(&format!("ok bootstrapped bronze axe from {label}"));
                *bank = nearest_bank(bot)?;
                return Ok(true);
            }
            Ok(_) => {}
            Err(error) if crate::skill_common::is_death_abort(&error) => return Err(error),
            Err(err) => bot.log(&format!("{label} axe buy failed: {err}, trying the bank")),
        }
    }
    Ok(AXE_SPEC.best_held(bot).is_some())
}

/// Axe bootstrap, reusable after every death: reattach from the pack,
/// sell the pack and buy from the keeper, else bank-trip, restore a
/// banked axe, else withdraw and sell banked logs to fund one. Returns
/// the banked coin total for upgrade budgeting. Loud when broke: an axe
/// cannot be conjured from nothing.
pub(crate) fn ensure_axe(
    bot: &mut Bot,
    bank: &mut knowledge::banks::Bank,
    coins: u16,
    log_ids: &HashSet<u16>,
) -> Result<Option<u32>, ScriptError> {
    let mut bank_coins: Option<u32> = None;
    if AXE_SPEC.best_held(bot).is_none() || pack_ids(bot).iter().any(|id| !AXE_SPEC.known_ids(bot).contains(id) && !log_ids.contains(id) && *id != coins) {
        if reattach_tool(bot, HANDLE_NAME, "_hatchethead", &AXE_SPEC)? {
            bot.log("ok reattached axe from pack, no bootstrap needed");
        }
        let keep: Vec<u16> = log_ids.iter().copied().collect();
        // An axe is bought with wood, not with a bank trip: sell the pack
        // (logs and ground-loot junk alike) at the nearby general store
        // and buy from the axe keeper first. Every trek below only runs
        // when the pack cannot fund the axe on its own.
        // Sale hiccups are never fatal here: the hunt below is exactly
        // what funds the axe, so log and keep going.
        if AXE_SPEC.best_held(bot).is_none() {
            let bought = match sell_pack_and_buy(bot, &mut *bank, coins, "pack sale") {
                Ok(bought) => bought,
                Err(error) if crate::skill_common::is_death_abort(&error) => return Err(error),
                Err(error) if crate::skill_common::is_conn_abort(&error) => return Err(error),
                Err(error)
                    if matches!(error, ScriptError::Sdk(lifescape_sdk::Error::Interrupted(_))) =>
                {
                    return Err(error)
                }
                Err(error) => {
                    bot.log(&format!("pack sale failed: {error}, hunting to fund the axe"));
                    false
                }
            };
            // Broke with nothing worth selling: earn the axe on goblins
            // east of Lumbridge first, then sell THAT loot too before
            // falling to the bank below.
            if !bought && pack_total(bot, coins) < 16 {
                match hunt_goblins(bot, coins) {
                    Ok(true) => {}
                    Ok(false) => bot.log("goblin hunt came up short, trying the bank"),
                    Err(error) if crate::skill_common::is_death_abort(&error) => return Err(error),
                    Err(err) => bot.log(&format!("goblin hunt failed: {err}, trying the bank")),
                }
                let _ = match sell_pack_and_buy(bot, &mut *bank, coins, "hunt loot") {
                    Ok(bought) => bought,
                    Err(error) if crate::skill_common::is_death_abort(&error) => return Err(error),
                    Err(error) if crate::skill_common::is_conn_abort(&error) => return Err(error),
                    Err(error)
                        if matches!(error, ScriptError::Sdk(lifescape_sdk::Error::Interrupted(_))) =>
                    {
                        return Err(error)
                    }
                    Err(error) => {
                        bot.log(&format!("hunt loot sale failed: {error}, trying the bank"));
                        false
                    }
                };
            }
        }
        if AXE_SPEC.best_held(bot).is_none() {
            bank_coins = Some(bank_trip(bot, bank, &keep, 0)?);
            if crate::kit::tools::restore_from_bank(bot, bank, &AXE_SPEC, HANDLE_NAME, "_hatchethead")? {
                bot.log("ok axe restored from bank");
            }
        }
        if AXE_SPEC.best_held(bot).is_none() {
            // Toll crossing costs 10 coins each way: budget it only when
            // the bank sits across the fence, then pay the guard.
            let need_cross = crate::skill_common::toll_gate_x(bot)
                .is_some_and(|line| (bank.x >= line as i64) != (bot.world.x >= line));
            let target = 16u32 + if need_cross { 10 } else { 0 };
            bank_coins = Some(bank_trip(bot, bank, &keep, u32::MAX / 2)?);
            let have = pack_total(bot, coins);
            if have < target {
                // No wood in the pack: pull the banked logs back out and
                // sell those (an axe is bought with wood, not with nothing).
                let log_list: Vec<u16> = log_ids.iter().copied().collect();
                let carried: u32 = log_list.iter().map(|id| pack_total(bot, *id)).sum();
                if carried == 0 {
                    let pulled = crate::skill_common::bank_withdraw_items(bot, bank, &log_list, 10)?;
                    if pulled > 0 {
                        bot.log(&format!("withdrew {pulled} logs from the bank to fund the axe"));
                    }
                }
                sell_for_target(bot, &log_list, coins, target - pack_total(bot, coins))?;
            }
            if need_cross && !crate::skill_common::cross_toll_gate(bot)? {
                return Err(ScriptError::Usage(format!("cannot cross toll gate at {:?}", bot.position())));
            }
            // Bronze-first bootstrap: one attempt, then fall through broke.
            // Being broke is never fatal: the main loop sees no axe and
            // hunts again. Only death/conn/interrupt abort. Callers fall
            // back to the hunt, never to a buy loop.
            if pack_total(bot, coins) >= 16 {
                match buy_stock(bot, "bronze_axe", 1) {
                    Ok(bought) if bought > 0 => {
                        bot.log("ok bootstrapped bronze axe");
                        *bank = nearest_bank(bot)?;
                    }
                    Ok(_) => {
                        bot.log(&format!(
                            "cannot bootstrap yet: no axe, {} coins (bronze trade failed at {:?}), hunting again",
                            pack_total(bot, coins),
                            bot.position()
                        ));
                    }
                    Err(error) if crate::skill_common::is_death_abort(&error) => return Err(error),
                    Err(error) if crate::skill_common::is_conn_abort(&error) => return Err(error),
                    Err(error) if matches!(error, ScriptError::Sdk(lifescape_sdk::Error::Interrupted(_))) => {
                        return Err(error)
                    }
                    Err(error) => {
                        bot.log(&format!(
                            "cannot bootstrap yet: no axe, {} coins (bronze out of stock at {:?}: {error}), hunting again",
                            pack_total(bot, coins),
                            bot.position()
                        ));
                    }
                }
            } else {
                bot.log(&format!(
                    "cannot bootstrap yet: no axe, {} coins, hunting again",
                    pack_total(bot, coins)
                ));
            }
        }
    }
    Ok(bank_coins)
}

#[cfg(test)]
mod tests {
    use super::{bootstrap_products, can_buy_now, combat_pack_dirty};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    /// Combat pack rule: logs or shop junk force a stash, axes and coins
    /// ride along. A missed sale must bank its stock, never march it into
    /// the hunt.
    #[test]
    fn combat_pack_dirty_iff_noncombat_carried() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let coins = crate::skill_common::obj_id(&bot, "coins").expect("coins in tables");
        let logs = crate::skill_common::obj_id(&bot, "logs").expect("logs in tables");
        let com = (bot.tables.com_id)("inventory:inv").expect("inventory:inv in dumps");
        assert!(!combat_pack_dirty(&bot, coins), "empty pack hunts as-is");
        bot.world.inv.insert(com, [(0u8, (coins, 3u32))].into_iter().collect());
        assert!(!combat_pack_dirty(&bot, coins), "coins ride along");
        bot.world.inv.insert(com, [(0u8, (coins, 3u32)), (1, (logs, 6))].into_iter().collect());
        assert!(combat_pack_dirty(&bot, coins), "banked logs must stash before the hunt");
    }

    /// Bootstrap sale set: ground loot and logs fund the axe, axes/coins
    /// and the unbankable lamp never sell.
    #[test]
    fn bootstrap_products_sells_loot_never_axe_or_lamp() {        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let coins = crate::skill_common::obj_id(&bot, "coins").expect("coins in tables");
        let axe = crate::skill_common::obj_id(&bot, "bronze_axe").expect("bronze_axe in tables");
        let dagger = crate::skill_common::obj_id(&bot, "bronze_dagger").expect("bronze_dagger in tables");
        let logs = crate::skill_common::obj_id(&bot, "logs").expect("logs in tables");
        let com = (bot.tables.com_id)("inventory:inv").expect("inventory:inv in dumps");
        bot.world.inv.insert(
            com,
            [(0u8, (coins, 3u32)), (1, (axe, 1)), (2, (dagger, 1)), (3, (logs, 2))]
                .into_iter()
                .collect(),
        );
        let funded = bootstrap_products(&bot, coins);
        assert!(funded.contains(&dagger), "ground loot funds the axe");
        assert!(funded.contains(&logs), "logs fund the axe");
        assert!(!funded.contains(&coins), "coins never sell");
        assert!(!funded.contains(&axe), "axes never sell");
        if let Some(lamp) = (bot.tables.find_obj_by_name)("macro_genilamp") {
            assert!(!funded.contains(&lamp), "lamp never sells");
        }
        if let Some(cube) = (bot.tables.find_obj_by_name)("macro_cube") {
            assert!(!funded.contains(&cube), "event cube never sells");
        }
    }

    /// Affordable upgrade: buy right away. Short, no candidates, or no
    /// price: coins stay reserved for the bank, never spent mid-chop.
    #[test]
    fn upgrade_bought_immediately_when_affordable() {
        let cands = vec!["iron_axe".to_string()];
        assert!(can_buy_now(613, 500, &cands), "613 coins pay 500 now");
        assert!(can_buy_now(500, 500, &cands), "exact price pays now");
        assert!(!can_buy_now(109, 500, &cands), "109 coins stay reserved for the bank");
        assert!(!can_buy_now(999, 500, &[]), "no candidates, nothing to buy");
        assert!(!can_buy_now(999, 0, &cands), "no price, nothing to buy");
    }
}
