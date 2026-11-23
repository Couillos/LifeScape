//! Market shop buys: NPC shop legs of the planner's `shop:buy` method.
//! Split from `market.rs` so every activity stays under the plafond.
//! Uses the shared `buy_stock` helper (nearest stocking shop, travel,
//! keeper trade): the brain files the obtain task, this only executes.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::pack::{free_slots, obj_id, pack_slots, pack_total, BACKPACK_SIZE};
use crate::{ScriptError, ScriptResult};

/// One `market shopbuy` order as parsed: the item, its quantity, the
/// planner's shop key (`--shop`, `gamedata::shops` name) and keeper site
/// (`--site`), and whether the toll gate may be paid (`toll`) and the
/// Karamja boat sailed (`boat`: a keeper on the island, the round trip
/// budgeted by the brain).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShopOrder {
    pub item: String,
    pub qty: u32,
    pub shop: Option<String>,
    pub site: Option<(i32, i32, i32)>,
    pub toll: bool,
    pub boat: bool,
    /// Uniform a guild keeper's door checks (`--wear`, worn before the walk).
    pub wear: Vec<String>,
}

/// Parse `market shopbuy <item> <qty> [--shop KEY] [--site x,z,level]
/// [toll] [boat] [--node ID] [--budget-* N]` (no world access).
pub fn parse_shopbuy(args: &[String]) -> Result<ShopOrder, ScriptError> {
    const USAGE: &str = "market shopbuy <item> <qty> [--shop KEY] [--site x,z,level] [--wear ITEM]... [toll] [boat]";
    let usage = || ScriptError::Usage(USAGE.to_string());
    let mut positionals: Vec<String> = Vec::new();
    let mut order = ShopOrder { item: String::new(), qty: 1, shop: None, site: None, toll: false, boat: false, wear: Vec::new() };
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--budget-max" | "--budget-min" | "--node" => index += 2,
            "toll" => {
                order.toll = true;
                index += 1;
            }
            "boat" => {
                order.boat = true;
                index += 1;
            }
            "--shop" => {
                order.shop = Some(args.get(index + 1).filter(|key| !key.is_empty() && !key.starts_with("--")).ok_or_else(usage)?.clone());
                index += 2;
            }
            "--wear" => {
                order.wear.push(args.get(index + 1).filter(|item| !item.is_empty() && !item.starts_with("--")).ok_or_else(usage)?.clone());
                index += 2;
            }
            "--site" => {
                let raw = args.get(index + 1).ok_or_else(usage)?;
                let parts: Vec<i32> = raw.split(',').map(|part| part.trim().parse::<i32>()).collect::<Result<_, _>>().map_err(|_| usage())?;
                let [x, z, level] = parts[..] else { return Err(usage()) };
                order.site = Some((x, z, level));
                index += 2;
            }
            other if other.starts_with("--") => return Err(usage()),
            _ => {
                positionals.push(args[index].clone());
                index += 1;
            }
        }
    }
    order.item = positionals.first().cloned().ok_or_else(usage)?;
    if let Some(raw) = positionals.get(1) {
        order.qty = raw.parse().map_err(|_| ScriptError::Usage("qty must be a number".to_string()))?;
    }
    Ok(order)
}

/// `market shopbuy <item> <qty> [--shop KEY] [--site x,z,level] [toll]`:
/// buy at the planner's shop (`--shop`), from its keeper nearest the
/// planner's site, never another shop. Without `--shop` (manual runs)
/// the nearest stocking shop sells. The exit is 0 when the full quantity
/// landed, 1 otherwise (the brain closes obtain nodes on holdings, never
/// on this code alone).
pub fn run_shopbuy(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let ShopOrder { item, qty, shop, site, toll, boat, wear } = parse_shopbuy(args)?;
    if boat {
        // A Karamja keeper: the boat's round trip in the pack, paid
        // crossings open (the toll too when the brain budgeted it).
        crate::kit::toll::arm_fares(bot, toll, true, site)?;
    } else if toll {
        bot.allow_paid = true;
    }
    let entry = lifescape_gamedata::objs_by_name(&item)
        .into_iter()
        .next()
        .ok_or_else(|| ScriptError::Usage(format!("unknown item {item:?}")))?;
    if entry.members == Some(true) {
        return Err(ScriptError::Usage(format!("{item:?} is not F2P (L1 refusal)")));
    }
    // Wallet is pack-first, bank-backed: withdraw the shortfall so a
    // funded bot is never stopped by pocket change. The shop itself
    // refuses true broke (0 bought, loud, no silent spend). A fresh
    // session has not seen its bank yet (live 2026-11-12: harry_page
    // went to the shop "carrying 0 coins" with 25 banked, three times):
    // the bank is opened to look whenever the pack falls short.
    let coins = obj_id(bot, "coins")?;
    let item_id = obj_id(bot, &item)?;
    // The keeper's dearest unit (stock 1, server formula), never the
    // dump value: a shop sells over it and the counter refuses the
    // difference ("You don't have enough coins").
    let price_each = lifescape_gamedata::shop_buy::unit_price_ceiling(&item, shop.as_deref()).unwrap_or(entry.price).clamp(1, i64::from(u32::MAX)) as u32;
    let need = (qty.max(1) as u32).saturating_mul(price_each);
    if site.is_some() {
        bot.assigned_site = site;
        crate::kit::uniform::wear_uniform(bot, &wear)?;
    }
    // One pass: coins, the keeper's site, the trade (`kit::site::pass_leg`).
    // A market pause banks the coins and leaves the bot at the Varrock
    // square (live 2026-11-19 lara_lane: "You don't have enough coins" at
    // the gem trader, "trade gave no emerald"): coins and walk again.
    let mut pauses_at_load: Option<u64> = None;
    let mut restarts = 0u32;
    let mut route_reloads = 0u32;
    let mut walked = false;
    let bought = loop {
        let here = (bot.world.x, bot.world.z, bot.world.level);
        match crate::kit::site::pass_leg(pauses_at_load, bot.pauses_done, walked, here, site) {
            crate::kit::site::PassLeg::Load => {
                if pauses_at_load.is_some() {
                    restarts += 1;
                    if restarts > crate::kit::site::PASS_PAUSE_RESTARTS {
                        return Err(ScriptError::Failed(format!("shopbuy {item}: {} market pauses already broke the trip, at {:?}", crate::kit::site::PASS_PAUSE_RESTARTS, bot.position())));
                    }
                    bot.log(&format!("shopbuy {item}: a market pause left us at {:?}, coins and shop again", bot.position()));
                }
                pauses_at_load = Some(bot.pauses_done);
                walked = false;
                let held = carry_coins(bot, coins, need, item_id, entry.stackable, qty)?;
                bot.log(&format!("shopbuy {item} x{qty}: carrying {held} coins, {} free slots", free_slots(bot)));
                let reserve = crate::kit::toll::pack_reserve(bot);
                if let Some(blocked) = unpaid_load(held, price_each, need, reserve) {
                    bot.log(&format!("shopbuy {item}: {held} coins carried, one unit costs up to {price_each} (+{reserve} fares): no walk"));
                    return Err(blocked);
                }
                if free_slots(bot) < item_slots(entry.stackable, pack_total(bot, item_id) > 0, 1) {
                    return Err(ScriptError::Failed(format!("shopbuy {item}: no pack slot for it after the bank (what stays fills the pack) at {:?}", bot.position())));
                }
            }
            crate::kit::site::PassLeg::Walk => {
                // The keeper's own floor: Scavvo and Valaine trade upstairs in the
                // Champions' Guild (level 1); the keeper pick reads the bot's level.
                let Some((x, z, level)) = site else { continue };
                if !crate::skill_walk::walk_checked_at(bot, x, z, level, crate::kit::site::SITE_WALK_TICKS)? {
                    return Err(ScriptError::Failed(format!("shopbuy {item}: shop site ({x},{z}) not reached from {:?}", bot.position())));
                }
                walked = true;
            }
            crate::kit::site::PassLeg::Act => {
                // A fare paid on the way (live 2026-11-21 tom_king: 30
                // carried on Karamja, the boat back took them, « trade
                // gave no lobster_pot »): coins again once, never a
                // counter that refuses every click.
                if pack_total(bot, coins) < price_each {
                    if route_reloads > 0 {
                        return Err(unpaid_load(pack_total(bot, coins), price_each, need, 0).unwrap_or_else(|| ScriptError::Failed(format!("shopbuy {item}: coins spent on the way at {:?}", bot.position()))));
                    }
                    route_reloads += 1;
                    bot.log(&format!("shopbuy {item}: {} coins left at the shop, one unit costs up to {price_each}: coins again", pack_total(bot, coins)));
                    pauses_at_load = None;
                    continue;
                }
                let outcome = crate::kit::shop::buy_stock_at(bot, &item, qty, shop.as_deref());
                let paused = pauses_at_load.is_some_and(|mark| crate::kit::site::paused_since(mark, bot.pauses_done));
                match outcome {
                    Ok(bought) if bought > 0 || !paused => break bought,
                    Err(ScriptError::Failed(_)) if paused => continue,
                    Ok(_) => continue,
                    Err(error) => return Err(error),
                }
            }
        }
    };
    // The pack alone: the shop's stock and the bank stack stream too.
    let held = pack_total(bot, item_id);
    bot.log(&format!("shopbuy {item} x{qty}: bought {bought}, holding {held}"));
    if !shop_bought_any(bought, held, qty) {
        let from = shop.as_deref().map_or_else(|| "no reachable shop".to_string(), |key| format!("shop {key}"));
        return Err(ScriptError::Failed(format!("shopbuy {item}: {from} sold none at {:?}", bot.position())));
    }
    if held < qty {
        bot.log(&format!("shopbuy {item}: the shop had {bought} of {qty}, the rest comes another way"));
    }
    Ok(0)
}

/// What a buy load lacks before the walk: coins to withdraw, and whether
/// the pack must bank its load to make room (coins, bought object).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadLack {
    pub coins: u32,
    pub bank_load: bool,
}

/// Pure: pack slots the bought object takes: none when it stacks onto a
/// carried stack, one for a new stack, one per unit otherwise (at most
/// the pack).
pub fn item_slots(stackable: bool, item_carried: bool, qty: u32) -> usize {
    match (stackable, item_carried) {
        (true, true) => 0,
        (true, false) => 1,
        (false, _) => (qty.max(1) as usize).min(BACKPACK_SIZE),
    }
}

/// The load's lack, read on the pack alone: `bot.inventory()` also sums
/// the bank stack still streamed after a booth closes (live 2026-11-21
/// ida_reed: « carrying 555543 coins » with an empty pack after a market
/// pause, « trade gave no harpoon »). The coins' slot counts only when
/// coins must come and none are carried: the server refuses coins into a
/// full pack (« You don't have enough inventory space »), and the shop
/// refuses the object the same way.
pub fn load_lack(bot: &Bot, coins: u16, want: u32, item: u16, stackable: bool, qty: u32) -> LoadLack {
    let held = pack_total(bot, coins);
    let short = want.saturating_sub(held);
    let room = usize::from(short > 0 && held == 0) + item_slots(stackable, pack_total(bot, item) > 0, qty);
    LoadLack { coins: short, bank_load: free_slots(bot) < room }
}

/// Pure: coins a shop load carries: the order's price and the fares its
/// routes keep in the pack.
pub fn coins_wanted(need: u32, fare_reserve: u32) -> u32 {
    need.saturating_add(fare_reserve)
}

/// Coins in the pack up to `want` and room for the bought object
/// (pack-first, bank-backed): a fresh session has not seen its bank yet
/// (live 2026-11-12: harry_page went to the shop "carrying 0 coins" with
/// 25 banked), so the bank is visited whenever the pack falls short. A
/// pack without the room banks its load first, keeping what stays
/// (tools, held objects), the coins and the object (`kit::toll::fare_keep`
/// rule). Returns the coins carried.
fn carry_coins(bot: &mut Bot, coins: u16, need: u32, item: u16, stackable: bool, qty: u32) -> Result<u32, ScriptError> {
    // A toll-only run arms no fares (`bot.allow_paid = true`, reserve 0):
    // the bank trip prices its reserve AFTER the want was computed, took
    // it from the withdrawal and `unpaid_load` blocked `coins:25` with
    // 884k banked (live 2026-11-22 sean_lane). The reserve is priced
    // first, so the want carries it.
    if bot.allow_paid && bot.assigned_site.is_some() && bot.fare_bank.is_none() {
        let bank = crate::kit::bank::nearest_bank(bot)?;
        crate::kit::toll::refresh_reserve(bot, (bank.x as i32, bank.z as i32, bank.level as i32))?;
    }
    let want = coins_wanted(need, bot.fare_reserve);
    let lack = load_lack(bot, coins, want, item, stackable, qty);
    if lack == (LoadLack { coins: 0, bank_load: false }) {
        return Ok(pack_total(bot, coins));
    }
    let carried: Vec<u16> = pack_slots(bot).iter().map(|(_, (held, _))| *held).collect();
    let keep = if lack.bank_load {
        let mut keep = crate::market_lots::pack_kept_after_trades(&carried, &crate::market_lots::pack_stays(bot));
        keep.extend([coins, item]);
        bot.log(&format!("shopbuy: {} free slots, banking the load before the shop (kept {} types)", free_slots(bot), keep.len()));
        keep
    } else {
        carried
    };
    let bank = crate::kit::bank::nearest_bank(bot)?;
    crate::kit::bank::bank_trip(bot, &bank, &keep, lack.coins)?;
    Ok(pack_total(bot, coins))
}

/// Pure: the typed end of a load that cannot pay one unit: a `coins:N`
/// block on the order's price (the brain files a `have_coins` node),
/// never a walk to a counter that refuses every click (live 2026-11-21:
/// coins locked by market orders, leo_king « carrying 1 coins », three
/// « trade gave no earthrune »).
pub fn unpaid_load(held: u32, price_each: u32, need: u32, reserve: u32) -> Option<ScriptError> {
    (held < price_each.saturating_add(reserve)).then(|| ScriptError::Blocked { missing: vec![format!("coins:{}", need.saturating_add(reserve))] })
}

/// True when the trip did its part: some units bought, or the wanted
/// count already held. A shop with less than wanted is no failure (user
/// 2026-11-19; 197 "run returned 1" today, 43 nodes abandoned): the units
/// bought stay, the node relaunches for the rest (market, another shop).
fn shop_bought_any(bought: u32, held: u32, wanted: u32) -> bool {
    bought > 0 || held >= wanted
}

#[cfg(test)]
mod tests {
    use super::{item_slots, load_lack, LoadLack};

    fn live_bot() -> lifescape_sdk::Bot {
        let mut bot = lifescape_sdk::Bot::new("127.0.0.1", 43594, ".", ".");
        crate::tables_live::attach_live_tables(&mut bot);
        bot
    }

    fn id(bot: &lifescape_sdk::Bot, name: &str) -> u16 {
        crate::kit::pack::obj_id(bot, name).expect("known obj")
    }

    /// Fills the pack: `filler` in `slots` slots, then the extra stacks.
    fn pack(bot: &mut lifescape_sdk::Bot, filler: u16, slots: u8, extra: &[(u16, u32)]) {
        let pack_com = (bot.tables.com_id)("inventory:inv").expect("pack com");
        let mut held: std::collections::HashMap<u8, (u16, u32)> = (0..slots).map(|slot| (slot, (filler, 1))).collect();
        for (index, stack) in extra.iter().enumerate() {
            held.insert(slots + index as u8, *stack);
        }
        bot.world.inv.insert(pack_com, held);
    }

    /// Live 2026-11-21 (ida_reed « carrying 555543 coins » right after a
    /// market pause banked the pack, then « You don't have enough coins »
    /// at Gerrant, « trade gave no harpoon »): `bot.inventory()` sums the
    /// bank stack still streamed. The load reads the pack alone.
    #[test]
    fn a_banked_coin_stack_is_never_carried() {
        let mut bot = live_bot();
        let (coins, logs, harpoon) = (id(&bot, "coins"), id(&bot, "logs"), id(&bot, "harpoon"));
        pack(&mut bot, logs, 3, &[]);
        let bank_com = (bot.tables.com_id)("bank_main:inv").expect("bank com");
        bot.world.inv.insert(bank_com, [(0u8, (coins, 555_543u32))].into_iter().collect());
        assert_eq!(load_lack(&bot, coins, 100, harpoon, false, 1), LoadLack { coins: 100, bank_load: false });
    }

    /// Live 2026-11-21: a full pack (felix_rook, ida_reed: the bank
    /// refused the coins, « You don't have enough inventory space »,
    /// « carrying 0 coins ») or a pack with coins but no slot for the item
    /// (wolfe_jack: 26 leather, needle, 2 coins, « trade gave no thread »
    /// 12 times) banks its load before the walk; a pack with room never.
    #[test]
    fn a_full_pack_banks_its_load_before_the_walk() {
        let mut bot = live_bot();
        let (coins, logs, thread, harpoon) = (id(&bot, "coins"), id(&bot, "logs"), id(&bot, "thread"), id(&bot, "harpoon"));
        pack(&mut bot, logs, 28, &[]);
        assert_eq!(load_lack(&bot, coins, 5, harpoon, false, 1), LoadLack { coins: 5, bank_load: true }, "no slot for the coins");
        pack(&mut bot, logs, 27, &[(coins, 2)]);
        assert_eq!(load_lack(&bot, coins, 2, thread, true, 1), LoadLack { coins: 0, bank_load: true }, "coins carried, no slot for the thread");
        pack(&mut bot, logs, 26, &[(coins, 2), (thread, 3)]);
        assert_eq!(load_lack(&bot, coins, 2, thread, true, 5), LoadLack { coins: 0, bank_load: false }, "the thread stacks on the carried one");
        pack(&mut bot, logs, 10, &[(coins, 100)]);
        assert_eq!(load_lack(&bot, coins, 20, harpoon, false, 1), LoadLack { coins: 0, bank_load: false }, "room and coins: no bank");
        pack(&mut bot, logs, 25, &[(coins, 100)]);
        assert_eq!(load_lack(&bot, coins, 20, harpoon, false, 5), LoadLack { coins: 0, bank_load: true }, "five harpoons, two slots");
    }

    /// Live 2026-11-21 (leo_king « carrying 1 coins », the rest locked by
    /// market orders; three « trade gave no earthrune »): a load that
    /// cannot pay one unit blocks on the order's coins, no walk.
    #[test]
    fn a_load_short_of_one_unit_blocks_on_coins() {
        match super::unpaid_load(1, 5, 3250, 0) {
            Some(crate::ScriptError::Blocked { missing }) => assert_eq!(missing, vec!["coins:3250".to_string()]),
            other => panic!("one coin for a 5-coin rune must block, got {other:?}"),
        }
        assert!(super::unpaid_load(5, 5, 3250, 0).is_none(), "one unit paid: the shop sells what it can");
        assert!(super::unpaid_load(40, 20, 20, 30).is_some(), "the fares come on top");
    }

    /// Live 2026-11-22 sean_lane: want 5 (price) while the bank trip then
    /// priced a 20-coin reserve out of that same withdrawal and the load
    /// blocked `coins:25`. The want always includes the priced reserve.
    #[test]
    fn the_wanted_coins_carry_the_priced_reserve() {
        assert_eq!(super::coins_wanted(5, 20), 25);
        assert!(super::unpaid_load(super::coins_wanted(5, 20), 5, 5, 20).is_none(), "a load carrying the want is never unpaid");
    }

    #[test]
    fn a_buy_needs_one_slot_per_unstacked_unit() {
        assert_eq!(item_slots(true, true, 50), 0, "stacks on the carried stack");
        assert_eq!(item_slots(true, false, 50), 1, "a new stack");
        assert_eq!(item_slots(false, false, 5), 5);
        assert_eq!(item_slots(false, false, 400), crate::kit::pack::BACKPACK_SIZE);
    }

    /// User 2026-11-19: a shop holding 30 of 58 sapphires sells the 30,
    /// the trip succeeds and the node relaunches for the rest; a shop that
    /// sold nothing (and nothing held) still fails.
    #[test]
    fn a_partial_shop_buy_is_no_failure() {
        assert!(super::shop_bought_any(30, 30, 58), "partial: done");
        assert!(super::shop_bought_any(0, 58, 58), "already held");
        assert!(!super::shop_bought_any(0, 0, 58), "nothing sold: typed failure");
    }

    use super::run_shopbuy;

    #[test]
    fn shopbuy_rejects_junk() {
        let mut bot = lifescape_sdk::Bot::new("127.0.0.1", 43594, ".", ".");
        crate::tables_live::attach_live_tables(&mut bot);
        assert!(run_shopbuy(&mut bot, &["shopbuy".to_string()]).is_err(), "item required");
        assert!(run_shopbuy(&mut bot, &["shopbuy".to_string(), "net".to_string(), "x".to_string()]).is_err(), "qty numeric");
        assert!(run_shopbuy(&mut bot, &["shopbuy".to_string(), "nope_not_real".to_string(), "1".to_string()]).is_err(), "unknown item fails loud");
    }
}
