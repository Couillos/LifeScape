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
        let coins = (i64::from(row.qty) * row.price).max(0) as u32;
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

/// Pack ids a delivery handles (coins, every dealt form, base and
/// note): what the redeposit takes back to the bank.
pub fn touched_ids(groups: &[DealGroup], coins: u16) -> Vec<u16> {
    let mut touched = vec![coins];
    for group in groups {
        let trade = trade_id(group.item);
        touched.push(trade);
        if group.item != trade {
            touched.push(group.item);
        }
    }
    touched
}

/// Carried units of an object in either form (base or banknote).
pub fn carried(bot: &Bot, item: u16) -> u32 {
    let note = trade_id(item);
    pack_total(bot, item) + if note != item { pack_total(bot, note) } else { 0 }
}

/// Which groups the pack covers, walked in order: each covered group
/// takes its share, so two groups never count the same coins or stack.
pub fn covered(groups: &[DealGroup], mut coins: u32, mut stock: BTreeMap<u16, u32>) -> Vec<bool> {
    groups
        .iter()
        .map(|group| {
            if group.buying {
                let ok = coins >= group.coins;
                if ok {
                    coins -= group.coins;
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

fn open_bank_here(bot: &mut Bot) -> Result<bool, ScriptError> {
    let bank = nearest_bank(bot)?;
    if !bot.open_bank(bank.x as i32, bank.z as i32, bank.loc_id as u16, 300)? {
        return Ok(false);
    }
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

/// One stack the paused script carried: base object, units, and whether
/// it rode as banknotes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Carried {
    pub base: u16,
    pub units: u32,
    pub noted: bool,
}

/// What the delivery hands back to the paused script once the trades
/// are done (live 2026-11-13, bdr53jee: a shop buy had withdrawn its
/// coins and was walking to Gerrant when a delivery paused it; the trip
/// banked everything but the tools and the shop answered "You don't
/// have enough coins", `trade gave no harpoon` twice). Every carried
/// stack but the tools (they never leave the pack), merged per form.
pub fn pack_to_restore(carried: &[(u16, u32)], tools: &[u16], note_of: &dyn Fn(u16) -> Option<u16>) -> Vec<Carried> {
    let mut out: Vec<Carried> = Vec::new();
    for (id, units) in carried {
        if tools.contains(id) || *units == 0 {
            continue;
        }
        let (base, noted) = match note_of(*id) {
            Some(base) => (base, true),
            None => (*id, false),
        };
        match out.iter_mut().find(|stack| stack.base == base && stack.noted == noted) {
            Some(stack) => stack.units += units,
            None => out.push(Carried { base, units: *units, noted }),
        }
    }
    out
}

/// Live form of `pack_to_restore` on the current pack.
pub fn carried_before(bot: &Bot) -> Vec<Carried> {
    let carried: Vec<(u16, u32)> = bot.backpack().1.into_iter().map(|(_, held)| held).collect();
    pack_to_restore(&carried, &tool_ids(bot), &note_base)
}

/// Tools stay in the pack across the trip: the paused activity resumes
/// with its axe, pickaxe or net.
pub fn tool_ids(bot: &Bot) -> Vec<u16> {
    let mut keep: Vec<u16> = Vec::new();
    for spec in [&crate::kit::tools::AXE_SPEC, &crate::kit::tools::PICK_SPEC, &crate::kit::tools::FISH_SPEC] {
        keep.extend(spec.known_ids(bot));
    }
    keep
}

/// Banks everything but the tools at the nearest bank, then withdraws the
/// lots of every group at Varrock west. `None` when a bank could not be
/// reached or opened (a trip failure, never a short party); otherwise one
/// flag per group, false where the bank genuinely lacks the lots.
pub fn secure_lots(bot: &mut Bot, groups: &[DealGroup], coins: u16) -> Result<Option<Vec<bool>>, ScriptError> {
    let mut coins_needed = 0u32;
    let mut items: BTreeMap<u16, u32> = BTreeMap::new();
    for group in groups {
        if group.buying {
            coins_needed += group.coins;
        } else {
            *items.entry(group.item).or_insert(0) += group.qty;
        }
    }
    // 1. Nearest bank: nothing of the trade rides along.
    if !open_bank_here(bot)? {
        bot.log("handover: nearest bank did not open");
        return Ok(None);
    }
    if let Some(side_com) = (bot.tables.com_id)("bank_side:inv") {
        let keep = tool_ids(bot);
        let _ = bot.bank_deposit_all(side_com, &keep, 300);
        bot.pump(2)?;
    }
    let _ = bot.close_modal();
    // 2. Varrock west: the exact lots, a short safe walk from the square.
    let (stand_x, stand_z) = VARROCK_WEST_STAND;
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
    let flags = covered(groups, pack_total(bot, coins), stock);
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
        DealRow { id, partner: partner.to_string(), buying, item, qty, price }
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

    /// bdr53jee 2026-11-13: the shop buy carried its coins (and a pot of
    /// noted logs) into the pause; the delivery hands them back, tools
    /// never left, notes keep their form, split stacks merge.
    #[test]
    fn the_paused_pack_is_handed_back() {
        use super::{pack_to_restore, Carried};
        let (coins, harpoon_net, logs, logs_note) = (995u16, 303u16, 1511u16, 1512u16);
        let carried = [(coins, 40), (harpoon_net, 1), (logs, 3), (logs_note, 50), (logs, 2)];
        let note_of = |id: u16| (id == logs_note).then_some(logs);
        let restore = pack_to_restore(&carried, &[harpoon_net], &note_of);
        assert_eq!(
            restore,
            vec![
                Carried { base: coins, units: 40, noted: false },
                Carried { base: logs, units: 5, noted: false },
                Carried { base: logs, units: 50, noted: true },
            ]
        );
    }

    #[test]
    fn unknown_deals_come_back_apart() {
        let (groups, unknown) = group_deals(&[(9, 0, 0)], &[]);
        assert!(groups.is_empty());
        assert_eq!(unknown, vec![9]);
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
        assert_eq!(covered(&groups, 5, stock), vec![true, false, true], "14 of 20 shrimps taken, 11 more short");
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
