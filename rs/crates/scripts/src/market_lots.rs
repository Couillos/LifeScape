//! Market handover lots (user protocol, 2026-11-13).
//!
//! 1. Every deal between the same two bots on the same object trades as
//!    ONE exchange (summed quantity and coins), in ascending deal order on
//!    both sides. Live 2026-11-13: pairs held 6–7 deals each (730–735,
//!    740–746, 753–759); each side walked them in its own order, the buyer
//!    expected 2 shrimps while the seller offered 5 — `mismatch`, then the
//!    rest died `timeout`: 63 settled out of 620.
//! 2. The goods never travel: everything but the tools goes into the
//!    nearest bank, then the exact lots come out at Varrock west, a short
//!    safe walk from the square (a death on the road costs nothing).

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use lifescape_sdk::Bot;

use crate::kit::bank::nearest_bank;
use crate::kit::pack::pack_total;
use crate::market_client::{trade_id, DealRow};
use crate::ScriptError;

/// Varrock west bank stand: both sides withdraw their lots there.
pub const VARROCK_WEST_STAND: (i32, i32) = (3185, 3436);

/// One physical exchange: every posed deal with one partner on one object
/// and side, summed.
#[derive(Debug, Clone, PartialEq)]
pub struct DealGroup {
    pub ids: Vec<i64>,
    pub partner: String,
    pub buying: bool,
    pub item: u16,
    pub qty: u32,
    pub coins: u32,
    /// Meeting tile of the smallest deal of the group.
    pub tile: (i32, i32),
}

/// Groups the posed deals (`(id, x, z)`) per partner, object and side,
/// ordered by their smallest deal id: both parties hold the same deals
/// between them, so they build the same groups in the same order without
/// talking. Ids the market does not list for this bot come back apart.
pub fn group_deals(jobs: &[(i64, i32, i32)], rows: &[DealRow]) -> (Vec<DealGroup>, Vec<i64>) {
    let mut sorted: Vec<(i64, i32, i32)> = jobs.to_vec();
    sorted.sort_by_key(|job| job.0);
    let mut groups: Vec<DealGroup> = Vec::new();
    let mut unknown = Vec::new();
    for (id, x, z) in sorted {
        let Some(row) = rows.iter().find(|row| row.id == id) else {
            unknown.push(id);
            continue;
        };
        let coins = row.coins.max(0) as u32;
        let same = |group: &&mut DealGroup| group.partner == row.partner && group.item == row.item && group.buying == row.buying;
        if let Some(group) = groups.iter_mut().find(same) {
            group.ids.push(id);
            group.qty += row.qty;
            group.coins += coins;
        } else {
            groups.push(DealGroup {
                ids: vec![id],
                partner: row.partner.clone(),
                buying: row.buying,
                item: row.item,
                qty: row.qty,
                coins,
                tile: (x, z),
            });
        }
    }
    (groups, unknown)
}

/// Carried units of an object in either form (base or banknote) that a
/// trade may take: base units the running script holds (a quest's air
/// talisman, `Bot::hold_in_pack`) never count.
pub fn carried(bot: &Bot, item: u16) -> u32 {
    let note = trade_id(item);
    free_base(bot, item) + if note != item { pack_total(bot, note) } else { 0 }
}

/// Base units of `item` in the pack a trade may offer: none while the
/// running script holds that object.
pub fn free_base(bot: &Bot, item: u16) -> u32 {
    if bot.pack_holds().contains(&item) {
        0
    } else {
        pack_total(bot, item)
    }
}

/// The lots one side puts in the window: a zero lot never reaches it (a
/// gift's maker owes 0 coins, its donor wants 0 coins back).
pub fn window_lots(lots: Vec<(u16, u32)>) -> Vec<(u16, u32)> {
    lots.into_iter().filter(|(_, count)| *count > 0).collect()
}

/// A gift's maker (`fund-makers`: it "buys" coins at price 0): its pack
/// coins before the window, to count what it receives.
pub fn gift_purse(bot: &Bot, group: &DealGroup, coins: u16) -> Option<u32> {
    (group.buying && group.item == coins).then(|| pack_total(bot, coins))
}

/// Coins the gift's maker received once the window settled (the pack
/// update follows the acceptance message by a tick), logged; reported
/// to the daemon, which checks them against the amount.
pub fn gift_received(bot: &mut Bot, group: &DealGroup, coins: u16, before: Option<u32>, settled: bool) -> Result<Option<i64>, ScriptError> {
    let Some(before) = before.filter(|_| settled) else { return Ok(None) };
    bot.pump(3)?;
    let received = i64::from(pack_total(bot, coins)) - i64::from(before);
    bot.log(&format!("gift {:?}: received {received} coins of {}", group.ids, group.qty));
    Ok(Some(received))
}

/// Coins a group takes from the purse: a purchase pays its coins, a
/// gift's donor (`fund-makers`: the deal's object IS coins, price 0)
/// hands over its quantity; `None` for goods.
pub fn purse_need(group: &DealGroup, coins_id: u16) -> Option<u32> {
    if group.buying {
        Some(group.coins)
    } else if group.item == coins_id {
        Some(group.qty)
    } else {
        None
    }
}

/// Which groups the pack covers, walked in order: each covered group
/// takes its share, so two groups never count the same coins or stack.
pub fn covered(groups: &[DealGroup], coins_id: u16, mut coins: u32, mut stock: BTreeMap<u16, u32>) -> Vec<bool> {
    groups
        .iter()
        .map(|group| {
            if let Some(need) = purse_need(group, coins_id) {
                let ok = coins >= need;
                if ok {
                    coins -= need;
                }
                ok
            } else {
                let have = stock.entry(group.item).or_insert(0);
                let ok = *have >= group.qty;
                if ok {
                    *have -= group.qty;
                }
                ok
            }
        })
        .collect()
}

/// Ticks a delivery's walk to its bank gets (a bot resumed from the square
/// may already be well on its way back to work).
const BANK_APPROACH_TICKS: u64 = 2000;

/// True when the booth is out of `open_bank`'s short stand walk (more
/// than 6 tiles, or another level): the delivery travels there first.
pub(crate) fn bank_needs_approach(bot_at: (i32, i32, i32), bank: (i32, i32, i32)) -> bool {
    bot_at.0 != bank.2 || (bot_at.1 - bank.0).abs() + (bot_at.2 - bank.1).abs() > 6
}

/// Nearest bank, travelled to then opened, one retry (live 2026-11-19:
/// 39 deliveries "lots not secured in time", lucy_rhodes "refused from
/// every stand": `open_bank` only walks the last few tiles, the bot was
/// still on its way back to work).
pub(crate) fn open_bank_here(bot: &mut Bot) -> Result<bool, ScriptError> {
    for attempt in 1..=2 {
        let bank = nearest_bank(bot)?;
        let (bank_x, bank_z, bank_level) = (bank.x as i32, bank.z as i32, bank.level as i32);
        if bank_needs_approach(bot.position(), (bank_x, bank_z, bank_level)) {
            match bot.approach(bank_x, bank_z, bank_level, 6, BANK_APPROACH_TICKS) {
                Ok(()) => {}
                Err(error @ (lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_))) => return Err(ScriptError::Sdk(error)),
                Err(error) => bot.log(&format!("handover: walk to the bank at ({bank_x},{bank_z}) failed ({error}), attempt {attempt}")),
            }
        }
        if bot.open_bank(bank_x, bank_z, bank.loc_id as u16, 300)? {
            return Ok(stream_bank(bot)?);
        }
        bot.log(&format!("handover: bank at ({bank_x},{bank_z}) did not open from {:?}, attempt {attempt}", bot.position()));
    }
    Ok(false)
}

/// Waits for the bank pack, which streams a few ticks after the interface.
fn stream_bank(bot: &mut Bot) -> Result<bool, ScriptError> {
    // The bank pack streams a few ticks after the interface opens.
    let main_com = (bot.tables.com_id)("bank_main:inv");
    let streamed = bot.world.ticks + 60;
    while bot.world.ticks < streamed {
        if main_com.is_some_and(|com| bot.world.inv.get(&com).is_some_and(|slots| !slots.is_empty())) {
            break;
        }
        bot.pump(2)?;
    }
    Ok(true)
}

/// Base object of a banknote id (`cert_<name>`), `None` for a base id.
pub fn note_base(id: u16) -> Option<u16> {
    let name = lifescape_gamedata::obj_by_id(usize::from(id)).and_then(|entry| entry.name)?;
    let base = name.strip_prefix("cert_")?;
    lifescape_gamedata::objs_by_name(base).into_iter().next().map(|entry| entry.obj_id as u16)
}

/// Pack ids that stay in the pack once the trades are done (user
/// 2026-11-19: « ce serait plus intelligent qu'il relance son activité
/// depuis la place Varrock »): only the tools and the objects the paused
/// script holds (`stays`); everything else, the load it carried before
/// the pause included, goes to the bank, and the activity relaunches from
/// the square instead of walking the old pack back to its work.
pub fn pack_kept_after_trades(carried: &[u16], stays: &[u16]) -> Vec<u16> {
    let mut kept: Vec<u16> = carried.iter().copied().filter(|held| stays.contains(held)).collect();
    kept.sort_unstable();
    kept.dedup();
    kept
}

/// Pack ids that never leave the pack on a delivery: the tools and the
/// paused script's held objects (a quest's air talisman, live
/// 2026-11-14 bh99tw2f: banked, its listings then locked it).
pub fn pack_stays(bot: &Bot) -> Vec<u16> {
    let mut stays = tool_ids(bot);
    stays.extend(bot.pack_holds());
    stays
}

/// Tools stay in the pack across the trip: the paused task resumes from
/// the square with every tool it carried, its axe, pickaxe or net and any
/// object a recipe carries as a tool (user 2026-11-19: a gem cutter's
/// chisel was banked after a delivery, the task blocked on it).
pub fn tool_ids(bot: &Bot) -> Vec<u16> {
    let mut keep: Vec<u16> = Vec::new();
    for spec in [&crate::kit::tools::AXE_SPEC, &crate::kit::tools::PICK_SPEC, &crate::kit::tools::FISH_SPEC] {
        keep.extend(spec.known_ids(bot));
    }
    for (_, (held, _)) in bot.backpack().1 {
        let tool = lifescape_gamedata::obj_by_id(usize::from(held)).and_then(|entry| entry.name).is_some_and(lifescape_gamedata::obtain::is_carried_tool);
        if tool && !keep.contains(&held) {
            keep.push(held);
        }
    }
    keep
}

/// Pack ids a finished shop sale leaves behind: everything but the
/// tools, the held objects of a paused script and the coins (the unsold
/// remainder of the lots, in banknote or base form).
pub fn unsold_in_pack(bot: &Bot) -> Vec<u16> {
    let mut keep = pack_stays(bot);
    keep.extend(crate::kit::pack::obj_id(bot, "coins").ok());
    let mut left: Vec<u16> = crate::kit::pack::pack_slots(bot).into_iter().map(|(_, (held, _))| held).filter(|held| !keep.contains(held)).collect();
    left.sort_unstable();
    left.dedup();
    left
}

/// With the bank open: banks what a shop sale left in the pack (live
/// 2026-11-22, owen_wood: 7 637 iron ore notes in the pack, four `smelt`
/// blocks `missing: iron_ore` in 20 s). Tools and coins stay.
pub fn bank_leftovers(bot: &mut Bot) -> Result<(), ScriptError> {
    if unsold_in_pack(bot).is_empty() {
        return Ok(());
    }
    let Some(side_com) = (bot.tables.com_id)("bank_side:inv") else { return Ok(()) };
    let mut keep = pack_stays(bot);
    keep.extend(crate::kit::pack::obj_id(bot, "coins").ok());
    let _ = bot.bank_deposit_all(side_com, &keep, 300)?;
    bot.pump(2)?;
    Ok(())
}

/// Banks everything but the tools at the nearest bank, then withdraws the
/// lots of every group at Varrock west. `None` when a bank could not be
/// reached or opened (a trip failure, never a short party); otherwise one
/// flag per group, false where the bank genuinely lacks the lots.
pub fn secure_lots(bot: &mut Bot, groups: &[DealGroup], coins: u16) -> Result<Option<Vec<bool>>, ScriptError> {
    let mut coins_needed = 0u32;
    let mut items: BTreeMap<u16, u32> = BTreeMap::new();
    for group in groups {
        if let Some(need) = purse_need(group, coins) {
            coins_needed += need;
        } else {
            *items.entry(group.item).or_insert(0) += group.qty;
        }
    }
    // Behind the Al Kharid toll the walk to Varrock west may pay the gate
    // (live 2026-11-15: 233 handovers banked every coin at Al Kharid, then
    // `toll door loc 2883` refused the way): the fare its planned route
    // crosses stays in the pack, nothing when the free way wins (user
    // 2026-11-17: travel coins only for a trip that costs money).
    let (stand_x, stand_z) = VARROCK_WEST_STAND;
    let stand = lifescape_sdk::plan::Tile { x: stand_x, z: stand_z, level: 0 };
    let behind_toll = lifescape_gamedata::toll::behind(i64::from(bot.world.x), i64::from(bot.world.z), i64::from(bot.world.level));
    let fare = if behind_toll {
        let here = lifescape_sdk::plan::Tile { x: bot.world.x, z: bot.world.z, level: bot.world.level };
        match bot.route_fare(here, stand) {
            Ok(fare) => fare,
            Err(error) => {
                bot.log(&format!("handover: Varrock west unreachable: {error}"));
                return Ok(None);
            }
        }
    } else {
        0
    };
    // 1. Nearest bank: nothing of the trade rides along.
    if !open_bank_here(bot)? {
        bot.log("handover: nearest bank did not open");
        return Ok(None);
    }
    if let Some(side_com) = (bot.tables.com_id)("bank_side:inv") {
        let mut keep = tool_ids(bot);
        if fare > 0 {
            keep.push(coins);
        }
        let _ = bot.bank_deposit_all(side_com, &keep, 300);
        bot.pump(2)?;
        let carried = crate::kit::pack::pack_total(bot, coins);
        if carried < fare {
            let _ = bot.bank_withdraw(coins, fare - carried, 40);
        }
    }
    let _ = bot.close_modal();
    if fare > 0 {
        bot.allow_paid = true;
    }
    // 2. Varrock west: the exact lots, a short safe walk from the square.
    if let Err(error) = bot.travel_to(lifescape_sdk::plan::Tile { x: stand_x, z: stand_z, level: 0 }) {
        bot.log(&format!("handover: Varrock west unreachable: {error}"));
        return Ok(None);
    }
    if !open_bank_here(bot)? {
        bot.log("handover: Varrock west bank did not open");
        return Ok(None);
    }
    // The trade procedure is the only one that takes the locked lots out.
    bot.set_trading(true);
    let withdrawn = withdraw_lots(bot, coins, coins_needed, &items);
    bot.set_trading(false);
    withdrawn?;
    let _ = bot.close_modal();
    bot.pump(3)?;
    let stock: BTreeMap<u16, u32> = items.keys().map(|item| (*item, carried(bot, *item))).collect();
    let flags = covered(groups, coins, pack_total(bot, coins), stock);
    for (group, ok) in groups.iter().zip(&flags) {
        if !ok {
            bot.log(&format!(
                "handover: lots never landed ({} obj {} x{}, {} coins) for deals {:?}: the bank is short",
                if group.buying { "buying" } else { "selling" },
                group.item,
                group.qty,
                group.coins,
                group.ids
            ));
        }
    }
    Ok(Some(flags))
}

/// Withdraws the purse and every object's lots (notes when they exist).
fn withdraw_lots(bot: &mut Bot, coins: u16, coins_needed: u32, items: &BTreeMap<u16, u32>) -> Result<(), ScriptError> {
    if coins_needed > 0 {
        let _ = bot.bank_withdraw(coins, coins_needed, 300)?;
    }
    for (item, qty) in items {
        // Banknotes whenever the object has one: any quantity rides one
        // slot (27+ raw shrimps never fit as base objects), and the buyer
        // accepts either form at the window.
        let note = trade_id(*item);
        if note == *item {
            let _ = bot.bank_withdraw(*item, *qty, 300)?;
        } else {
            let _ = bot.bank_withdraw_noted(*item, note, *qty, 300)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{covered, group_deals, DealGroup};
    use crate::market_client::DealRow;
    use std::collections::BTreeMap;

    fn row(id: i64, partner: &str, buying: bool, item: u16, qty: u32, price: i64) -> DealRow {
        DealRow { id, partner: partner.to_string(), buying, item, qty, price, coins: i64::from(qty) * price }
    }

    /// Live 2026-11-13 (deals 740–746): seven deals between bcm9kxn4 and
    /// sara_miller on raw shrimps trade as one exchange, whatever order
    /// the pose listed them in — both sides build the same group.
    #[test]
    fn deals_with_one_partner_on_one_object_trade_once() {
        let rows = vec![
            row(742, "sara_miller", true, 317, 2, 1),
            row(740, "sara_miller", true, 317, 2, 1),
            row(741, "sara_miller", true, 317, 3, 2),
            row(745, "bh99tw2f", true, 317, 1, 1),
        ];
        let jobs = vec![(742, 3210, 3426), (745, 3212, 3426), (740, 3209, 3425), (741, 3210, 3426)];
        let (groups, unknown) = group_deals(&jobs, &rows);
        assert!(unknown.is_empty());
        assert_eq!(groups.len(), 2, "one exchange per partner and object");
        assert_eq!(groups[0].ids, vec![740, 741, 742], "ascending: same order on both sides");
        assert_eq!(groups[0].qty, 7);
        assert_eq!(groups[0].coins, 2 + 6 + 2);
        assert_eq!(groups[0].tile, (3209, 3425), "the smallest deal's tile");
        assert_eq!(groups[1].partner, "bh99tw2f");
        // The seller's side of the same deals builds the same group.
        let seller_rows: Vec<DealRow> = rows
            .iter()
            .filter(|row| row.partner == "sara_miller")
            .map(|row| DealRow { partner: "bcm9kxn4".to_string(), buying: false, ..row.clone() })
            .collect();
        let (seller_groups, _) = group_deals(&[(741, 3211, 3426), (740, 3210, 3425), (742, 3211, 3426)], &seller_rows);
        assert_eq!(seller_groups.len(), 1);
        assert_eq!(seller_groups[0].ids, groups[0].ids);
        assert_eq!(seller_groups[0].qty, groups[0].qty);
        assert_eq!(seller_groups[0].coins, groups[0].coins);
    }

    /// lucy_rhodes 2026-11-19: the booth 40 tiles away is no short stand
    /// walk, the delivery travels there first; next to it, or on another
    /// level, decides alike.
    #[test]
    fn a_far_booth_is_travelled_to_first() {
        use super::bank_needs_approach;
        assert!(!bank_needs_approach((0, 3185, 3436), (3186, 3436, 0)), "at the booth");
        assert!(bank_needs_approach((0, 3150, 3436), (3186, 3436, 0)), "36 tiles away");
        assert!(bank_needs_approach((1, 3186, 3436), (3186, 3436, 0)), "upstairs");
    }

    /// Live 2026-11-22 (owen_wood, pia_brown, 30 of 154 blocks in an
    /// hour): a shop trip that stopped at the shop's price left the
    /// unsold lot in the pack as banknotes; the next script's inputs read
    /// as missing. The notes are what the sale leaves behind; the pick,
    /// the coins stay.
    #[test]
    fn the_unsold_notes_are_what_a_sale_leaves_in_the_pack() {
        let mut bot = lifescape_sdk::Bot::new("127.0.0.1", 43594, ".", ".");
        crate::tables_live::attach_live_tables(&mut bot);
        let pack = (bot.tables.com_id)("inventory:inv").expect("pack com");
        let note = crate::market_client::trade_id(crate::kit::pack::obj_id(&bot, "iron_ore").expect("iron ore"));
        let pick = crate::kit::pack::obj_id(&bot, "bronze_pickaxe").expect("pickaxe");
        let coins = crate::kit::pack::obj_id(&bot, "coins").expect("coins");
        bot.world.inv.insert(pack, [(0u8, (note, 7637u32)), (1u8, (pick, 1u32)), (2u8, (coins, 80u32))].into_iter().collect());
        assert_eq!(super::unsold_in_pack(&bot), vec![note]);
    }

    /// User 2026-11-19: once the trades are done, the load the paused
    /// woodcutter carried (logs, their notes), the coins and the traded
    /// lots all go to the bank; only the tools stay.
    #[test]
    fn only_the_tools_stay_after_the_trades() {
        use super::pack_kept_after_trades;
        let (coins, axe, logs, logs_note, feather) = (995u16, 1351u16, 1511u16, 1512u16, 314u16);
        let carried = [coins, axe, logs, logs_note, logs, feather];
        assert_eq!(pack_kept_after_trades(&carried, &[axe, 1265]), vec![axe]);
        assert!(pack_kept_after_trades(&[coins, logs], &[axe]).is_empty(), "no tool carried: the pack empties");
    }

    #[test]
    fn unknown_deals_come_back_apart() {
        let (groups, unknown) = group_deals(&[(9, 0, 0)], &[]);
        assert!(groups.is_empty());
        assert_eq!(unknown, vec![9]);
    }

    /// Live 2026-11-14 (bh99tw2f): a delivery paused Rune Mysteries and
    /// banked the Duke's air talisman, which its listings then locked.
    /// The quest holds it: it stays in the pack with the tools.
    #[test]
    fn a_held_quest_object_never_leaves_the_pack() {
        use super::pack_kept_after_trades;
        let (coins, talisman) = (995u16, 1438u16);
        let held = lifescape_sdk::client::locks::deposit_keep(&[], &[talisman]);
        assert_eq!(pack_kept_after_trades(&[coins, talisman], &held), vec![talisman]);
    }

    /// Coverage walks the groups in order: a stack or a purse is never
    /// counted twice, and a short group does not hide a covered one.
    #[test]
    fn coverage_never_counts_the_same_stock_twice() {
        let group = |ids: Vec<i64>, buying: bool, item: u16, qty: u32, coins: u32| DealGroup {
            ids,
            partner: "p".to_string(),
            buying,
            item,
            qty,
            coins,
            tile: (0, 0),
        };
        let groups = vec![
            group(vec![1], false, 317, 14, 14),
            group(vec![2], false, 317, 11, 11),
            group(vec![3], true, 1511, 5, 5),
        ];
        let stock = BTreeMap::from([(317u16, 20u32)]);
        assert_eq!(covered(&groups, 995, 5, stock), vec![true, false, true], "14 of 20 shrimps taken, 11 more short");
    }

    /// A gift (`fund-makers`: coins at price 0) takes its quantity from
    /// the purse on the donor's side, nothing on the maker's; one purse
    /// is never counted twice between a gift and a purchase.
    #[test]
    fn a_gift_is_paid_from_the_purse() {
        let group = |buying: bool, item: u16, qty: u32, coins: u32| DealGroup { ids: vec![9], partner: "p".to_string(), buying, item, qty, coins, tile: (0, 0) };
        let (donor, maker, purchase) = (group(false, 995, 40_000, 0), group(true, 995, 40_000, 0), group(true, 1511, 5, 50));
        assert_eq!(super::purse_need(&donor, 995), Some(40_000));
        assert_eq!(super::purse_need(&maker, 995), Some(0), "the maker puts nothing in");
        assert_eq!(super::purse_need(&group(false, 1511, 5, 50), 995), None, "goods are no purse");
        assert_eq!(covered(&[donor.clone(), purchase.clone()], 995, 40_049, BTreeMap::new()), vec![true, false], "one purse, counted once");
        assert_eq!(covered(&[maker], 995, 0, BTreeMap::new()), vec![true]);
        assert_eq!(super::window_lots(vec![(995, 0)]), Vec::<(u16, u32)>::new(), "the maker offers nothing");
        assert_eq!(super::window_lots(vec![(995, 40_000)]), vec![(995, 40_000)]);
    }
}

#[cfg(test)]
mod offer_tests {
    use crate::market_client::{offer_state, OfferState};
    use std::collections::HashMap;

    /// Deals 798/801: 1 noted shrimp of 4 on the window is a partial
    /// offer, not a mismatch; the full stack completes it; anything else
    /// (another object, coins, too many) is refused.
    #[test]
    fn partial_offers_are_waited_for() {
        let (base, note) = (317u16, 318u16);
        assert_eq!(offer_state(&HashMap::from([(note, 1)]), base, note, 4), OfferState::Partial);
        assert_eq!(offer_state(&HashMap::new(), base, note, 4), OfferState::Partial);
        assert_eq!(offer_state(&HashMap::from([(note, 4)]), base, note, 4), OfferState::Complete(vec![(note, 4)]));
        assert_eq!(offer_state(&HashMap::from([(note, 5)]), base, note, 4), OfferState::Wrong, "more than the deal");
        assert_eq!(offer_state(&HashMap::from([(995, 4)]), base, note, 4), OfferState::Wrong, "coins are not the lot");
        assert_eq!(offer_state(&HashMap::from([(1511, 1)]), base, note, 4), OfferState::Wrong, "another object");
    }
}
